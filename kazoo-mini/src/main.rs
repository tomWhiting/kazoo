//! kazoo-mini — Moog Minimoog inspired bass/lead synth.
//!
//! Monophonic. Three VCOs, 24 dB/oct ladder filter with nonlinear
//! saturation, rate-based glide, cross-modulation.
//! See `studio/kazoo-mini.md` for full specification.
//!
//! Standalone operation: direct cpal audio output with lock-free
//! command channel (UI -> Audio) and display channel (Audio -> UI).

mod app;
mod input;
mod ipc;
#[allow(dead_code)]
mod synth;
mod ui;

use std::io;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::ipc::HubLink;
use crate::synth::MiniVoice;
use crate::synth::oscillator::{OctaveRange, Waveform};
use crate::synth::xmod::ModWheelDest;

// ---------------------------------------------------------------------------
// Display buffer size for waveform feedback (audio -> UI)
// ---------------------------------------------------------------------------

const DISPLAY_BUF_SIZE: usize = 1024;

/// Maximum number of frames the audio callback will ever be asked to render.
/// Used to pre-allocate the mono scratch buffer. 4096 covers all common
/// buffer sizes (128, 256, 512, 1024, 2048) with generous headroom.
const MAX_CALLBACK_FRAMES: usize = 4096;

// ---------------------------------------------------------------------------
// Audio command channel (UI -> Audio thread)
// ---------------------------------------------------------------------------

/// Commands sent from the UI thread to the audio callback via crossbeam channel.
/// No allocations — all data is inline.
#[derive(Debug)]
enum AudioCommand {
    NoteOn { note: u8 },
    NoteOff { note: u8 },
    UpdateParams(MiniParams),
}

/// Flat parameter snapshot sent from the UI to the audio thread.
///
/// Contains every user-editable parameter value. Sent on every param change.
/// No heap allocations, fits in ~120 bytes.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
struct MiniParams {
    // Oscillators (3 × 5 = 15 fields)
    osc1_waveform: Waveform,
    osc1_octave: OctaveRange,
    osc1_fine_tune: f32,
    osc1_level: f32,

    osc2_waveform: Waveform,
    osc2_octave: OctaveRange,
    osc2_fine_tune: f32,
    osc2_level: f32,

    osc3_waveform: Waveform,
    osc3_octave: OctaveRange,
    osc3_fine_tune: f32,
    osc3_level: f32,
    osc3_lfo_mode: bool,

    // Mixer (5 fields)
    mixer_osc1: f32,
    mixer_osc2: f32,
    mixer_osc3: f32,
    mixer_noise: f32,
    mixer_ext: f32,

    // Filter (5 fields)
    filter_cutoff: f32,
    filter_resonance: f32,
    filter_key_track: f32,
    filter_env_amount: f32,
    filter_drive: f32,

    // Envelopes (8 fields)
    filter_env_attack: f32,
    filter_env_decay: f32,
    filter_env_sustain: f32,
    filter_env_release: f32,

    amp_env_attack: f32,
    amp_env_decay: f32,
    amp_env_sustain: f32,
    amp_env_release: f32,

    // Performance (6 fields)
    glide_rate: f32,
    glide_enabled: bool,
    legato: bool,
    retrigger: bool,

    // Cross-mod (4 fields)
    xmod_osc3_to_osc2_fm: f32,
    xmod_osc2_to_filter: f32,
    xmod_mod_wheel: f32,
    xmod_mod_wheel_dest: ModWheelDest,
}

