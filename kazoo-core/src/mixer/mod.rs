//! Mixer: tracks, pan, levels, metering, master bus.
//!
//! The [`Mixer`] manages a set of [`Track`]s, each with a synthesis slot,
//! effect chain, volume/pan controls, and per-channel metering. All internal
//! buffers are pre-allocated — [`Mixer::process`] never allocates.

pub mod clip;

use crate::effects::EffectChain;
use crate::synthesis::SynthesisMode;
use crate::{DEFAULT_BUFFER_SIZE, Db, Pan, Processor, sanitize_buffer, sanitize_sample};

use std::fmt;

use clip::{AudioClip, ClipId};

// ---------------------------------------------------------------------------
// TrackId
// ---------------------------------------------------------------------------

/// Opaque identifier for a mixer track. Unique within a [`Mixer`] instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackId(pub usize);

impl fmt::Display for TrackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Track({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// SynthLayer
// ---------------------------------------------------------------------------

/// A single synth layer within a track's layer stack.
///
/// Each layer has its own synthesis processor, gain, and enable state.
/// During processing, enabled layers are mixed into the track's synth buffer
/// with their individual gains applied.
pub struct SynthLayer {
    synth: Box<dyn Processor>,
    mode: SynthesisMode,
    gain: Db,
    enabled: bool,
    label: String,
}

impl fmt::Debug for SynthLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SynthLayer")
            .field("synth", &self.synth.name())
            .field("mode", &self.mode)
            .field("gain", &self.gain)
            .field("enabled", &self.enabled)
            .field("label", &self.label)
            .finish()
    }
}

impl SynthLayer {
    /// Immutable access to the synth processor.
    #[must_use]
    pub fn synth(&self) -> &dyn Processor {
        &*self.synth
    }

    /// Mutable access to the synth processor.
    pub fn synth_mut(&mut self) -> &mut dyn Processor {
        &mut *self.synth
    }

    /// Replace the synth processor in this layer.
    pub fn replace_synth(&mut self, synth: Box<dyn Processor>, mode: SynthesisMode) {
        self.synth = synth;
        self.mode = mode;
    }

    /// Synthesis mode of this layer.
    #[must_use]
    pub const fn mode(&self) -> SynthesisMode {
        self.mode
    }

    /// Layer gain in dB.
    #[must_use]
    pub const fn gain(&self) -> Db {
        self.gain
    }

    /// Set the layer gain.
    pub const fn set_gain(&mut self, gain: Db) {
        self.gain = gain;
    }

    /// Whether this layer is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set the enabled state.
    pub const fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Human-readable label for this layer.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Set the layer label.
    pub fn set_label(&mut self, label: String) {
        self.label = label;
    }
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// A single mixer track with synthesis layers, effects, volume, pan, and metering.
///
/// Each track owns one or more [`SynthLayer`]s and an [`EffectChain`].
/// Audio flows: synth layers (summed) → effects → volume → pan → master bus.
pub struct Track {
    id: TrackId,
    name: String,
    layers: Vec<SynthLayer>,
    effects: EffectChain,
    volume: Db,
    pan: Pan,
    muted: bool,
    soloed: bool,
    armed: bool,
    // Pre-allocated processing buffers (mono, sized to buffer_size).
    synth_buffer: Vec<f32>,
    effect_buffer: Vec<f32>,
    // Pre-allocated buffer for clip audio before synth processing.
    // During playback, clips are read into this buffer, then fed through the
    // synth as "virtual mic input" so the user can shape clips with synth settings.
    clip_buffer: Vec<f32>,
    // Pre-allocated per-layer scratch buffers. `MAX_SYNTH_LAYERS` buffers are
    // always allocated even if fewer layers exist, to avoid allocation when
    // layers are added at runtime.
    layer_buffers: Vec<Vec<f32>>,
    // Per-channel peak values (linear). Held until explicit reset.
    peak_meter: [f32; 2],
    // Per-channel sum-of-squares for RMS computation (f64 for precision).
    rms_accumulator: [f64; 2],
    rms_sample_count: usize,
    // Number of valid samples in `effect_buffer` after the last
    // [`Mixer::process`] call. Only `effect_buffer[..processed_samples]` is
    // meaningful; the rest is stale pre-allocated capacity.
    processed_samples: usize,
    // Audio clips placed on this track's timeline.
    clips: Vec<AudioClip>,
}

impl fmt::Debug for Track {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Track")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("layers", &self.layers)
            .field("effects", &self.effects)
            .field("volume", &self.volume)
            .field("pan", &self.pan)
            .field("muted", &self.muted)
            .field("soloed", &self.soloed)
            .field("armed", &self.armed)
            .field("clips", &self.clips.len())
            .field("processed_samples", &self.processed_samples)
            .field("peak_meter", &self.peak_meter)
            .field("rms_sample_count", &self.rms_sample_count)
            .finish_non_exhaustive()
    }
}

impl Track {
    /// Track identifier, unique within its parent [`Mixer`].
    #[must_use]
    pub const fn id(&self) -> TrackId {
        self.id
    }

    /// Human-readable track name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the track name.
    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    /// Current volume level.
    #[must_use]
    pub const fn volume(&self) -> Db {
        self.volume
    }

    /// Set the track volume.
    pub const fn set_volume(&mut self, db: Db) {
        self.volume = db;
    }

    /// Current pan position.
    #[must_use]
    pub const fn pan(&self) -> Pan {
        self.pan
    }

    /// Set the track pan position.
    pub const fn set_pan(&mut self, pan: Pan) {
        self.pan = pan;
    }

    /// Whether this track is muted.
    #[must_use]
    pub const fn is_muted(&self) -> bool {
        self.muted
    }

    /// Set the mute state.
    pub const fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Whether this track is soloed.
    #[must_use]
    pub const fn is_soloed(&self) -> bool {
        self.soloed
    }

    /// Set the solo state.
    pub const fn set_soloed(&mut self, soloed: bool) {
        self.soloed = soloed;
    }

    /// Whether this track is armed for recording.
    #[must_use]
    pub const fn is_armed(&self) -> bool {
        self.armed
    }

    /// Set the armed-for-recording state.
    pub const fn set_armed(&mut self, armed: bool) {
        self.armed = armed;
    }

    /// Immutable access to the effect chain.
    #[must_use]
    pub const fn effects(&self) -> &EffectChain {
        &self.effects
    }

    /// Mutable access to the effect chain (add/remove/bypass effects).
    pub const fn effects_mut(&mut self) -> &mut EffectChain {
        &mut self.effects
    }

