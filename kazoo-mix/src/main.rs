//! kazoo-mix — terminal studio mixer.
//!
//! This is the first solid slice of the future mixer: it owns the output audio
//! device, maintains callback-safe meter state, and draws a robust terminal
//! console. Instrument registration/audio transport will be layered on top of
//! this crate rather than continuing to deepen `kazoo-tui` as the hub.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use color_eyre::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Row, Table};

use kazoo_core::Pan;
use kazoo_core::audio_transport::{AudioRingConfig, audio_block_ring};
use kazoo_core::protocol::{BufferId, ChannelId};
use kazoo_mix::control::{ControlServer, ControlSnapshot};
use kazoo_mix::engine::{
    ChannelControls, ChannelSnapshot, DEFAULT_CHANNEL_SLOTS, MAX_CALLBACK_SAMPLES, MixerEngine,
    StereoLevel,
};
use kazoo_mix::session::MixSession;
use kazoo_mix::source::EightOhEightSource;
use kazoo_mix::terminal::{MixTerminal, TerminalGuard};

const UI_TICK: Duration = Duration::from_millis(33);
const MAX_EVENTS_PER_FRAME: usize = 8;
const UI_CHANNELS: usize = 8;

const BG: Color = Color::Rgb(0x17, 0x14, 0x12);
const PANEL: Color = Color::Rgb(0x24, 0x1E, 0x19);
const PANEL_ALT: Color = Color::Rgb(0x2E, 0x26, 0x20);
const TEXT: Color = Color::Rgb(0xE7, 0xDC, 0xCB);
const TEXT_DIM: Color = Color::Rgb(0x96, 0x87, 0x74);
const BRASS: Color = Color::Rgb(0xCF, 0xA2, 0x47);
const SAGE: Color = Color::Rgb(0x87, 0xB3, 0x61);
const AMBER: Color = Color::Rgb(0xDE, 0xA2, 0x34);
const RED: Color = Color::Rgb(0xDE, 0x58, 0x45);
const STEEL: Color = Color::Rgb(0x73, 0x6B, 0x62);

fn main() -> Result<()> {
    color_eyre::install()?;

    let session = MixSession::create_default()?;
    let control = ControlServer::start(&session)?;
    let mut terminal_guard = TerminalGuard::enter();
    let audio = MixerAudio::start()?;
    let mut app = App::new(audio.info(), session);

    let result = run_app(terminal_guard.terminal_mut(), &mut app, &audio, &control);

    drop(control);
    drop(audio);
    terminal_guard.restore()?;

    result
}

