//! kazoo-808 — TR-808 drum machine.
//!
//! All sounds synthesized, no samples. Connects to kazoo-tui hub via IPC.
//! See `studio/kazoo-808.md` for full specification.

// Under active development — many synth voices and sequencer features
// are implemented but not yet wired to the UI or audio callback.
#![allow(dead_code)]

mod app;
pub mod ipc;
mod sequencer;
mod synth;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;
use ipc::HubLink;
use sequencer::Sequencer;
use synth::{DrumMachine, VoiceIndex, VoiceParam};

/// Maximum number of frames the audio callback will ever render.
/// Pre-allocated scratch buffers use this size.
const MAX_CALLBACK_FRAMES: usize = 4096;

/// Commands sent from the UI thread to the audio thread.
#[derive(Debug)]
enum AudioCommand {
    Play,
    Stop,
    SetBpm(f64),
    SetSwing(f64),
    ToggleStep {
        voice: usize,
        step: usize,
    },
    ToggleAccent {
        voice: usize,
        step: usize,
    },
    TriggerVoice {
        voice: usize,
        velocity: f32,
    },
    SetVoiceParam {
        voice: VoiceIndex,
        param: VoiceParam,
        value: f32,
    },
    SelectPattern(usize),
    AddPattern,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Set up audio output.
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate() as f32;
    let channels = config.channels() as usize;

    // Shared state between audio and UI threads.
    let playback_step = Arc::new(AtomicUsize::new(0));
    let playing = Arc::new(AtomicBool::new(false));

    // Command channel: UI -> Audio.
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(256);

    // Attempt IPC hub connection.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let hub_link = HubLink::new(2, sample_rate as u32, MAX_CALLBACK_FRAMES as u32);
    if hub_link.is_connected() {
        eprintln!("Connected to kazoo hub — audio will be routed to mixer");
    }

    // Build and start the audio stream.
    let playback_step_audio = Arc::clone(&playback_step);
    let playing_audio = Arc::clone(&playing);

    let stream = build_audio_stream(
        &device,
        &config.into(),
        sample_rate,
        channels,
        cmd_rx,
        playback_step_audio,
        playing_audio,
        hub_link,
    )?;
    stream.play()?;

    // Set up terminal.
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the TUI event loop.
    let mut app = App::new(sample_rate);
    let result = run_event_loop(&mut terminal, &mut app, &cmd_tx, &playback_step, &playing);

    // Restore terminal.
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Stop audio.
    drop(stream);

    result
}