impl MiniParams {
    /// Capture current parameter state from the UI-side voice.
    const fn from_voice(voice: &MiniVoice) -> Self {
        Self {
            osc1_waveform: voice.osc1.waveform,
            osc1_octave: voice.osc1.octave,
            osc1_fine_tune: voice.osc1.fine_tune_cents,
            osc1_level: voice.osc1.level,

            osc2_waveform: voice.osc2.waveform,
            osc2_octave: voice.osc2.octave,
            osc2_fine_tune: voice.osc2.fine_tune_cents,
            osc2_level: voice.osc2.level,

            osc3_waveform: voice.osc3.waveform,
            osc3_octave: voice.osc3.octave,
            osc3_fine_tune: voice.osc3.fine_tune_cents,
            osc3_level: voice.osc3.level,
            osc3_lfo_mode: voice.osc3.lfo_mode,

            mixer_osc1: voice.mixer.osc1_level,
            mixer_osc2: voice.mixer.osc2_level,
            mixer_osc3: voice.mixer.osc3_level,
            mixer_noise: voice.mixer.noise_level,
            mixer_ext: voice.mixer.ext_level,

            filter_cutoff: voice.filter.base_cutoff,
            filter_resonance: voice.filter.resonance(),
            filter_key_track: voice.filter.key_track,
            filter_env_amount: voice.filter_env_amount,
            filter_drive: voice.filter.drive(),

            filter_env_attack: voice.filter_env.attack,
            filter_env_decay: voice.filter_env.decay,
            filter_env_sustain: voice.filter_env.sustain,
            filter_env_release: voice.filter_env.release,

            amp_env_attack: voice.amp_env.attack,
            amp_env_decay: voice.amp_env.decay,
            amp_env_sustain: voice.amp_env.sustain,
            amp_env_release: voice.amp_env.release,

            glide_rate: voice.glide.rate,
            glide_enabled: voice.glide.enabled,
            legato: voice.legato,
            retrigger: voice.retrigger,

            xmod_osc3_to_osc2_fm: voice.xmod.osc3_to_osc2_fm,
            xmod_osc2_to_filter: voice.xmod.osc2_to_filter,
            xmod_mod_wheel: voice.xmod.mod_wheel,
            xmod_mod_wheel_dest: voice.xmod.mod_wheel_dest,
        }
    }

    /// Apply this parameter snapshot to an audio-thread voice.
    fn apply_to(&self, v: &mut MiniVoice) {
        // Oscillators
        v.osc1.waveform = self.osc1_waveform;
        v.osc1.octave = self.osc1_octave;
        v.osc1.fine_tune_cents = self.osc1_fine_tune;
        v.osc1.level = self.osc1_level;

        v.osc2.waveform = self.osc2_waveform;
        v.osc2.octave = self.osc2_octave;
        v.osc2.fine_tune_cents = self.osc2_fine_tune;
        v.osc2.level = self.osc2_level;

        v.osc3.waveform = self.osc3_waveform;
        v.osc3.octave = self.osc3_octave;
        v.osc3.fine_tune_cents = self.osc3_fine_tune;
        v.osc3.level = self.osc3_level;
        v.osc3.lfo_mode = self.osc3_lfo_mode;

        // Mixer
        v.mixer.osc1_level = self.mixer_osc1;
        v.mixer.osc2_level = self.mixer_osc2;
        v.mixer.osc3_level = self.mixer_osc3;
        v.mixer.noise_level = self.mixer_noise;
        v.mixer.ext_level = self.mixer_ext;

        // Filter
        v.filter.base_cutoff = self.filter_cutoff;
        v.filter.set_cutoff(self.filter_cutoff);
        v.filter.set_resonance(self.filter_resonance);
        v.filter.key_track = self.filter_key_track;
        v.filter.set_drive(self.filter_drive);
        v.filter_env_amount = self.filter_env_amount;

        // Filter envelope
        v.filter_env.attack = self.filter_env_attack;
        v.filter_env.decay = self.filter_env_decay;
        v.filter_env.sustain = self.filter_env_sustain;
        v.filter_env.release = self.filter_env_release;
        v.filter_env.recompute_coefficients();

        // Amp envelope
        v.amp_env.attack = self.amp_env_attack;
        v.amp_env.decay = self.amp_env_decay;
        v.amp_env.sustain = self.amp_env_sustain;
        v.amp_env.release = self.amp_env_release;
        v.amp_env.recompute_coefficients();

        // Performance
        v.glide.rate = self.glide_rate;
        v.glide.enabled = self.glide_enabled;
        v.legato = self.legato;
        v.retrigger = self.retrigger;

        // Cross-mod
        v.xmod.osc3_to_osc2_fm = self.xmod_osc3_to_osc2_fm;
        v.xmod.osc2_to_filter = self.xmod_osc2_to_filter;
        v.xmod.mod_wheel = self.xmod_mod_wheel;
        v.xmod.mod_wheel_dest = self.xmod_mod_wheel_dest;
    }
}