fn run_app(
    terminal: &mut MixTerminal,
    app: &mut App,
    audio: &MixerAudio,
    control: &ControlServer,
) -> Result<()> {
    while !app.should_quit {
        app.update(audio, control.snapshot());
        terminal.draw(|frame| draw(frame, app))?;

        let mut handled = 0;
        while handled < MAX_EVENTS_PER_FRAME
            && event::poll(if handled == 0 {
                UI_TICK
            } else {
                Duration::ZERO
            })?
        {
            handled += 1;
            if let Event::Key(key) = event::read()? {
                handle_key(app, audio, key);
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, audio: &MixerAudio, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c' | 'q' | 'd'))
    {
        app.should_quit = true;
        return;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char(' ') => app.transport_playing = !app.transport_playing,
        KeyCode::Char('c' | 'C') => app.metronome = !app.metronome,
        KeyCode::Char('+' | '=') => app.bpm = (app.bpm + 1.0).min(300.0),
        KeyCode::Char('-') => app.bpm = (app.bpm - 1.0).max(20.0),
        KeyCode::Up | KeyCode::Char('k') => app.select_previous_channel(),
        KeyCode::Down | KeyCode::Char('j') => app.select_next_channel(),
        KeyCode::Char('m' | 'M') => audio.toggle_mute(app.selected_channel),
        KeyCode::Char('s' | 'S') => audio.toggle_solo(app.selected_channel),
        KeyCode::Left | KeyCode::Char('h') => audio.adjust_pan(app.selected_channel, -0.1),
        KeyCode::Right | KeyCode::Char('l') => audio.adjust_pan(app.selected_channel, 0.1),
        KeyCode::Char('[') => audio.adjust_gain(app.selected_channel, -0.05),
        KeyCode::Char(']') => audio.adjust_gain(app.selected_channel, 0.05),
        KeyCode::Char('0') => audio.reset_channel(app.selected_channel),
        KeyCode::Char(ch @ '1'..='8') => app.selected_channel = (ch as usize - '1' as usize).min(UI_CHANNELS - 1),
        _ => {}
    }
}

#[derive(Debug, Clone, Copy)]
struct AudioInfo {
    sample_rate: u32,
    channels: u16,
    buffer_size: Option<u32>,
}

struct MixerAudio {
    _stream: cpal::Stream,
    _source: EightOhEightSource,
    state: Arc<AudioCallbackState>,
    info: AudioInfo,
}

impl MixerAudio {
    fn start() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| color_eyre::eyre::eyre!("no default output audio device found"))?;
        let supported = device.default_output_config()?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let info = AudioInfo {
            sample_rate: config.sample_rate,
            channels: config.channels,
            buffer_size: match config.buffer_size {
                cpal::BufferSize::Default => None,
                cpal::BufferSize::Fixed(frames) => Some(frames),
            },
        };

        let state = Arc::new(AudioCallbackState::new());
        let block_frames = info.buffer_size.unwrap_or(128).max(1);
        let ring_config = AudioRingConfig::new(BufferId(1), info.channels, block_frames, 8);
        let (producer, consumer) = audio_block_ring(ring_config);
        let source =
            EightOhEightSource::start(producer, info.sample_rate, info.channels, block_frames);
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build_output_stream::<f32>(&device, &config, Arc::clone(&state), consumer)?
            }
            cpal::SampleFormat::I16 => {
                build_output_stream::<i16>(&device, &config, Arc::clone(&state), consumer)?
            }
            cpal::SampleFormat::U16 => {
                build_output_stream::<u16>(&device, &config, Arc::clone(&state), consumer)?
            }
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "unsupported output sample format: {other:?}"
                ));
            }
        };
        stream.play()?;

        let audio = Self {
            _stream: stream,
            _source: source,
            state,
            info,
        };
        audio.store_controls(0, ChannelControls {
            gain: 0.95,
            pan: Pan::CENTER,
            muted: false,
            soloed: false,
        });
        Ok(audio)
    }

    const fn info(&self) -> AudioInfo {
        self.info
    }

    fn controls(&self, slot: usize) -> ChannelControls {
        self.state.controls(slot)
    }

    fn store_controls(&self, slot: usize, controls: ChannelControls) {
        self.state.store_controls(slot, controls);
    }

    fn adjust_gain(&self, slot: usize, delta: f32) {
        let mut controls = self.controls(slot);
        controls.gain = (controls.gain + delta).clamp(0.0, 2.0);
        self.store_controls(slot, controls);
    }

    fn adjust_pan(&self, slot: usize, delta: f32) {
        let mut controls = self.controls(slot);
        controls.pan = Pan::new((controls.pan.value() + delta).clamp(-1.0, 1.0));
        self.store_controls(slot, controls);
    }

    fn toggle_mute(&self, slot: usize) {
        let mut controls = self.controls(slot);
        controls.muted = !controls.muted;
        self.store_controls(slot, controls);
    }

    fn toggle_solo(&self, slot: usize) {
        let mut controls = self.controls(slot);
        controls.soloed = !controls.soloed;
        self.store_controls(slot, controls);
    }

    fn reset_channel(&self, slot: usize) {
        self.store_controls(slot, ChannelControls::default());
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<AudioCallbackState>,
    consumer: kazoo_core::audio_transport::AudioBlockConsumer,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let mut engine = MixerEngine::new(DEFAULT_CHANNEL_SLOTS);
    engine
        .attach_consumer(0, "808", consumer)
        .map_err(|err| color_eyre::eyre::eyre!("failed to attach 808 source: {err:?}"))?;
    engine
        .configure_channel(
            0,
            ChannelControls {
                gain: 0.95,
                pan: Pan::CENTER,
                muted: false,
                soloed: false,
            },
        )
        .map_err(|err| color_eyre::eyre::eyre!("failed to configure 808 channel: {err:?}"))?;
    engine.set_master_gain(0.9);

    let control_state = Arc::clone(&state);
    let mut mix = vec![0.0_f32; MAX_CALLBACK_SAMPLES];
    let mut channel_snapshots = [ChannelSnapshot::EMPTY; UI_CHANNELS];
    let render_state = Arc::clone(&state);
    let error_state = state;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for slot in 0..DEFAULT_CHANNEL_SLOTS.min(UI_CHANNELS) {
                let _ = engine.configure_channel(slot, control_state.controls(slot));
            }

            let render_len = data.len().min(mix.len());
            engine.render_f32(&mut mix[..render_len], channels);

            for (target, sample) in data.iter_mut().zip(mix.iter().copied()) {
                *target = T::from_sample(sample);
            }
            if render_len < data.len() {
                for target in &mut data[render_len..] {
                    *target = T::from_sample(0.0);
                }
            }

            render_state.publish_master(engine.master_peak(), engine.master_rms());
            engine.copy_channel_snapshots(&mut channel_snapshots);
            render_state.publish_channels(&channel_snapshots);
            render_state.frames_rendered.fetch_add(
                u64::try_from(data.len() / channels).unwrap_or(0),
                Ordering::Relaxed,
            );
        },
        move |err| {
            error_state.stream_errors.fetch_add(1, Ordering::Relaxed);
            eprintln!("kazoo-mix audio stream error: {err}");
        },
        None,
    )?;

    Ok(stream)
}