    /// Immutable access to the primary synth processor (layer 0).
    #[must_use]
    pub fn synth(&self) -> &dyn Processor {
        self.layers[0].synth()
    }

    /// Mutable access to the primary synth processor (layer 0).
    pub fn synth_mut(&mut self) -> &mut dyn Processor {
        self.layers[0].synth_mut()
    }

    /// Replace the primary synth processor (layer 0) in-place without
    /// changing the track ID, name, effects chain, volume, pan, or any
    /// other track state.
    ///
    /// The new synth's [`Processor::set_sample_rate`] is NOT called here;
    /// the caller is responsible for ensuring the synth is already configured
    /// at the correct sample rate.
    pub fn replace_synth(&mut self, synth: Box<dyn Processor>, mode: SynthesisMode) {
        self.layers[0].replace_synth(synth, mode);
    }

    // -- Layer management ---------------------------------------------------

    /// Number of synth layers on this track.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Immutable access to a synth layer by index.
    #[must_use]
    pub fn layer(&self, index: usize) -> Option<&SynthLayer> {
        self.layers.get(index)
    }

    /// Mutable access to a synth layer by index.
    pub fn layer_mut(&mut self, index: usize) -> Option<&mut SynthLayer> {
        self.layers.get_mut(index)
    }

    /// All synth layers.
    #[must_use]
    pub fn layers(&self) -> &[SynthLayer] {
        &self.layers
    }

    /// All synth layers (mutable).
    pub fn layers_mut(&mut self) -> &mut [SynthLayer] {
        &mut self.layers
    }

    /// Add a new synth layer. Returns `false` if [`MAX_SYNTH_LAYERS`] reached.
    ///
    /// [`MAX_SYNTH_LAYERS`]: crate::MAX_SYNTH_LAYERS
    pub fn add_layer(
        &mut self,
        synth: Box<dyn Processor>,
        mode: SynthesisMode,
        label: String,
    ) -> bool {
        if self.layers.len() >= crate::MAX_SYNTH_LAYERS {
            return false;
        }
        self.layers.push(SynthLayer {
            synth,
            mode,
            gain: Db::UNITY,
            enabled: true,
            label,
        });
        true
    }

    /// Remove a synth layer by index. Layer 0 cannot be removed.
    /// Returns `true` if the layer was found and removed.
    pub fn remove_layer(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.layers.len() {
            return false;
        }
        self.layers.remove(index);
        true
    }

    /// All clips on this track, in insertion order.
    #[must_use]
    pub fn clips(&self) -> &[AudioClip] {
        &self.clips
    }

    /// Add a clip to this track. Respects [`clip::MAX_CLIPS_PER_TRACK`].
    /// Returns `true` if the clip was added, `false` if the limit was reached.
    pub fn add_clip(&mut self, clip: AudioClip) -> bool {
        if self.clips.len() >= clip::MAX_CLIPS_PER_TRACK {
            return false;
        }
        self.clips.push(clip);
        true
    }

    /// Remove a clip by its [`ClipId`]. Returns `true` if found and removed.
    pub fn remove_clip(&mut self, clip_id: ClipId) -> bool {
        if let Some(pos) = self.clips.iter().position(|c| c.id() == clip_id) {
            self.clips.remove(pos);
            true
        } else {
            false
        }
    }

    /// Find a clip by its [`ClipId`].
    #[must_use]
    pub fn find_clip(&self, clip_id: ClipId) -> Option<&AudioClip> {
        self.clips.iter().find(|c| c.id() == clip_id)
    }

    /// Find a clip by its [`ClipId`] (mutable).
    pub fn find_clip_mut(&mut self, clip_id: ClipId) -> Option<&mut AudioClip> {
        self.clips.iter_mut().find(|c| c.id() == clip_id)
    }

    /// Access the post-effect mono buffer (valid after [`Mixer::process`]).
    ///
    /// Returns only the `[..processed_samples]` slice that was filled by the
    /// most recent [`Mixer::process`] call, not the full pre-allocated buffer.
    #[must_use]
    pub fn effect_buffer(&self) -> &[f32] {
        &self.effect_buffer[..self.processed_samples]
    }

    /// Reset peak and RMS meters to zero.
    const fn reset_meters(&mut self) {
        self.peak_meter = [0.0; 2];
        self.rms_accumulator = [0.0; 2];
        self.rms_sample_count = 0;
    }

    /// Build a [`TrackMeter`] snapshot from the current accumulated values.
    #[must_use]
    fn meter_snapshot(&self) -> TrackMeter {
        let rms_l = rms_linear(self.rms_accumulator[0], self.rms_sample_count);
        let rms_r = rms_linear(self.rms_accumulator[1], self.rms_sample_count);

        TrackMeter {
            peak_db: [
                Db::from_linear(self.peak_meter[0]).value(),
                Db::from_linear(self.peak_meter[1]).value(),
            ],
            rms_db: [
                Db::from_linear(rms_l).value(),
                Db::from_linear(rms_r).value(),
            ],
            clipping: self.peak_meter[0] > 1.0 || self.peak_meter[1] > 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// TrackMeter
// ---------------------------------------------------------------------------

/// Per-track meter readings (stereo: index 0 = left, 1 = right).
#[derive(Debug, Clone)]
pub struct TrackMeter {
    /// Peak level in dB per channel.
    pub peak_db: [f32; 2],
    /// RMS level in dB per channel.
    pub rms_db: [f32; 2],
    /// `true` if peak exceeds 0 dBFS (linear > 1.0).
    pub clipping: bool,
}

// ---------------------------------------------------------------------------
// MixerSnapshot
// ---------------------------------------------------------------------------

/// A snapshot of all mixer meters, suitable for sending to the UI thread.
#[derive(Debug, Clone, Default)]
pub struct MixerSnapshot {
    /// Per-track meter readings (same order as [`Mixer::tracks`]).
    pub track_meters: Vec<TrackMeter>,
    /// Master bus peak level in dB per channel.
    pub master_peak_db: [f32; 2],
    /// Master bus RMS level in dB per channel.
    pub master_rms_db: [f32; 2],
    /// `true` if master peak exceeds 0 dBFS.
    pub master_clipping: bool,
}

// ---------------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------------

/// Multi-track mixer with per-track synthesis, effects, and metering.
///
/// All internal buffers are pre-allocated in [`Mixer::new`] or
/// [`Mixer::prepare`]. The [`Mixer::process`] method never allocates.
pub struct Mixer {
    tracks: Vec<Track>,
    master_volume: Db,
    /// Interleaved stereo output buffer: `[L0, R0, L1, R1, ...]`.
    master_buffer: Vec<f32>,
    /// Mixed clip audio from all tracks (mono), for feeding to the analysis
    /// thread during playback so pitch detection runs on clip content.
    clip_mix_buffer: Vec<f32>,
    master_peak: [f32; 2],
    master_rms_accumulator: [f64; 2],
    master_rms_count: usize,
    next_track_id: usize,
    sample_rate: f32,
    buffer_size: usize,
}

impl fmt::Debug for Mixer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mixer")
            .field("tracks", &self.tracks)
            .field("master_volume", &self.master_volume)
            .field("master_peak", &self.master_peak)
            .field("master_rms_count", &self.master_rms_count)
            .field("next_track_id", &self.next_track_id)
            .field("sample_rate", &self.sample_rate)
            .field("buffer_size", &self.buffer_size)
            .finish_non_exhaustive()
    }
}

impl Mixer {
    /// Create a new mixer with default buffer size and sample rate.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tracks: Vec::with_capacity(crate::MAX_TRACKS),
            master_volume: Db::UNITY,
            master_buffer: vec![0.0; DEFAULT_BUFFER_SIZE * 2],
            clip_mix_buffer: vec![0.0; DEFAULT_BUFFER_SIZE],
            master_peak: [0.0; 2],
            master_rms_accumulator: [0.0; 2],
            master_rms_count: 0,
            next_track_id: 0,
            sample_rate: crate::DEFAULT_SAMPLE_RATE as f32,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }

    /// Re-allocate all buffers for a new sample rate and buffer size.
    ///
    /// Must be called before the first [`Mixer::process`] call if the engine
    /// sample rate or buffer size differ from the defaults. Also propagates
    /// the sample rate to every track's synth processor.
    pub fn prepare(&mut self, sample_rate: f32, buffer_size: usize) {
        self.sample_rate = sample_rate;
        self.buffer_size = buffer_size;

        // Resize master buffer (interleaved stereo) and clip mix buffer (mono).
        self.master_buffer.resize(buffer_size * 2, 0.0);
        self.clip_mix_buffer.resize(buffer_size, 0.0);

        // Update every track.
        for track in &mut self.tracks {
            track.synth_buffer.resize(buffer_size, 0.0);
            track.effect_buffer.resize(buffer_size, 0.0);
            track.clip_buffer.resize(buffer_size, 0.0);
            track.effects.prepare(buffer_size);

            // Update all synth layers.
            for layer in &mut track.layers {
                layer.synth.set_sample_rate(sample_rate);
                layer.synth.prepare(buffer_size);
            }

            // Ensure layer buffers are sized correctly.
            for buf in &mut track.layer_buffers {
                buf.resize(buffer_size, 0.0);
            }
            // Ensure we always have MAX_SYNTH_LAYERS buffers pre-allocated.
            while track.layer_buffers.len() < crate::MAX_SYNTH_LAYERS {
                track.layer_buffers.push(vec![0.0; buffer_size]);
            }
        }
    }

    /// Add a track with the given name, synth engine, and synthesis mode.
    /// Returns the new track's unique identifier.
    ///
    /// The synth's [`Processor::set_sample_rate`] is called immediately with
    /// the mixer's current sample rate. The synth becomes layer 0 of the
    /// new track's layer stack.
    pub fn add_track(
        &mut self,
        name: String,
        mut synth: Box<dyn Processor>,
        mode: SynthesisMode,
    ) -> TrackId {
        let id = TrackId(self.next_track_id);
        self.next_track_id += 1;

        synth.set_sample_rate(self.sample_rate);
        synth.prepare(self.buffer_size);

        let primary_layer = SynthLayer {
            synth,
            mode,
            gain: Db::UNITY,
            enabled: true,
            label: "Layer 1".into(),
        };

        let track = Track {
            id,
            name,
            layers: vec![primary_layer],
            effects: EffectChain::new_with_capacity(self.buffer_size),
            volume: Db::UNITY,
            pan: Pan::CENTER,
            muted: false,
            soloed: false,
            armed: false,
            synth_buffer: vec![0.0; self.buffer_size],
            effect_buffer: vec![0.0; self.buffer_size],
            clip_buffer: vec![0.0; self.buffer_size],
            layer_buffers: (0..crate::MAX_SYNTH_LAYERS)
                .map(|_| vec![0.0; self.buffer_size])
                .collect(),
            processed_samples: 0,
            peak_meter: [0.0; 2],
            rms_accumulator: [0.0; 2],
            rms_sample_count: 0,
            clips: Vec::with_capacity(32),
        };

        self.tracks.push(track);
        id
    }

    /// Remove the track with the given ID. Returns `true` if it was found
    /// and removed, `false` if no such track exists.
    pub fn remove_track(&mut self, id: TrackId) -> bool {
        if let Some(pos) = self.tracks.iter().position(|t| t.id == id) {
            self.tracks.remove(pos);
            true
        } else {
            false
        }
    }

    /// Look up a track by ID.
    #[must_use]
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Look up a track by ID (mutable).
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// All tracks in insertion order.
    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// All tracks in insertion order (mutable).
    pub fn tracks_mut(&mut self) -> &mut [Track] {
        &mut self.tracks
    }

    /// Number of tracks currently in the mixer.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Current master volume level.
    #[must_use]
    pub const fn master_volume(&self) -> Db {
        self.master_volume
    }

    /// Set the master bus volume.
    pub const fn set_master_volume(&mut self, db: Db) {
        self.master_volume = db;
    }

    /// The interleaved stereo master output buffer from the last
    /// [`Mixer::process`] call. Layout: `[L0, R0, L1, R1, ...]`.
    #[must_use]
    pub fn master_buffer(&self) -> &[f32] {
        &self.master_buffer
    }

    /// Mutable access to the interleaved stereo master output buffer.
    ///
    /// Used by the output callback to mix metronome clicks into the output
    /// after the mixer has produced its output.
    pub fn master_buffer_mut(&mut self) -> &mut [f32] {
        &mut self.master_buffer
    }

    /// Mixed clip audio from all tracks (mono), valid after [`process`].
    ///
    /// During playback, this buffer contains the raw clip audio before synth
    /// processing. Fed to the analysis thread so pitch detection runs on clip
    /// content rather than ambient mic noise.
    #[must_use]
    pub fn clip_mix_buffer(&self) -> &[f32] {
        &self.clip_mix_buffer
    }

