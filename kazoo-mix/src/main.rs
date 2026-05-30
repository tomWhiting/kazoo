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

use kazoo_core::audio_transport::{AudioRingConfig, audio_block_ring};
use kazoo_core::protocol::{BufferId, ChannelId};
use kazoo_mix::control::{ControlServer, ControlSnapshot};
use kazoo_mix::engine::{
    ChannelSnapshot, DEFAULT_CHANNEL_SLOTS, MAX_CALLBACK_SAMPLES, MixerEngine,
};
use kazoo_mix::session::MixSession;
use kazoo_mix::source::EightOhEightSource;
use kazoo_mix::terminal::{MixTerminal, TerminalGuard};

const UI_TICK: Duration = Duration::from_millis(33);
const MAX_EVENTS_PER_FRAME: usize = 8;
const UI_CHANNELS: usize = 8;

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
                handle_key(app, key);
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c' | 'q' | 'd'))
    {
        app.should_quit = true;
        return;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char(' ') => app.transport_playing = !app.transport_playing,
        KeyCode::Char('m' | 'M') => app.metronome = !app.metronome,
        KeyCode::Char('+' | '=') => app.bpm = (app.bpm + 1.0).min(300.0),
        KeyCode::Char('-') => app.bpm = (app.bpm - 1.0).max(20.0),
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

        Ok(Self {
            _stream: stream,
            _source: source,
            state,
            info,
        })
    }

    const fn info(&self) -> AudioInfo {
        self.info
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
    let mut mix = vec![0.0_f32; MAX_CALLBACK_SAMPLES];
    let mut channel_snapshots = [ChannelSnapshot::EMPTY; UI_CHANNELS];
    let render_state = Arc::clone(&state);
    let error_state = state;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
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

            render_state
                .master_peak_bits
                .store(engine.master_peak().to_bits(), Ordering::Relaxed);
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
    master_peak_bits: AtomicU32,
    frames_rendered: AtomicU64,
    stream_errors: AtomicU64,
    xruns: AtomicU64,
    channel_peak_bits: [AtomicU32; UI_CHANNELS],
    channel_connected: [AtomicBool; UI_CHANNELS],
    channel_underruns: [AtomicU64; UI_CHANNELS],
    channel_sequence_gaps: [AtomicU64; UI_CHANNELS],
}

impl AudioCallbackState {
    const fn new() -> Self {
        Self {
            master_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            frames_rendered: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            channel_peak_bits: [const { AtomicU32::new(0.0_f32.to_bits()) }; UI_CHANNELS],
            channel_connected: [const { AtomicBool::new(false) }; UI_CHANNELS],
            channel_underruns: [const { AtomicU64::new(0) }; UI_CHANNELS],
            channel_sequence_gaps: [const { AtomicU64::new(0) }; UI_CHANNELS],
        }
    }

    fn master_peak(&self) -> f32 {
        f32::from_bits(self.master_peak_bits.load(Ordering::Relaxed))
    }

    fn publish_channels(&self, snapshots: &[ChannelSnapshot]) {
        for (idx, snapshot) in snapshots.iter().enumerate().take(UI_CHANNELS) {
            self.channel_peak_bits[idx].store(snapshot.peak.to_bits(), Ordering::Relaxed);
            self.channel_connected[idx].store(snapshot.connected, Ordering::Relaxed);
            self.channel_underruns[idx].store(snapshot.underruns, Ordering::Relaxed);
            self.channel_sequence_gaps[idx].store(snapshot.sequence_gaps, Ordering::Relaxed);
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
    master_peak: f32,
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
            master_peak: 0.0,
            channels: [UiChannel::EMPTY; UI_CHANNELS],
            control: ControlSnapshot {
                accepted_connections: 0,
                accept_errors: 0,
            },
        }
    }

