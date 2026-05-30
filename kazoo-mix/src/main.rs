//! kazoo-mix — terminal studio mixer.
//!
//! This binary owns the local audio device, keeps callback-safe mixer/control
//! state, and delegates the desk rendering to `mixer_ui`.

mod mixer_ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use color_eyre::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

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
pub(crate) const UI_CHANNELS: usize = 8;

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
        terminal.draw(|frame| mixer_ui::draw(frame, app))?;

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
        KeyCode::Char(ch @ '1'..='8') => {
            app.selected_channel = (ch as usize - '1' as usize).min(UI_CHANNELS - 1);
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AudioInfo {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) buffer_size: Option<u32>,
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
        audio.store_controls(
            0,
            ChannelControls {
                gain: 0.95,
                pan: Pan::CENTER,
                muted: false,
                soloed: false,
            },
        );
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
            channel_name: [const { [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)] };
                UI_CHANNELS],
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
            pan: Pan::new(f32::from_bits(
                self.control_pan_bits[slot].load(Ordering::Relaxed),
            )),
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
        self.control_gain_bits[slot]
            .store(controls.gain.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
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
            self.channel_peak_right_bits[idx]
                .store(snapshot.peak.right.to_bits(), Ordering::Relaxed);
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
pub(crate) struct App {
    pub(crate) audio_info: AudioInfo,
    pub(crate) session: MixSession,
    should_quit: bool,
    pub(crate) transport_playing: bool,
    pub(crate) metronome: bool,
    pub(crate) bpm: f64,
    pub(crate) started: Instant,
    pub(crate) frames_rendered: u64,
    pub(crate) stream_errors: u64,
    pub(crate) xruns: u64,
    pub(crate) master_peak: StereoLevel,
    pub(crate) master_rms: StereoLevel,
    pub(crate) selected_channel: usize,
    pub(crate) channels: [UiChannel; UI_CHANNELS],
    pub(crate) control: ControlSnapshot,
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
                    left: f32::from_bits(
                        audio.state.channel_peak_left_bits[idx].load(Ordering::Relaxed),
                    ),
                    right: f32::from_bits(
                        audio.state.channel_peak_right_bits[idx].load(Ordering::Relaxed),
                    ),
                },
                rms: StereoLevel {
                    left: f32::from_bits(
                        audio.state.channel_rms_left_bits[idx].load(Ordering::Relaxed),
                    ),
                    right: f32::from_bits(
                        audio.state.channel_rms_right_bits[idx].load(Ordering::Relaxed),
                    ),
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
pub(crate) struct UiChannel {
    pub(crate) id: ChannelId,
    pub(crate) name: [u8; 12],
    pub(crate) connected: bool,
    pub(crate) peak: StereoLevel,
    pub(crate) rms: StereoLevel,
    pub(crate) gain: f32,
    pub(crate) pan: f32,
    pub(crate) muted: bool,
    pub(crate) soloed: bool,
    pub(crate) underruns: u64,
    pub(crate) sequence_gaps: u64,
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
