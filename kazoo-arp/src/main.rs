//! kazoo-arp — Jupiter-8 style arpeggiator standalone TUI.
//!
//! The arpeggiator engine lives in `lib.rs` for embedding in other crates.
//! This binary provides a standalone terminal interface with a simple
//! audition synth so arpeggiated notes are audible.

mod app;
mod ipc;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ringbuf::HeapProd;
use ringbuf::traits::{Producer, Split};

use crate::app::{App, DisplayEvent};
use crate::ipc::HubLink;
use kazoo_arp::{ArpClock, Arpeggiator, NoteEvent};

/// Maximum number of frames the audio callback will ever render.
const MAX_CALLBACK_FRAMES: usize = 4096;

/// Target frame rate for the TUI.
const FPS: u64 = 30;

/// Commands from the UI thread to the audio thread.
#[derive(Debug)]
enum AudioCommand {
    NoteOn { midi_note: u8, velocity: u8 },
    NoteOff { midi_note: u8 },
    SetBpm(f32),
    SetSwing(f32),
    SetDivision(kazoo_arp::ClockDivision),
    SetMode(kazoo_arp::ArpMode),
    SetGate(f32),
    SetOctaveRange(u8),
    SetLatch(bool),
}

/// Simple sine audition voice for hearing arpeggiated notes.
struct AuditionVoice {
    sample_rate: f32,
    phase: f32,
    frequency: f32,
    /// Per-sample gain envelope for click-free note on/off.
    gain: f32,
    target_gain: f32,
    /// Smoothing coefficient for envelope.
    smooth: f32,
}

impl AuditionVoice {
    fn new(sample_rate: f32) -> Self {
        // ~5ms attack/release for click-free transitions.
        let smooth = 1.0 - (-1.0 / (sample_rate * 0.005)).exp();
        Self {
            sample_rate,
            phase: 0.0,
            frequency: 0.0,
            gain: 0.0,
            target_gain: 0.0,
            smooth,
        }
    }

    fn note_on(&mut self, midi_note: u8, velocity: u8) {
        self.frequency = 440.0 * ((f32::from(midi_note) - 69.0) / 12.0).exp2();
        self.target_gain = f32::from(velocity) / 127.0;
    }

    const fn note_off(&mut self) {
        self.target_gain = 0.0;
    }

    fn tick(&mut self) -> f32 {
        self.gain += self.smooth * (self.target_gain - self.gain);

        if self.gain < 0.0001 {
            self.gain = 0.0;
            return 0.0;
        }

        let sample = (self.phase * std::f32::consts::TAU).sin();
        self.phase += self.frequency / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        kazoo_core::sanitize_sample(sample * self.gain * 0.3)
    }
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // -----------------------------------------------------------------------
    // Audio setup
    // -----------------------------------------------------------------------

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate() as f32;
    let channels = supported_config.channels() as usize;

    // Command channel: UI -> Audio.
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(256);

    // Display event ring buffer: Audio -> UI (lock-free SPSC).
    let display_rb = ringbuf::HeapRb::<DisplayEvent>::new(256);
    let (display_prod, display_cons) = display_rb.split();

    // Attempt IPC hub connection.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let hub_link = HubLink::new(2, sample_rate as u32, MAX_CALLBACK_FRAMES as u32);
    let hub_connected = hub_link.connected_flag();
    if hub_link.is_connected() {
        eprintln!("Connected to kazoo hub — audio will be routed to mixer");
    }

    // Build audio stream with arp engine + audition voice.
    let stream = build_audio_stream(
        &device,
        &supported_config.into(),
        sample_rate,
        channels,
        cmd_rx,
        hub_link,
        display_prod,
    )?;
    stream.play()?;

    // -----------------------------------------------------------------------
    // Terminal setup
    // -----------------------------------------------------------------------

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // Enable keyboard enhancement (kitty protocol) so we receive
    // KeyEventKind::Release events for note-off.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    );

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(
        &mut terminal,
        sample_rate,
        &cmd_tx,
        hub_connected,
        display_cons,
    );

