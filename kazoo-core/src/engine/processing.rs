//! Audio processing: the real-time audio workhorse.
//!
//! The [`process_block`] function is called by the cpal output callback on
//! every buffer cycle. It drains commands, reads microphone input from the
//! ring buffer, runs the mixer (synths + effects), applies the soft limiter,
//! writes directly to the output buffer, and pushes display snapshots for
//! the UI. All state is owned by the output callback closure.

use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use ringbuf::traits::{Consumer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::analysis::{EnvelopeFollower, PitchEstimate};
use crate::mixer::Mixer;
use crate::mixer::TrackId;
use crate::mixer::clip::{AudioClip, ClipData, ClipId};
use crate::synthesis::SynthesisMode;
use crate::transport::metronome::Metronome;
use crate::transport::{TransportClock, TransportCommand};
use crate::{Db, sanitize_buffer, soft_limit_buffer};

use super::command::EngineCommand;
use super::display::{ClipSnapshot, DisplayState, TimelineSnapshot, TrackClipSnapshot};

/// Create a synthesis processor for the given mode and sample rate.
///
/// This factory function centralises the mapping from [`SynthesisMode`] to a
/// concrete `Box<dyn Processor>` so it can be reused in `AddTrack` and
/// `SetTrackSynthesisMode` commands.
#[must_use]
pub fn create_synth(mode: SynthesisMode, sample_rate: f32) -> Box<dyn crate::Processor> {
    match mode {
        SynthesisMode::Passthrough => Box::new(crate::synthesis::PassthroughSynth),
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

/// Per-track recording buffer state.
///
/// Created when recording starts for each armed track. The buffer is
/// pre-allocated on the command handler (not the tight audio loop). When
/// recording stops, the captured audio is converted into a [`ClipData`] and
/// added to the track as a clip.
struct TrackRecordingState {
    /// Pre-allocated sample buffer. Only `len` samples are valid.
    buffer: Vec<f32>,
    /// Number of valid samples written so far.
    len: usize,
    /// Transport position (in samples) when recording began.
    start_position: u64,
    /// The track being recorded.
    track_id: TrackId,
}

/// Maximum recording duration in seconds before auto-stop (5 minutes).
const MAX_RECORDING_SECONDS: usize = 5 * 60;

/// State bundle for audio processing.
///
/// Owned by the output callback closure. All fields are pre-allocated at
/// engine start and reused throughout the lifetime — no allocations in the
/// audio callback.
pub(super) struct ProcessingState {
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
    /// Samples processed since last meter reset.
    meter_sample_counter: u32,
    /// Number of samples between meter resets (~50ms worth).
    meter_reset_interval: u32,
    /// Pre-allocated display snapshot reused each frame to avoid per-block
    /// heap allocations in the output callback.
    display_scratch: DisplayState,
    /// Monotonically increasing counter for assigning unique clip IDs.
    next_clip_id: u64,
    /// Active per-track recording sessions. Each armed track gets one when
    /// the transport enters Record state.
    track_recordings: Vec<TrackRecordingState>,
    /// Whether clips have changed since the last timeline snapshot was built.
    clips_dirty: bool,
    /// Metronome click generator.
    metronome: Metronome,
    /// Set to `true` when a Shutdown command is received or the command
    /// channel disconnects. Once set, `process_block` fills silence.
    shutdown: bool,
}

impl ProcessingState {
    pub(super) fn new(sample_rate: u32, buffer_size: usize) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let sr_f32 = sample_rate as f32;
        let mut mixer = Mixer::new();
        mixer.prepare(sr_f32, buffer_size);

        // Reset meters every ~50ms (sample_rate / 20 samples).
        let meter_reset_interval = sample_rate / 20;

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
            meter_sample_counter: 0,
            meter_reset_interval,
            display_scratch: DisplayState::initial(sample_rate),
            next_clip_id: 0,
            track_recordings: Vec::with_capacity(crate::MAX_TRACKS),
            clips_dirty: false,
            metronome: Metronome::new(sample_rate),
            shutdown: false,
        }
    }
}

/// All ring buffer handles and channels used by the audio processing callback.
pub(super) struct ProcessingIO {
    pub(super) mic_cons: HeapCons<f32>,
    pub(super) display_prod: HeapProd<DisplayState>,
    pub(super) analysis_prod: HeapProd<f32>,
    pub(super) disk_prod: HeapProd<f32>,
    pub(super) pitch_cons: HeapCons<PitchEstimate>,
    pub(super) spectrum_cons: HeapCons<Vec<f32>>,
    pub(super) formant_cons: HeapCons<Option<crate::analysis::FormantData>>,
    pub(super) command_rx: Receiver<EngineCommand>,
    pub(super) disk_cmd_tx: Sender<super::DiskCommand>,
}

/// Process one audio block, writing directly to the cpal output buffer.
///
/// Called by the cpal output callback on every buffer cycle. This is the
/// main audio processing entry point — it drains commands, reads mic input,
/// runs the mixer (synths + effects), applies the soft limiter, and writes
/// interleaved stereo output directly to `output_buffer`.
///
/// # Contract
///
/// - `output_buffer` is interleaved stereo (`[L, R, L, R, ...]`).
/// - This function MUST fill the entire `output_buffer` every time, even
///   if no mic data is available or after shutdown (fills silence).
/// - No allocations, no locks, no panics.
pub(super) fn process_block(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    output_buffer: &mut [f32],
) {
    // After shutdown, fill silence and return immediately.
    if state.shutdown {
        for sample in output_buffer.iter_mut() {
            *sample = 0.0;
        }
        return;
    }

    // Drain commands — may set state.shutdown.
    drain_commands(io, state);
    if state.shutdown {
        for sample in output_buffer.iter_mut() {
            *sample = 0.0;
        }
        return;
    }

    let block_start = Instant::now();

    // Determine how many mono frames to process based on the output buffer
    // size (stereo interleaved). Cap at mic_block capacity for safety.
    let num_samples = (output_buffer.len() / 2).min(state.mic_block.len());

    let num_read = read_mic_input(io, state, num_samples);

    feed_analysis(io, state, num_read);
    drain_analysis_results(io, state);

    let input_level_db = compute_input_level(state, num_read);
    let position_before_advance = state.transport.position_samples();
    let master_slice_len = run_mixer(state, num_samples, position_before_advance);
    feed_disk(io, state, master_slice_len);
    mix_metronome_and_limit(
        state,
        num_samples,
        position_before_advance,
        master_slice_len,
    );
    feed_clip_analysis(io, state, num_samples);

    // Copy the limited master buffer to the output buffer.
    let master_buf = state.mixer.master_buffer();
    let copy_len = master_slice_len.min(output_buffer.len());
    output_buffer[..copy_len].copy_from_slice(&master_buf[..copy_len]);
    // Fill any remainder with silence.
    for sample in &mut output_buffer[copy_len..] {
        *sample = 0.0;
    }

    // Advance transport BEFORE capturing recordings so that count-in
    // completion allocates recording buffers in time for this block's
    // mic data to be captured (avoids one-block latency at count-in start).
    advance_transport(state, num_samples);
    capture_track_recordings(state, num_samples);
    capture_waveform(state, num_read);
    let cpu_load = compute_cpu_load(block_start, num_samples, state.sample_rate);

    push_display_state(io, state, input_level_db, cpu_load);

    // Only reset meters every ~50ms to preserve meaningful peak-hold.
    let n = u32::try_from(num_samples).unwrap_or(u32::MAX);
    state.meter_sample_counter = state.meter_sample_counter.saturating_add(n);
    if state.meter_sample_counter >= state.meter_reset_interval {
        state.mixer.reset_meters();
        state.meter_sample_counter = 0;
    }
}

/// Drain all pending commands from the command channel.
///
/// Sets `state.shutdown` to `true` if a Shutdown command is received or the
/// channel disconnects. The caller should check `state.shutdown` after this
/// returns.
fn drain_commands(io: &ProcessingIO, state: &mut ProcessingState) {
    loop {
        match io.command_rx.try_recv() {
            Ok(cmd) => {
                if matches!(cmd, EngineCommand::Shutdown) {
                    state.shutdown = true;
                    return;
                }
                apply_command(cmd, state, &io.disk_cmd_tx);
            }
            Err(crossbeam_channel::TryRecvError::Empty) => return,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                state.shutdown = true;
                return;
            }
        }
    }
}