/// Build the cpal output stream. All synthesis and sequencing happens here.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    playback_step: Arc<AtomicUsize>,
    playing_flag: Arc<AtomicBool>,
    mut hub_link: HubLink,
) -> color_eyre::Result<cpal::Stream> {
    let mut drum_machine = DrumMachine::new(sample_rate);
    let mut sequencer = Sequencer::new(sample_rate);

    // Pre-allocated scratch buffers for IPC audio send.
    let mut mono_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES];
    let mut stereo_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES * 2];

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Drain commands from the UI thread.
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::Play => {
                        sequencer.play();
                        playing_flag.store(true, Ordering::Release);
                    }
                    AudioCommand::Stop => {
                        sequencer.stop();
                        playing_flag.store(false, Ordering::Release);
                    }
                    AudioCommand::SetBpm(bpm) => sequencer.clock.set_bpm(bpm),
                    AudioCommand::SetSwing(swing) => sequencer.clock.set_swing(swing),
                    AudioCommand::ToggleStep { voice, step } => {
                        sequencer.toggle_step(voice, step);
                    }
                    AudioCommand::ToggleAccent { voice, step } => {
                        sequencer.toggle_accent(voice, step);
                    }
                    AudioCommand::TriggerVoice { voice, velocity } => {
                        if let Some(vi) = VoiceIndex::from_index(voice) {
                            drum_machine.trigger(vi, velocity);
                        }
                    }
                    AudioCommand::SetVoiceParam {
                        voice,
                        param,
                        value,
                    } => {
                        drum_machine.set_voice_param(voice, param, value);
                    }
                    AudioCommand::SelectPattern(idx) => {
                        sequencer.select_pattern(idx);
                    }
                    AudioCommand::AddPattern => {
                        sequencer.add_pattern();
                    }
                }
            }

            // Drain messages from the hub (transport sync, note events).
            while let Some(msg) = hub_link.try_recv() {
                match msg {
                    kazoo_core::ipc::client::HubMessage::TransportSync(sync) => {
                        sequencer.clock.set_bpm(f64::from(sync.bpm));
                        match sync.state {
                            kazoo_core::ipc::types::TRANSPORT_PLAYING
                            | kazoo_core::ipc::types::TRANSPORT_RECORDING => {
                                if !playing_flag.load(Ordering::Acquire) {
                                    sequencer.play();
                                    playing_flag.store(true, Ordering::Release);
                                }
                            }
                            kazoo_core::ipc::types::TRANSPORT_STOPPED
                            | kazoo_core::ipc::types::TRANSPORT_PAUSED => {
                                if playing_flag.load(Ordering::Acquire) {
                                    sequencer.stop();
                                    playing_flag.store(false, Ordering::Release);
                                }
                            }
                            _ => {}
                        }
                    }
                    kazoo_core::ipc::client::HubMessage::NoteEvent(event) => {
                        if event.event_type == kazoo_core::ipc::types::NOTE_ON {
                            let velocity = f32::from(event.velocity) / 127.0;
                            if let Some(vi) = VoiceIndex::from_index(event.note as usize) {
                                drum_machine.trigger(vi, velocity);
                            }
                        }
                    }
                    kazoo_core::ipc::client::HubMessage::ParameterChange(_)
                    | kazoo_core::ipc::client::HubMessage::Shutdown => {}
                }
            }

            // Generate audio sample-by-sample.
            let frames = data.len() / channels;
            let mut frame_idx = 0;
            let mut i = 0;
            while i < data.len() {
                // Advance sequencer (fires triggers into drum machine).
                if let Some(step) = sequencer.tick(&mut drum_machine) {
                    playback_step.store(step, Ordering::Release);
                }

                // Generate one mono sample and duplicate to all channels.
                let sample = kazoo_core::soft_limit(drum_machine.process());
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
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    playback_step: &Arc<AtomicUsize>,
    playing: &Arc<AtomicBool>,
) -> color_eyre::Result<()> {
    loop {
        // Update display state from audio thread.
        app.playback_step = playback_step.load(Ordering::Acquire);
        app.sequencer.playing = playing.load(Ordering::Acquire);

        // Draw.
        terminal.draw(|frame| {
            ui::draw(frame, app);
        })?;

        // Poll for events with a timeout for smooth playback animation.
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(app, key, cmd_tx) {
                    break;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Handle a key event. Returns `true` if the app should quit.
#[allow(clippy::too_many_lines)]
fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }

    // Help overlay: any key dismisses it.
    if app.show_help {
        app.show_help = false;
        return false;
    }

    // Pattern select mode: next key selects pattern or cancels.
    if app.pattern_select_mode {
        app.pattern_select_mode = false;
        if let KeyCode::Char(c @ '0'..='9') = key.code {
            let idx = if c == '0' {
                9
            } else {
                (c as usize) - ('1' as usize)
            };
            if idx < app.sequencer.patterns.len() {
                app.sequencer.select_pattern(idx);
                let _ = cmd_tx.try_send(AudioCommand::SelectPattern(idx));
            }
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return true;
        }

        // Help overlay.
        KeyCode::Char('?') => {
            app.show_help = true;
        }

        // Pattern select mode.
        KeyCode::Char('p') => {
            app.pattern_select_mode = true;
        }

        // New pattern.
        KeyCode::Char('n') => {
            let idx = app.sequencer.add_pattern();
            let _ = cmd_tx.try_send(AudioCommand::AddPattern);
            app.sequencer.select_pattern(idx);
            let _ = cmd_tx.try_send(AudioCommand::SelectPattern(idx));
        }

        // Navigation — Left/Right adjust params in Params focus.
        KeyCode::Left => {
            if let Some((voice, param, value)) = app.cursor_left() {
                let _ = cmd_tx.try_send(AudioCommand::SetVoiceParam {
                    voice,
                    param,
                    value,
                });
            }
        }
        KeyCode::Right => {
            if let Some((voice, param, value)) = app.cursor_right() {
                let _ = cmd_tx.try_send(AudioCommand::SetVoiceParam {
                    voice,
                    param,
                    value,
                });
            }
        }
        KeyCode::Up => app.cursor_up(),
        KeyCode::Down => app.cursor_down(),

        // Toggle step.
        KeyCode::Char(' ') => {
            app.toggle_current_step();
            let _ = cmd_tx.try_send(AudioCommand::ToggleStep {
                voice: app.selected_voice,
                step: app.cursor_step,
            });
        }

        // Toggle accent.
        KeyCode::Char('a') => {
            app.toggle_current_accent();
            let _ = cmd_tx.try_send(AudioCommand::ToggleAccent {
                voice: app.selected_voice,
                step: app.cursor_step,
            });
        }

        // Play/Stop.
        KeyCode::Enter => {
            if app.sequencer.playing {
                app.sequencer.playing = false;
                let _ = cmd_tx.try_send(AudioCommand::Stop);
            } else {
                app.sequencer.playing = true;
                let _ = cmd_tx.try_send(AudioCommand::Play);
            }
        }

        // Tab cycles focus.
        KeyCode::Tab => app.cycle_focus(),

        // Number keys select voice.
        KeyCode::Char(c @ '0'..='9') => {
            app.select_voice_by_key(c);
        }

        // BPM adjust with +/-.
        KeyCode::Char('+' | '=') => {
            let new_bpm = app.sequencer.clock.bpm() + 1.0;
            app.sequencer.clock.set_bpm(new_bpm);
            let _ = cmd_tx.try_send(AudioCommand::SetBpm(app.sequencer.clock.bpm()));
        }
        KeyCode::Char('-') => {
            let new_bpm = app.sequencer.clock.bpm() - 1.0;
            app.sequencer.clock.set_bpm(new_bpm);
            let _ = cmd_tx.try_send(AudioCommand::SetBpm(app.sequencer.clock.bpm()));
        }

        // Trigger selected voice manually (auditioning).
        KeyCode::Char('t') => {
            let _ = cmd_tx.try_send(AudioCommand::TriggerVoice {
                voice: app.selected_voice,
                velocity: 0.8,
            });
        }

        _ => {}
    }

    false
}