#[derive(Debug)]
struct AudioCallbackState {
    master_peak_left_bits: AtomicU32,
    master_peak_right_bits: AtomicU32,
    master_rms_left_bits: AtomicU32,
    master_rms_right_bits: AtomicU32,
    frames_rendered: AtomicU64,
    stream_errors: AtomicU64,
    xruns: AtomicU64,
    control_gain_bits: [AtomicU32; UI_CHANNELS],
    control_pan_bits: [AtomicU32; UI_CHANNELS],
    control_flags: [AtomicU32; UI_CHANNELS],
    channel_peak_left_bits: [AtomicU32; UI_CHANNELS],
    channel_peak_right_bits: [AtomicU32; UI_CHANNELS],
    channel_rms_left_bits: [AtomicU32; UI_CHANNELS],
    channel_rms_right_bits: [AtomicU32; UI_CHANNELS],
    channel_connected: [AtomicBool; UI_CHANNELS],
    channel_gain_bits: [AtomicU32; UI_CHANNELS],
    channel_pan_bits: [AtomicU32; UI_CHANNELS],
    channel_muted: [AtomicBool; UI_CHANNELS],
    channel_soloed: [AtomicBool; UI_CHANNELS],
    channel_underruns: [AtomicU64; UI_CHANNELS],
    channel_sequence_gaps: [AtomicU64; UI_CHANNELS],
    channel_name: [[AtomicU32; 3]; UI_CHANNELS],
}

impl AudioCallbackState {
    const fn new() -> Self {
        Self {
            master_peak_left_bits: AtomicU32::new(0.0_f32.to_bits()),
            master_peak_right_bits: AtomicU32::new(0.0_f32.to_bits()),
            master_rms_left_bits: AtomicU32::new(0.0_f32.to_bits()),
            master_rms_right_bits: AtomicU32::new(0.0_f32.to_bits()),
            frames_rendered: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            control_gain_bits: [const { AtomicU32::new(1.0_f32.to_bits()) }; UI_CHANNELS],
            control_pan_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            control_flags: [const { AtomicU32::new(0) }; UI_CHANNELS],
            channel_peak_left_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            channel_peak_right_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            channel_rms_left_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            channel_rms_right_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            channel_connected: [const { AtomicBool::new(false) }; UI_CHANNELS],
            channel_gain_bits: [const { AtomicU32::new(1.0_f32.to_bits()) }; UI_CHANNELS],
            channel_pan_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            channel_muted: [const { AtomicBool::new(false) }; UI_CHANNELS],
            channel_soloed: [const { AtomicBool::new(false) }; UI_CHANNELS],
            channel_underruns: [const { AtomicU64::new(0) }; UI_CHANNELS],
            channel_sequence_gaps: [const { AtomicU64::new(0) }; UI_CHANNELS],
            channel_name: [const {
                [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]
            }; UI_CHANNELS],
        }
    }

    fn publish_master(&self, peak: StereoLevel, rms: StereoLevel) {
        self.master_peak_left_bits
            .store(peak.left.to_bits(), Ordering::Relaxed);
        self.master_peak_right_bits
            .store(peak.right.to_bits(), Ordering::Relaxed);
        self.master_rms_left_bits
            .store(rms.left.to_bits(), Ordering::Relaxed);
        self.master_rms_right_bits
            .store(rms.right.to_bits(), Ordering::Relaxed);
    }