/// Read mic samples from the ring buffer into the state's mic block.
///
/// Reads at most `max_read` samples to match the output buffer size. Any
/// remaining slots up to `max_read` are zero-padded (silence). Returns
/// the number of samples actually read from the ring buffer.
fn read_mic_input(io: &mut ProcessingIO, state: &mut ProcessingState, max_read: usize) -> usize {
    let limit = max_read.min(state.mic_block.len());
    let num_read = io.mic_cons.pop_slice(&mut state.mic_block[..limit]);
    for sample in &mut state.mic_block[num_read..limit] {
        *sample = 0.0;
    }
    sanitize_buffer(&mut state.mic_block[..num_read]);
    num_read
}

/// Feed raw mic samples to the analysis thread's ring buffer.
///
/// Called before the mixer runs to feed mic audio for pitch detection during
/// recording and monitoring. During playback, [`feed_clip_analysis`] is
/// called after the mixer to feed clip audio instead.
fn feed_analysis(io: &mut ProcessingIO, state: &ProcessingState, num_read: usize) {
    // During playback (not recording), skip mic — clip audio will be fed after the mixer.
    if state.transport.is_playing() && !state.transport.is_recording() {
        return;
    }
    if num_read > 0 {
        let _ = io.analysis_prod.push_slice(&state.mic_block[..num_read]);
    }
}

/// Feed mixed clip audio to the analysis thread during playback.
///
/// Called after `run_mixer` so the `clip_mix_buffer` is populated.
/// This enables pitch detection on clip content so synths receive the correct
/// frequency data when processing clips.
fn feed_clip_analysis(io: &mut ProcessingIO, state: &ProcessingState, num_samples: usize) {
    if state.transport.is_playing() && !state.transport.is_recording() {
        let clip_buf = state.mixer.clip_mix_buffer();
        let len = num_samples.min(clip_buf.len());
        if len > 0 {
            let _ = io.analysis_prod.push_slice(&clip_buf[..len]);
        }
    }
}