    /// Current sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Current buffer size in samples (mono frames).
    #[must_use]
    pub const fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// Process one block of audio through all tracks and sum to the master bus.
    ///
    /// `input` is the mono microphone/voice data passed to armed tracks' synths.
    /// `num_samples` is the number of frames to process (capped to `buffer_size`).
    /// `position_samples` is the current transport timeline position; clips on
    /// tracks are read from this position when `is_playing` is true.
    /// `is_playing` indicates whether the transport is in Playing or Recording
    /// state (clips only play when the transport is running).
    /// `is_recording` indicates whether the transport is in Recording state;
    /// armed tracks only run their synth during recording to avoid interference
    /// with clip playback.
    ///
    /// After this call, [`Mixer::master_buffer`] contains interleaved stereo
    /// output of length `2 * num_samples`.
    #[allow(clippy::too_many_lines)]
    pub fn process(
        &mut self,
        input: &[f32],
        num_samples: usize,
        position_samples: u64,
        is_playing: bool,
        is_recording: bool,
    ) {
        let num_samples = num_samples.min(self.buffer_size);
        if num_samples == 0 {
            return;
        }
        let stereo_len = num_samples * 2;

        // 1. Zero the master and clip-mix buffers for this block.
        for sample in &mut self.master_buffer[..stereo_len] {
            *sample = 0.0;
        }
        for sample in &mut self.clip_mix_buffer[..num_samples] {
            *sample = 0.0;
        }

        // 2. Determine if any track is soloed.
        let any_soloed = self.tracks.iter().any(|t| t.soloed);

        // 3. Process each track.
        for track in &mut self.tracks {
            // Muted tracks are always silent.
            if track.muted {
                continue;
            }
            // If any track is soloed, only soloed tracks produce output.
            if any_soloed && !track.soloed {
                continue;
            }

            // 3a. Zero buffers for this block.
            for s in &mut track.synth_buffer[..num_samples] {
                *s = 0.0;
            }
            for s in &mut track.clip_buffer[..num_samples] {
                *s = 0.0;
            }

            // 3b. Read clip audio into the clip buffer (if playing).
            let has_clips = is_playing && !track.clips.is_empty();
            if has_clips {
                for clip in &track.clips {
                    clip.read_into(position_samples, &mut track.clip_buffer[..num_samples]);
                }
                // Accumulate raw clip audio for analysis-thread pitch detection.
                for i in 0..num_samples {
                    self.clip_mix_buffer[i] += track.clip_buffer[i];
                }
            }

            // 3c. Route audio through synth layers.
            //
            // Determine input source based on playback/recording mode:
            //  - Playback: clip audio → synth layers → synth_buffer
            //  - Recording: mic → synth layers → synth_buffer, plus raw clips for backing
            //  - Monitoring (stopped/paused): mic → synth layers → synth_buffer
            //
            // Each enabled layer processes the same input independently, then
            // its output is gain-scaled and summed into synth_buffer.
            let has_synth_input = if has_clips && !is_recording {
                // Playback: clip audio through synth layers.
                true
            } else if track.armed && (is_recording || !is_playing) {
                // Recording or monitoring: mic through synth layers.
                true
            } else {
                false
            };
            let use_clip_input = has_clips && !is_recording;
            let add_clip_backing = has_clips && track.armed && is_recording;

            if has_synth_input {
                for layer_idx in 0..track.layers.len() {
                    if !track.layers[layer_idx].enabled {
                        continue;
                    }
                    let layer_buf = &mut track.layer_buffers[layer_idx][..num_samples];
                    for s in layer_buf.iter_mut() {
                        *s = 0.0;
                    }
                    if use_clip_input {
                        track.layers[layer_idx]
                            .synth
                            .process(&track.clip_buffer[..num_samples], layer_buf);
                    } else {
                        track.layers[layer_idx].synth.process(input, layer_buf);
                    }
                    let gain = track.layers[layer_idx].gain.to_linear();
                    for (sb, &lb) in track.synth_buffer[..num_samples]
                        .iter_mut()
                        .zip(layer_buf.iter())
                    {
                        *sb += sanitize_sample(lb) * gain;
                    }
                }
            }

            // Sum raw clip audio for backing (hear existing clips while recording).
            if add_clip_backing {
                for i in 0..num_samples {
                    track.synth_buffer[i] += track.clip_buffer[i];
                }
            }

            // 3d. Effects: mono → mono through the chain.
            track.effects.process(
                &track.synth_buffer[..num_samples],
                &mut track.effect_buffer[..num_samples],
            );
            track.processed_samples = num_samples;

            // 3e. Apply volume, pan, sum into master, and update meters.
            let volume_gain = track.volume.to_linear();
            let (pan_l, pan_r) = track.pan.gains();

            for i in 0..num_samples {
                let dry = sanitize_sample(track.effect_buffer[i]);
                let gained = dry * volume_gain;
                let left = gained * pan_l;
                let right = gained * pan_r;

                self.master_buffer[i * 2] += left;
                self.master_buffer[i * 2 + 1] += right;

                // Peak hold: keep the maximum seen since last reset.
                let abs_l = left.abs();
                let abs_r = right.abs();
                if abs_l > track.peak_meter[0] {
                    track.peak_meter[0] = abs_l;
                }
                if abs_r > track.peak_meter[1] {
                    track.peak_meter[1] = abs_r;
                }

                // RMS accumulation (f64 for precision).
                track.rms_accumulator[0] += f64::from(left) * f64::from(left);
                track.rms_accumulator[1] += f64::from(right) * f64::from(right);
            }
            track.rms_sample_count += num_samples;
        }

        // 4. Apply master volume.
        let master_gain = self.master_volume.to_linear();
        for sample in &mut self.master_buffer[..stereo_len] {
            *sample *= master_gain;
        }

        // 5. Update master meters.
        for i in 0..num_samples {
            let left = self.master_buffer[i * 2];
            let right = self.master_buffer[i * 2 + 1];

            let abs_l = left.abs();
            let abs_r = right.abs();
            if abs_l > self.master_peak[0] {
                self.master_peak[0] = abs_l;
            }
            if abs_r > self.master_peak[1] {
                self.master_peak[1] = abs_r;
            }

            self.master_rms_accumulator[0] += f64::from(left) * f64::from(left);
            self.master_rms_accumulator[1] += f64::from(right) * f64::from(right);
        }
        self.master_rms_count += num_samples;

        // 6. Final sanitization of the master output.
        sanitize_buffer(&mut self.master_buffer[..stereo_len]);
    }

    /// Build a snapshot of all meter readings for UI display.
    #[must_use]
    pub fn snapshot(&self) -> MixerSnapshot {
        let mut snap = MixerSnapshot::default();
        self.write_snapshot(&mut snap);
        snap
    }