    fn controls(&self, slot: usize) -> ChannelControls {
        let slot = slot.min(UI_CHANNELS - 1);
        let flags = self.control_flags[slot].load(Ordering::Relaxed);
        ChannelControls {
            gain: f32::from_bits(self.control_gain_bits[slot].load(Ordering::Relaxed)),
            pan: Pan::new(f32::from_bits(self.control_pan_bits[slot].load(Ordering::Relaxed))),
            muted: flags & 0b01 != 0,
            soloed: flags & 0b10 != 0,
        }
    }

    fn store_controls(&self, slot: usize, controls: ChannelControls) {
        if slot >= UI_CHANNELS {
            return;
        }
        let mut flags = 0_u32;
        if controls.muted {
            flags |= 0b01;
        }
        if controls.soloed {
            flags |= 0b10;
        }
        self.control_gain_bits[slot].store(controls.gain.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
        self.control_pan_bits[slot].store(controls.pan.value().to_bits(), Ordering::Relaxed);
        self.control_flags[slot].store(flags, Ordering::Relaxed);
    }

    fn master_peak(&self) -> StereoLevel {
        StereoLevel {
            left: f32::from_bits(self.master_peak_left_bits.load(Ordering::Relaxed)),
            right: f32::from_bits(self.master_peak_right_bits.load(Ordering::Relaxed)),
        }
    }

    fn master_rms(&self) -> StereoLevel {
        StereoLevel {
            left: f32::from_bits(self.master_rms_left_bits.load(Ordering::Relaxed)),
            right: f32::from_bits(self.master_rms_right_bits.load(Ordering::Relaxed)),
        }
    }

    fn publish_channels(&self, snapshots: &[ChannelSnapshot]) {
        for (idx, snapshot) in snapshots.iter().enumerate().take(UI_CHANNELS) {
            self.channel_peak_left_bits[idx].store(snapshot.peak.left.to_bits(), Ordering::Relaxed);
            self.channel_peak_right_bits[idx].store(snapshot.peak.right.to_bits(), Ordering::Relaxed);
            self.channel_rms_left_bits[idx].store(snapshot.rms.left.to_bits(), Ordering::Relaxed);
            self.channel_rms_right_bits[idx].store(snapshot.rms.right.to_bits(), Ordering::Relaxed);
            self.channel_connected[idx].store(snapshot.connected, Ordering::Relaxed);
            self.channel_gain_bits[idx].store(snapshot.gain.to_bits(), Ordering::Relaxed);
            self.channel_pan_bits[idx].store(snapshot.pan.to_bits(), Ordering::Relaxed);
            self.channel_muted[idx].store(snapshot.muted, Ordering::Relaxed);
            self.channel_soloed[idx].store(snapshot.soloed, Ordering::Relaxed);
            self.channel_underruns[idx].store(snapshot.underruns, Ordering::Relaxed);
            self.channel_sequence_gaps[idx].store(snapshot.sequence_gaps, Ordering::Relaxed);
            for word in 0..3 {
                let start = word * 4;
                let chunk = u32::from_le_bytes([
                    snapshot.name[start],
                    snapshot.name[start + 1],
                    snapshot.name[start + 2],
                    snapshot.name[start + 3],
                ]);
                self.channel_name[idx][word].store(chunk, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Debug)]
struct App {
    audio_info: AudioInfo,
    session: MixSession,
    should_quit: bool,
    transport_playing: bool,
    metronome: bool,
    bpm: f64,
    started: Instant,
    frames_rendered: u64,
    stream_errors: u64,
    xruns: u64,
    master_peak: StereoLevel,
    master_rms: StereoLevel,
    selected_channel: usize,
    channels: [UiChannel; UI_CHANNELS],
    control: ControlSnapshot,
}

impl App {
    fn new(audio_info: AudioInfo, session: MixSession) -> Self {
        Self {
            audio_info,
            session,
            should_quit: false,
            transport_playing: false,
            metronome: false,
            bpm: 120.0,
            started: Instant::now(),
            frames_rendered: 0,
            stream_errors: 0,
            xruns: 0,
            master_peak: StereoLevel::ZERO,
            master_rms: StereoLevel::ZERO,
            selected_channel: 0,
            channels: [UiChannel::EMPTY; UI_CHANNELS],
            control: ControlSnapshot {
                accepted_connections: 0,
                accept_errors: 0,
            },
        }
    }

    fn select_previous_channel(&mut self) {
        self.selected_channel = self.selected_channel.saturating_sub(1);
    }

    fn select_next_channel(&mut self) {
        self.selected_channel = (self.selected_channel + 1).min(UI_CHANNELS - 1);
    }

    fn update(&mut self, audio: &MixerAudio, control: ControlSnapshot) {
        self.control = control;
        self.frames_rendered = audio.state.frames_rendered.load(Ordering::Relaxed);
        self.stream_errors = audio.state.stream_errors.load(Ordering::Relaxed);
        self.xruns = audio.state.xruns.load(Ordering::Relaxed);
        self.master_peak = audio.state.master_peak();
        self.master_rms = audio.state.master_rms();
        for idx in 0..UI_CHANNELS {
            self.channels[idx] = UiChannel {
                id: ChannelId(u16::try_from(idx).unwrap_or(u16::MAX)),
                name: load_name(&audio.state.channel_name[idx]),
                connected: audio.state.channel_connected[idx].load(Ordering::Relaxed),
                peak: StereoLevel {
                    left: f32::from_bits(audio.state.channel_peak_left_bits[idx].load(Ordering::Relaxed)),
                    right: f32::from_bits(audio.state.channel_peak_right_bits[idx].load(Ordering::Relaxed)),
                },
                rms: StereoLevel {
                    left: f32::from_bits(audio.state.channel_rms_left_bits[idx].load(Ordering::Relaxed)),
                    right: f32::from_bits(audio.state.channel_rms_right_bits[idx].load(Ordering::Relaxed)),
                },
                gain: f32::from_bits(audio.state.channel_gain_bits[idx].load(Ordering::Relaxed)),
                pan: f32::from_bits(audio.state.channel_pan_bits[idx].load(Ordering::Relaxed)),
                muted: audio.state.channel_muted[idx].load(Ordering::Relaxed),
                soloed: audio.state.channel_soloed[idx].load(Ordering::Relaxed),
                underruns: audio.state.channel_underruns[idx].load(Ordering::Relaxed),
                sequence_gaps: audio.state.channel_sequence_gaps[idx].load(Ordering::Relaxed),
            };
        }
    }
}

fn load_name(words: &[AtomicU32; 3]) -> [u8; 12] {
    let mut out = [0_u8; 12];
    for (idx, word) in words.iter().enumerate() {
        let bytes = word.load(Ordering::Relaxed).to_le_bytes();
        let start = idx * 4;
        out[start..start + 4].copy_from_slice(&bytes);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct UiChannel {
    id: ChannelId,
    name: [u8; 12],
    connected: bool,
    peak: StereoLevel,
    rms: StereoLevel,
    gain: f32,
    pan: f32,
    muted: bool,
    soloed: bool,
    underruns: u64,
    sequence_gaps: u64,
}

impl UiChannel {
    const EMPTY: Self = Self {
        id: ChannelId(0),
        name: [0; 12],
        connected: false,
        peak: StereoLevel::ZERO,
        rms: StereoLevel::ZERO,
        gain: 1.0,
        pan: 0.0,
        muted: false,
        soloed: false,
        underruns: 0,
        sequence_gaps: 0,
    };
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    frame.render_widget(Block::default().style(Style::default().bg(BG)), frame.area());

    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(7),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_console(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let play = if app.transport_playing { "RUN" } else { "STOP" };
    let met = if app.metronome { "CLK ●" } else { "CLK ○" };
    let buffer = app
        .audio_info
        .buffer_size
        .map_or_else(|| "default".to_string(), |frames| frames.to_string());
    let line = Line::from(vec![
        Span::styled(
            " KAZOO MIX ",
            Style::default()
                .fg(BG)
                .bg(BRASS)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            play,
            Style::default().fg(if app.transport_playing { SAGE } else { AMBER }),
        ),
        Span::styled(format!("  {:.2} BPM  {met}  ", app.bpm), Style::default().fg(TEXT)),
        Span::styled(
            format!("{} Hz / {} ch / buffer {}", app.audio_info.sample_rate, app.audio_info.channels, buffer),
            Style::default().fg(TEXT_DIM),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL).style(Style::default().bg(PANEL).fg(STEEL))),
        area,
    );
}

fn draw_console(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = app.channels.iter().enumerate().map(|(idx, channel)| {
        let name = name_string(channel.name);
        let health = if channel.connected { "LIVE" } else { "OPEN" };
        let state = if channel.muted {
            "MUT"
        } else if channel.soloed {
            "SOL"
        } else {
            "---"
        };
        Row::new([
            if idx == app.selected_channel {
                format!("▶CH {:02}", usize::from(channel.id.0) + 1)
            } else {
                format!(" CH {:02}", usize::from(channel.id.0) + 1)
            },
            name,
            health.to_string(),
            state.to_string(),
            format!("{:.2}", channel.gain),
            format_pan(channel.pan),
            format_db(max_stereo(channel.peak)),
            format_db(max_stereo(channel.rms)),
            channel.underruns.to_string(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["Strip", "Source", "I/O", "Bus", "Gain", "Pan", "Peak", "RMS", "Drops"])
            .style(Style::default().fg(BRASS).add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .title(" Console / Channel Strips ")
            .borders(Borders::ALL)
            .style(Style::default().bg(PANEL_ALT).fg(STEEL)),
    )
    .row_highlight_style(Style::default().bg(PANEL));

    frame.render_widget(table, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(50),
        ])
        .split(area);

    draw_meter_block(frame, chunks[0], "Master Peak", app.master_peak);
    draw_meter_block(frame, chunks[1], "Master RMS", app.master_rms);

    let seconds = app.started.elapsed().as_secs_f64();
    let status = vec![
        Line::from(vec![
            Span::styled("session ", Style::default().fg(TEXT_DIM)),
            Span::styled(app.session.runtime_dir.display().to_string(), Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("control ", Style::default().fg(TEXT_DIM)),
            Span::styled(app.session.control_socket.display().to_string(), Style::default().fg(TEXT)),
        ]),
        Line::from(format!(
            "frames {}   runtime {:.1}s   accepts/errors {}/{}",
            app.frames_rendered, app.started.elapsed().as_secs_f64(), app.control.accepted_connections, app.control.accept_errors
        )),
        Line::from(format!(
            "stream errors {}   xruns {}   master pk {}   master rms {}",
            app.stream_errors,
            app.xruns,
            format_db(max_stereo(app.master_peak)),
            format_db(max_stereo(app.master_rms))
        )),
        Line::from(format!(
            "selected CH {:02}   keys: 1-8 select  j/k move  [/] fader  h/l pan  m mute  s solo  c clock  q quit",
            app.selected_channel + 1
        ).fg(TEXT_DIM)),
    ];
    let _ = seconds;
    frame.render_widget(
        Paragraph::new(status)
            .block(Block::default().title(" Machine Room ").borders(Borders::ALL).style(Style::default().bg(PANEL).fg(STEEL))),
        chunks[2],
    );
}

fn draw_meter_block(frame: &mut Frame<'_>, area: Rect, title: &str, level: StereoLevel) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Length(1)])
        .split(area);

    let left = level.left.clamp(0.0, 1.0);
    let right = level.right.clamp(0.0, 1.0);
    let left_label = format!("L {}", format_db(level.left));
    let right_label = format!("R {}", format_db(level.right));

    frame.render_widget(
        Gauge::default()
            .block(Block::default().title(title).borders(Borders::ALL).style(Style::default().bg(PANEL).fg(STEEL)))
            .gauge_style(Style::default().fg(level_color(left)).bg(PANEL))
            .label(left_label)
            .ratio(f64::from(left)),
        chunks[0],
    );
    frame.render_widget(
        Gauge::default()
            .block(Block::default().title(" ").borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM).style(Style::default().bg(PANEL).fg(STEEL)))
            .gauge_style(Style::default().fg(level_color(right)).bg(PANEL))
            .label(right_label)
            .ratio(f64::from(right)),
        chunks[1],
    );
}

fn name_string(bytes: [u8; 12]) -> String {
    let len = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if len == 0 {
        return "empty".to_string();
    }
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

fn format_pan(pan: f32) -> String {
    if pan <= -0.05 {
        format!("L{:02.0}", (-pan * 10.0).round())
    } else if pan >= 0.05 {
        format!("R{:02.0}", (pan * 10.0).round())
    } else {
        "C".to_string()
    }
}

fn format_db(level: f32) -> String {
    if level <= 0.000_001 {
        "-∞ dB".to_string()
    } else {
        format!("{:.1} dB", 20.0 * level.log10())
    }
}

fn level_color(level: f32) -> Color {
    if level >= 0.9 {
        RED
    } else if level >= 0.7 {
        AMBER
    } else {
        SAGE
    }
}

fn max_stereo(level: StereoLevel) -> f32 {
    level.left.max(level.right)
}