/// Drain analysis results (pitch, spectrum, formants) from ring buffers
/// and feed detected pitch to armed tracks' synths.
///
/// Note: replacing `latest_spectrum` by move drops the old `Vec`, which is a
/// heap deallocation inside the output callback. This is an accepted tradeoff
/// — the analysis thread produces spectrums infrequently (once per FFT hop),
/// the Vecs are small (~1-4 KB), and the allocator's thread-local cache makes
/// these deallocations effectively free after warm-up. This is standard DAW
/// practice (JUCE, `PortAudio`, etc. all do equivalent operations in their
/// audio callbacks for meter/display data).
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

    // Feed detected pitch to all synth layers on tracks.
    // During playback: all tracks need pitch (synths process clip audio).
    // During recording/monitoring: only armed tracks need pitch.
    if let Some(freq) = state.latest_pitch.frequency {
        let playback_mode = state.transport.is_playing() && !state.transport.is_recording();
        for track in state.mixer.tracks_mut() {
            if playback_mode || track.is_armed() {
                for layer in track.layers_mut() {
                    layer.synth_mut().set_pitch(freq);
                }
            }
        }
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
///
/// After advancing, checks the returned [`AdvanceFlags`] for count-in
/// completion and auto-stop triggers, starting or finalizing per-track
/// recordings accordingly.
fn advance_transport(state: &mut ProcessingState, num_samples: usize) {
    let n = u32::try_from(num_samples).unwrap_or(u32::MAX);
    let flags = state.transport.advance(n);

    if flags.count_in_completed {
        // Count-in finished — start recording on all armed tracks and
        // transition the transport from Playing to Recording. Use the
        // exact bar-boundary position from AdvanceFlags rather than the
        // current (overshot) transport position for bar-aligned clips.
        start_track_recordings_at(state, flags.record_start_position);
        state.transport.apply_command(TransportCommand::Record);
    }

    if flags.auto_stop_triggered {
        // Auto-stop boundary reached — finalize recordings and stop.
        finalize_track_recordings(state);
        state.transport.apply_command(TransportCommand::Stop);
        state.metronome.reset();
    }
}

/// Run the mixer to produce a stereo master buffer from all tracks.
///
/// Returns the number of interleaved stereo samples written. The master
/// buffer is ready for disk recording at this point (no metronome mixed in).
fn run_mixer(state: &mut ProcessingState, num_samples: usize, position: u64) -> usize {
    state.mixer.process(
        &state.mic_block[..num_samples],
        num_samples,
        position,
        state.transport.is_playing() || state.transport.is_recording(),
        state.transport.is_recording(),
    );

    (num_samples * 2).min(state.mixer.master_buffer().len())
}

/// Mix metronome clicks into the master buffer and apply the soft limiter.
///
/// The metronome is mixed AFTER the disk recorder has already captured the
/// clean master buffer, so clicks go to speakers but NOT to disk recordings.
/// A sanitization pass runs after the metronome to guard against NaN/Inf.
///
/// The caller is responsible for copying `master_buffer[..stereo_len]` to
/// the output buffer after this returns.
fn mix_metronome_and_limit(
    state: &mut ProcessingState,
    num_samples: usize,
    position: u64,
    stereo_len: usize,
) {
    if state.transport.metronome_enabled()
        && (state.transport.is_playing() || state.transport.is_recording())
    {
        let master_buf = state.mixer.master_buffer_mut();
        state.metronome.generate(
            &mut master_buf[..stereo_len],
            position,
            state.transport.bpm(),
            state.transport.beats_per_bar(),
            num_samples,
        );
        // Sanitize after metronome mixing to uphold NaN/Inf defense.
        sanitize_buffer(&mut master_buf[..stereo_len]);
    }

    // Apply soft limiter — last processing step before audio reaches the DAC.
    // Without limiting, multi-track summing and master volume (up to +24 dB)
    // can produce samples well above 1.0 which get hard-clipped by the
    // hardware, producing harsh "bit-crushed" distortion.
    let master_buf = state.mixer.master_buffer_mut();
    soft_limit_buffer(&mut master_buf[..stereo_len]);
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
    // Only update when we actually received new mic samples. Keeping the
    // previous snapshot avoids blinking on frames where the ring buffer
    // had nothing new (common at higher UI refresh rates).
    if num_read == 0 {
        return;
    }
    state.waveform_snapshot.clear();
    let max_len = 256;
    let step = (num_read / max_len).max(1);
    let mut i = 0;
    while i < num_read && state.waveform_snapshot.len() < max_len {
        state.waveform_snapshot.push(state.mic_block[i]);
        i += step;
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
///
/// Reuses `state.display_scratch` to update mixer/spectrum/waveform in-place,
/// then clones the result for the ring buffer push.
///
/// The clone allocates heap memory for Vec/String fields inside the output
/// callback. This is an accepted tradeoff — standard DAW practice (JUCE,
/// `PortAudio`, etc. all clone display/meter data in their audio callbacks).
/// The allocator's thread-local cache makes these clones effectively free
/// after the first frame, and the ring buffer consumer drops old values,
/// recycling their allocations.
fn push_display_state(
    io: &mut ProcessingIO,
    state: &mut ProcessingState,
    input_level_db: f32,
    cpu_load: f32,
) {
    // Rebuild timeline snapshot when clips have changed. This allocates
    // Strings/Vecs but only runs when the clip set actually changes (not
    // every audio block).
    if state.clips_dirty {
        update_timeline_snapshot(
            &mut state.display_scratch.timeline,
            &state.mixer,
            &state.track_recordings,
        );
        state.clips_dirty = false;
    }

    // Update recording positions in-place every block during active
    // recording (no allocation — just overwrites scalar fields so the TUI
    // can show a growing recording rectangle).
    if !state.track_recordings.is_empty() {
        update_recording_positions(&mut state.display_scratch.timeline, &state.track_recordings);
    }

    let scratch = &mut state.display_scratch;
    scratch.transport = state.transport.snapshot();
    state.mixer.write_snapshot(&mut scratch.mixer);
    scratch.pitch = state.latest_pitch;

    // Reuse existing Vec capacity — clone_from only reallocates if capacity
    // is insufficient, which after the first frame it never is.
    scratch
        .spectrum_magnitudes
        .clone_from(&state.latest_spectrum);
    scratch.waveform.clone_from(&state.waveform_snapshot);
    scratch.input_level_db = input_level_db;
    scratch.is_recording = state.is_recording;
    scratch.formants.clone_from(&state.latest_formants);
    scratch.cpu_load = cpu_load;

    let _ = io.display_prod.try_push(scratch.clone());
}

// ---------------------------------------------------------------------------
// Timeline snapshot construction
// ---------------------------------------------------------------------------

/// Update a timeline snapshot in-place from the current mixer state and any
/// active recordings.
///
/// This is called in the output callback when clips have changed or when
/// recordings are active. Reuses existing `Vec` capacity in `snapshot` to
/// avoid per-call heap allocation.
fn update_timeline_snapshot(
    snapshot: &mut TimelineSnapshot,
    mixer: &Mixer,
    recordings: &[TrackRecordingState],
) {
    snapshot.tracks.clear();
    snapshot.total_length = 0;

    for track in mixer.tracks() {
        let mut clip_snapshots: Vec<ClipSnapshot> = track
            .clips()
            .iter()
            .map(|clip| {
                let end = clip.end_position();
                if end > snapshot.total_length {
                    snapshot.total_length = end;
                }
                ClipSnapshot {
                    id: clip.id().0,
                    name: clip.data().name().to_string(),
                    position: clip.position(),
                    #[allow(clippy::cast_possible_truncation)]
                    length: clip.effective_length() as u64,
                    gain_db: clip.gain().value(),
                    muted: clip.is_muted(),
                    waveform_overview: clip.waveform_overview().to_vec(),
                }
            })
            .collect();

        // Sort clips by position for consistent rendering.
        clip_snapshots.sort_by_key(|c| c.position);

        // Check if this track has an active recording.
        let active_rec = recordings.iter().find(|r| r.track_id == track.id());
        let (is_recording_clip, recording_start, recording_length) =
            active_rec.map_or((false, 0, 0), |rec| {
                #[allow(clippy::cast_possible_truncation)]
                let rec_len = rec.len as u64;
                let rec_end = rec.start_position.saturating_add(rec_len);
                if rec_end > snapshot.total_length {
                    snapshot.total_length = rec_end;
                }
                (true, rec.start_position, rec_len)
            });

        snapshot.tracks.push(TrackClipSnapshot {
            track_id: track.id().0,
            track_name: track.name().to_string(),
            clips: clip_snapshots,
            armed: track.is_armed(),
            muted: track.is_muted(),
            soloed: track.is_soloed(),
            is_recording_clip,
            recording_start,
            recording_length,
        });
    }
}

/// Update recording positions in-place within an existing timeline snapshot.
///
/// This is called every audio block during active recording to update the
/// recording length/position fields without rebuilding the entire snapshot
/// (which would allocate Strings and Vecs). Only scalar fields are written.
fn update_recording_positions(snapshot: &mut TimelineSnapshot, recordings: &[TrackRecordingState]) {
    for rec in recordings {
        let tid = rec.track_id.0;
        if let Some(track_snap) = snapshot.tracks.iter_mut().find(|t| t.track_id == tid) {
            track_snap.is_recording_clip = true;
            track_snap.recording_start = rec.start_position;
            #[allow(clippy::cast_possible_truncation)]
            let rec_len = rec.len as u64;
            track_snap.recording_length = rec_len;
            let rec_end = rec.start_position.saturating_add(rec_len);
            if rec_end > snapshot.total_length {
                snapshot.total_length = rec_end;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Command application (split into sub-functions for line count compliance)
// ---------------------------------------------------------------------------

/// Apply a single engine command to the processing state.
#[allow(clippy::too_many_lines)]
fn apply_command(
    cmd: EngineCommand,
    state: &mut ProcessingState,
    disk_cmd_tx: &Sender<super::DiskCommand>,
) {
    match cmd {
        EngineCommand::Shutdown => {}
        EngineCommand::Transport(c) => apply_transport_command(state, c),
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
        EngineCommand::AddSynthLayer {
            track_id,
            synthesis_mode,
            label,
        } => {
            apply_add_synth_layer(state, track_id, synthesis_mode, label);
        }
        EngineCommand::RemoveSynthLayer {
            track_id,
            layer_index,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                t.remove_layer(layer_index);
            }
        }
        EngineCommand::SetSynthLayerGain {
            track_id,
            layer_index,
            gain,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(layer) = t.layer_mut(layer_index) {
                    layer.set_gain(gain);
                }
            }
        }
        EngineCommand::SetSynthLayerEnabled {
            track_id,
            layer_index,
            enabled,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(layer) = t.layer_mut(layer_index) {
                    layer.set_enabled(enabled);
                }
            }
        }
        EngineCommand::SetSynthLayerParameter {
            track_id,
            layer_index,
            param_index,
            value,
        } => {
            apply_set_synth_layer_param(state, track_id, layer_index, param_index, value);
        }
        EngineCommand::SetMasterVolume(db) => state.mixer.set_master_volume(db),
        EngineCommand::StartRecording { path } => {
            let _ = disk_cmd_tx.try_send(super::DiskCommand::Start(path));
            state.is_recording = true;
        }
        EngineCommand::StopRecording => {
            let _ = disk_cmd_tx.try_send(super::DiskCommand::Stop);
            state.is_recording = false;
        }
        cmd @ (EngineCommand::AddClip { .. }
        | EngineCommand::RemoveClip { .. }
        | EngineCommand::MoveClip { .. }
        | EngineCommand::TrimClipStart { .. }
        | EngineCommand::TrimClipEnd { .. }
        | EngineCommand::SplitClip { .. }
        | EngineCommand::SetClipGain { .. }
        | EngineCommand::SetClipMute { .. }
        | EngineCommand::DuplicateClip { .. }) => {
            apply_clip_command(cmd, state);
        }
    }
}

/// Apply a clip-related engine command to the processing state.
///
/// Factored out of [`apply_command`] to keep each function within clippy's
/// line-count limit.
fn apply_clip_command(cmd: EngineCommand, state: &mut ProcessingState) {
    match cmd {
        EngineCommand::AddClip {
            track_id,
            clip_data,
            position,
        } => {
            apply_add_clip(state, track_id, clip_data, position);
        }
        EngineCommand::RemoveClip { track_id, clip_id } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                t.remove_clip(clip_id);
                state.clips_dirty = true;
            }
        }
        EngineCommand::MoveClip {
            track_id,
            clip_id,
            new_position,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(clip) = t.find_clip_mut(clip_id) {
                    clip.set_position(new_position);
                    state.clips_dirty = true;
                }
            }
        }
        EngineCommand::TrimClipStart {
            track_id,
            clip_id,
            samples,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(clip) = t.find_clip_mut(clip_id) {
                    clip.trim_start(samples);
                    state.clips_dirty = true;
                }
            }
        }
        EngineCommand::TrimClipEnd {
            track_id,
            clip_id,
            samples,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(clip) = t.find_clip_mut(clip_id) {
                    clip.trim_end(samples);
                    state.clips_dirty = true;
                }
            }
        }
        EngineCommand::SplitClip {
            track_id,
            clip_id,
            split_position,
        } => {
            apply_split_clip(state, track_id, clip_id, split_position);
        }
        EngineCommand::SetClipGain {
            track_id,
            clip_id,
            gain,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(clip) = t.find_clip_mut(clip_id) {
                    clip.set_gain(gain);
                    state.clips_dirty = true;
                }
            }
        }
        EngineCommand::SetClipMute {
            track_id,
            clip_id,
            muted,
        } => {
            if let Some(t) = state.mixer.track_mut(track_id) {
                if let Some(clip) = t.find_clip_mut(clip_id) {
                    clip.set_muted(muted);
                    state.clips_dirty = true;
                }
            }
        }
        EngineCommand::DuplicateClip {
            track_id,
            clip_id,
            new_position,
        } => {
            apply_duplicate_clip(state, track_id, clip_id, new_position);
        }
        // Non-clip commands are never passed to this function.
        _ => {}
    }
}