    // Cleanup.
    drop(stream);
    let _ = execute!(
        terminal.backend_mut(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Build the cpal output stream. Arp + audition voice run here.
#[allow(clippy::too_many_lines)]
fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    mut hub_link: HubLink,
    mut display_prod: HeapProd<DisplayEvent>,
) -> color_eyre::Result<cpal::Stream> {
    let mut arp = Arpeggiator::new();
    let mut clock = ArpClock::new(sample_rate, 120.0);
    clock.start();
    let mut voice = AuditionVoice::new(sample_rate);

    // Pre-allocated scratch buffers for IPC audio send.
    let mut mono_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES];
    let mut stereo_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES * 2];

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Drain commands.
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::NoteOn {
                        midi_note,
                        velocity,
                    } => {
                        arp.note_on(midi_note, velocity);
                    }
                    AudioCommand::NoteOff { midi_note } => {
                        arp.note_off(midi_note);
                    }
                    AudioCommand::SetBpm(bpm) => clock.set_bpm(bpm),
                    AudioCommand::SetSwing(swing) => clock.set_swing(swing),
                    AudioCommand::SetDivision(div) => clock.set_division(div),
                    AudioCommand::SetMode(mode) => arp.set_mode(mode),
                    AudioCommand::SetGate(gate) => arp.set_gate_pct(gate),
                    AudioCommand::SetOctaveRange(range) => arp.set_octave_range(range),
                    AudioCommand::SetLatch(enabled) => {
                        if arp.latch != enabled {
                            arp.toggle_latch();
                        }
                    }
                }
            }

            // Drain messages from the hub (transport sync, note events).
            while let Some(msg) = hub_link.try_recv() {
                match msg {
                    kazoo_core::ipc::client::HubMessage::TransportSync(sync) => {
                        clock.set_bpm(sync.bpm);
                        match sync.state {
                            kazoo_core::ipc::types::TRANSPORT_PLAYING
                            | kazoo_core::ipc::types::TRANSPORT_RECORDING => {
                                clock.start();
                            }
                            kazoo_core::ipc::types::TRANSPORT_STOPPED
                            | kazoo_core::ipc::types::TRANSPORT_PAUSED => {
                                clock.stop();
                            }
                            _ => {}
                        }
                    }
                    kazoo_core::ipc::client::HubMessage::NoteEvent(event) => {
                        match event.event_type {
                            kazoo_core::ipc::types::NOTE_ON => {
                                arp.note_on(event.note, event.velocity);
                            }
                            kazoo_core::ipc::types::NOTE_OFF => {
                                arp.note_off(event.note);
                            }
                            _ => {}
                        }
                    }
                    kazoo_core::ipc::client::HubMessage::ParameterChange(_)
                    | kazoo_core::ipc::client::HubMessage::Shutdown => {}
                }
            }

            // Process sample-by-sample.
            let frames = data.len() / channels;
            let mut frame_idx = 0;
            let mut i = 0;
            while i < data.len() {
                // Tick the arp clock — fires note events.
                let events = clock.tick(&mut arp);
                if let Some(NoteEvent::NoteOn {
                    midi_note,
                    velocity,
                }) = events.note_on
                {
                    voice.note_on(midi_note, velocity);
                    // Push display event to UI thread (best-effort, drop if full).
                    let _ = display_prod.try_push(DisplayEvent { midi_note });
                }
                if let Some(NoteEvent::NoteOff { .. }) = events.note_off {
                    voice.note_off();
                }

                // Generate audition sample.
                let sample = kazoo_core::soft_limit(voice.tick());
                if frame_idx < mono_buf.len() {
                    mono_buf[frame_idx] = sample;
                }
                for ch in 0..channels {
                    if i + ch < data.len() {
                        data[i + ch] = sample;
                    }
                }
                frame_idx += 1;
                i += channels;
            }

            // Send audio to the hub for mixing (if connected).
            if hub_link.is_connected() {
                let process_len = frames.min(mono_buf.len());
                let stereo_len = process_len * 2;
                for (idx, &sample) in mono_buf[..process_len].iter().enumerate() {
                    stereo_buf[idx * 2] = sample;
                    stereo_buf[idx * 2 + 1] = sample;
                }
                #[allow(clippy::cast_possible_truncation)]
                hub_link.send_audio(process_len as u32, &stereo_buf[..stereo_len]);
            }
        },
        |err| {
            eprintln!("audio stream error: {err}");
        },
        None,
    )?;

    Ok(stream)
}