// ---------------------------------------------------------------------------
// Display state (Audio -> UI thread) via lock-free crossbeam channel
// ---------------------------------------------------------------------------

/// Display snapshot sent from the audio callback to the UI thread.
/// Sent via crossbeam channel — no mutexes in the audio path.
struct DisplaySnapshot {
    /// Current MIDI note (None when no note is active).
    current_note: Option<u8>,
    /// Waveform display buffer — circular buffer contents.
    waveform: [f32; DISPLAY_BUF_SIZE],
    /// Write position at time of snapshot (for ring buffer linearization).
    write_pos: usize,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // -----------------------------------------------------------------------
    // Audio setup — direct cpal output, no input stream needed
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

    // Attempt IPC hub connection. If the hub (kazoo-tui) is running, the
    // instrument's audio will be mixed into the hub's master bus. If not,
    // we fall back to standalone mode (local audio output only).
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
    execute!(stdout, EnterAlternateScreen)?;

    // Enable keyboard enhancement (kitty protocol) so we receive
    // KeyEventKind::Release events for note-off. Without this, keys latch.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    );

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // -----------------------------------------------------------------------
    // App state
    // -----------------------------------------------------------------------

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let mut app = App::new(sample_rate as u32);

    // Track which QWERTY keys are currently held (for note-off on key release).
    // Only accessed from the UI thread, so no synchronization needed.
    let mut held_notes = [false; 128];

    // -----------------------------------------------------------------------
    // Main event loop
    // -----------------------------------------------------------------------

    let tick_rate = Duration::from_millis(16); // ~60 FPS