/// Handle a transport command, starting or finalizing per-track recordings
/// when the transport enters or leaves Record state.
fn apply_transport_command(state: &mut ProcessingState, cmd: TransportCommand) {
    match cmd {
        TransportCommand::Record => {
            // Start per-track recording for each armed track.
            start_track_recordings(state);
            state.transport.apply_command(cmd);
        }
        TransportCommand::RecordWithCountIn => {
            apply_record_with_count_in(state);
        }
        TransportCommand::Stop | TransportCommand::Pause => {
            // Finalize any active track recordings before changing state.
            finalize_track_recordings(state);
            state.transport.apply_command(cmd);
            // Reset metronome click state on stop so it doesn't
            // continue mid-click when playback resumes.
            if matches!(cmd, TransportCommand::Stop) {
                state.metronome.reset();
            }
        }
        TransportCommand::SetMetronomeVolume(db) => {
            state.metronome.set_volume(db);
        }
        other => state.transport.apply_command(other),
    }
}

/// Apply the `RecordWithCountIn` command based on the configured workflow.
///
/// Depending on the [`RecordingWorkflow`], this either:
/// - `FreeRecord`: starts recording immediately (same as `Record`)
/// - `CountIn`: begins a count-in (transport plays with metronome, then
///   recording starts automatically when `advance()` signals completion)
/// - `FixedLength`: starts recording immediately with auto-stop
fn apply_record_with_count_in(state: &mut ProcessingState) {
    use crate::transport::RecordingWorkflow;

    match state.transport.recording_workflow() {
        RecordingWorkflow::FreeRecord => {
            // Behaves identically to a normal Record command.
            start_track_recordings(state);
            state.transport.apply_command(TransportCommand::Record);
        }
        RecordingWorkflow::CountIn {
            count_in_bars,
            record_bars,
        } => {
            // Start playing with count-in. The metronome will sound.
            // When advance() signals count_in_completed, we start
            // track recordings and transition to Recording.
            state.transport.start_count_in(count_in_bars, record_bars);
        }
        RecordingWorkflow::FixedLength { bars } => {
            // Start recording immediately with auto-stop.
            start_track_recordings(state);
            state.transport.start_fixed_length(bars);
        }
    }
}

/// Begin recording on all armed tracks using the current transport position.
///
/// This is the normal entry point for free recording (`r` key). Pre-allocates
/// a buffer for each armed track (this is a command handler, not the hot
/// audio path).
fn start_track_recordings(state: &mut ProcessingState) {
    let position = state.transport.position_samples();
    start_track_recordings_at(state, position);
}

/// Begin recording on all armed tracks at the given timeline position.
///
/// Used by the count-in workflow to place clips at the exact bar boundary
/// (the count-in end) rather than the current (overshot) transport position.
fn start_track_recordings_at(state: &mut ProcessingState, position: u64) {
    // Don't start if already recording.
    if !state.track_recordings.is_empty() {
        return;
    }

    let capacity = MAX_RECORDING_SECONDS * state.sample_rate as usize;

    for track in state.mixer.tracks() {
        if track.is_armed() {
            state.track_recordings.push(TrackRecordingState {
                buffer: vec![0.0; capacity],
                len: 0,
                start_position: position,
                track_id: track.id(),
            });
        }
    }
}