    /// Write meter readings into an existing snapshot, reusing its allocated
    /// `Vec` to avoid per-frame heap allocation in the output callback.
    pub fn write_snapshot(&self, snapshot: &mut MixerSnapshot) {
        snapshot.track_meters.clear();
        snapshot.track_meters.reserve(self.tracks.len());
        snapshot
            .track_meters
            .extend(self.tracks.iter().map(Track::meter_snapshot));

        let master_rms_l = rms_linear(self.master_rms_accumulator[0], self.master_rms_count);
        let master_rms_r = rms_linear(self.master_rms_accumulator[1], self.master_rms_count);

        snapshot.master_peak_db = [
            Db::from_linear(self.master_peak[0]).value(),
            Db::from_linear(self.master_peak[1]).value(),
        ];
        snapshot.master_rms_db = [
            Db::from_linear(master_rms_l).value(),
            Db::from_linear(master_rms_r).value(),
        ];
        snapshot.master_clipping = self.master_peak[0] > 1.0 || self.master_peak[1] > 1.0;
    }

    /// Reset all peak and RMS meters (tracks + master) to zero.
    pub fn reset_meters(&mut self) {
        for track in &mut self.tracks {
            track.reset_meters();
        }
        self.master_peak = [0.0; 2];
        self.master_rms_accumulator = [0.0; 2];
        self.master_rms_count = 0;
    }
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute RMS in linear scale from accumulated sum-of-squares and sample count.
#[must_use]
fn rms_linear(sum_of_squares: f64, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let n = count as f64;
    let mean = sum_of_squares / n;
    #[allow(clippy::cast_possible_truncation)]
    let result = mean.sqrt() as f32;
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitize_sample;
    use std::f32::consts::FRAC_1_SQRT_2;

    // -- Test processor: passes input through scaled by a gain factor --------

    struct TestSynth {
        gain: f32,
    }

    impl TestSynth {
        fn new(gain: f32) -> Self {
            Self { gain }
        }
    }

    impl fmt::Debug for TestSynth {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TestSynth")
                .field("gain", &self.gain)
                .finish()
        }
    }

    impl Processor for TestSynth {
        fn process(&mut self, input: &[f32], output: &mut [f32]) {
            let len = input.len().min(output.len());
            for i in 0..len {
                output[i] = sanitize_sample(input[i] * self.gain);
            }
            // Zero any remaining output.
            for sample in &mut output[len..] {
                *sample = 0.0;
            }
        }

        fn reset(&mut self) {}

        fn name(&self) -> &str {
            "TestSynth"
        }

        fn set_sample_rate(&mut self, _sample_rate: f32) {}
    }

    /// Creates a mixer with a single unity-gain, armed track (centered, 0 dB).
    fn mixer_with_unity_track() -> (Mixer, TrackId) {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Test".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        (mixer, id)
    }

    // -- Track lifecycle -----------------------------------------------------

    #[test]
    fn add_and_remove_track() {
        let mut mixer = Mixer::new();
        assert_eq!(mixer.track_count(), 0);

        let id1 = mixer.add_track(
            "Track 1".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        let id2 = mixer.add_track(
            "Track 2".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        assert_eq!(mixer.track_count(), 2);

        // IDs are unique.
        assert_ne!(id1, id2);

        // Lookup works.
        assert_eq!(mixer.track(id1).unwrap().name(), "Track 1");
        assert_eq!(mixer.track(id2).unwrap().name(), "Track 2");

        // Remove first track.
        assert!(mixer.remove_track(id1));
        assert_eq!(mixer.track_count(), 1);
        assert!(mixer.track(id1).is_none());
        assert!(mixer.track(id2).is_some());

        // Removing nonexistent track returns false.
        assert!(!mixer.remove_track(id1));

        // Remove second track.
        assert!(mixer.remove_track(id2));
        assert_eq!(mixer.track_count(), 0);
    }

    #[test]
    fn track_ids_are_stable() {
        let mut mixer = Mixer::new();
        let id1 = mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        let id2 = mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.remove_track(id1);
        let id3 = mixer.add_track(
            "C".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );

        // id3 should not reuse id1's value.
        assert_ne!(id3, id1);
        assert_ne!(id3, id2);
    }

    #[test]
    fn track_default_state() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Test".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        let track = mixer.track(id).unwrap();

        assert_eq!(track.volume(), Db::UNITY);
        assert_eq!(track.pan(), Pan::CENTER);
        assert!(!track.is_muted());
        assert!(!track.is_soloed());
        assert!(!track.is_armed());
        assert_eq!(track.synth().name(), "TestSynth");
        assert!(track.effects().is_empty());
    }

    #[test]
    fn track_setters() {
        let (mut mixer, id) = mixer_with_unity_track();
        let track = mixer.track_mut(id).unwrap();

        track.set_volume(Db::new(-6.0));
        assert!((track.volume().value() - (-6.0)).abs() < f32::EPSILON);

        track.set_pan(Pan::new(0.5));
        assert!((track.pan().value() - 0.5).abs() < f32::EPSILON);

        track.set_muted(true);
        assert!(track.is_muted());

        track.set_soloed(true);
        assert!(track.is_soloed());

        track.set_armed(true);
        assert!(track.is_armed());

        track.set_name("Renamed".into());
        assert_eq!(track.name(), "Renamed");
    }

    // -- Process: basic audio flow -------------------------------------------

    #[test]
    fn process_no_tracks_outputs_silence() {
        let mut mixer = Mixer::new();
        let input = [0.5_f32; 64];
        mixer.process(&input, 64, 0, false, false);

        let master = mixer.master_buffer();
        for (i, &s) in master[..128].iter().enumerate() {
            assert!(
                s.abs() < f32::EPSILON,
                "no-tracks master[{i}] should be 0.0, got {s}"
            );
        }
    }

    #[test]
    fn process_empty_input_no_panic() {
        let (mut mixer, _id) = mixer_with_unity_track();
        mixer.process(&[], 0, 0, false, false);
    }

    #[test]
    fn process_unity_track_center_pan() {
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [0.8_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        let (pan_l, pan_r) = Pan::CENTER.gains();

        for i in 0..4 {
            let expected_l = 0.8 * pan_l;
            let expected_r = 0.8 * pan_r;
            assert!(
                (master[i * 2] - expected_l).abs() < 1e-6,
                "L[{i}]: expected {expected_l}, got {}",
                master[i * 2]
            );
            assert!(
                (master[i * 2 + 1] - expected_r).abs() < 1e-6,
                "R[{i}]: expected {expected_r}, got {}",
                master[i * 2 + 1]
            );
        }
    }

    // -- Pan -----------------------------------------------------------------

    #[test]
    fn pan_hard_left_routes_left_only() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "L".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(-1.0));

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        for i in 0..4 {
            assert!(
                (master[i * 2] - 1.0).abs() < 1e-6,
                "hard-left L[{i}] should be 1.0"
            );
            assert!(
                master[i * 2 + 1].abs() < 1e-6,
                "hard-left R[{i}] should be 0.0"
            );
        }
    }