/// Main TUI event loop.
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    sample_rate: f32,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    hub_connected: Arc<AtomicBool>,
    display_cons: ringbuf::HeapCons<DisplayEvent>,
) -> color_eyre::Result<()> {
    let default_bpm = 120.0_f32;
    let mut app = App::new(sample_rate, default_bpm, display_cons, hub_connected);
    let frame_duration = Duration::from_millis(1000 / FPS);

    // Track held keys for note-off.
    let mut held_keys: [bool; 128] = [false; 128];

    loop {
        let frame_start = Instant::now();

        // Drain display events from the audio thread (replaces UI-side simulation).
        app.drain_display_events();

        // Render.
        terminal.draw(|frame| ui::draw(frame, &app))?;

        // Handle input events.
        let elapsed = frame_start.elapsed();
        let poll_time = frame_duration.saturating_sub(elapsed);
        if event::poll(poll_time)? {
            if let Event::Key(key) = event::read()? {
                match key.kind {
                    KeyEventKind::Press | KeyEventKind::Repeat => {
                        handle_key_press(&mut app, key.code, key.modifiers, &mut held_keys, cmd_tx);
                    }
                    KeyEventKind::Release => {
                        handle_key_release(&mut app, key.code, &mut held_keys, cmd_tx);
                    }
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key_press(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    held: &mut [bool; 128],
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    let shift = modifiers.contains(KeyModifiers::SHIFT);

    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Left => app.selected_param = app.selected_param.prev(),
        KeyCode::Right => app.selected_param = app.selected_param.next(),
        KeyCode::Up => {
            app.increment_param(shift);
            sync_param(app, cmd_tx);
        }
        KeyCode::Down => {
            app.decrement_param(shift);
            sync_param(app, cmd_tx);
        }
        KeyCode::Char(' ') => {
            app.arp.toggle_latch();
            let _ = cmd_tx.try_send(AudioCommand::SetLatch(app.arp.latch));
        }
        // Mode shortcuts: 1-5.
        KeyCode::Char('1') => {
            app.arp.set_mode(kazoo_arp::ArpMode::Up);
            let _ = cmd_tx.try_send(AudioCommand::SetMode(kazoo_arp::ArpMode::Up));
        }
        KeyCode::Char('2') => {
            app.arp.set_mode(kazoo_arp::ArpMode::Down);
            let _ = cmd_tx.try_send(AudioCommand::SetMode(kazoo_arp::ArpMode::Down));
        }
        KeyCode::Char('3') => {
            app.arp.set_mode(kazoo_arp::ArpMode::UpDown);
            let _ = cmd_tx.try_send(AudioCommand::SetMode(kazoo_arp::ArpMode::UpDown));
        }
        KeyCode::Char('4') => {
            app.arp.set_mode(kazoo_arp::ArpMode::Random);
            let _ = cmd_tx.try_send(AudioCommand::SetMode(kazoo_arp::ArpMode::Random));
        }
        KeyCode::Char('5') => {
            app.arp.set_mode(kazoo_arp::ArpMode::AsPlayed);
            let _ = cmd_tx.try_send(AudioCommand::SetMode(kazoo_arp::ArpMode::AsPlayed));
        }
        // Piano keys.
        KeyCode::Char(ch) => {
            if let Some(note) = App::key_to_note(ch) {
                if !held[note as usize] {
                    held[note as usize] = true;
                    app.arp.note_on(note, 100);
                    let _ = cmd_tx.try_send(AudioCommand::NoteOn {
                        midi_note: note,
                        velocity: 100,
                    });
                }
            }
        }
        _ => {}
    }
}

fn handle_key_release(
    app: &mut App,
    code: KeyCode,
    held: &mut [bool; 128],
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    if let KeyCode::Char(ch) = code {
        if let Some(note) = App::key_to_note(ch) {
            held[note as usize] = false;
            app.arp.note_off(note);
            let _ = cmd_tx.try_send(AudioCommand::NoteOff { midi_note: note });
        }
    }
}

/// Sync current parameter state to the audio thread after a change.
fn sync_param(app: &App, cmd_tx: &crossbeam_channel::Sender<AudioCommand>) {
    let _ = cmd_tx.try_send(AudioCommand::SetBpm(app.clock.bpm));
    let _ = cmd_tx.try_send(AudioCommand::SetSwing(app.clock.swing));
    let _ = cmd_tx.try_send(AudioCommand::SetDivision(app.clock.division));
    let _ = cmd_tx.try_send(AudioCommand::SetMode(app.arp.mode));
    let _ = cmd_tx.try_send(AudioCommand::SetGate(app.arp.gate_pct));
    let _ = cmd_tx.try_send(AudioCommand::SetOctaveRange(app.arp.octave_range));
    let _ = cmd_tx.try_send(AudioCommand::SetLatch(app.arp.latch));
}