/// Finalize active track recordings: trim the buffers to the recorded
/// length, create [`ClipData`] from each, and add as clips to the tracks.
///
/// For `FreeRecord` workflows, quantizes the clip start position and length
/// to the nearest bar boundaries so recordings align with the tempo grid.
fn finalize_track_recordings(state: &mut ProcessingState) {
    use crate::transport::RecordingWorkflow;

    let recordings: Vec<TrackRecordingState> = state.track_recordings.drain(..).collect();
    let is_free_record = matches!(
        state.transport.recording_workflow(),
        RecordingWorkflow::FreeRecord
    );

    for mut rec in recordings {
        if rec.len == 0 {
            continue;
        }

        // For FreeRecord, quantize clip boundaries to the nearest bar
        // so recordings align with the tempo grid. Only quantize when the
        // recording spans at least one full bar — very short recordings
        // (e.g. quick test punches) keep their raw boundaries.
        let (clip_position, clip_len) = if is_free_record {
            let spb = state.transport.samples_per_bar();
            #[allow(clippy::cast_possible_truncation)]
            let spb_usize = spb as usize;
            if spb > 0 && rec.len >= spb_usize {
                let quantized_start = state.transport.quantize_to_bar(rec.start_position);
                let raw_end = rec.start_position.saturating_add(rec.len as u64);
                let quantized_end = state.transport.quantize_to_bar(raw_end);
                // Ensure quantized end is at least one bar past the start.
                let quantized_end = quantized_end.max(quantized_start.saturating_add(spb));
                let q_len = quantized_end.saturating_sub(quantized_start);
                #[allow(clippy::cast_possible_truncation)]
                let final_len = (q_len as usize).min(rec.len);
                (quantized_start, final_len.max(1))
            } else {
                (rec.start_position, rec.len)
            }
        } else {
            (rec.start_position, rec.len)
        };

        // Trim the buffer to the final clip length.
        rec.buffer.truncate(clip_len);

        let clip_id = ClipId(state.next_clip_id);
        state.next_clip_id += 1;

        let name = format!("Recording {}", clip_id.0);
        let clip_data = ClipData::new(rec.buffer, name, None, state.sample_rate);
        let clip = AudioClip::new(clip_id, clip_data, clip_position);

        if let Some(t) = state.mixer.track_mut(rec.track_id) {
            let _ = t.add_clip(clip);
            state.clips_dirty = true;
        }
    }
}

/// Capture raw mic input into the recording buffers for each armed track.
///
/// Records the unprocessed microphone signal so clips contain the user's
/// actual voice rather than synthesized output. This avoids feedback loops
/// and produces clean recordings suitable for later playback through the
/// track's effect chain.
///
/// Called once per processing block, after `run_mixer`. This is on
/// the hot audio path, so it must not allocate — it only copies into the
/// pre-allocated buffers.
fn capture_track_recordings(state: &mut ProcessingState, num_samples: usize) {
    let mic = &state.mic_block[..num_samples];

    for rec in &mut state.track_recordings {
        let remaining = rec.buffer.len() - rec.len;
        if remaining == 0 {
            // Buffer full — can't record any more.
            continue;
        }
        let to_copy = num_samples.min(remaining).min(mic.len());
        if to_copy > 0 {
            rec.buffer[rec.len..rec.len + to_copy].copy_from_slice(&mic[..to_copy]);
            rec.len += to_copy;
        }
    }
}