    fn update(&mut self, audio: &MixerAudio, control: ControlSnapshot) {
        self.control = control;
        self.frames_rendered = audio.state.frames_rendered.load(Ordering::Relaxed);
        self.stream_errors = audio.state.stream_errors.load(Ordering::Relaxed);
        self.xruns = audio.state.xruns.load(Ordering::Relaxed);
        self.master_peak = audio.state.master_peak();
        for idx in 0..UI_CHANNELS {
            self.channels[idx] = UiChannel {
                id: ChannelId(u16::try_from(idx).unwrap_or(u16::MAX)),
                connected: audio.state.channel_connected[idx].load(Ordering::Relaxed),
                peak: f32::from_bits(audio.state.channel_peak_bits[idx].load(Ordering::Relaxed)),
                underruns: audio.state.channel_underruns[idx].load(Ordering::Relaxed),
                sequence_gaps: audio.state.channel_sequence_gaps[idx].load(Ordering::Relaxed),
            };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct UiChannel {
    id: ChannelId,
    connected: bool,
    peak: f32,
    underruns: u64,
    sequence_gaps: u64,
}

impl UiChannel {
    const EMPTY: Self = Self {
        id: ChannelId(0),
        connected: false,
        peak: 0.0,
        underruns: 0,
        sequence_gaps: 0,
    };
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(5),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_console(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let play = if app.transport_playing {
        "PLAY"
    } else {
        "STOP"
    };
    let met = if app.metronome { "MET ●" } else { "MET ○" };
    let buffer = app
        .audio_info
        .buffer_size
        .map_or_else(|| "default".to_string(), |frames| frames.to_string());
    let line = Line::from(vec![
        Span::styled(
            " KAZOO MIX ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            play,
            Style::default().fg(if app.transport_playing {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
        Span::raw(format!("  {:.2} BPM  {met}  ", app.bpm)),
        Span::raw(format!(
            "{} Hz / {} ch / buffer {}",
            app.audio_info.sample_rate, app.audio_info.channels, buffer
        )),
    ]);

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_console(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = app.channels.iter().enumerate().map(|(idx, channel)| {
        let name = if idx == 0 && channel.connected {
            "808"
        } else {
            "empty"
        };
        let health = if channel.connected {
            "IN ●"
        } else {
            "IN ○"
        };
        let peak_db = if channel.peak <= 0.000_001 {
            "-∞ dB".to_string()
        } else {
            format!("{:.1} dB", 20.0 * channel.peak.log10())
        };
        Row::new([
            format!("CH {:02}", usize::from(channel.id.0) + 1),
            name.to_string(),
            health.to_string(),
            channel.underruns.to_string(),
            peak_db,
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(["Strip", "Client", "Health", "Underruns", "Peak"])
            .style(Style::default().fg(Color::Cyan)),
    )
    .block(Block::default().title(" Console ").borders(Borders::ALL));

    frame.render_widget(table, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let peak_ratio = app.master_peak.clamp(0.0, 1.0);
    let gauge = Gauge::default()
        .block(Block::default().title(" Master ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(if peak_ratio > 0.9 {
            Color::Red
        } else {
            Color::Green
        }))
        .ratio(f64::from(peak_ratio));
    frame.render_widget(gauge, chunks[0]);

    let seconds = app.started.elapsed().as_secs_f64();
    let status = vec![
        Line::from(format!("frames rendered: {}", app.frames_rendered)),
        Line::from(format!("runtime: {seconds:.1}s")),
        Line::from(format!("session: {}", app.session.runtime_dir.display())),
        Line::from(format!("control: {}", app.session.control_socket.display())),
        Line::from(format!(
            "control accepts/errors: {}/{}",
            app.control.accepted_connections, app.control.accept_errors
        )),
        Line::from(format!(
            "stream errors: {}   xruns: {}",
            app.stream_errors, app.xruns
        )),
        Line::from("keys: Space play/stop  +/- tempo  m metronome  q/Esc quit".dark_gray()),
    ];
    frame.render_widget(
        Paragraph::new(status).block(Block::default().title(" Status ").borders(Borders::ALL)),
        chunks[1],
    );
}
