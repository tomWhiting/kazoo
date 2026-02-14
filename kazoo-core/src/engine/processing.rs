//! Processing thread: the real-time audio workhorse.
//!
//! This thread runs in a tight loop, draining commands, reading microphone
//! input from the ring buffer, running the mixer, writing output to the
//! ring buffer, and pushing display snapshots for the UI.

use std::time::Instant;

use crossbeam_channel::Receiver;
use ringbuf::traits::{Consumer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::analysis::{EnvelopeFollower, PitchEstimate};
use crate::mixer::Mixer;
use crate::synthesis::SynthesisMode;
use crate::transport::TransportClock;
use crate::{Db, sanitize_buffer};

use super::command::EngineCommand;
use super::display::DisplayState;

/// Create a synthesis processor for the given mode and sample rate.
///
/// This factory function centralises the mapping from [`SynthesisMode`] to a
/// concrete `Box<dyn Processor>` so it can be reused in `AddTrack` and
/// `SetTrackSynthesisMode` commands.
#[must_use]
pub fn create_synth(mode: SynthesisMode, sample_rate: f32) -> Box<dyn crate::Processor> {
    match mode {
        SynthesisMode::PitchTracked => {
            Box::new(crate::synthesis::PitchTrackedSynth::new(sample_rate))
        }
        SynthesisMode::Wavetable => {
            Box::new(crate::synthesis::WavetableOscillator::new(sample_rate))
        }
        SynthesisMode::Granular => Box::new(crate::synthesis::GranularSynth::new(sample_rate)),
        SynthesisMode::Vocoder => Box::new(crate::synthesis::Vocoder::new(sample_rate)),
        SynthesisMode::PhaseVocoder => Box::new(crate::synthesis::PhaseVocoder::new(sample_rate)),
    }
}

/// State bundle owned by the processing thread.
///
/// Kept as a separate struct so the thread body remains a simple loop and
/// state can be passed to helper functions without a massive parameter list.
struct ProcessingState {
    transport: TransportClock,
    mixer: Mixer,
    envelope: EnvelopeFollower,
    latest_pitch: PitchEstimate,
    latest_spectrum: Vec<f32>,
    latest_formants: Option<crate::analysis::FormantData>,
    is_recording: bool,
    sample_rate: u32,
    buffer_size: usize,
    mic_block: Vec<f32>,
    waveform_snapshot: Vec<f32>,
}

impl ProcessingState {
    fn new(sample_rate: u32, buffer_size: usize) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let sr_f32 = sample_rate as f32;
        let mut mixer = Mixer::new();
        mixer.prepare(sr_f32, buffer_size);

        Self {
            transport: TransportClock::new(sample_rate),
            mixer,
            envelope: EnvelopeFollower::new(5.0, 50.0, sr_f32),
            latest_pitch: PitchEstimate {
                frequency: None,
                voiced_probability: 0.0,
                midi_note: None,
            },
            latest_spectrum: Vec::new(),
            latest_formants: None,
            is_recording: false,
            sample_rate,
            buffer_size,
            mic_block: vec![0.0; buffer_size],
            waveform_snapshot: Vec::with_capacity(buffer_size),
        }
    }
}

/// All ring buffer handles and channels used by the processing thread.
struct ProcessingIO {
    mic_cons: HeapCons<f32>,
    output_prod: HeapProd<f32>,
    display_prod: HeapProd<DisplayState>,
    analysis_prod: HeapProd<f32>,
    disk_prod: HeapProd<f32>,
    pitch_cons: HeapCons<PitchEstimate>,
    spectrum_cons: HeapCons<Vec<f32>>,
    formant_cons: HeapCons<Option<crate::analysis::FormantData>>,
    command_rx: Receiver<EngineCommand>,
}