fn apply_add_track(state: &mut ProcessingState, name: String, mode: SynthesisMode) {
    if state.mixer.track_count() < crate::MAX_TRACKS {
        #[allow(clippy::cast_precision_loss)]
        let sr = state.sample_rate as f32;
        let synth = create_synth(mode, sr);
        state.mixer.add_track(name, synth, mode);
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
    let mut new_synth = create_synth(mode, sr);
    new_synth.prepare(state.buffer_size);

    track.replace_synth(new_synth, mode);
}

fn apply_add_effect(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    effect: Box<dyn crate::Processor>,
) {
    if let Some(track) = state.mixer.track_mut(track_id) {
        if track.effects().len() < crate::MAX_EFFECTS_PER_TRACK {
            track.effects_mut().push(effect);
            track.effects_mut().prepare(state.buffer_size);
        }
    }
}

fn apply_add_synth_layer(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    mode: SynthesisMode,
    label: String,
) {
    let Some(track) = state.mixer.track_mut(track_id) else {
        return;
    };
    if track.layer_count() >= crate::MAX_SYNTH_LAYERS {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let sr = state.sample_rate as f32;
    let mut synth = create_synth(mode, sr);
    synth.prepare(state.buffer_size);
    track.add_layer(synth, mode, label);
}

fn apply_set_synth_layer_param(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    layer_index: usize,
    param_index: usize,
    value: f32,
) {
    let Some(track) = state.mixer.track_mut(track_id) else {
        return;
    };
    let Some(layer) = track.layer_mut(layer_index) else {
        return;
    };
    let _ = layer.synth_mut().set_param(param_index, value);
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
    let _ = track
        .effects_mut()
        .set_effect_param(effect_index, param_index, value);
}

fn apply_add_clip(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    clip_data: ClipData,
    position: u64,
) {
    if let Some(t) = state.mixer.track_mut(track_id) {
        let clip_id = ClipId(state.next_clip_id);
        state.next_clip_id += 1;
        let clip = AudioClip::new(clip_id, clip_data, position);
        // Silently ignore if the track is full (MAX_CLIPS_PER_TRACK).
        let _ = t.add_clip(clip);
        state.clips_dirty = true;
    }
}

fn apply_split_clip(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    clip_id: ClipId,
    split_position: u64,
) {
    let new_clip_id = ClipId(state.next_clip_id);
    if let Some(t) = state.mixer.track_mut(track_id) {
        // Do the split. `find_clip_mut` returns a mutable reference; `split_at`
        // modifies the clip in place and returns the right half as an owned value.
        let right_half = t
            .find_clip_mut(clip_id)
            .and_then(|clip| clip.split_at(split_position, new_clip_id));
        if let Some(right) = right_half {
            // Silently ignore if the track is full.
            if t.add_clip(right) {
                state.next_clip_id += 1;
            }
            state.clips_dirty = true;
        }
    }
}

fn apply_duplicate_clip(
    state: &mut ProcessingState,
    track_id: crate::mixer::TrackId,
    clip_id: ClipId,
    new_position: u64,
) {
    let new_id = ClipId(state.next_clip_id);
    if let Some(t) = state.mixer.track_mut(track_id) {
        let new_clip = t
            .find_clip(clip_id)
            .map(|source| AudioClip::new(new_id, source.data().clone(), new_position));
        if let Some(clip) = new_clip {
            // Silently ignore if the track is full.
            if t.add_clip(clip) {
                state.next_clip_id += 1;
                state.clips_dirty = true;
            }
        }
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
    fn create_synth_passthrough() {
        let synth = create_synth(SynthesisMode::Passthrough, 44100.0);
        assert_eq!(synth.name(), "Passthrough");
    }

    #[test]
    fn create_synth_all_modes_at_48k() {
        for mode in [
            SynthesisMode::Passthrough,
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

    #[test]
    fn processing_state_next_clip_id_starts_at_zero() {
        let state = ProcessingState::new(44_100, 256);
        assert_eq!(state.next_clip_id, 0);
    }

    // -- Clip command handler tests ------------------------------------------

    use crate::engine::DiskCommand;
    use crate::mixer::TrackId;
    use crate::mixer::clip::ClipData;

    /// Create a `ProcessingState` with one track (id=`TrackId(0)`) and a
    /// disk command sender/receiver pair.
    fn state_with_track() -> (ProcessingState, crossbeam_channel::Sender<DiskCommand>) {
        let mut state = ProcessingState::new(44_100, 256);
        let synth = create_synth(SynthesisMode::PitchTracked, 44_100.0);
        state
            .mixer
            .add_track("Test".into(), synth, SynthesisMode::PitchTracked);
        let (tx, _rx) = crossbeam_channel::bounded(64);
        (state, tx)
    }

    /// Create test clip data of a given length.
    fn test_clip_data(len: usize) -> ClipData {
        let samples: Vec<f32> = (0..len).map(|i| (i as f32) / len as f32).collect();
        ClipData::new(samples, "TestClip".into(), None, 44_100)
    }

    #[test]
    fn apply_add_clip_assigns_unique_ids() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(200),
                position: 1000,
            },
            &mut state,
            &tx,
        );

        assert_eq!(state.next_clip_id, 2);
        let track = state.mixer.track(track_id).unwrap();
        assert_eq!(track.clips().len(), 2);
        assert_eq!(track.clips()[0].id(), ClipId(0));
        assert_eq!(track.clips()[1].id(), ClipId(1));
    }

    #[test]
    fn apply_add_clip_nonexistent_track_is_noop() {
        let (mut state, tx) = state_with_track();
        let bogus_id = TrackId(999);

        apply_command(
            EngineCommand::AddClip {
                track_id: bogus_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );

        // next_clip_id should not have been incremented.
        assert_eq!(state.next_clip_id, 0);
    }

    #[test]
    fn apply_remove_clip_removes_correct_clip() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(50),
                position: 500,
            },
            &mut state,
            &tx,
        );
        assert_eq!(state.mixer.track(track_id).unwrap().clips().len(), 2);

        apply_command(
            EngineCommand::RemoveClip {
                track_id,
                clip_id: ClipId(0),
            },
            &mut state,
            &tx,
        );

        let clips = state.mixer.track(track_id).unwrap().clips();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id(), ClipId(1));
    }

    #[test]
    fn apply_move_clip_changes_position() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::MoveClip {
                track_id,
                clip_id: ClipId(0),
                new_position: 5000,
            },
            &mut state,
            &tx,
        );

        let clip = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(0))
            .unwrap();
        assert_eq!(clip.position(), 5000);
    }

    #[test]
    fn apply_trim_clip_start() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::TrimClipStart {
                track_id,
                clip_id: ClipId(0),
                samples: 20,
            },
            &mut state,
            &tx,
        );

        let clip = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(0))
            .unwrap();
        assert_eq!(clip.source_start(), 20);
        assert_eq!(clip.effective_length(), 80);
    }

    #[test]
    fn apply_trim_clip_end() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::TrimClipEnd {
                track_id,
                clip_id: ClipId(0),
                samples: 30,
            },
            &mut state,
            &tx,
        );

        let clip = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(0))
            .unwrap();
        assert_eq!(clip.source_end(), 70);
        assert_eq!(clip.effective_length(), 70);
    }

    #[test]
    fn apply_split_clip_creates_two_clips() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 1000,
            },
            &mut state,
            &tx,
        );
        // Split at position 1040 (40 samples into the clip).
        apply_command(
            EngineCommand::SplitClip {
                track_id,
                clip_id: ClipId(0),
                split_position: 1040,
            },
            &mut state,
            &tx,
        );

        let clips = state.mixer.track(track_id).unwrap().clips();
        assert_eq!(clips.len(), 2);

        // Left half: ClipId(0), position 1000, length 40.
        let left = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(0))
            .unwrap();
        assert_eq!(left.position(), 1000);
        assert_eq!(left.effective_length(), 40);

        // Right half: ClipId(1), position 1040, length 60.
        let right = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(1))
            .unwrap();
        assert_eq!(right.position(), 1040);
        assert_eq!(right.effective_length(), 60);

        // next_clip_id incremented for AddClip (0->1) and SplitClip (1->2).
        assert_eq!(state.next_clip_id, 2);
    }

    #[test]
    fn apply_split_clip_at_boundary_is_noop() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 1000,
            },
            &mut state,
            &tx,
        );
        // Split at the clip start should be a no-op.
        apply_command(
            EngineCommand::SplitClip {
                track_id,
                clip_id: ClipId(0),
                split_position: 1000,
            },
            &mut state,
            &tx,
        );

        let clips = state.mixer.track(track_id).unwrap().clips();
        assert_eq!(clips.len(), 1);
        // next_clip_id: 1 from AddClip, not incremented by failed split.
        assert_eq!(state.next_clip_id, 1);
    }

    #[test]
    fn apply_set_clip_gain() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::SetClipGain {
                track_id,
                clip_id: ClipId(0),
                gain: Db::new(-12.0),
            },
            &mut state,
            &tx,
        );

        let clip = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(0))
            .unwrap();
        assert!((clip.gain().value() - (-12.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_set_clip_mute() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        assert!(
            !state
                .mixer
                .track(track_id)
                .unwrap()
                .find_clip(ClipId(0))
                .unwrap()
                .is_muted()
        );

        apply_command(
            EngineCommand::SetClipMute {
                track_id,
                clip_id: ClipId(0),
                muted: true,
            },
            &mut state,
            &tx,
        );

        assert!(
            state
                .mixer
                .track(track_id)
                .unwrap()
                .find_clip(ClipId(0))
                .unwrap()
                .is_muted()
        );
    }

    #[test]
    fn apply_duplicate_clip_creates_new_clip() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::AddClip {
                track_id,
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::DuplicateClip {
                track_id,
                clip_id: ClipId(0),
                new_position: 5000,
            },
            &mut state,
            &tx,
        );

        let clips = state.mixer.track(track_id).unwrap().clips();
        assert_eq!(clips.len(), 2);

        let original = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(0))
            .unwrap();
        let duplicate = state
            .mixer
            .track(track_id)
            .unwrap()
            .find_clip(ClipId(1))
            .unwrap();

        assert_eq!(original.position(), 0);
        assert_eq!(duplicate.position(), 5000);
        assert_eq!(original.effective_length(), duplicate.effective_length());
        // Both share the same underlying data (Arc clone).
        assert_eq!(
            original.data().samples().as_ptr(),
            duplicate.data().samples().as_ptr(),
        );

        assert_eq!(state.next_clip_id, 2);
    }

    #[test]
    fn apply_duplicate_clip_nonexistent_is_noop() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::DuplicateClip {
                track_id,
                clip_id: ClipId(99),
                new_position: 0,
            },
            &mut state,
            &tx,
        );

        let clips = state.mixer.track(track_id).unwrap().clips();
        assert!(clips.is_empty());
        assert_eq!(state.next_clip_id, 0);
    }

    #[test]
    fn apply_move_clip_nonexistent_clip_is_noop() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        // Move a clip that does not exist -- should not panic.
        apply_command(
            EngineCommand::MoveClip {
                track_id,
                clip_id: ClipId(42),
                new_position: 9999,
            },
            &mut state,
            &tx,
        );
        assert!(state.mixer.track(track_id).unwrap().clips().is_empty());
    }

    #[test]
    fn apply_remove_clip_nonexistent_is_noop() {
        let (mut state, tx) = state_with_track();
        let track_id = TrackId(0);

        apply_command(
            EngineCommand::RemoveClip {
                track_id,
                clip_id: ClipId(0),
            },
            &mut state,
            &tx,
        );
        // No panic, no clips affected.
        assert!(state.mixer.track(track_id).unwrap().clips().is_empty());
    }

    // -- Per-track recording tests --------------------------------------------

    /// Create a `ProcessingState` with one armed track for recording tests.
    fn state_with_armed_track() -> (ProcessingState, crossbeam_channel::Sender<DiskCommand>) {
        let mut state = ProcessingState::new(44_100, 256);
        let synth = create_synth(SynthesisMode::PitchTracked, 44_100.0);
        let id = state
            .mixer
            .add_track("Armed".into(), synth, SynthesisMode::PitchTracked);
        state.mixer.track_mut(id).unwrap().set_armed(true);
        let (tx, _rx) = crossbeam_channel::bounded(64);
        (state, tx)
    }

    #[test]
    fn start_recording_creates_track_recording_state() {
        let (mut state, tx) = state_with_armed_track();

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        assert_eq!(state.track_recordings.len(), 1);
        assert_eq!(state.track_recordings[0].track_id, TrackId(0));
        assert_eq!(state.track_recordings[0].len, 0);
        assert_eq!(
            state.track_recordings[0].buffer.len(),
            MAX_RECORDING_SECONDS * 44_100
        );
    }

    #[test]
    fn start_recording_unarmed_track_not_recorded() {
        let (mut state, tx) = state_with_track(); // unarmed track

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        assert!(state.track_recordings.is_empty());
    }

    #[test]
    fn capture_track_recordings_fills_buffer() {
        let (mut state, tx) = state_with_armed_track();

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        // Simulate processing a block — fill mic_block with sample data,
        // then capture raw mic input into the recording buffer.
        for sample in &mut state.mic_block[..64] {
            *sample = 0.5;
        }
        capture_track_recordings(&mut state, 64);

        assert_eq!(state.track_recordings[0].len, 64);
    }

    #[test]
    fn stop_recording_creates_clip() {
        let (mut state, tx) = state_with_armed_track();

        // Start recording.
        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        // Simulate a few processing blocks — fill mic_block with sample data.
        for _ in 0..4 {
            for sample in &mut state.mic_block[..64] {
                *sample = 0.5;
            }
            capture_track_recordings(&mut state, 64);
        }
        assert_eq!(state.track_recordings[0].len, 256);

        // Stop recording — should finalize into a clip.
        apply_command(
            EngineCommand::Transport(TransportCommand::Stop),
            &mut state,
            &tx,
        );

        // Recording state should be drained.
        assert!(state.track_recordings.is_empty());

        // A clip should now exist on the track.
        let clips = state.mixer.track(TrackId(0)).unwrap().clips();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].effective_length(), 256);
    }

    #[test]
    fn pause_recording_creates_clip() {
        let (mut state, tx) = state_with_armed_track();

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        for sample in &mut state.mic_block[..64] {
            *sample = 0.5;
        }
        capture_track_recordings(&mut state, 64);

        // Pause also finalizes recordings.
        apply_command(
            EngineCommand::Transport(TransportCommand::Pause),
            &mut state,
            &tx,
        );

        assert!(state.track_recordings.is_empty());
        let clips = state.mixer.track(TrackId(0)).unwrap().clips();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].effective_length(), 64);
    }

    #[test]
    fn recording_empty_produces_no_clip() {
        let (mut state, tx) = state_with_armed_track();

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        // Stop immediately — no samples captured.
        apply_command(
            EngineCommand::Transport(TransportCommand::Stop),
            &mut state,
            &tx,
        );

        assert!(state.track_recordings.is_empty());
        let clips = state.mixer.track(TrackId(0)).unwrap().clips();
        assert!(clips.is_empty(), "empty recording should not create a clip");
    }

    #[test]
    fn multiple_armed_tracks_record_independently() {
        let mut state = ProcessingState::new(44_100, 256);
        let synth_a = create_synth(SynthesisMode::PitchTracked, 44_100.0);
        let synth_b = create_synth(SynthesisMode::PitchTracked, 44_100.0);
        let id_a = state
            .mixer
            .add_track("A".into(), synth_a, SynthesisMode::PitchTracked);
        let id_b = state
            .mixer
            .add_track("B".into(), synth_b, SynthesisMode::PitchTracked);
        state.mixer.track_mut(id_a).unwrap().set_armed(true);
        state.mixer.track_mut(id_b).unwrap().set_armed(true);
        let (tx, _rx) = crossbeam_channel::bounded::<DiskCommand>(64);

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        assert_eq!(state.track_recordings.len(), 2);

        // Fill mic_block and capture.
        for sample in &mut state.mic_block[..32] {
            *sample = 0.5;
        }
        capture_track_recordings(&mut state, 32);

        // Stop.
        apply_command(
            EngineCommand::Transport(TransportCommand::Stop),
            &mut state,
            &tx,
        );

        // Each track should have a clip.
        let clips_a = state.mixer.track(TrackId(0)).unwrap().clips();
        let clips_b = state.mixer.track(TrackId(1)).unwrap().clips();
        assert_eq!(clips_a.len(), 1);
        assert_eq!(clips_b.len(), 1);
        assert_eq!(clips_a[0].effective_length(), 32);
        assert_eq!(clips_b[0].effective_length(), 32);
    }

    #[test]
    fn recording_buffer_full_stops_capturing() {
        let mut state = ProcessingState::new(44_100, 256);
        let synth = create_synth(SynthesisMode::PitchTracked, 44_100.0);
        let id = state
            .mixer
            .add_track("T".into(), synth, SynthesisMode::PitchTracked);
        state.mixer.track_mut(id).unwrap().set_armed(true);
        let (tx, _rx) = crossbeam_channel::bounded::<DiskCommand>(64);

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        // Manually set len to near capacity to simulate a nearly full buffer.
        let cap = state.track_recordings[0].buffer.len();
        state.track_recordings[0].len = cap - 10;

        // Try to capture 64 samples — only 10 should fit.
        for sample in &mut state.mic_block[..64] {
            *sample = 0.5;
        }
        capture_track_recordings(&mut state, 64);

        assert_eq!(state.track_recordings[0].len, cap);
    }

    #[test]
    fn double_record_command_does_not_double_allocate() {
        let (mut state, tx) = state_with_armed_track();

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );
        assert_eq!(state.track_recordings.len(), 1);

        // Second Record command should be a no-op (recordings already active).
        start_track_recordings(&mut state);
        assert_eq!(state.track_recordings.len(), 1);
    }

    #[test]
    fn track_recordings_empty_after_init() {
        let state = ProcessingState::new(44_100, 256);
        assert!(state.track_recordings.is_empty());
    }

    // -- clips_dirty flag tests -----------------------------------------------

    #[test]
    fn clips_dirty_initially_false() {
        let state = ProcessingState::new(44_100, 256);
        assert!(!state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_add_clip() {
        let (mut state, tx) = state_with_track();
        assert!(!state.clips_dirty);

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_remove_clip() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::RemoveClip {
                track_id: TrackId(0),
                clip_id: ClipId(0),
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_move_clip() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::MoveClip {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                new_position: 500,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_trim_start() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::TrimClipStart {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                samples: 10,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_trim_end() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::TrimClipEnd {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                samples: 10,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_split_clip() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 1000,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::SplitClip {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                split_position: 1050,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_set_clip_gain() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::SetClipGain {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                gain: Db::new(-6.0),
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_set_clip_mute() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::SetClipMute {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                muted: true,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_duplicate_clip() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        state.clips_dirty = false;

        apply_command(
            EngineCommand::DuplicateClip {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                new_position: 500,
            },
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    #[test]
    fn clips_dirty_flag_set_on_finalize_recording() {
        let (mut state, tx) = state_with_armed_track();

        // Start recording.
        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        // Simulate a processing block — fill mic_block with sample data.
        for sample in &mut state.mic_block[..64] {
            *sample = 0.5;
        }
        capture_track_recordings(&mut state, 64);

        state.clips_dirty = false;

        // Stop recording -- should finalize into a clip and set dirty.
        apply_command(
            EngineCommand::Transport(TransportCommand::Stop),
            &mut state,
            &tx,
        );
        assert!(state.clips_dirty);
    }

    // -- update_timeline_snapshot tests ----------------------------------------

    #[test]
    fn update_timeline_snapshot_empty_mixer() {
        let mixer = Mixer::new();
        let recordings: Vec<TrackRecordingState> = Vec::new();
        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &mixer, &recordings);

        assert!(snapshot.tracks.is_empty());
        assert_eq!(snapshot.total_length, 0);
    }

    #[test]
    fn update_timeline_snapshot_with_clips() {
        let (mut state, tx) = state_with_track();

        // Add two clips at different positions.
        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(200),
                position: 500,
            },
            &mut state,
            &tx,
        );

        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &state.mixer, &state.track_recordings);

        assert_eq!(snapshot.tracks.len(), 1);
        assert_eq!(snapshot.tracks[0].clips.len(), 2);
        assert_eq!(snapshot.tracks[0].track_name, "Test");

        // First clip: position 0, length 100.
        assert_eq!(snapshot.tracks[0].clips[0].position, 0);
        assert_eq!(snapshot.tracks[0].clips[0].length, 100);

        // Second clip: position 500, length 200.
        assert_eq!(snapshot.tracks[0].clips[1].position, 500);
        assert_eq!(snapshot.tracks[0].clips[1].length, 200);

        // Total length should be end of the last clip: 500 + 200 = 700.
        assert_eq!(snapshot.total_length, 700);
    }

    #[test]
    fn update_timeline_snapshot_clips_sorted_by_position() {
        let (mut state, tx) = state_with_track();

        // Add clips in reverse order.
        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(50),
                position: 1000,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(50),
                position: 200,
            },
            &mut state,
            &tx,
        );

        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &state.mixer, &state.track_recordings);

        assert_eq!(snapshot.tracks[0].clips[0].position, 200);
        assert_eq!(snapshot.tracks[0].clips[1].position, 1000);
    }

    #[test]
    fn update_timeline_snapshot_with_active_recording() {
        let (mut state, tx) = state_with_armed_track();

        apply_command(
            EngineCommand::Transport(TransportCommand::Record),
            &mut state,
            &tx,
        );

        // Simulate some recording — fill mic_block with sample data.
        for sample in &mut state.mic_block[..64] {
            *sample = 0.5;
        }
        capture_track_recordings(&mut state, 64);

        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &state.mixer, &state.track_recordings);

        assert_eq!(snapshot.tracks.len(), 1);
        assert!(snapshot.tracks[0].is_recording_clip);
        assert_eq!(snapshot.tracks[0].recording_start, 0);
        assert_eq!(snapshot.tracks[0].recording_length, 64);
        assert_eq!(snapshot.total_length, 64);
    }

    #[test]
    fn update_timeline_snapshot_track_flags() {
        let mut state = ProcessingState::new(44_100, 256);
        let synth = create_synth(SynthesisMode::PitchTracked, 44_100.0);
        let id = state
            .mixer
            .add_track("Flagged".into(), synth, SynthesisMode::PitchTracked);
        state.mixer.track_mut(id).unwrap().set_armed(true);
        state.mixer.track_mut(id).unwrap().set_muted(true);
        state.mixer.track_mut(id).unwrap().set_soloed(true);

        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &state.mixer, &state.track_recordings);

        assert_eq!(snapshot.tracks.len(), 1);
        assert!(snapshot.tracks[0].armed);
        assert!(snapshot.tracks[0].muted);
        assert!(snapshot.tracks[0].soloed);
        assert!(!snapshot.tracks[0].is_recording_clip);
    }

    #[test]
    fn update_timeline_snapshot_clip_gain_and_mute() {
        let (mut state, tx) = state_with_track();

        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(100),
                position: 0,
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::SetClipGain {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                gain: Db::new(-12.0),
            },
            &mut state,
            &tx,
        );
        apply_command(
            EngineCommand::SetClipMute {
                track_id: TrackId(0),
                clip_id: ClipId(0),
                muted: true,
            },
            &mut state,
            &tx,
        );

        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &state.mixer, &state.track_recordings);

        let clip = &snapshot.tracks[0].clips[0];
        assert!((clip.gain_db - (-12.0)).abs() < f32::EPSILON);
        assert!(clip.muted);
    }

    #[test]
    fn update_timeline_snapshot_waveform_overview_populated() {
        let (mut state, tx) = state_with_track();

        // Create a clip with enough samples to generate a waveform overview.
        apply_command(
            EngineCommand::AddClip {
                track_id: TrackId(0),
                clip_data: test_clip_data(1000),
                position: 0,
            },
            &mut state,
            &tx,
        );

        let mut snapshot = TimelineSnapshot::empty();
        update_timeline_snapshot(&mut snapshot, &state.mixer, &state.track_recordings);
        assert!(!snapshot.tracks[0].clips[0].waveform_overview.is_empty());
    }
}