    loop {
        // Drain display snapshots from audio thread, keep only the latest.
        let mut latest_snapshot: Option<DisplaySnapshot> = None;
        while let Ok(snap) = display_rx.try_recv() {
            latest_snapshot = Some(snap);
        }
        if let Some(snap) = latest_snapshot {
            app.voice.set_display_note(snap.current_note);
            app.voice.set_display_write_pos(snap.write_pos);
            let app_display = app.voice.display_samples_mut();
            let copy_len = app_display.len().min(DISPLAY_BUF_SIZE);
            app_display[..copy_len].copy_from_slice(&snap.waveform[..copy_len]);
        }

        terminal.draw(|f| ui::draw(f, &app))?;

        // Poll for events.
        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Check for QWERTY note input first.
                    if let Some(note) = input::key_to_midi_note(key.code) {
                        // Ignore key repeat — only trigger on initial press.
                        if !held_notes[note as usize] {
                            let _ = cmd_tx.try_send(AudioCommand::NoteOn { note });
                            held_notes[note as usize] = true;
                        }
                    } else {
                        // Navigation / parameter adjustment.
                        input::handle_key(&mut app, key.code, key.modifiers);

                        // Sync parameter changes to the audio thread via command channel.
                        let _ = cmd_tx.try_send(AudioCommand::UpdateParams(
                            MiniParams::from_voice(&app.voice),
                        ));
                    }
                } else if key.kind == KeyEventKind::Release {
                    // Note off on key release.
                    if let Some(note) = input::key_to_midi_note(key.code) {
                        if held_notes[note as usize] {
                            let _ = cmd_tx.try_send(AudioCommand::NoteOff { note });
                            held_notes[note as usize] = false;
                        }
                    }
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

    // Stop audio before restoring terminal.
    drop(stream);

    let _ = execute!(
        terminal.backend_mut(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Audio stream builder
// ---------------------------------------------------------------------------

/// Build the cpal output stream. All synthesis happens in the audio callback.
///
/// The `MiniVoice` is owned entirely by this callback closure. No shared
/// ownership, no mutex contention. Commands arrive via crossbeam channel,
/// display snapshots are pushed out via a separate crossbeam channel.
/// The `HubLink` sends audio to the hub (if connected) for mixing.
fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    display_tx: crossbeam_channel::Sender<DisplaySnapshot>,
    mut hub_link: HubLink,
) -> color_eyre::Result<cpal::Stream> {
    let mut voice = MiniVoice::new(sample_rate);

    // Pre-allocated mono scratch buffer for batch processing.
    let mut mono_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES];

    // Pre-allocated stereo interleaved buffer for IPC (hub sends stereo).
    let mut stereo_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES * 2];

    // Display update throttle: push at ~60Hz.
    // At 44.1kHz with 256-sample buffers, that's ~172 callbacks/sec.
    // Update every 3rd callback ≈ 57Hz.
    let mut display_counter: u32 = 0;
    let display_interval: u32 = 3;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Drain commands from the UI thread (non-blocking).
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::NoteOn { note } => voice.note_on(note),
                    AudioCommand::NoteOff { note } => voice.note_off(note),
                    AudioCommand::UpdateParams(params) => params.apply_to(&mut voice),
                }
            }

            // Drain messages from the hub (transport sync, note events).
            while let Some(msg) = hub_link.try_recv() {
                match msg {
                    kazoo_core::ipc::client::HubMessage::TransportSync(_sync) => {
                        // Mini doesn't have tempo-dependent features yet.
                    }
                    kazoo_core::ipc::client::HubMessage::NoteEvent(event) => {
                        match event.event_type {
                            kazoo_core::ipc::types::NOTE_ON => {
                                voice.note_on(event.note);
                            }
                            kazoo_core::ipc::types::NOTE_OFF => {
                                voice.note_off(event.note);
                            }
                            _ => {}
                        }
                    }
                    kazoo_core::ipc::client::HubMessage::ParameterChange(_)
                    | kazoo_core::ipc::client::HubMessage::Shutdown => {}
                }
            }

            // Batch-process audio into the mono scratch buffer.
            let frames = data.len() / channels;
            let process_len = frames.min(mono_buf.len());

            // Zero the section we'll use.
            mono_buf[..process_len].fill(0.0);

            // Process the entire block in one call.
            voice.process_block(&mut mono_buf[..process_len]);

            // Send audio to the hub for mixing (if connected).
            // Convert mono to interleaved stereo for the hub's mixer.
            if hub_link.is_connected() {
                let stereo_len = process_len * 2;
                for (i, &sample) in mono_buf[..process_len].iter().enumerate() {
                    stereo_buf[i * 2] = sample;
                    stereo_buf[i * 2 + 1] = sample;
                }
                #[allow(clippy::cast_possible_truncation)]
                hub_link.send_audio(process_len as u32, &stereo_buf[..stereo_len]);
            }

            // Scatter mono to all output channels.
            // MiniVoice::process_sample already applies soft_limit + sanitize_sample.
            for (frame, &sample) in mono_buf[..process_len].iter().enumerate() {
                let base = frame * channels;
                for ch in 0..channels {
                    if base + ch < data.len() {
                        data[base + ch] = sample;
                    }
                }
            }

            // Push display snapshot to UI at throttled rate (~60Hz).
            display_counter += 1;
            if display_counter >= display_interval {
                display_counter = 0;

                let mut waveform = [0.0_f32; DISPLAY_BUF_SIZE];
                let src = voice.display_samples();
                let copy_len = src.len().min(DISPLAY_BUF_SIZE);
                waveform[..copy_len].copy_from_slice(&src[..copy_len]);

                let snapshot = DisplaySnapshot {
                    current_note: voice.current_note(),
                    waveform,
                    write_pos: voice.display_write_pos(),
                };

                // If the channel is full, just drop this snapshot.
                // The UI drains at ~60Hz so it will clear by the next interval.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `MiniParams::from_voice` captures all editable parameters
    /// and `apply_to` correctly restores them on a fresh voice.
    #[test]
    fn mini_params_round_trip() {
        let mut voice = MiniVoice::new(44100.0);

        // Modify every parameter to non-default values.
        voice.osc1.waveform = Waveform::Square;
        voice.osc1.octave = OctaveRange::Footage16;
        voice.osc1.fine_tune_cents = 7.5;
        voice.osc1.level = 0.42;

        voice.osc2.waveform = Waveform::NarrowPulse;
        voice.osc2.octave = OctaveRange::Footage4;
        voice.osc2.fine_tune_cents = -12.0;
        voice.osc2.level = 0.33;

        voice.osc3.waveform = Waveform::WidePulse;
        voice.osc3.octave = OctaveRange::Footage32;
        voice.osc3.fine_tune_cents = 25.0;
        voice.osc3.level = 0.91;
        voice.osc3.lfo_mode = true;

        voice.mixer.osc1_level = 0.11;
        voice.mixer.osc2_level = 0.22;
        voice.mixer.osc3_level = 0.33;
        voice.mixer.noise_level = 0.44;
        voice.mixer.ext_level = 0.55;

        voice.filter.base_cutoff = 1234.0;
        voice.filter.set_cutoff(1234.0);
        voice.filter.set_resonance(0.77);
        voice.filter.set_drive(2.5);
        voice.filter.key_track = 0.65;
        voice.filter_env_amount = 0.88;

        voice.filter_env.attack = 0.05;
        voice.filter_env.decay = 0.3;
        voice.filter_env.sustain = 0.45;
        voice.filter_env.release = 0.8;

        voice.amp_env.attack = 0.002;
        voice.amp_env.decay = 0.15;
        voice.amp_env.sustain = 0.7;
        voice.amp_env.release = 1.2;

        voice.glide.rate = 42.0;
        voice.glide.enabled = true;
        voice.legato = false;
        voice.retrigger = true;

        voice.xmod.osc3_to_osc2_fm = 0.6;
        voice.xmod.osc2_to_filter = 0.35;
        voice.xmod.mod_wheel = 0.8;
        voice.xmod.mod_wheel_dest = ModWheelDest::Pitch;

        // Capture params.
        let params = MiniParams::from_voice(&voice);

        // Apply to a fresh voice.
        let mut target = MiniVoice::new(44100.0);
        params.apply_to(&mut target);

        // Verify all fields.
        assert_eq!(target.osc1.waveform, Waveform::Square);
        assert_eq!(target.osc1.octave, OctaveRange::Footage16);
        assert!((target.osc1.fine_tune_cents - 7.5).abs() < f32::EPSILON);
        assert!((target.osc1.level - 0.42).abs() < f32::EPSILON);

        assert_eq!(target.osc2.waveform, Waveform::NarrowPulse);
        assert_eq!(target.osc2.octave, OctaveRange::Footage4);
        assert!((target.osc2.fine_tune_cents - (-12.0)).abs() < f32::EPSILON);
        assert!((target.osc2.level - 0.33).abs() < f32::EPSILON);

        assert_eq!(target.osc3.waveform, Waveform::WidePulse);
        assert_eq!(target.osc3.octave, OctaveRange::Footage32);
        assert!((target.osc3.fine_tune_cents - 25.0).abs() < f32::EPSILON);
        assert!((target.osc3.level - 0.91).abs() < f32::EPSILON);
        assert!(target.osc3.lfo_mode);

        assert!((target.mixer.osc1_level - 0.11).abs() < f32::EPSILON);
        assert!((target.mixer.osc2_level - 0.22).abs() < f32::EPSILON);
        assert!((target.mixer.osc3_level - 0.33).abs() < f32::EPSILON);
        assert!((target.mixer.noise_level - 0.44).abs() < f32::EPSILON);
        assert!((target.mixer.ext_level - 0.55).abs() < f32::EPSILON);

        assert!((target.filter.base_cutoff - 1234.0).abs() < f32::EPSILON);
        assert!((target.filter.resonance() - 0.77).abs() < 0.01);
        assert!((target.filter.drive() - 2.5).abs() < f32::EPSILON);
        assert!((target.filter.key_track - 0.65).abs() < f32::EPSILON);
        assert!((target.filter_env_amount - 0.88).abs() < f32::EPSILON);

        assert!((target.filter_env.attack - 0.05).abs() < f32::EPSILON);
        assert!((target.filter_env.decay - 0.3).abs() < f32::EPSILON);
        assert!((target.filter_env.sustain - 0.45).abs() < f32::EPSILON);
        assert!((target.filter_env.release - 0.8).abs() < f32::EPSILON);

        assert!((target.amp_env.attack - 0.002).abs() < f32::EPSILON);
        assert!((target.amp_env.decay - 0.15).abs() < f32::EPSILON);
        assert!((target.amp_env.sustain - 0.7).abs() < f32::EPSILON);
        assert!((target.amp_env.release - 1.2).abs() < f32::EPSILON);

        assert!((target.glide.rate - 42.0).abs() < f32::EPSILON);
        assert!(target.glide.enabled);
        assert!(!target.legato);
        assert!(target.retrigger);

        assert!((target.xmod.osc3_to_osc2_fm - 0.6).abs() < f32::EPSILON);
        assert!((target.xmod.osc2_to_filter - 0.35).abs() < f32::EPSILON);
        assert!((target.xmod.mod_wheel - 0.8).abs() < f32::EPSILON);
        assert_eq!(target.xmod.mod_wheel_dest, ModWheelDest::Pitch);
    }

    /// Verify that applying params doesn't disrupt an active note.
    #[test]
    fn apply_params_preserves_audio_state() {
        let mut voice = MiniVoice::new(44100.0);
        voice.note_on(60);

        // Process some audio.
        let mut buf = vec![0.0; 441]; // 10ms
        voice.process_block(&mut buf);
        let pre_max = buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            pre_max > 0.01,
            "voice should produce output before param change"
        );

        // Change a parameter via the MiniParams pipeline.
        let mut params = MiniParams::from_voice(&voice);
        params.filter_cutoff = 800.0;
        params.osc2_level = 0.5;
        params.apply_to(&mut voice);

        // Process more audio — should still be producing sound.
        let mut buf = vec![0.0; 441];
        voice.process_block(&mut buf);
        let post_max = buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            post_max > 0.01,
            "voice should still produce output after param change, got {post_max}"
        );
    }

    /// Verify MiniParams::from_voice captures defaults correctly.
    #[test]
    fn default_params_capture() {
        let voice = MiniVoice::new(48000.0);
        let params = MiniParams::from_voice(&voice);

        // Check known defaults from MiniVoice::new.
        assert_eq!(params.osc1_waveform, Waveform::Saw);
        assert_eq!(params.osc1_octave, OctaveRange::Footage8);
        assert!((params.osc1_level - 0.8).abs() < f32::EPSILON);
        assert!((params.osc2_fine_tune - 2.0).abs() < f32::EPSILON); // slight detune
        assert_eq!(params.osc3_waveform, Waveform::Triangle);
        assert!(!params.osc3_lfo_mode);
        assert!(params.legato); // default legato on
        assert!(!params.retrigger); // default retrigger off
    }

    /// Verify display channel snapshot round-trip.
    #[test]
    fn display_snapshot_channel() {
        let (tx, rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);

        let mut waveform = [0.0_f32; DISPLAY_BUF_SIZE];
        waveform[0] = 0.5;
        waveform[1] = -0.3;

        let snap = DisplaySnapshot {
            current_note: Some(60),
            waveform,
            write_pos: 512,
        };

        tx.try_send(snap).unwrap();

        let received = rx.try_recv().unwrap();
        assert_eq!(received.current_note, Some(60));
        assert!((received.waveform[0] - 0.5).abs() < f32::EPSILON);
        assert!((received.waveform[1] - (-0.3)).abs() < f32::EPSILON);
        assert_eq!(received.write_pos, 512);
    }
}
