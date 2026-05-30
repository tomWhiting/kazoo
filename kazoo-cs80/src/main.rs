//! kazoo-cs80 — Yamaha CS-80 inspired pad synth.
//!
//! 8-voice polyphonic, dual-layer per voice, per-voice analog drift.
//! Also the home for generative/modular synthesis (node graph patching).
//! See `studio/kazoo-cs80.md` for full specification.

mod app;
mod input;
pub mod ipc;
pub mod modular;
pub mod synth;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::ExecutableCommand;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::ipc::HubLink;
use crate::synth::{Cs80Synth, NUM_VOICES, SynthParams, VoiceStatus};

use crate::app::WAVEFORM_BUF_SIZE;

/// Target frame rate for the TUI.
const TARGET_FPS: u64 = 30;

/// Maximum number of frames the audio callback will ever render.
const MAX_CALLBACK_FRAMES: usize = 4096;

/// Display snapshot sent from the audio callback to the UI thread.
///
/// Sent via crossbeam channel (lock-free bounded SPSC) — no mutexes in
/// the audio path.
struct DisplaySnapshot {
    voice_status: [VoiceStatus; NUM_VOICES],
    waveform: [f32; WAVEFORM_BUF_SIZE],
}

/// Commands sent from the UI thread to the audio thread.
#[derive(Debug)]
enum AudioCommand {
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    Aftertouch { note: u8, pressure: f32 },
    UpdateParams(SynthParams),
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // -----------------------------------------------------------------------
    // Audio setup — direct cpal output
    // -----------------------------------------------------------------------

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate() as f32;
    let channels = supported_config.channels() as usize;

    // Command channel: UI -> Audio (lock-free bounded MPSC).
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(256);

    // Display channel: Audio -> UI (lock-free bounded SPSC).
    // Capacity 2: audio writes latest snapshot, UI drains and keeps last.
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);

    // Attempt IPC hub connection.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let hub_link = HubLink::new(2, sample_rate as u32, MAX_CALLBACK_FRAMES as u32);
    if hub_link.is_connected() {
        eprintln!("Connected to kazoo hub — audio will be routed to mixer");
    }

    // Build and start the audio stream.
    let stream = build_audio_stream(
        &device,
        &supported_config.into(),
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
        hub_link,
    )?;
    stream.play()?;

    // -----------------------------------------------------------------------
    // Terminal setup
    // -----------------------------------------------------------------------

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;

    // Enable keyboard enhancement (kitty protocol) so we receive
    // KeyEventKind::Release events. Without this, note-off never fires
    // and keys latch permanently.
    let _ = stdout.execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::REPORT_EVENT_TYPES,
    ));

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // -----------------------------------------------------------------------
    // App state + event loop
    // -----------------------------------------------------------------------

    let mut app = App::new(sample_rate);
    let frame_duration = Duration::from_millis(1000 / TARGET_FPS);

    loop {
        let frame_start = Instant::now();

        // Drain display snapshots from audio thread, keep only the latest.
        let mut latest_snapshot: Option<DisplaySnapshot> = None;
        while let Ok(snap) = display_rx.try_recv() {
            latest_snapshot = Some(snap);
        }
        if let Some(snap) = latest_snapshot {
            app.voice_status = snap.voice_status;
            app.waveform_buf.copy_from_slice(&snap.waveform);
        }
        app.frame += 1;

        // Draw.
        terminal.draw(|f| ui::draw(f, &app))?;

        // Handle input events.
        let timeout = frame_duration.saturating_sub(frame_start.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.shift_held = key.modifiers.contains(KeyModifiers::SHIFT);
                    handle_key(&mut app, key.code, key.modifiers, &cmd_tx);
                } else if key.kind == KeyEventKind::Release {
                    handle_key_release(&mut app, key.code, &cmd_tx);
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // -----------------------------------------------------------------------
    // Cleanup
    // -----------------------------------------------------------------------

    drop(stream);
    let _ = io::stdout().execute(crossterm::event::PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

/// Build the cpal output stream. All synthesis happens in the audio callback.
///
/// Display state is sent to the UI thread via a lock-free crossbeam channel
/// instead of a Mutex, following the same pattern as kazoo-mini.
fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    display_tx: crossbeam_channel::Sender<DisplaySnapshot>,
    mut hub_link: HubLink,
) -> color_eyre::Result<cpal::Stream> {
    let mut synth = Cs80Synth::new(sample_rate);

    // Pre-allocated scratch buffers for IPC audio send.
    let mut mono_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES];
    let mut stereo_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES * 2];

    // Display update throttle: push at ~60Hz.
    // At 44.1kHz with 256-sample buffers, that's ~172 callbacks/sec.
    // Update every 3rd callback ≈ 57Hz.
    let mut display_counter: u32 = 0;
    let display_interval: u32 = 3;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Drain commands from the UI thread.
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::NoteOn { note, velocity } => {
                        synth.note_on(note, velocity);
                    }
                    AudioCommand::NoteOff { note } => {
                        synth.note_off(note);
                    }
                    AudioCommand::Aftertouch { note, pressure } => {
                        synth.aftertouch(note, pressure);
                    }
                    AudioCommand::UpdateParams(params) => {
                        synth.params = params;
                        synth.apply_params();
                    }
                }
            }

            // Drain messages from the hub (transport sync, note events).
            while let Some(msg) = hub_link.try_recv() {
                match msg {
                    kazoo_core::ipc::client::HubMessage::TransportSync(_sync) => {
                        // CS-80 doesn't have tempo-synced modulation yet.
                    }
                    kazoo_core::ipc::client::HubMessage::NoteEvent(event) => {
                        match event.event_type {
                            kazoo_core::ipc::types::NOTE_ON => {
                                let velocity = f32::from(event.velocity) / 127.0;
                                synth.note_on(event.note, velocity);
                            }
                            kazoo_core::ipc::types::NOTE_OFF => {
                                synth.note_off(event.note);
                            }
                            _ => {}
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
                let sample = synth.tick();
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

            // Push display snapshot to UI at throttled rate (~60Hz).
            display_counter += 1;
            if display_counter >= display_interval {
                display_counter = 0;

                let mut waveform = [0.0_f32; WAVEFORM_BUF_SIZE];
                let history = synth.output_history_linearized();
                let copy_len = history.len().min(WAVEFORM_BUF_SIZE);
                waveform[..copy_len].copy_from_slice(&history[..copy_len]);

                let snapshot = DisplaySnapshot {
                    voice_status: synth.voice_status(),
                    waveform,
                };

                // If the channel is full, just drop this snapshot.
                let _ = display_tx.try_send(snapshot);
            }
        },
        |err| {
            eprintln!("audio stream error: {err}");
        },
        None,
    )?;

    Ok(stream)
}

/// Handle a key press event.
fn handle_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let shift = modifiers.contains(KeyModifiers::SHIFT);

    match code {
        // Quit.
        KeyCode::Char('`') | KeyCode::Esc => app.should_quit = true,

        // Toggle modular view (F2).
        KeyCode::F(2) => app.toggle_view(),

        // Preset save/load (Ctrl+S / Ctrl+L).
        KeyCode::Char('s') if ctrl => save_preset(app),
        KeyCode::Char('l') if ctrl => load_preset(app, cmd_tx),

        // Section navigation.
        KeyCode::Tab => app.next_section(),
        KeyCode::BackTab => app.prev_section(),

        // Aftertouch (Shift+Up/Down).
        KeyCode::Up if shift => {
            let pressure = app.increase_aftertouch();
            send_aftertouch_for_held_notes(app, pressure, cmd_tx);
        }
        KeyCode::Down if shift => {
            let pressure = app.decrease_aftertouch();
            send_aftertouch_for_held_notes(app, pressure, cmd_tx);
        }

        // Parameter navigation (arrow keys only — j/k are musical keys).
        KeyCode::Up => app.prev_param(),
        KeyCode::Down => app.next_param(),

        // Parameter adjustment (Shift+arrow = coarse via app.shift_held).
        KeyCode::Char('+' | '=') | KeyCode::Right => {
            app.increment_param();
            let _ = cmd_tx.try_send(AudioCommand::UpdateParams(app.synth.params.clone()));
        }
        KeyCode::Char('-' | '_') | KeyCode::Left => {
            app.decrement_param();
            let _ = cmd_tx.try_send(AudioCommand::UpdateParams(app.synth.params.clone()));
        }

        // Octave shift.
        KeyCode::Char('[') => app.octave_down(),
        KeyCode::Char(']') => app.octave_up(),

        // Musical keyboard — note on.
        KeyCode::Char(ch) => {
            if let Some(note) = input::key_to_note(ch, app.octave) {
                // Ignore key repeat — only trigger note_on on the initial press.
                // Without this guard, held keys fire repeated note_on messages
                // which causes the synth to "build up" or re-trigger.
                let ascii = ch as u32;
                if ascii < 128 && app.key_note_map[ascii as usize].is_some() {
                    return;
                }
                if ascii < 128 {
                    app.key_note_map[ascii as usize] = Some(note);
                }
                app.note_on(note, input::DEFAULT_VELOCITY);
                let _ = cmd_tx.try_send(AudioCommand::NoteOn {
                    note,
                    velocity: input::DEFAULT_VELOCITY,
                });
            }
        }

        _ => {}
    }
}