    #[test]
    fn pan_hard_right_routes_right_only() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "R".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(1.0));

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        for i in 0..4 {
            assert!(
                master[i * 2].abs() < 1e-6,
                "hard-right L[{i}] should be 0.0"
            );
            assert!(
                (master[i * 2 + 1] - 1.0).abs() < 1e-6,
                "hard-right R[{i}] should be 1.0"
            );
        }
    }

    #[test]
    fn pan_equal_power_center_gain() {
        // At center, each channel should receive ≈ 0.707 of the signal.
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        for i in 0..4 {
            assert!(
                (master[i * 2] - FRAC_1_SQRT_2).abs() < 1e-5,
                "center L[{i}] should be ~0.707, got {}",
                master[i * 2]
            );
            assert!(
                (master[i * 2 + 1] - FRAC_1_SQRT_2).abs() < 1e-5,
                "center R[{i}] should be ~0.707, got {}",
                master[i * 2 + 1]
            );
        }
    }

    // -- Volume --------------------------------------------------------------

    #[test]
    fn volume_unity_passes_through() {
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [0.5_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        let (pan_l, _) = Pan::CENTER.gains();
        let expected = 0.5 * pan_l; // unity volume
        assert!(
            (master[0] - expected).abs() < 1e-6,
            "unity volume: expected {expected}, got {}",
            master[0]
        );
    }

    #[test]
    fn volume_silence_outputs_zero() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Silent".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_volume(Db::SILENCE);

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        for (i, &s) in master[..8].iter().enumerate() {
            assert!(
                s.abs() < f32::EPSILON,
                "silence volume master[{i}] should be 0.0, got {s}"
            );
        }
    }

    #[test]
    fn volume_minus_6db_halves_amplitude() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Half".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(-1.0)); // hard left for simplicity
        mixer.track_mut(id).unwrap().set_volume(Db::new(-6.0));

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        let expected = Db::new(-6.0).to_linear(); // ≈ 0.501
        for i in 0..4 {
            assert!(
                (master[i * 2] - expected).abs() < 1e-3,
                "-6dB L[{i}]: expected ~{expected}, got {}",
                master[i * 2]
            );
        }
    }

    // -- Master volume -------------------------------------------------------

    #[test]
    fn master_volume_scales_output() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "T".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(-1.0)); // hard left
        mixer.set_master_volume(Db::new(-6.0));

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        let expected = Db::new(-6.0).to_linear(); // ≈ 0.501
        for i in 0..4 {
            assert!(
                (master[i * 2] - expected).abs() < 1e-3,
                "master -6dB L[{i}]: expected ~{expected}, got {}",
                master[i * 2]
            );
        }
    }

    #[test]
    fn master_volume_default_is_unity() {
        let mixer = Mixer::new();
        assert_eq!(mixer.master_volume(), Db::UNITY);
    }

    // -- Mute ----------------------------------------------------------------

    #[test]
    fn muted_track_produces_silence() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Muted".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_muted(true);

        let input = [1.0_f32; 8];
        mixer.process(&input, 8, 0, false, false);

        let master = mixer.master_buffer();
        for (i, &s) in master[..16].iter().enumerate() {
            assert!(
                s.abs() < f32::EPSILON,
                "muted track master[{i}] should be 0.0, got {s}"
            );
        }
    }

    // -- Solo ----------------------------------------------------------------

    #[test]
    fn solo_only_outputs_soloed_tracks() {
        let mut mixer = Mixer::new();
        let id_a = mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        let id_b = mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(0.5)),
            SynthesisMode::PitchTracked,
        );

        mixer.track_mut(id_a).unwrap().set_armed(true);
        mixer.track_mut(id_b).unwrap().set_armed(true);

        // Both panned hard left for easy measurement.
        mixer.track_mut(id_a).unwrap().set_pan(Pan::new(-1.0));
        mixer.track_mut(id_b).unwrap().set_pan(Pan::new(-1.0));

        // Solo track B only.
        mixer.track_mut(id_b).unwrap().set_soloed(true);

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        // Only track B (gain 0.5) should be heard.
        for i in 0..4 {
            assert!(
                (master[i * 2] - 0.5).abs() < 1e-6,
                "solo L[{i}]: expected 0.5, got {}",
                master[i * 2]
            );
        }
    }

    #[test]
    fn solo_muted_track_is_still_silent() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Both".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_soloed(true);
        mixer.track_mut(id).unwrap().set_muted(true);

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        for (i, &s) in master[..8].iter().enumerate() {
            assert!(
                s.abs() < f32::EPSILON,
                "soloed+muted master[{i}] should be 0.0, got {s}"
            );
        }
    }

    #[test]
    fn no_solo_all_tracks_output() {
        let mut mixer = Mixer::new();
        let id_a = mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        let id_b = mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );

        mixer.track_mut(id_a).unwrap().set_armed(true);
        mixer.track_mut(id_b).unwrap().set_armed(true);

        // Both panned hard left, unity volume.
        mixer.track_mut(id_a).unwrap().set_pan(Pan::new(-1.0));
        mixer.track_mut(id_b).unwrap().set_pan(Pan::new(-1.0));

        let input = [0.3_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        // Both tracks sum: 0.3 + 0.3 = 0.6
        for i in 0..4 {
            assert!(
                (master[i * 2] - 0.6).abs() < 1e-5,
                "no-solo sum L[{i}]: expected 0.6, got {}",
                master[i * 2]
            );
        }
    }

    // -- Multiple tracks sum -------------------------------------------------

    #[test]
    fn multiple_tracks_sum_correctly() {
        let mut mixer = Mixer::new();
        let id_a = mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(0.25)),
            SynthesisMode::PitchTracked,
        );
        let id_b = mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(0.75)),
            SynthesisMode::PitchTracked,
        );

        mixer.track_mut(id_a).unwrap().set_armed(true);
        mixer.track_mut(id_b).unwrap().set_armed(true);

        // Both hard left for easy measurement.
        mixer.track_mut(id_a).unwrap().set_pan(Pan::new(-1.0));
        mixer.track_mut(id_b).unwrap().set_pan(Pan::new(-1.0));

        let input = [1.0_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let master = mixer.master_buffer();
        // Sum: 0.25 + 0.75 = 1.0
        for i in 0..4 {
            assert!(
                (master[i * 2] - 1.0).abs() < 1e-5,
                "sum L[{i}]: expected 1.0, got {}",
                master[i * 2]
            );
        }
    }

    // -- Metering ------------------------------------------------------------

    #[test]
    fn peak_detection() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Peak".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(-1.0)); // hard left

        let input = [0.3, 0.7, 0.5, 0.1_f32];
        mixer.process(&input, 4, 0, false, false);

        let snap = mixer.snapshot();
        // Peak should be 0.7 on left channel.
        let peak_linear = 0.7_f32;
        let expected_db = Db::from_linear(peak_linear).value();
        assert!(
            (snap.track_meters[0].peak_db[0] - expected_db).abs() < 0.1,
            "peak L: expected ~{expected_db} dB, got {}",
            snap.track_meters[0].peak_db[0]
        );
        // Right channel should be silence (hard left pan).
        assert!(
            snap.track_meters[0].peak_db[1] <= Db::SILENCE.value() + 1.0,
            "peak R should be near silence, got {}",
            snap.track_meters[0].peak_db[1]
        );
    }

    #[test]
    fn peak_hold_persists_across_calls() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Hold".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(-1.0));

        // First block: loud.
        let loud = [0.9_f32; 4];
        mixer.process(&loud, 4, 0, false, false);

        // Second block: quiet.
        let quiet = [0.1_f32; 4];
        mixer.process(&quiet, 4, 0, false, false);

        let snap = mixer.snapshot();
        // Peak should still reflect the loud block.
        let expected_db = Db::from_linear(0.9).value();
        assert!(
            (snap.track_meters[0].peak_db[0] - expected_db).abs() < 0.1,
            "peak hold: expected ~{expected_db}, got {}",
            snap.track_meters[0].peak_db[0]
        );
    }

    #[test]
    fn clipping_detected_when_peak_exceeds_unity() {
        let mut mixer = Mixer::new();
        let id_a = mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        let id_b = mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );

        mixer.track_mut(id_a).unwrap().set_armed(true);
        mixer.track_mut(id_b).unwrap().set_armed(true);
        mixer.track_mut(id_a).unwrap().set_pan(Pan::new(-1.0));
        mixer.track_mut(id_b).unwrap().set_pan(Pan::new(-1.0));

        // Both tracks at full volume, summing to > 1.0.
        let input = [0.8_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let snap = mixer.snapshot();
        // Master left peak should be 1.6 → clipping.
        assert!(snap.master_clipping, "master should be clipping");
    }

    #[test]
    fn no_clipping_below_unity() {
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [0.5_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let snap = mixer.snapshot();
        assert!(!snap.master_clipping, "should not be clipping at 0.5");
    }

    #[test]
    fn reset_meters_clears_peak_and_rms() {
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [0.9_f32; 64];
        mixer.process(&input, 64, 0, false, false);

        // Verify meters are non-zero.
        let snap_before = mixer.snapshot();
        assert!(
            snap_before.master_peak_db[0] > Db::SILENCE.value(),
            "peak should be non-silent before reset"
        );

        mixer.reset_meters();

        let snap_after = mixer.snapshot();
        // After reset, RMS should be zero → silence dB.
        assert!(
            (snap_after.master_rms_db[0] - Db::SILENCE.value()).abs() < f32::EPSILON,
            "RMS should be silence after reset, got {}",
            snap_after.master_rms_db[0]
        );
        // Peak should be zero → silence dB.
        assert!(
            (snap_after.master_peak_db[0] - Db::SILENCE.value()).abs() < f32::EPSILON,
            "peak should be silence after reset, got {}",
            snap_after.master_peak_db[0]
        );
    }

    #[test]
    fn rms_value_is_reasonable() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "RMS".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);
        mixer.track_mut(id).unwrap().set_pan(Pan::new(-1.0)); // hard left

        // Constant signal of 0.5 → RMS should be 0.5.
        let input = [0.5_f32; 64];
        mixer.process(&input, 64, 0, false, false);

        let snap = mixer.snapshot();
        let expected_rms_db = Db::from_linear(0.5).value();
        assert!(
            (snap.track_meters[0].rms_db[0] - expected_rms_db).abs() < 0.5,
            "RMS: expected ~{expected_rms_db} dB, got {}",
            snap.track_meters[0].rms_db[0]
        );
    }

    // -- Prepare -------------------------------------------------------------

    #[test]
    fn prepare_resizes_buffers() {
        let mut mixer = Mixer::new();
        let _id = mixer.add_track(
            "T".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );

        // Prepare with larger buffer.
        mixer.prepare(48000.0, 512);
        assert_eq!(mixer.buffer_size(), 512);
        assert!((mixer.sample_rate() - 48000.0).abs() < f32::EPSILON);

        // Master buffer should be 512 * 2 = 1024.
        assert!(mixer.master_buffer().len() >= 1024);

        // Process should work with the new size.
        let input = [0.5_f32; 512];
        mixer.process(&input, 512, 0, false, false);
    }

    #[test]
    fn prepare_updates_synth_sample_rate() {
        let mut mixer = Mixer::new();
        let _id = mixer.add_track(
            "T".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.prepare(96000.0, 256);
        // TestSynth.set_sample_rate is a no-op but shouldn't panic.
    }

    // -- Snapshot structure ---------------------------------------------------

    #[test]
    fn snapshot_has_correct_track_count() {
        let mut mixer = Mixer::new();
        mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );

        let input = [0.1_f32; 4];
        mixer.process(&input, 4, 0, false, false);

        let snap = mixer.snapshot();
        assert_eq!(snap.track_meters.len(), 2);
    }

    #[test]
    fn snapshot_empty_mixer() {
        let mixer = Mixer::new();
        let snap = mixer.snapshot();
        assert!(snap.track_meters.is_empty());
        assert!(!snap.master_clipping);
        assert!(
            (snap.master_peak_db[0] - Db::SILENCE.value()).abs() < f32::EPSILON,
            "empty mixer peak should be silence"
        );
    }

    // -- NaN/Inf defense -----------------------------------------------------

    #[test]
    fn nan_input_produces_finite_output() {
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [f32::NAN; 8];
        mixer.process(&input, 8, 0, false, false);

        let master = mixer.master_buffer();
        for (i, &s) in master[..16].iter().enumerate() {
            assert!(s.is_finite(), "NaN defense: master[{i}] = {s}");
        }
    }

    #[test]
    fn inf_input_produces_finite_output() {
        let (mut mixer, _id) = mixer_with_unity_track();
        let input = [f32::INFINITY; 8];
        mixer.process(&input, 8, 0, false, false);

        let master = mixer.master_buffer();
        for (i, &s) in master[..16].iter().enumerate() {
            assert!(s.is_finite(), "Inf defense: master[{i}] = {s}");
        }
    }

    // -- Debug impls ---------------------------------------------------------

    #[test]
    fn track_debug_does_not_panic() {
        let (mixer, id) = mixer_with_unity_track();
        let track = mixer.track(id).unwrap();
        let debug = format!("{track:?}");
        assert!(debug.contains("TestSynth"));
    }

    #[test]
    fn mixer_debug_does_not_panic() {
        let (mixer, _id) = mixer_with_unity_track();
        let debug = format!("{mixer:?}");
        assert!(debug.contains("Mixer"));
    }

    // -- Edge cases ----------------------------------------------------------

    #[test]
    fn process_num_samples_capped_to_buffer_size() {
        let (mut mixer, _id) = mixer_with_unity_track();
        // Request more samples than the buffer size — should not panic.
        let input = [0.5_f32; 1024];
        mixer.process(&input, 1024, 0, false, false);
    }

    #[test]
    fn tracks_slice_returns_all_tracks() {
        let mut mixer = Mixer::new();
        mixer.add_track(
            "A".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.add_track(
            "B".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.add_track(
            "C".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );

        let tracks = mixer.tracks();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].name(), "A");
        assert_eq!(tracks[1].name(), "B");
        assert_eq!(tracks[2].name(), "C");
    }

    #[test]
    fn effects_chain_accessible_via_track() {
        let (mut mixer, id) = mixer_with_unity_track();
        let track = mixer.track_mut(id).unwrap();
        assert!(track.effects().is_empty());

        // We can add effects.
        track.effects_mut().push(Box::new(TestSynth::new(0.5)));
        assert_eq!(track.effects().len(), 1);
    }

    // -- Multi-layer processing tests ----------------------------------------

    #[test]
    fn multi_layer_sums_enabled_layers() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Multi".into(),
            Box::new(TestSynth::new(0.5)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);

        // Add a second layer with the same gain.
        mixer.track_mut(id).unwrap().add_layer(
            Box::new(TestSynth::new(0.5)),
            SynthesisMode::Wavetable,
            "Layer 2".into(),
        );
        assert_eq!(mixer.track_mut(id).unwrap().layer_count(), 2);

        mixer.prepare(44_100.0, 64);
        let input = [1.0_f32; 128]; // stereo: 64 samples × 2 channels
        mixer.process(&input, 64, 0, false, false);
        let master = mixer.master_buffer();

        // Both layers process at gain 0.5 with unity dB.
        // Layer 1: input * 0.5 = 0.5
        // Layer 2: input * 0.5 = 0.5
        // Sum: 1.0
        // Then through volume (0 dB = 1.0) and center pan.
        // Center pan: L = signal * cos(pi/4), R = signal * cos(pi/4)
        let expected = 1.0 * FRAC_1_SQRT_2;
        for &sample in &master[..128] {
            assert!(
                (sample - expected).abs() < 0.01,
                "expected ~{expected}, got {sample}",
            );
        }
    }

    #[test]
    fn disabled_layer_not_processed() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Multi".into(),
            Box::new(TestSynth::new(0.5)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);

        // Add a second layer with gain 100 — would dominate output if enabled.
        mixer.track_mut(id).unwrap().add_layer(
            Box::new(TestSynth::new(100.0)),
            SynthesisMode::Wavetable,
            "Loud".into(),
        );

        // Disable the loud layer.
        let track = mixer.track_mut(id).unwrap();
        track.layers_mut()[1].set_enabled(false);

        mixer.prepare(44_100.0, 64);
        let input = [1.0_f32; 128];
        mixer.process(&input, 64, 0, false, false);
        let master = mixer.master_buffer();

        // Only layer 0 (gain 0.5) should contribute.
        let expected = 0.5 * FRAC_1_SQRT_2;
        for &sample in &master[..128] {
            assert!(
                (sample - expected).abs() < 0.01,
                "disabled layer should not contribute: expected ~{expected}, got {sample}",
            );
        }
    }

    #[test]
    fn layer_gain_scales_contribution() {
        let mut mixer = Mixer::new();
        let id = mixer.add_track(
            "Multi".into(),
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::PitchTracked,
        );
        mixer.track_mut(id).unwrap().set_armed(true);

        // Add a second layer at -6 dB (approx 0.5 linear).
        mixer.track_mut(id).unwrap().add_layer(
            Box::new(TestSynth::new(1.0)),
            SynthesisMode::Wavetable,
            "Quiet".into(),
        );
        let track = mixer.track_mut(id).unwrap();
        track.layers_mut()[1].set_gain(Db::new(-6.0));

        mixer.prepare(44_100.0, 64);
        let input = [1.0_f32; 128];
        mixer.process(&input, 64, 0, false, false);
        let master = mixer.master_buffer();

        // Layer 0: 1.0 * 1.0 (unity dB) = 1.0
        // Layer 1: 1.0 * ~0.501 (-6 dB) ≈ 0.501
        // Sum ≈ 1.501, then center pan (* cos(pi/4)).
        let layer1_gain = Db::new(-6.0).to_linear();
        let expected = (1.0 + layer1_gain) * FRAC_1_SQRT_2;
        for &sample in &master[..128] {
            assert!(
                (sample - expected).abs() < 0.05,
                "layer gain should scale: expected ~{expected:.3}, got {sample:.3}",
            );
        }
    }

    #[test]
    fn remove_layer_zero_rejected() {
        let (mut mixer, id) = mixer_with_unity_track();
        let track = mixer.track_mut(id).unwrap();
        assert!(!track.remove_layer(0));
        assert_eq!(track.layer_count(), 1);
    }

    #[test]
    fn remove_layer_out_of_bounds_rejected() {
        let (mut mixer, id) = mixer_with_unity_track();
        let track = mixer.track_mut(id).unwrap();
        assert!(!track.remove_layer(10));
    }

    #[test]
    fn add_layer_respects_max() {
        let (mut mixer, id) = mixer_with_unity_track();
        let track = mixer.track_mut(id).unwrap();
        for i in 1..crate::MAX_SYNTH_LAYERS {
            assert!(
                track.add_layer(
                    Box::new(TestSynth::new(1.0)),
                    SynthesisMode::PitchTracked,
                    format!("Layer {}", i + 1),
                ),
                "should be able to add layer {i}"
            );
        }
        assert_eq!(track.layer_count(), crate::MAX_SYNTH_LAYERS);
        assert!(
            !track.add_layer(
                Box::new(TestSynth::new(1.0)),
                SynthesisMode::PitchTracked,
                "Too Many".into(),
            ),
            "should reject beyond MAX_SYNTH_LAYERS"
        );
    }
}