/// Entry point for the processing thread.
///
/// This function runs until a `Shutdown` command is received or the command
/// channel is disconnected. It is designed to be called from
/// `std::thread::spawn`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    command_rx: Receiver<EngineCommand>,
    mic_cons: HeapCons<f32>,
    output_prod: HeapProd<f32>,
    display_prod: HeapProd<DisplayState>,
    analysis_prod: HeapProd<f32>,
    disk_prod: HeapProd<f32>,
    pitch_cons: HeapCons<PitchEstimate>,
    spectrum_cons: HeapCons<Vec<f32>>,
    formant_cons: HeapCons<Option<crate::analysis::FormantData>>,
    sample_rate: u32,
    buffer_size: usize,
) {
    let mut state = ProcessingState::new(sample_rate, buffer_size);
    let mut io = ProcessingIO {
        mic_cons,
        output_prod,
        display_prod,
        analysis_prod,
        disk_prod,
        pitch_cons,
        spectrum_cons,
        formant_cons,
        command_rx,
    };

    loop {
        let block_start = Instant::now();

        if drain_commands(&io, &mut state) {
            break;
        }

        let num_read = read_mic_input(&mut io, &mut state);
        let num_samples = if num_read == 0 {
            state.buffer_size
        } else {
            num_read
        };

        feed_analysis(&mut io, &state, num_read);
        drain_analysis_results(&mut io, &mut state);

        let input_level_db = compute_input_level(&mut state, num_read);
        advance_transport(&mut state, num_samples);
        let master_slice_len = run_mixer_and_output(&mut io, &mut state, num_samples);
        feed_disk(&mut io, &state, master_slice_len);
        capture_waveform(&mut state, num_read);
        let cpu_load = compute_cpu_load(block_start, num_samples, state.sample_rate);

        push_display_state(&mut io, &state, input_level_db, cpu_load);
        state.mixer.reset_meters();

        if num_read == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// Drain all pending commands. Returns `true` if shutdown was requested.
fn drain_commands(io: &ProcessingIO, state: &mut ProcessingState) -> bool {
    loop {
        match io.command_rx.try_recv() {
            Ok(cmd) => {
                if matches!(cmd, EngineCommand::Shutdown) {
                    return true;
                }
                apply_command(cmd, state);
            }
            Err(crossbeam_channel::TryRecvError::Empty) => return false,
            Err(crossbeam_channel::TryRecvError::Disconnected) => return true,
        }
    }
}

/// Read mic samples from the ring buffer into the state's mic block.
/// Returns the number of samples actually read.
fn read_mic_input(io: &mut ProcessingIO, state: &mut ProcessingState) -> usize {
    let num_read = io.mic_cons.pop_slice(&mut state.mic_block);
    for sample in &mut state.mic_block[num_read..] {
        *sample = 0.0;
    }
    sanitize_buffer(&mut state.mic_block[..num_read]);
    num_read
}

/// Feed raw mic samples to the analysis thread's ring buffer.
fn feed_analysis(io: &mut ProcessingIO, state: &ProcessingState, num_read: usize) {
    if num_read > 0 {
        let _ = io.analysis_prod.push_slice(&state.mic_block[..num_read]);
    }
}

/// Drain analysis results (pitch, spectrum, formants) from ring buffers.
fn drain_analysis_results(io: &mut ProcessingIO, state: &mut ProcessingState) {
    while let Some(pitch) = io.pitch_cons.try_pop() {
        state.latest_pitch = pitch;
    }
    while let Some(spectrum) = io.spectrum_cons.try_pop() {
        state.latest_spectrum = spectrum;
    }
    while let Some(formant_data) = io.formant_cons.try_pop() {
        state.latest_formants = formant_data;
    }
}

/// Compute the input signal level in dB via the envelope follower.
fn compute_input_level(state: &mut ProcessingState, num_read: usize) -> f32 {
    let linear = if num_read > 0 {
        state.envelope.process_block(&state.mic_block[..num_read])
    } else {
        state.envelope.current()
    };
    Db::from_linear(linear).value()
}

/// Advance the transport clock by the given number of samples.
fn advance_transport(state: &mut ProcessingState, num_samples: usize) {
    let n = u32::try_from(num_samples).unwrap_or(u32::MAX);
    state.transport.advance(n);
}

/// Run the mixer, write output to ring buffer, return stereo slice length.
fn run_mixer_and_output(
    io: &mut ProcessingIO,
    state: &mut ProcessingState,
    num_samples: usize,
) -> usize {
    state
        .mixer
        .process(&state.mic_block[..num_samples], num_samples);
    let master_buf = state.mixer.master_buffer();
    let stereo_len = (num_samples * 2).min(master_buf.len());
    let _ = io.output_prod.push_slice(&master_buf[..stereo_len]);
    stereo_len
}

/// Feed interleaved stereo output to the disk recorder ring buffer.
fn feed_disk(io: &mut ProcessingIO, state: &ProcessingState, stereo_len: usize) {
    if state.is_recording {
        let master_buf = state.mixer.master_buffer();
        let len = stereo_len.min(master_buf.len());
        let _ = io.disk_prod.push_slice(&master_buf[..len]);
    }
}

/// Capture a downsampled waveform snapshot for the oscilloscope display.
fn capture_waveform(state: &mut ProcessingState, num_read: usize) {
    state.waveform_snapshot.clear();
    let max_len = 256;
    if num_read > 0 {
        let step = (num_read / max_len).max(1);
        let mut i = 0;
        while i < num_read && state.waveform_snapshot.len() < max_len {
            state.waveform_snapshot.push(state.mic_block[i]);
            i += step;
        }
    }
}

/// Estimate CPU load as the ratio of processing time to audio buffer duration.
fn compute_cpu_load(block_start: Instant, num_samples: usize, sample_rate: u32) -> f32 {
    let elapsed = block_start.elapsed();
    let n = u32::try_from(num_samples).unwrap_or(u32::MAX);
    let budget = std::time::Duration::from_secs_f64(f64::from(n) / f64::from(sample_rate.max(1)));
    if budget.as_nanos() == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = elapsed.as_nanos() as f64 / budget.as_nanos() as f64;
    #[allow(clippy::cast_possible_truncation)]
    let load = ratio as f32;
    if load.is_finite() {
        load.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Build and push a display state snapshot to the UI ring buffer.
fn push_display_state(
    io: &mut ProcessingIO,
    state: &ProcessingState,
    input_level_db: f32,
    cpu_load: f32,
) {
    let display = DisplayState {
        transport: state.transport.snapshot(),
        mixer: state.mixer.snapshot(),
        pitch: state.latest_pitch,
        spectrum_magnitudes: state.latest_spectrum.clone(),
        waveform: state.waveform_snapshot.clone(),
        input_level_db,
        is_recording: state.is_recording,
        formants: state.latest_formants.clone(),
        cpu_load,
    };
    let _ = io.display_prod.try_push(display);
}

// ---------------------------------------------------------------------------
// Command application (split into sub-functions for line count compliance)
// ---------------------------------------------------------------------------

/// Apply a single engine command to the processing state.
fn apply_command(cmd: EngineCommand, state: &mut ProcessingState) {
    match cmd {
        EngineCommand::Shutdown => {}
        EngineCommand::Transport(c) => state.transport.apply_command(c),
        EngineCommand::AddTrack {
            name,
            synthesis_mode,
        } => {
            apply_add_track(state, name, synthesis_mode);
        }
        EngineCommand::RemoveTrack(id) => {
            state.mixer.remove_track(id);
        }
        EngineCommand::SetTrackVolume(id, db) => {
            if let Some(t) = state.mixer.track_mut(id) {
                t.set_volume(db);
            }
        }
        EngineCommand::SetTrackPan(id, pan) => {
            if let Some(t) = state.mixer.track_mut(id) {
                t.set_pan(pan);
            }
        }
        EngineCommand::SetTrackMute(id, m) => {
            if let Some(t) = state.mixer.track_mut(id) {
                t.set_muted(m);
            }
        }
        EngineCommand::SetTrackSolo(id, s) => {
            if let Some(t) = state.mixer.track_mut(id) {
                t.set_soloed(s);
            }
        }
        EngineCommand::SetTrackArm(id, a) => {
            if let Some(t) = state.mixer.track_mut(id) {
                t.set_armed(a);
            }
        }
        EngineCommand::SetTrackSynthesisMode(id, mode) => {
            apply_set_synth_mode(state, id, mode);
        }
        EngineCommand::AddEffect { track_id, effect } => {
            apply_add_effect(state, track_id, effect);
        }
        EngineCommand::RemoveEffect {
            track_id,
            effect_index,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                t.effects_mut().remove(effect_index);
            }
        }
        EngineCommand::SetEffectBypass {
            track_id,
            effect_index,
            bypassed,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                t.effects_mut().set_bypass(effect_index, bypassed);
            }
        }
        EngineCommand::SetEffectParameter {
            track_id,
            effect_index,
            param_index,
            value,
        } => {
            apply_set_effect_param(state, track_id, effect_index, param_index, value);
        }
        EngineCommand::SetSynthParameter {
            track_id,
            param_index,
            value,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                let _ = t.synth_mut().set_param(param_index, value);
            }
        }
        EngineCommand::SetMasterVolume(db) => state.mixer.set_master_volume(db),
        EngineCommand::StartRecording { path: _ } => {
            state.is_recording = true;
        }
        EngineCommand::StopRecording => {
            state.is_recording = false;
        }
    }
}

fn apply_add_track(state: &mut ProcessingState, name: String, mode: SynthesisMode) {
    if state.mixer.track_count() < crate::MAX_TRACKS {
        #[allow(clippy::cast_precision_loss)]
        let sr = state.sample_rate as f32;
        let synth = create_synth(mode, sr);
        state.mixer.add_track(name, synth);
    }
}

fn apply_set_synth_mode(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    mode: SynthesisMode,
) {
    let Some(track) = state.mixer.track_mut(track_id) else {
        return;
    };

    #[allow(clippy::cast_precision_loss)]
    let sr = state.sample_rate as f32;
    let new_synth = create_synth(mode, sr);

    let name = track.name().to_owned();
    let volume = track.volume();
    let pan = track.pan();
    let muted = track.is_muted();
    let soloed = track.is_soloed();
    let armed = track.is_armed();

    state.mixer.remove_track(track_id);
    let new_id = state.mixer.add_track(name, new_synth);

    if let Some(new_track) = state.mixer.track_mut(new_id) {
        new_track.set_volume(volume);
        new_track.set_pan(pan);
        new_track.set_muted(muted);
        new_track.set_soloed(soloed);
        new_track.set_armed(armed);
    }
}

fn apply_add_effect(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    effect: Box<dyn crate::Processor>,
) {
    if let Some(track) = state.mixer.track_mut(track_id) {
        if track.effects().len() < crate::MAX_EFFECTS_PER_TRACK {
            track.effects_mut().push(effect);
        }
    }
}

fn apply_set_effect_param(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    effect_index: usize,
    param_index: usize,
    value: f32,
) {
    let Some(track) = state.mixer.track_mut(track_id) else {
        return;
    };
    let effects = track.effects_mut();

    // EffectChain only exposes push/remove/set_bypass. To set a param on an
    // effect at a specific index, we temporarily remove it, set the param,
    // then re-insert at the same position. O(n) but chains are short (max 8).
    let Some(mut processor) = effects.remove(effect_index) else {
        return;
    };
    let _ = processor.set_param(param_index, value);

    let remaining = effects.len() - effect_index;
    let mut tail: Vec<Box<dyn crate::Processor>> = Vec::with_capacity(remaining);
    for _ in 0..remaining {
        if let Some(p) = effects.remove(effect_index) {
            tail.push(p);
        }
    }
    effects.push(processor);
    for p in tail {
        effects.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_synth_pitch_tracked() {
        let synth = create_synth(SynthesisMode::PitchTracked, 44100.0);
        assert_eq!(synth.name(), "Pitch Tracked Synth");
    }

    #[test]
    fn create_synth_wavetable() {
        let synth = create_synth(SynthesisMode::Wavetable, 44100.0);
        assert_eq!(synth.name(), "Wavetable Oscillator");
    }

    #[test]
    fn create_synth_granular() {
        let synth = create_synth(SynthesisMode::Granular, 44100.0);
        assert_eq!(synth.name(), "Granular Synth");
    }

    #[test]
    fn create_synth_vocoder() {
        let synth = create_synth(SynthesisMode::Vocoder, 44100.0);
        assert_eq!(synth.name(), "Vocoder");
    }

    #[test]
    fn create_synth_phase_vocoder() {
        let synth = create_synth(SynthesisMode::PhaseVocoder, 44100.0);
        assert_eq!(synth.name(), "Phase Vocoder");
    }

    #[test]
    fn create_synth_all_modes_at_48k() {
        for mode in [
            SynthesisMode::PitchTracked,
            SynthesisMode::Wavetable,
            SynthesisMode::Granular,
            SynthesisMode::Vocoder,
            SynthesisMode::PhaseVocoder,
        ] {
            let synth = create_synth(mode, 48000.0);
            assert!(!synth.name().is_empty(), "synth for {mode:?} has no name");
        }
    }

    #[test]
    fn processing_state_initializes_correctly() {
        let state = ProcessingState::new(44_100, 256);
        assert_eq!(state.sample_rate, 44_100);
        assert_eq!(state.buffer_size, 256);
        assert_eq!(state.mic_block.len(), 256);
        assert!(!state.is_recording);
        assert!(state.latest_pitch.frequency.is_none());
        assert!(state.latest_spectrum.is_empty());
        assert!(state.latest_formants.is_none());
    }

    #[test]
    fn processing_state_initializes_at_48k_512() {
        let state = ProcessingState::new(48_000, 512);
        assert_eq!(state.sample_rate, 48_000);
        assert_eq!(state.buffer_size, 512);
        assert_eq!(state.mic_block.len(), 512);
    }
}