/// Send aftertouch for all currently held notes.
fn send_aftertouch_for_held_notes(
    app: &App,
    pressure: f32,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    for (note, &held) in app.held_notes.iter().enumerate() {
        if held {
            #[allow(clippy::cast_possible_truncation)]
            let _ = cmd_tx.try_send(AudioCommand::Aftertouch {
                note: note as u8,
                pressure,
            });
        }
    }
}

/// Save current synth params to a preset file.
fn save_preset(app: &App) {
    let dir = preset_dir();
    if let Ok(json) = serde_json::to_string_pretty(&app.synth.params) {
        let path = dir.join("last_preset.json");
        let _ = std::fs::write(path, json);
    }
}

/// Load synth params from a preset file.
fn load_preset(app: &mut App, cmd_tx: &crossbeam_channel::Sender<AudioCommand>) {
    let dir = preset_dir();
    let path = dir.join("last_preset.json");
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(params) = serde_json::from_str::<SynthParams>(&data) {
            app.synth.params = params;
            app.synth.apply_params();
            let _ = cmd_tx.try_send(AudioCommand::UpdateParams(app.synth.params.clone()));
        }
    }
}

/// Get the preset directory, creating it if needed.
fn preset_dir() -> std::path::PathBuf {
    let base = std::env::var("HOME")
        .map_or_else(|_| std::path::PathBuf::from("."), std::path::PathBuf::from);
    let dir = base.join(".config").join("kazoo-cs80").join("presets");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Handle a key release event (for note-off).
///
/// Uses the stored `key_note_map` to find the MIDI note that was triggered
/// when this key was originally pressed. This prevents stuck notes when the
/// octave is changed while a key is held — the release sends note-off for
/// the original note, not the one the key would map to at the new octave.
fn handle_key_release(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    if let KeyCode::Char(ch) = code {
        let ascii = ch as u32;
        if ascii < 128 {
            if let Some(note) = app.key_note_map[ascii as usize].take() {
                app.note_off(note);
                let _ = cmd_tx.try_send(AudioCommand::NoteOff { note });
            }
        }
    }
}
