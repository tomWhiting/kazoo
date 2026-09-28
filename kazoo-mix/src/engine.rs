//! Mixer engine for `kazoo-mix`.
//!
//! The engine is owned by the CPAL output callback. All storage is allocated
//! in [`MixerEngine::new`], before the stream starts; rendering performs fixed
//! work over the configured channel slots and never allocates, frees, locks,
//! blocks, or performs I/O. Consumers leaving the engine are handed back to the
//! caller so they can be dropped off the audio thread.
//!
//! Signal flow per strip:
//!
//! ```text
//! source ─ trim ─ 3-band EQ ─(clip detect)─ fader·mute ─ pan ─┬─ mix bus
//!                                                              └─ × aux send ─ aux bus
//! aux bus ─ reverb ─ × aux return ─┐
//! mix bus ─────────────────────────┴─ master fader ─(clip detect)─ soft limit ─ out
//! ```
//!
//! Every gain glides linearly to a new value over a fixed 5 ms, and EQ gains
//! slew at a fixed rate in dB per second, both measured in audio frames, so
//! control moves never click and behave the same at any buffer size.
//!
//! Each strip keeps its source locked to the studio frame clock: it consumes
//! exactly the frames it renders (even when muted), discards stale audio,
//! leaves gaps for audio that starts later, and re-anchors a source whose
//! frame numbering has jumped (a restart, or a source that can no longer catch
//! up) instead of going silent for good.

use kazoo_core::audio_transport::{AudioBlockConsumer, AudioRingPopError, PoppedAudioBlock};
use kazoo_core::effects::Reverb;
use kazoo_core::protocol::ChannelId;
use kazoo_core::{Pan, Processor, sanitize_sample, soft_limit};

use crate::controls::{ChannelControls, MasterControls, db_to_gain, fader_db_to_gain};
use crate::eq::StereoEq;
use crate::metronome::Metronome;
use crate::shared::DEFAULT_BPM;
use crate::song::{SongAnchor, SongClock};

/// Frames rendered per internal chunk. Larger callbacks are split into
/// chunks of this size, so any device buffer size is fully rendered.
pub const MAX_RENDER_FRAMES: usize = 4096;

/// Largest source ring (in interleaved samples) a strip will accept. Every
/// strip pre-allocates a block buffer of this size so no block can ever be too
/// large to pop.
pub const MAX_SOURCE_RING_SAMPLES: usize = 65_536;

/// Sample magnitude at or above which a clip light latches (0 dBFS).
pub const CLIP_THRESHOLD: f32 = 0.999;

/// Length of a strip name in bytes.
pub const NAME_BYTES: usize = 12;

/// How fast EQ gains move toward a new setting, in dB per second of audio. A
/// full 30 dB sweep settles in 0.3 s at any buffer size.
const EQ_SLEW_DB_PER_SECOND: f32 = 100.0;

/// Length of every gain glide, in seconds of audio.
const RAMP_SECONDS: f32 = 0.005;

/// Interval between EQ coefficient updates while it slews: about 32 frames at
/// 48 kHz, fine enough that each step is inaudible, independent of how the
/// device chunks its buffers.
const EQ_UPDATE_SECONDS: f32 = 1.0 / 1500.0;

/// A source whose next block starts more than this many seconds ahead of the
/// studio clock is re-anchored instead of waited for.
const RESYNC_AHEAD_SECONDS: u64 = 1;

/// Aux reverb room size (0‥1).
const REVERB_ROOM_SIZE: f32 = 0.78;

/// Aux reverb damping (0‥1).
const REVERB_DAMPING: f32 = 0.42;

/// Freeverb's input scaling. The core reverb sums eight feedback combs without
/// it, which returns about +24 dB hotter than the send.
const REVERB_INPUT_GAIN: f32 = 0.015;

/// Freeverb's wet scaling, restoring a return level close to the send level.
const REVERB_WET_GAIN: f32 = 3.0;

/// Right-channel comb and allpass tunings are 2% longer than the left
/// (roughly 24–35 samples at 48 kHz), decorrelating the two sides the way
/// Freeverb's fixed stereo spread does.
const REVERB_STEREO_SPREAD: f32 = 1.02;

/// Linear stereo level pair.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StereoLevel {
    /// Left-channel value.
    pub left: f32,
    /// Right-channel value.
    pub right: f32,
}

impl StereoLevel {
    /// Zeroed stereo level.
    pub const ZERO: Self = Self {
        left: 0.0,
        right: 0.0,
    };

    /// The louder of the two sides.
    #[must_use]
    pub const fn max(self) -> f32 {
        if self.left > self.right {
            self.left
        } else {
            self.right
        }
    }
}

/// An attach that the engine refused, returning the consumer untouched so the
/// caller can drop it off the audio thread.
#[derive(Debug)]
pub struct AttachRejected {
    /// Why the attach failed.
    pub error: MixerEngineError,
    /// The consumer that was offered.
    pub consumer: AudioBlockConsumer,
}

/// Error returned by mixer engine operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerEngineError {
    /// Requested channel slot does not exist.
    InvalidSlot {
        /// Requested slot index.
        slot: usize,
    },
    /// The source ring is larger than a strip can buffer.
    SourceTooLarge {
        /// Ring capacity in interleaved samples.
        samples: usize,
        /// Largest accepted capacity.
        max: usize,
    },
    /// The aux reverb rejected one of its settings.
    Reverb {
        /// Reverb parameter index that was rejected.
        parameter: usize,
    },
}

/// A gain that glides linearly to a new target over a fixed number of frames.
///
/// The glide is counted in frames, not render calls, so it takes the same
/// time at any buffer size and carries on across chunk boundaries.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ramp {
    current: f32,
    target: f32,
    step: f32,
    remaining: usize,
}

impl Ramp {
    const fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    /// Glide to `target` over `frames` frames. Re-setting the same target
    /// leaves a glide in progress untouched.
    fn set_target(&mut self, target: f32, frames: usize) {
        let target = sanitize_sample(target);
        if target.to_bits() == self.target.to_bits() {
            return;
        }
        self.target = target;
        let frames = frames.max(1);
        self.step = (target - self.current) / frames as f32;
        self.remaining = frames;
        if !self.step.is_finite() {
            self.jump(target);
        }
    }

    fn next(&mut self) -> f32 {
        let value = self.current;
        self.advance(1);
        value
    }

    /// Move along the glide by `frames` frames (mixed or not), so the ramp
    /// stays in step with the audio clock.
    fn advance(&mut self, frames: usize) {
        if self.remaining == 0 {
            return;
        }
        let n = frames.min(self.remaining);
        self.current = self.step.mul_add(n as f32, self.current);
        self.remaining -= n;
        if self.remaining == 0 {
            self.current = self.target;
            self.step = 0.0;
        }
    }

    const fn jump(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.step = 0.0;
        self.remaining = 0;
    }
}

/// Running peak / sum-of-squares accumulator for one render call.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct MeterAccumulator {
    peak: StereoLevel,
    sum_sq: StereoLevel,
    frames: usize,
}

impl MeterAccumulator {
    fn add(&mut self, left: f32, right: f32) {
        self.peak.left = self.peak.left.max(left.abs());
        self.peak.right = self.peak.right.max(right.abs());
        self.sum_sq.left = left.mul_add(left, self.sum_sq.left);
        self.sum_sq.right = right.mul_add(right, self.sum_sq.right);
    }

    fn rms(&self) -> StereoLevel {
        if self.frames == 0 {
            return StereoLevel::ZERO;
        }
        let n = self.frames as f32;
        StereoLevel {
            left: sanitize_sample((self.sum_sq.left / n).sqrt()),
            right: sanitize_sample((self.sum_sq.right / n).sqrt()),
        }
    }
}

/// Mutable mixer engine owned by the audio callback.
#[derive(Debug)]
pub struct MixerEngine {
    channels: Vec<ChannelStrip>,
    ramp_frames: usize,
    next_frame: u64,
    master: MasterControls,
    master_gain: Ramp,
    aux_return: Ramp,
    aux_in_left: Vec<f32>,
    aux_in_right: Vec<f32>,
    aux_reverb: AuxReverb,
    metronome: Metronome,
    song: SongClock,
    master_meter: MeterAccumulator,
    master_peak: StereoLevel,
    master_rms: StereoLevel,
    master_clipped: bool,
}

impl MixerEngine {
    /// Create a new engine with fixed channel slots at `sample_rate`.
    ///
    /// # Errors
    ///
    /// Fails if the aux reverb rejects its settings.
    pub fn new(channel_slots: usize, sample_rate: u32) -> Result<Self, MixerEngineError> {
        let song = SongClock::new(sample_rate, f64::from(DEFAULT_BPM));
        let sample_rate = sample_rate.max(1) as f32;
        let mut channels = Vec::with_capacity(channel_slots);
        for idx in 0..channel_slots {
            channels.push(ChannelStrip::empty(
                ChannelId(u16::try_from(idx).unwrap_or(u16::MAX)),
                sample_rate,
            ));
        }

        let master = MasterControls::DEFAULT;
        Ok(Self {
            channels,
            ramp_frames: ramp_frames(sample_rate),
            next_frame: 0,
            master,
            master_gain: Ramp::new(fader_db_to_gain(master.fader_db)),
            aux_return: Ramp::new(master.aux_return),
            aux_in_left: vec![0.0; MAX_RENDER_FRAMES],
            aux_in_right: vec![0.0; MAX_RENDER_FRAMES],
            aux_reverb: AuxReverb::new(sample_rate)?,
            metronome: Metronome::new(sample_rate),
            song,
            master_meter: MeterAccumulator::default(),
            master_peak: StereoLevel::ZERO,
            master_rms: StereoLevel::ZERO,
            master_clipped: false,
        })
    }

    /// Switch the metronome click on or off.
    pub const fn set_click(&mut self, audible: bool) {
        self.metronome.set_audible(audible);
    }

    /// Schedule a transport change on studio frame `frame` (see
    /// [`SongClock::schedule`]) and return the anchor to tell the studio.
    pub fn schedule_transport(&mut self, frame: u64, playing: bool, bpm: f64) -> SongAnchor {
        self.song.schedule(frame, playing, bpm)
    }

    /// The song anchor in force now.
    #[must_use]
    pub const fn song(&self) -> SongAnchor {
        self.song.current()
    }

    /// Beat within the bar the metronome last struck, or `None` while
    /// stopped.
    #[must_use]
    pub fn metronome_beat(&self) -> Option<u32> {
        self.metronome.beat_in_bar()
    }

    /// Number of channel slots.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Master peak of the last render, after the limiter.
    #[must_use]
    pub const fn master_peak(&self) -> StereoLevel {
        self.master_peak
    }

    /// Master RMS of the last render, after the limiter.
    #[must_use]
    pub const fn master_rms(&self) -> StereoLevel {
        self.master_rms
    }

    /// Whether the master bus hit full scale before the limiter during the
    /// last render.
    #[must_use]
    pub const fn master_clipped(&self) -> bool {
        self.master_clipped
    }

    /// Current absolute output frame.
    #[must_use]
    pub const fn next_frame(&self) -> u64 {
        self.next_frame
    }

    /// Current master controls.
    #[must_use]
    pub const fn master_controls(&self) -> MasterControls {
        self.master
    }

    /// Current controls for a slot.
    #[must_use]
    pub fn channel_controls(&self, slot: usize) -> Option<ChannelControls> {
        self.channels.get(slot).map(|channel| channel.controls)
    }

    /// Channel snapshots for tests and non-callback status code. Allocates.
    #[must_use]
    pub fn channel_snapshots(&self) -> Vec<ChannelSnapshot> {
        self.channels.iter().map(ChannelStrip::snapshot).collect()
    }

    /// Copy channel snapshots into a caller-provided buffer without allocating.
    /// Returns the number of snapshots written.
    pub fn copy_channel_snapshots(&self, output: &mut [ChannelSnapshot]) -> usize {
        let mut written = 0;
        for (target, channel) in output.iter_mut().zip(self.channels.iter()) {
            *target = channel.snapshot();
            written += 1;
        }
        written
    }

    /// Attach an audio block consumer to a channel slot.
    ///
    /// On success, returns the consumer previously attached to the slot (if
    /// any). On failure, the offered consumer is handed back untouched.
    pub fn attach_consumer(
        &mut self,
        slot: usize,
        name: [u8; NAME_BYTES],
        consumer: AudioBlockConsumer,
    ) -> Result<Option<AudioBlockConsumer>, AttachRejected> {
        let capacity = consumer.config().sample_capacity();
        if capacity > MAX_SOURCE_RING_SAMPLES {
            return Err(AttachRejected {
                error: MixerEngineError::SourceTooLarge {
                    samples: capacity,
                    max: MAX_SOURCE_RING_SAMPLES,
                },
                consumer,
            });
        }
        let Some(channel) = self.channels.get_mut(slot) else {
            return Err(AttachRejected {
                error: MixerEngineError::InvalidSlot { slot },
                consumer,
            });
        };
        Ok(channel.attach(name, consumer))
    }

    /// Detach a channel slot, returning its consumer (if any).
    pub fn detach_consumer(
        &mut self,
        slot: usize,
    ) -> Result<Option<AudioBlockConsumer>, MixerEngineError> {
        self.channels
            .get_mut(slot)
            .map(ChannelStrip::detach)
            .ok_or(MixerEngineError::InvalidSlot { slot })
    }

    /// Update the controls for a slot. Values are sanitised.
    pub fn configure_channel(
        &mut self,
        slot: usize,
        controls: ChannelControls,
    ) -> Result<(), MixerEngineError> {
        let channel = self
            .channels
            .get_mut(slot)
            .ok_or(MixerEngineError::InvalidSlot { slot })?;
        channel.configure(controls);
        Ok(())
    }

    /// Update the master section. Values are sanitised.
    pub const fn configure_master(&mut self, controls: MasterControls) {
        self.master = controls.sanitized();
    }

    /// Render one self-contained block: opens a meter window, renders, and
    /// closes it. Any buffer length is accepted; a trailing partial frame is
    /// silenced.
    pub fn render_f32(&mut self, output: &mut [f32], output_channels: usize) {
        self.begin_meter_window();
        self.render_block(output, output_channels);
        self.end_meter_window();
    }

    /// Start a meter window: peaks, RMS and clip flags accumulate over every
    /// [`Self::render_block`] until [`Self::end_meter_window`].
    pub fn begin_meter_window(&mut self) {
        for channel in &mut self.channels {
            channel.begin_metering();
        }
        self.master_meter = MeterAccumulator::default();
        self.master_clipped = false;
    }

    /// Close the meter window and publish its peaks, RMS and clip flags to
    /// the snapshot accessors.
    pub fn end_meter_window(&mut self) {
        for channel in &mut self.channels {
            channel.finish_metering();
        }
        self.master_peak = self.master_meter.peak;
        self.master_rms = self.master_meter.rms();
    }

    /// Render into an interleaved `f32` buffer, accumulating meters into the
    /// open window. Any buffer length is accepted; a trailing partial frame is
    /// silenced.
    pub fn render_block(&mut self, output: &mut [f32], output_channels: usize) {
        let output_channels = output_channels.max(1);
        let frames = output.len() / output_channels;
        let render_len = frames * output_channels;
        output[render_len..].fill(0.0);

        // A soloed strip silences every other strip, connected or not, as on
        // a hardware console.
        let any_solo = self.channels.iter().any(|channel| channel.controls.soloed);
        for chunk in output[..render_len].chunks_mut(MAX_RENDER_FRAMES * output_channels) {
            self.render_chunk(chunk, output_channels, any_solo);
        }
    }

    fn render_chunk(&mut self, output: &mut [f32], output_channels: usize, any_solo: bool) {
        let frames = output.len() / output_channels;
        output.fill(0.0);
        let aux_left = &mut self.aux_in_left[..frames];
        let aux_right = &mut self.aux_in_right[..frames];
        aux_left.fill(0.0);
        aux_right.fill(0.0);

        let mut bus = BusTargets {
            output,
            output_channels,
            aux_left,
            aux_right,
        };
        for channel in &mut self.channels {
            let audible = !channel.controls.muted && (!any_solo || channel.controls.soloed);
            channel.render_into(self.next_frame, &mut bus, audible);
        }
        let BusTargets {
            output,
            aux_left,
            aux_right,
            ..
        } = bus;

        let (return_left, return_right) = self.aux_reverb.process(aux_left, aux_right);

        self.aux_return
            .set_target(self.master.aux_return, self.ramp_frames);
        self.master_gain
            .set_target(fader_db_to_gain(self.master.fader_db), self.ramp_frames);

        for frame in 0..frames {
            let base = frame * output_channels;
            let aux_gain = self.aux_return.next();
            let master_gain = self.master_gain.next();
            let wet_left = return_left[frame] * aux_gain;
            let wet_right = return_right[frame] * aux_gain;
            // The click is added after the master fader, so its level does
            // not depend on the mix.
            let beat = self
                .song
                .advance(self.next_frame.wrapping_add(frame as u64));
            let click = self.metronome.tick(beat, self.song.beats_per_frame());

            if output_channels == 1 {
                let mono = sanitize_sample(
                    (wet_left + wet_right)
                        .mul_add(0.5, output[base])
                        .mul_add(master_gain, click),
                );
                if mono.abs() >= CLIP_THRESHOLD {
                    self.master_clipped = true;
                }
                let limited = soft_limit(mono);
                output[base] = limited;
                self.master_meter.add(limited, limited);
            } else {
                let left = sanitize_sample((output[base] + wet_left).mul_add(master_gain, click));
                let right =
                    sanitize_sample((output[base + 1] + wet_right).mul_add(master_gain, click));
                if left.abs() >= CLIP_THRESHOLD || right.abs() >= CLIP_THRESHOLD {
                    self.master_clipped = true;
                }
                let left = soft_limit(left);
                let right = soft_limit(right);
                output[base] = left;
                output[base + 1] = right;
                self.master_meter.add(left, right);
            }
        }
        self.master_meter.frames += frames;
        self.next_frame = self.next_frame.wrapping_add(frames as u64);
    }
}

/// The aux return: a stereo Freeverb fed by the aux bus, level-calibrated so
/// the return sits close to the send level, with Freeverb's stereo spread.
#[derive(Debug)]
struct AuxReverb {
    left: Reverb,
    right: Reverb,
    scaled_left: Vec<f32>,
    scaled_right: Vec<f32>,
    out_left: Vec<f32>,
    out_right: Vec<f32>,
}

impl AuxReverb {
    fn new(sample_rate: f32) -> Result<Self, MixerEngineError> {
        Ok(Self {
            left: tuned_reverb(sample_rate)?,
            // The core reverb scales its comb tunings by sample rate, so a
            // slightly higher nominal rate lengthens every right-hand comb.
            right: tuned_reverb(sample_rate * REVERB_STEREO_SPREAD)?,
            scaled_left: vec![0.0; MAX_RENDER_FRAMES],
            scaled_right: vec![0.0; MAX_RENDER_FRAMES],
            out_left: vec![0.0; MAX_RENDER_FRAMES],
            out_right: vec![0.0; MAX_RENDER_FRAMES],
        })
    }

    /// Process one chunk of the aux bus. Both inputs must be the same length,
    /// at most [`MAX_RENDER_FRAMES`]; returns the wet return for that length.
    fn process(&mut self, send_left: &[f32], send_right: &[f32]) -> (&[f32], &[f32]) {
        let frames = send_left.len().min(send_right.len()).min(MAX_RENDER_FRAMES);
        for (scaled, send) in self.scaled_left[..frames].iter_mut().zip(send_left) {
            *scaled = send * REVERB_INPUT_GAIN;
        }
        for (scaled, send) in self.scaled_right[..frames].iter_mut().zip(send_right) {
            *scaled = send * REVERB_INPUT_GAIN;
        }
        self.left
            .process(&self.scaled_left[..frames], &mut self.out_left[..frames]);
        self.right
            .process(&self.scaled_right[..frames], &mut self.out_right[..frames]);
        for sample in &mut self.out_left[..frames] {
            *sample = sanitize_sample(*sample * REVERB_WET_GAIN);
        }
        for sample in &mut self.out_right[..frames] {
            *sample = sanitize_sample(*sample * REVERB_WET_GAIN);
        }
        (&self.out_left[..frames], &self.out_right[..frames])
    }
}

/// Frames between EQ slew steps at `sample_rate`.
fn eq_update_frames(sample_rate: f32) -> usize {
    ((sample_rate * EQ_UPDATE_SECONDS).round() as usize).max(1)
}

/// Frames in one gain glide at `sample_rate`.
fn ramp_frames(sample_rate: f32) -> usize {
    ((sample_rate * RAMP_SECONDS).round() as usize).max(1)
}

/// The aux return's reverb: 100% wet, since it is a return, never inline.
/// Runs once, before the stream starts.
fn tuned_reverb(sample_rate: f32) -> Result<Reverb, MixerEngineError> {
    let mut reverb = Reverb::new(sample_rate);
    for (index, value) in [(0, REVERB_ROOM_SIZE), (1, REVERB_DAMPING), (2, 1.0)] {
        reverb
            .set_param(index, value)
            .map_err(|_| MixerEngineError::Reverb { parameter: index })?;
    }
    Ok(reverb)
}

/// Encode a display name into a fixed buffer, truncating on a character
/// boundary so multi-byte names never split.
#[must_use]
pub fn short_name(name: &str) -> [u8; NAME_BYTES] {
    let mut out = [0_u8; NAME_BYTES];
    let mut len = 0;
    for ch in name.chars() {
        let width = ch.len_utf8();
        if len + width > NAME_BYTES {
            break;
        }
        ch.encode_utf8(&mut out[len..len + width]);
        len += width;
    }
    out
}

/// Decode a fixed name buffer written by [`short_name`].
#[must_use]
pub fn name_str(name: &[u8; NAME_BYTES]) -> &str {
    let len = name.iter().position(|b| *b == 0).unwrap_or(NAME_BYTES);
    // Names written by `short_name` are always valid UTF-8; anything else
    // (e.g. torn reads across atomics) decodes to its valid prefix.
    match std::str::from_utf8(&name[..len]) {
        Ok(text) => text,
        Err(err) => std::str::from_utf8(&name[..err.valid_up_to()]).unwrap_or(""),
    }
}

#[derive(Debug)]
struct ChannelStrip {
    id: ChannelId,
    name: [u8; NAME_BYTES],
    consumer: Option<AudioBlockConsumer>,
    controls: ChannelControls,
    eq: StereoEq,
    trim: Ramp,
    level: Ramp,
    pan_left: Ramp,
    pan_right: Ramp,
    aux_send: Ramp,
    source_channels: usize,
    meter: MeterAccumulator,
    peak: StereoLevel,
    rms: StereoLevel,
    clipped: bool,
    clipped_now: bool,
    slips: u64,
    resyncs: u64,
    faulted: bool,
    /// Offset added (wrapping) to source frame numbers to place them on the
    /// studio clock.
    anchor: u64,
    /// Frames in one gain glide.
    ramp_frames: usize,
    /// Frames between EQ slew steps.
    eq_update_frames: usize,
    /// Largest EQ move, in dB, per slew step.
    eq_step_db: f32,
    /// Frames until the next EQ slew step (0: due before the next frame).
    eq_countdown: usize,
    /// Consecutive whole blocks discarded as stale.
    stale_run: u32,
    /// Stale blocks in a row after which the source is re-anchored.
    resync_after_stale: u32,
    /// Most blocks popped in one render, bounding callback work.
    max_pops: usize,
    /// Largest lead a source may have before it is re-anchored.
    max_ahead_frames: u64,
    /// Furthest behind a source can be and still be catching up: a full ring
    /// plus the largest lead. Anything older is a restart, re-anchored at
    /// once.
    max_behind_frames: u64,
    buffered: Option<PoppedAudioBlock>,
    buffered_offset: usize,
    buffered_samples: Vec<f32>,
}

impl ChannelStrip {
    fn empty(id: ChannelId, sample_rate: f32) -> Self {
        let controls = ChannelControls::DEFAULT;
        let (pan_left, pan_right) = pan_gains(controls.pan, 2);
        Self {
            id,
            name: [0; NAME_BYTES],
            consumer: None,
            controls,
            eq: StereoEq::new(sample_rate),
            trim: Ramp::new(db_to_gain(controls.trim_db)),
            level: Ramp::new(fader_db_to_gain(controls.fader_db)),
            pan_left: Ramp::new(pan_left),
            pan_right: Ramp::new(pan_right),
            aux_send: Ramp::new(controls.aux_send),
            source_channels: 2,
            meter: MeterAccumulator::default(),
            peak: StereoLevel::ZERO,
            rms: StereoLevel::ZERO,
            clipped: false,
            clipped_now: false,
            slips: 0,
            resyncs: 0,
            faulted: false,
            anchor: 0,
            ramp_frames: ramp_frames(sample_rate),
            eq_update_frames: eq_update_frames(sample_rate),
            eq_step_db: EQ_SLEW_DB_PER_SECOND * eq_update_frames(sample_rate) as f32 / sample_rate,
            eq_countdown: 0,
            stale_run: 0,
            resync_after_stale: 2,
            max_pops: 4,
            max_ahead_frames: (sample_rate as u64).saturating_mul(RESYNC_AHEAD_SECONDS),
            max_behind_frames: (sample_rate as u64).saturating_mul(RESYNC_AHEAD_SECONDS),
            buffered: None,
            buffered_offset: 0,
            buffered_samples: vec![0.0; MAX_SOURCE_RING_SAMPLES],
        }
    }

    fn attach(
        &mut self,
        name: [u8; NAME_BYTES],
        consumer: AudioBlockConsumer,
    ) -> Option<AudioBlockConsumer> {
        let config = consumer.config();
        self.source_channels = usize::from(config.channels.max(1));
        let capacity_blocks = config.capacity_blocks.max(1);
        // A source may legitimately be a full ring behind; beyond that it
        // will not catch up and is re-anchored.
        self.resync_after_stale = capacity_blocks.max(2);
        let capacity = usize::try_from(capacity_blocks).unwrap_or(usize::MAX);
        self.max_pops = capacity.saturating_mul(2).saturating_add(2);
        let ring_frames = u64::from(capacity_blocks) * u64::from(config.block_frames.max(1));
        self.max_behind_frames = ring_frames.saturating_add(self.max_ahead_frames);
        let previous = self.consumer.replace(consumer);
        self.name = name;
        self.reset_runtime();
        // New source: start the gains where they will settle rather than
        // ramping up from the previous source's state.
        self.eq.set(self.controls.eq);
        self.trim.jump(db_to_gain(self.controls.trim_db));
        self.level.jump(0.0);
        let (left, right) = pan_gains(self.controls.pan, self.source_channels);
        self.pan_left.jump(left);
        self.pan_right.jump(right);
        self.aux_send.jump(self.controls.aux_send);
        previous
    }

    fn detach(&mut self) -> Option<AudioBlockConsumer> {
        self.name = [0; NAME_BYTES];
        self.reset_runtime();
        self.consumer.take()
    }

    fn reset_runtime(&mut self) {
        self.eq.reset();
        self.eq_countdown = 0;
        self.meter = MeterAccumulator::default();
        self.peak = StereoLevel::ZERO;
        self.rms = StereoLevel::ZERO;
        self.clipped = false;
        self.clipped_now = false;
        self.slips = 0;
        self.resyncs = 0;
        self.faulted = false;
        self.anchor = 0;
        self.stale_run = 0;
        self.buffered = None;
        self.buffered_offset = 0;
    }

    /// Store new controls. Gains ramp and EQ steps toward them during the
    /// following renders.
    fn configure(&mut self, controls: ChannelControls) {
        self.controls = controls.sanitized();
    }

    fn snapshot(&self) -> ChannelSnapshot {
        ChannelSnapshot {
            id: self.id,
            connected: self.consumer.is_some(),
            name: self.name,
            peak: self.peak,
            rms: self.rms,
            clipped: self.clipped,
            underruns: self
                .consumer
                .as_ref()
                .map_or(0, AudioBlockConsumer::underruns),
            slips: self.slips,
            resyncs: self.resyncs,
            faulted: self.faulted,
        }
    }

    fn begin_metering(&mut self) {
        self.meter = MeterAccumulator::default();
        self.clipped_now = false;
    }

    fn finish_metering(&mut self) {
        self.peak = self.meter.peak;
        self.rms = self.meter.rms();
        self.clipped = self.clipped_now;
    }

    /// Consume exactly the frames `[start_frame, start_frame + frames)` from
    /// the source, mixing them into the bus.
    ///
    /// Frames are consumed even when the strip is muted or solo-excluded, so
    /// the source never falls behind the studio clock. Blocks older than the
    /// requested window are discarded, blocks that start later leave a silent
    /// gap, and a missing block leaves the remainder silent. A source that
    /// stays a full ring behind, or jumps more than a second ahead, is
    /// re-anchored to the studio clock.
    fn render_into(&mut self, start_frame: u64, bus: &mut BusTargets<'_>, audible: bool) {
        let frames = bus.aux_left.len();
        if self.consumer.is_none() || self.faulted {
            return;
        }
        self.set_ramp_targets(audible);

        let mut pos = 0_usize;
        let mut blocks_popped = 0_usize;
        // Re-anchoring places the block exactly at the expected frame, so a
        // block never needs it twice; if one ever did, drop it rather than
        // spin.
        let mut reanchored_block = false;
        while pos < frames {
            if self.buffered.is_none() {
                if blocks_popped >= self.max_pops {
                    break;
                }
                blocks_popped += 1;
                reanchored_block = false;
            }
            let Some(block) = self.current_block() else {
                break;
            };
            let block_frames = block.header.frames as usize;
            if self.buffered_offset >= block_frames {
                self.buffered = None;
                continue;
            }
            let remaining = block_frames - self.buffered_offset;
            // Frame numbers wrap; compare them by signed distance so a block
            // that straddles the wrap plays through it.
            let source_pos = block
                .header
                .start_frame
                .wrapping_add(self.buffered_offset as u64);
            let block_pos = to_studio_frame(source_pos, self.anchor);
            let expected = start_frame.wrapping_add(pos as u64);
            // Two's-complement reinterpretation: the signed distance.
            let lead = i64::from_ne_bytes(block_pos.wrapping_sub(expected).to_ne_bytes());

            if lead < 0 {
                let behind = lead.unsigned_abs();
                if behind < remaining as u64 {
                    // Partly stale: skip forward to the frame we need.
                    self.slips = self.slips.wrapping_add(1);
                    self.buffered_offset += behind as usize;
                } else {
                    self.stale_run = self.stale_run.saturating_add(1);
                    let hopeless =
                        behind > self.max_behind_frames || self.stale_run > self.resync_after_stale;
                    if hopeless && !reanchored_block {
                        // Too far behind to ever catch up (or restarted):
                        // play this block now and keep following from it.
                        self.reanchor(source_pos, expected);
                        reanchored_block = true;
                    } else {
                        self.slips = self.slips.wrapping_add(1);
                        self.buffered = None;
                    }
                }
                continue;
            }
            if lead > 0 {
                let ahead = lead.unsigned_abs();
                if ahead > self.max_ahead_frames {
                    if reanchored_block {
                        self.slips = self.slips.wrapping_add(1);
                        self.buffered = None;
                    } else {
                        self.reanchor(source_pos, expected);
                        reanchored_block = true;
                    }
                    continue;
                }
                // Source is ahead: leave a silent gap until its block starts.
                self.slips = self.slips.wrapping_add(1);
                let gap = usize::try_from(ahead)
                    .unwrap_or(usize::MAX)
                    .min(frames - pos);
                self.advance_ramps(gap);
                pos += gap;
                continue;
            }

            self.stale_run = 0;
            let count = remaining.min(frames - pos);
            self.mix_block_frames(block, pos, count, bus);
            pos += count;
            self.buffered_offset += count;
            if self.buffered_offset >= block_frames {
                self.buffered = None;
            }
        }
        self.meter.frames += frames;
        // Frames left unmixed (underrun or pop limit) still pass in time.
        self.advance_ramps(frames - pos.min(frames));
    }

    /// Place `source_frame` at `studio_frame` from now on. Wrapping
    /// arithmetic makes this exact for any pair of frame numbers.
    const fn reanchor(&mut self, source_frame: u64, studio_frame: u64) {
        self.anchor = studio_frame.wrapping_sub(source_frame);
        self.resyncs = self.resyncs.wrapping_add(1);
        self.stale_run = 0;
    }

    fn advance_ramps(&mut self, frames: usize) {
        self.tick_eq(frames);
        self.trim.advance(frames);
        self.level.advance(frames);
        self.pan_left.advance(frames);
        self.pan_right.advance(frames);
        self.aux_send.advance(frames);
    }

    /// Let `frames` of time pass for the EQ slew: one bounded step every
    /// `eq_update_frames`, wherever the buffer boundaries fall.
    fn tick_eq(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        if frames <= self.eq_countdown {
            self.eq_countdown -= frames;
            return;
        }
        // Steps fall at offsets eq_countdown, + eq_update_frames, ... below
        // `frames`. With no audio between them (a gap), they combine exactly
        // into one move.
        let steps = 1 + (frames - 1 - self.eq_countdown) / self.eq_update_frames;
        self.eq_countdown = self.eq_countdown + steps * self.eq_update_frames - frames;
        let target = self.controls.eq.clamped();
        if self.eq.settings() != target {
            self.eq.step_toward(target, self.eq_step_db * steps as f32);
        }
    }

    fn set_ramp_targets(&mut self, audible: bool) {
        let frames = self.ramp_frames;
        self.trim
            .set_target(db_to_gain(self.controls.trim_db), frames);
        let level = if audible {
            fader_db_to_gain(self.controls.fader_db)
        } else {
            0.0
        };
        self.level.set_target(level, frames);
        let (pan_left, pan_right) = pan_gains(self.controls.pan, self.source_channels);
        self.pan_left.set_target(pan_left, frames);
        self.pan_right.set_target(pan_right, frames);
        self.aux_send.set_target(self.controls.aux_send, frames);
    }

    /// The block being read, popping the next one from the source if needed.
    fn current_block(&mut self) -> Option<PoppedAudioBlock> {
        if self.buffered.is_none() {
            let consumer = self.consumer.as_mut()?;
            match consumer.pop_block(&mut self.buffered_samples) {
                Ok(block) => {
                    self.buffered = Some(block);
                    self.buffered_offset = 0;
                }
                Err(AudioRingPopError::Empty) => return None,
                Err(AudioRingPopError::OutputTooSmall { .. }) => {
                    // Impossible given the attach-time capacity check, but a
                    // stuck block would wedge this strip forever; stop reading
                    // and show the fault instead.
                    self.faulted = true;
                    return None;
                }
            }
        }
        self.buffered
    }

    /// Mix `count` frames of the buffered block, starting at the current read
    /// offset, into the bus starting at frame `pos`.
    fn mix_block_frames(
        &mut self,
        block: PoppedAudioBlock,
        pos: usize,
        count: usize,
        bus: &mut BusTargets<'_>,
    ) {
        let input_channels = usize::from(block.header.channels.max(1));
        let valid_samples = block.samples_copied.min(self.buffered_samples.len());
        for i in 0..count {
            let in_base = (self.buffered_offset + i) * input_channels;
            let left = if in_base < valid_samples {
                self.buffered_samples[in_base]
            } else {
                0.0
            };
            let right = if input_channels > 1 && in_base + 1 < valid_samples {
                self.buffered_samples[in_base + 1]
            } else {
                left
            };
            self.mix_frame(left, right, pos + i, bus);
        }
    }

    fn mix_frame(&mut self, left: f32, right: f32, frame: usize, bus: &mut BusTargets<'_>) {
        self.tick_eq(1);
        let trim = self.trim.next();
        let level = self.level.next();
        let pan_left = self.pan_left.next();
        let pan_right = self.pan_right.next();
        let aux_send = self.aux_send.next();

        let (left, right) = self
            .eq
            .process(sanitize_sample(left) * trim, sanitize_sample(right) * trim);
        if left.abs() >= CLIP_THRESHOLD || right.abs() >= CLIP_THRESHOLD {
            self.clipped_now = true;
        }

        let (out_left, out_right) = if self.source_channels == 1 {
            let mono = left * level;
            (mono * pan_left, mono * pan_right)
        } else {
            (left * level * pan_left, right * level * pan_right)
        };
        let out_left = sanitize_sample(out_left);
        let out_right = sanitize_sample(out_right);

        let base = frame * bus.output_channels;
        if bus.output_channels == 1 {
            bus.output[base] = (out_left + out_right).mul_add(0.5, bus.output[base]);
        } else {
            bus.output[base] += out_left;
            bus.output[base + 1] += out_right;
        }
        bus.aux_left[frame] = out_left.mul_add(aux_send, bus.aux_left[frame]);
        bus.aux_right[frame] = out_right.mul_add(aux_send, bus.aux_right[frame]);
        self.meter.add(out_left, out_right);
    }
}

/// Where strips write during one render chunk: the interleaved mix bus and
/// the stereo aux bus. All slices cover the same frames.
struct BusTargets<'a> {
    output: &'a mut [f32],
    output_channels: usize,
    aux_left: &'a mut [f32],
    aux_right: &'a mut [f32],
}

/// A source frame number on the studio clock.
const fn to_studio_frame(source_frame: u64, anchor: u64) -> u64 {
    source_frame.wrapping_add(anchor)
}

/// Pan gains for a source: equal-power pan for mono sources, balance for
/// stereo sources (unity at centre, so a stereo image is not attenuated).
fn pan_gains(pan: Pan, source_channels: usize) -> (f32, f32) {
    if source_channels == 1 {
        pan.gains()
    } else {
        let p = pan.value();
        ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
    }
}

/// UI-safe snapshot of a channel strip's runtime state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelSnapshot {
    /// Channel id.
    pub id: ChannelId,
    /// Whether an audio consumer is attached.
    pub connected: bool,
    /// Fixed-size short name buffer.
    pub name: [u8; NAME_BYTES],
    /// Post-fader peak of the last render.
    pub peak: StereoLevel,
    /// Post-fader RMS of the last render.
    pub rms: StereoLevel,
    /// Whether the strip hit full scale (post-EQ, pre-fader) in the last render.
    pub clipped: bool,
    /// Empty-ring pops observed by the source consumer.
    pub underruns: u64,
    /// Timing corrections: stale audio skipped or silent gaps inserted.
    pub slips: u64,
    /// Times the source was re-anchored to the studio clock.
    pub resyncs: u64,
    /// The strip stopped reading because its source misbehaved.
    pub faulted: bool,
}

impl ChannelSnapshot {
    /// Empty disconnected snapshot for fixed-size status buffers.
    pub const EMPTY: Self = Self {
        id: ChannelId(0),
        connected: false,
        name: [0; NAME_BYTES],
        peak: StereoLevel::ZERO,
        rms: StereoLevel::ZERO,
        clipped: false,
        underruns: 0,
        slips: 0,
        resyncs: 0,
        faulted: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::FADER_OFF_DB;
    use crate::eq::EqSettings;
    use crate::test_support::{assert_float_eq, assert_floats_eq};
    use kazoo_core::audio_transport::{
        AudioBlock, AudioBlockProducer, AudioRingConfig, audio_block_ring,
    };
    use kazoo_core::protocol::{AudioBlockHeader, BlockFlags, BufferId};

    /// Test sample rate: a 5 ms glide is exactly 4 frames, and 1 s is 800.
    const RATE: u32 = 800;

    fn ring(
        channels: u16,
        block_frames: u32,
        blocks: u32,
    ) -> (AudioBlockProducer, AudioBlockConsumer) {
        audio_block_ring(AudioRingConfig::new(
            BufferId(1),
            channels,
            block_frames,
            blocks,
        ))
    }

    fn push(producer: &mut AudioBlockProducer, start_frame: u64, channels: u16, samples: &[f32]) {
        let frames = u32::try_from(samples.len() / usize::from(channels)).unwrap();
        producer
            .push_block(AudioBlock {
                header: AudioBlockHeader {
                    start_frame,
                    frames,
                    channels,
                    sequence: 0,
                    flags: BlockFlags::default(),
                },
                samples,
            })
            .unwrap();
    }

    fn engine_with_source(
        channels: u16,
        block_frames: u32,
        blocks: u32,
    ) -> (MixerEngine, AudioBlockProducer) {
        let (producer, consumer) = ring(channels, block_frames, blocks);
        let mut engine = MixerEngine::new(2, RATE).unwrap();
        engine
            .attach_consumer(0, short_name("test"), consumer)
            .unwrap();
        // Warm the ramps so gain starts at its settled value.
        let mut warm = [0.0; 8];
        engine.render_f32(&mut warm, 2);
        (engine, producer)
    }

    fn left_samples(output: &[f32]) -> Vec<f32> {
        output.iter().step_by(2).copied().collect()
    }

    #[test]
    fn empty_engine_renders_silence_and_advances_frame() {
        let mut engine = MixerEngine::new(2, RATE).unwrap();
        let mut output = [1.0; 16];
        engine.render_f32(&mut output, 2);
        assert_floats_eq(&output, &[0.0; 16]);
        assert_eq!(engine.next_frame(), 8);
        assert_eq!(engine.master_peak(), StereoLevel::ZERO);
    }

    #[test]
    fn attached_channel_mixes_expected_block() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let start = engine.next_frame();
        push(&mut producer, start, 2, &[0.5; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.5).abs() < 1e-5), "{output:?}");
    }

    #[test]
    fn source_block_can_span_multiple_renders() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let start = engine.next_frame();
        push(&mut producer, start, 2, &[0.25; 8]);
        let mut first = [0.0; 4];
        engine.render_f32(&mut first, 2);
        let mut second = [0.0; 4];
        engine.render_f32(&mut second, 2);
        assert!(first.iter().chain(&second).all(|s| (s - 0.25).abs() < 1e-5));
        assert_eq!(engine.channel_snapshots()[0].slips, 0);
    }

    #[test]
    fn missing_block_renders_silence_and_counts_underrun() {
        let (_producer, consumer) = ring(2, 4, 2);
        let mut engine = MixerEngine::new(1, RATE).unwrap();
        engine
            .attach_consumer(0, short_name("t"), consumer)
            .unwrap();
        let mut output = [1.0; 8];
        engine.render_f32(&mut output, 2);
        assert_floats_eq(&output, &[0.0; 8]);
        assert_eq!(engine.channel_snapshots()[0].underruns, 1);
    }

    #[test]
    fn muted_channel_is_silent_but_keeps_consuming() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    muted: true,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut frame = engine.next_frame();
        // Two renders while muted: the first ramps down, the second is silent.
        for _ in 0..2 {
            push(&mut producer, frame, 2, &[0.5; 8]);
            let mut output = [0.0; 8];
            engine.render_f32(&mut output, 2);
            frame += 4;
        }
        push(&mut producer, frame, 2, &[0.5; 8]);
        let mut silent = [0.0; 8];
        engine.render_f32(&mut silent, 2);
        assert_floats_eq(&silent, &[0.0; 8]);
        frame += 4;

        // Regression: unmuting used to leave the strip permanently silent
        // because muted renders stopped reading the ring.
        engine
            .configure_channel(0, ChannelControls::DEFAULT)
            .unwrap();
        for _ in 0..2 {
            push(&mut producer, frame, 2, &[0.5; 8]);
            frame += 4;
        }
        let mut ramp = [0.0; 8];
        engine.render_f32(&mut ramp, 2);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.5).abs() < 1e-5), "{output:?}");
        assert_eq!(engine.channel_snapshots()[0].slips, 0);
    }

    #[test]
    fn strip_recovers_after_underrun() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        // Underrun: the engine advances with no audio available.
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        // The source now delivers the frames it would have produced (late),
        // then the current ones.
        let late = engine.next_frame() - 4;
        push(&mut producer, late, 2, &[0.9; 8]);
        push(&mut producer, engine.next_frame(), 2, &[0.5; 8]);
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.5).abs() < 1e-5), "{output:?}");
        assert_eq!(engine.channel_snapshots()[0].slips, 1);
    }

    #[test]
    fn partially_stale_block_is_trimmed_to_the_right_frame() {
        let (mut engine, mut producer) = engine_with_source(1, 8, 4);
        let now = engine.next_frame();
        // Block started two frames ago: frames 0 and 1 are stale.
        let samples: Vec<f32> = (0..8).map(|i| i as f32 * 0.01).collect();
        push(&mut producer, now - 2, 1, &samples);
        let mut output = [0.0; 4];
        engine.render_f32(&mut output, 1);
        let expected: Vec<f32> = (2..6).map(|i| i as f32 * 0.01 * 0.5).collect();
        for (got, want) in output.iter().zip(&expected) {
            // Mono source, centre equal-power pan: each side ~0.707, summed to
            // mono output as the average.
            assert!(
                (got - want * std::f32::consts::SQRT_2).abs() < 1e-4,
                "{got} vs {want}"
            );
        }
    }

    #[test]
    fn future_block_leaves_silent_gap_then_plays() {
        let (mut engine, mut producer) = engine_with_source(2, 2, 4);
        let now = engine.next_frame();
        push(&mut producer, now + 2, 2, &[0.5; 4]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert_eq!(&output[..4], &[0.0; 4]);
        assert!(output[4..].iter().all(|s| (s - 0.5).abs() < 1e-5));
    }

    #[test]
    fn fader_off_is_silent_after_ramp() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    fader_db: FADER_OFF_DB,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut frame = engine.next_frame();
        let mut ramp = [0.0; 8];
        push(&mut producer, frame, 2, &[0.5; 8]);
        engine.render_f32(&mut ramp, 2);
        frame += 4;
        // The ramp starts at unity and falls monotonically: no step.
        let left = left_samples(&ramp);
        assert!((left[0] - 0.5).abs() < 1e-5);
        assert!(left.windows(2).all(|w| w[1] <= w[0]));

        push(&mut producer, frame, 2, &[0.5; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert_floats_eq(&output, &[0.0; 8]);
    }

    #[test]
    fn trim_boost_increases_level() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    trim_db: 6.0,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut frame = engine.next_frame();
        let mut output = [0.0; 8];
        for _ in 0..2 {
            push(&mut producer, frame, 2, &[0.1; 8]);
            engine.render_f32(&mut output, 2);
            frame += 4;
        }
        let expected = 0.1 * db_to_gain(6.0);
        assert!(
            output.iter().all(|s| (s - expected).abs() < 1e-4),
            "{output:?}"
        );
    }

    #[test]
    fn eq_is_applied_to_the_strip() {
        let (mut engine, mut producer) = engine_with_source(2, 64, 16);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    eq: EqSettings {
                        low_db: -15.0,
                        ..EqSettings::FLAT
                    },
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        // DC is all "low": a -15 dB low shelf settles DC toward -15 dB.
        let mut frame = engine.next_frame();
        let mut output = vec![0.0; 128];
        for _ in 0..48 {
            push(&mut producer, frame, 2, &[0.5; 128]);
            engine.render_f32(&mut output, 2);
            frame += 64;
        }
        let settled = output[126];
        assert!(settled < 0.5 * db_to_gain(-12.0), "settled {settled}");
    }

    #[test]
    fn channel_meter_reflects_only_its_own_signal() {
        let (loud_prod, loud_cons) = ring(2, 4, 4);
        let (quiet_prod, quiet_cons) = ring(2, 4, 4);
        let mut engine = MixerEngine::new(2, RATE).unwrap();
        engine
            .attach_consumer(0, short_name("loud"), loud_cons)
            .unwrap();
        engine
            .attach_consumer(1, short_name("quiet"), quiet_cons)
            .unwrap();
        let (mut loud_prod, mut quiet_prod) = (loud_prod, quiet_prod);
        let mut warm = [0.0; 8];
        engine.render_f32(&mut warm, 2);
        let now = engine.next_frame();
        push(&mut loud_prod, now, 2, &[0.8; 8]);
        push(&mut quiet_prod, now, 2, &[0.05; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        let snapshots = engine.channel_snapshots();
        assert!((snapshots[0].peak.left - 0.8).abs() < 1e-4);
        assert!((snapshots[1].peak.left - 0.05).abs() < 1e-4);
    }

    #[test]
    fn clip_latches_on_full_scale_and_clears_when_quiet() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        let mut frame = engine.next_frame();
        push(&mut producer, frame, 2, &[1.2; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        frame += 4;
        assert!(engine.channel_snapshots()[0].clipped);
        assert!(engine.master_clipped());
        assert!(output.iter().all(|s| s.abs() <= 1.0));

        push(&mut producer, frame, 2, &[0.1; 8]);
        engine.render_f32(&mut output, 2);
        assert!(!engine.channel_snapshots()[0].clipped);
    }

    #[test]
    fn aux_send_produces_a_reverb_tail() {
        let (mut engine, mut producer) = engine_with_source(2, 256, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    aux_send: 1.0,
                    fader_db: 0.0,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut frame = engine.next_frame();
        let mut output = vec![0.0; 512];
        push(&mut producer, frame, 2, &[0.5; 512]);
        engine.render_f32(&mut output, 2);
        frame += 256;
        // Input stops; only the reverb return remains.
        let mut tail_energy = 0.0;
        for _ in 0..8 {
            push(&mut producer, frame, 2, &[0.0; 512]);
            engine.render_f32(&mut output, 2);
            frame += 256;
            tail_energy += output.iter().map(|s| s * s).sum::<f32>();
        }
        assert!(tail_energy > 1e-6, "tail energy {tail_energy}");

        // With the return closed the same scenario leaves no tail.
        let (mut dry, mut dry_prod) = engine_with_source(2, 256, 8);
        dry.configure_master(MasterControls {
            aux_return: 0.0,
            ..MasterControls::DEFAULT
        });
        dry.configure_channel(
            0,
            ChannelControls {
                aux_send: 1.0,
                ..ChannelControls::DEFAULT
            },
        )
        .unwrap();
        let mut frame = dry.next_frame();
        push(&mut dry_prod, frame, 2, &[0.5; 512]);
        dry.render_f32(&mut output, 2);
        frame += 256;
        let mut dry_tail = 0.0;
        for _ in 0..8 {
            push(&mut dry_prod, frame, 2, &[0.0; 512]);
            dry.render_f32(&mut output, 2);
            frame += 256;
            dry_tail += output.iter().map(|s| s * s).sum::<f32>();
        }
        assert!(dry_tail < 1e-9, "dry tail {dry_tail}");
    }

    #[test]
    fn solo_silences_other_strips() {
        let (a_prod, a_cons) = ring(2, 4, 8);
        let (b_prod, b_cons) = ring(2, 4, 8);
        let (mut a_prod, mut b_prod) = (a_prod, b_prod);
        let mut engine = MixerEngine::new(2, RATE).unwrap();
        engine.attach_consumer(0, short_name("a"), a_cons).unwrap();
        engine.attach_consumer(1, short_name("b"), b_cons).unwrap();
        engine
            .configure_channel(
                1,
                ChannelControls {
                    soloed: true,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut output = [0.0; 8];
        let mut frame = engine.next_frame();
        for _ in 0..3 {
            push(&mut a_prod, frame, 2, &[0.4; 8]);
            push(&mut b_prod, frame, 2, &[0.1; 8]);
            engine.render_f32(&mut output, 2);
            frame += 4;
        }
        assert!(output.iter().all(|s| (s - 0.1).abs() < 1e-5), "{output:?}");
    }

    #[test]
    fn hard_pan_biases_energy_to_one_side() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    pan: Pan::new(-1.0),
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut frame = engine.next_frame();
        let mut output = [0.0; 8];
        for _ in 0..2 {
            push(&mut producer, frame, 2, &[0.5; 8]);
            engine.render_f32(&mut output, 2);
            frame += 4;
        }
        let left: f32 = output.iter().step_by(2).map(|s| s.abs()).sum();
        let right: f32 = output.iter().skip(1).step_by(2).map(|s| s.abs()).sum();
        assert!(left > 1.9 && right < 1e-5, "left {left} right {right}");
    }

    #[test]
    fn non_finite_source_samples_render_silence() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let now = engine.next_frame();
        // The ring sanitises on push; feed through the engine path anyway.
        push(&mut producer, now, 2, &[f32::NAN; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn large_callback_is_fully_rendered_in_chunks() {
        let frames = MAX_RENDER_FRAMES * 2 + 17;
        let (mut engine, mut producer) = engine_with_source(2, 1024, 16);
        let now = engine.next_frame();
        let mut pushed = 0;
        while pushed < frames {
            push(&mut producer, now + pushed as u64, 2, &[0.3; 2048]);
            pushed += 1024;
        }
        let mut output = vec![0.0; frames * 2];
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.3).abs() < 1e-5));
        assert_eq!(engine.next_frame(), now + frames as u64);
    }

    #[test]
    fn extra_output_channels_are_silent_and_partial_frame_zeroed() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let now = engine.next_frame();
        push(&mut producer, now, 2, &[0.5; 8]);
        let mut output = [9.0; 4 * 4 + 1];
        engine.render_f32(&mut output, 4);
        for frame in output[..16].chunks(4) {
            assert!((frame[0] - 0.5).abs() < 1e-5 && (frame[1] - 0.5).abs() < 1e-5);
            assert_eq!(&frame[2..], &[0.0, 0.0]);
        }
        assert_float_eq(output[16], 0.0);
    }

    #[test]
    fn attach_to_invalid_slot_returns_the_consumer() {
        let (_producer, consumer) = ring(2, 4, 2);
        let mut engine = MixerEngine::new(1, RATE).unwrap();
        let rejected = engine
            .attach_consumer(5, short_name("x"), consumer)
            .unwrap_err();
        assert_eq!(rejected.error, MixerEngineError::InvalidSlot { slot: 5 });
        assert_eq!(rejected.consumer.config().block_frames, 4);
    }

    #[test]
    fn oversized_source_is_rejected() {
        let (_producer, consumer) = ring(2, 8_192, 8);
        let mut engine = MixerEngine::new(1, RATE).unwrap();
        let rejected = engine
            .attach_consumer(0, short_name("big"), consumer)
            .unwrap_err();
        assert!(matches!(
            rejected.error,
            MixerEngineError::SourceTooLarge { .. }
        ));
    }

    #[test]
    fn replacing_and_detaching_return_previous_consumers() {
        let (_p1, c1) = ring(2, 4, 2);
        let (_p2, c2) = ring(2, 4, 2);
        let mut engine = MixerEngine::new(1, RATE).unwrap();
        assert!(
            engine
                .attach_consumer(0, short_name("one"), c1)
                .unwrap()
                .is_none()
        );
        assert!(
            engine
                .attach_consumer(0, short_name("two"), c2)
                .unwrap()
                .is_some()
        );
        assert_eq!(name_str(&engine.channel_snapshots()[0].name), "two");
        assert!(engine.detach_consumer(0).unwrap().is_some());
        assert_eq!(
            engine.detach_consumer(9).unwrap_err(),
            MixerEngineError::InvalidSlot { slot: 9 }
        );
        let snapshot = engine.channel_snapshots()[0];
        assert!(!snapshot.connected);
        assert_eq!(name_str(&snapshot.name), "");
    }

    #[test]
    fn short_name_truncates_on_char_boundary() {
        let name = short_name("ヤマハCS80"); // 3 × 3-byte chars + 4 ASCII = 13 bytes
        assert_eq!(name_str(&name), "ヤマハCS8");
        assert_eq!(name_str(&short_name("")), "");
    }

    #[test]
    fn name_str_survives_invalid_utf8() {
        let mut name = short_name("ok");
        name[2] = 0xFF;
        assert_eq!(name_str(&name), "ok");
    }

    #[test]
    fn master_fader_scales_output() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine.configure_master(MasterControls {
            fader_db: -6.0,
            ..MasterControls::DEFAULT
        });
        let mut frame = engine.next_frame();
        let mut output = [0.0; 8];
        for _ in 0..2 {
            push(&mut producer, frame, 2, &[0.5; 8]);
            engine.render_f32(&mut output, 2);
            frame += 4;
        }
        let expected = 0.5 * db_to_gain(-6.0);
        assert!(
            output.iter().all(|s| (s - expected).abs() < 1e-3),
            "{output:?}"
        );
    }

    #[test]
    fn ramp_stays_in_step_across_a_gap() {
        let (mut engine, mut producer) = engine_with_source(2, 8, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    fader_db: FADER_OFF_DB,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        // The next block starts 2 frames late while the fader glides from
        // unity to off over 4 frames: the gap uses up half the glide.
        let now = engine.next_frame();
        push(&mut producer, now + 2, 2, &[0.5; 16]);
        let mut output = [0.0; 16];
        engine.render_f32(&mut output, 2);
        let left = left_samples(&output);
        assert_eq!(&left[..2], &[0.0; 2]);
        assert!((left[2] - 0.25).abs() < 1e-6, "{left:?}");
        assert!((left[3] - 0.125).abs() < 1e-6, "{left:?}");
        assert!(left[4..].iter().all(|s| *s == 0.0), "{left:?}");
    }

    #[test]
    fn glide_carries_on_across_render_calls() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    fader_db: FADER_OFF_DB,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let now = engine.next_frame();
        push(&mut producer, now, 2, &[0.5; 8]);
        let mut first = [0.0; 4];
        engine.render_f32(&mut first, 2);
        let mut second = [0.0; 4];
        engine.render_f32(&mut second, 2);
        let left: Vec<f32> = left_samples(&first)
            .into_iter()
            .chain(left_samples(&second))
            .collect();
        let expected = [0.5, 0.375, 0.25, 0.125];
        for (got, want) in left.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{left:?}");
        }
    }

    #[test]
    fn huge_source_frame_numbers_cannot_hang_the_render() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let far = 1_u64 << 62;
        push(&mut producer, far, 2, &[0.5; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.5).abs() < 1e-5), "{output:?}");
        assert_eq!(engine.channel_snapshots()[0].resyncs, 1);
        // Following blocks continue from the new anchor.
        push(&mut producer, far + 4, 2, &[0.25; 8]);
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.25).abs() < 1e-5), "{output:?}");
        assert_eq!(engine.channel_snapshots()[0].resyncs, 1);
    }

    /// A source that restarts far behind, or is numbered half the frame
    /// space away, re-anchors on its first block, like one far ahead.
    #[test]
    fn a_hopelessly_distant_source_re_anchors_at_once() {
        for offset in [(1_u64 << 63) + 10, u64::MAX - 5_000, 1_u64 << 62] {
            let (mut engine, mut producer) = engine_with_source(2, 4, 4);
            let start = engine.next_frame().wrapping_add(offset);
            push(&mut producer, start, 2, &[0.5; 8]);
            let mut output = [0.0; 8];
            engine.render_f32(&mut output, 2);
            assert!(
                output.iter().all(|s| (s - 0.5).abs() < 1e-5),
                "{offset}: {output:?}"
            );
            let snapshot = engine.channel_snapshots()[0];
            assert_eq!((snapshot.resyncs, snapshot.slips), (1, 0), "{offset}");
        }
    }

    #[test]
    fn a_block_straddling_the_frame_wrap_plays_through_it() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        // A source numbered just below the wrap is a few frames behind the
        // studio clock: four stale blocks, then the fifth re-anchors.
        for block in 0..4_u64 {
            push(&mut producer, u64::MAX - 17 + block * 4, 2, &[0.1; 8]);
        }
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert_floats_eq(&output, &[0.0; 8]);
        // Frames MAX-1, MAX, 0, 1 played over two renders of two frames.
        push(&mut producer, u64::MAX - 1, 2, &[0.5; 8]);
        let mut first = [0.0; 4];
        engine.render_f32(&mut first, 2);
        let mut second = [0.0; 4];
        engine.render_f32(&mut second, 2);
        assert!(first.iter().all(|s| (s - 0.5).abs() < 1e-5), "{first:?}");
        assert!(second.iter().all(|s| (s - 0.5).abs() < 1e-5), "{second:?}");
        let snapshot = engine.channel_snapshots()[0];
        assert_eq!(snapshot.resyncs, 1);
        // The next block, numbered after the wrap, follows on seamlessly.
        let slips = snapshot.slips;
        push(&mut producer, 2, 2, &[0.25; 8]);
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.25).abs() < 1e-5), "{output:?}");
        assert_eq!(engine.channel_snapshots()[0].slips, slips);
    }

    /// EQ moves step at a fixed frame interval, so the same audio rendered
    /// in one large buffer or many small ones comes out the same.
    #[test]
    fn eq_slew_does_not_depend_on_buffer_size() {
        fn render_sweep(chunk_frames: usize) -> Vec<f32> {
            const FRAMES: usize = 4096;
            let (mut producer, consumer) = ring(2, 4096, 2);
            let mut engine = MixerEngine::new(1, 48_000).unwrap();
            engine
                .attach_consumer(0, short_name("sweep"), consumer)
                .unwrap();
            let samples: Vec<f32> = (0..FRAMES * 2)
                .map(|i| (i as f32 * 0.37).sin() * 0.5)
                .collect();
            push(&mut producer, 0, 2, &samples);
            engine
                .configure_channel(
                    0,
                    ChannelControls {
                        eq: EqSettings {
                            low_db: 15.0,
                            mid_db: -15.0,
                            high_db: 15.0,
                        },
                        ..ChannelControls::DEFAULT
                    },
                )
                .unwrap();
            let mut out = vec![0.0; FRAMES * 2];
            for chunk in out.chunks_mut(chunk_frames * 2) {
                engine.render_f32(chunk, 2);
            }
            out
        }
        let large = render_sweep(4096);
        for chunk_frames in [64, 100, 1] {
            let small = render_sweep(chunk_frames);
            let worst = large
                .iter()
                .zip(&small)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(worst < 1e-6, "chunk {chunk_frames}: differs by {worst}");
        }
    }

    #[test]
    fn source_that_restarts_its_clock_is_reanchored() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        // Run the studio clock well ahead of the source.
        let mut output = [0.0; 8];
        for _ in 0..50 {
            engine.render_f32(&mut output, 2);
        }
        // The source restarts at frame 0: every block is hopelessly stale.
        let mut source_frame = 0;
        let mut heard = false;
        for _ in 0..8 {
            while producer
                .push_block(AudioBlock {
                    header: AudioBlockHeader {
                        start_frame: source_frame,
                        frames: 4,
                        channels: 2,
                        sequence: 0,
                        flags: BlockFlags::default(),
                    },
                    samples: &[0.5; 8],
                })
                .is_ok()
            {
                source_frame += 4;
            }
            engine.render_f32(&mut output, 2);
            heard |= output.iter().any(|s| s.abs() > 0.4);
        }
        assert!(heard, "strip stayed silent after the source restarted");
        assert!(engine.channel_snapshots()[0].resyncs >= 1);
    }

    #[test]
    fn source_far_ahead_is_reanchored_immediately() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let now = engine.next_frame();
        push(&mut producer, now + u64::from(RATE) * 10, 2, &[0.5; 8]);
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        assert!(output.iter().all(|s| (s - 0.5).abs() < 1e-5), "{output:?}");
        assert_eq!(engine.channel_snapshots()[0].resyncs, 1);
    }

    #[test]
    fn a_ring_full_of_stale_blocks_is_bounded_work() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 4);
        let mut output = [0.0; 8];
        for _ in 0..10 {
            engine.render_f32(&mut output, 2);
        }
        for block in 0..4 {
            push(&mut producer, block * 4, 2, &[0.5; 8]);
        }
        engine.render_f32(&mut output, 2);
        // Four whole stale blocks are within the tolerated run: discarded,
        // counted, silent, and the render returns.
        assert_floats_eq(&output, &[0.0; 8]);
        assert_eq!(engine.channel_snapshots()[0].slips, 4);
        assert_eq!(engine.channel_snapshots()[0].resyncs, 0);
    }

    #[test]
    fn eq_changes_move_in_small_steps() {
        let (mut engine, _producer) = engine_with_source(2, 4, 4);
        engine
            .configure_channel(
                0,
                ChannelControls {
                    eq: EqSettings {
                        high_db: -15.0,
                        ..EqSettings::FLAT
                    },
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);
        // 4 frames at 800 Hz is 5 ms of audio: half a dB at 100 dB/s.
        let first_step = EQ_SLEW_DB_PER_SECOND * 4.0 / RATE as f32;
        assert!((engine.channels[0].eq.settings().high_db + first_step).abs() < 1e-5);
        for _ in 0..100 {
            engine.render_f32(&mut output, 2);
        }
        assert_eq!(engine.channels[0].eq.settings().high_db, -15.0);
    }

    #[test]
    fn soloing_an_empty_strip_silences_the_others() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        engine
            .configure_channel(
                1,
                ChannelControls {
                    soloed: true,
                    ..ChannelControls::DEFAULT
                },
            )
            .unwrap();
        let mut frame = engine.next_frame();
        let mut output = [0.0; 8];
        for _ in 0..3 {
            push(&mut producer, frame, 2, &[0.5; 8]);
            engine.render_f32(&mut output, 2);
            frame += 4;
        }
        assert_floats_eq(&output, &[0.0; 8]);
    }

    #[test]
    fn meter_window_spans_several_render_blocks() {
        let (mut engine, mut producer) = engine_with_source(2, 4, 8);
        let now = engine.next_frame();
        push(&mut producer, now, 2, &[1.5; 8]);
        push(&mut producer, now + 4, 2, &[0.1; 8]);
        engine.begin_meter_window();
        let mut first = [0.0; 8];
        engine.render_block(&mut first, 2);
        let mut second = [0.0; 8];
        engine.render_block(&mut second, 2);
        engine.end_meter_window();
        let snapshot = engine.channel_snapshots()[0];
        assert!(snapshot.clipped, "clip in the first block was lost");
        assert!(snapshot.peak.left >= 1.5 - 1e-5);
        assert!(engine.master_clipped());
    }

    #[test]
    fn aux_return_is_level_calibrated_and_stereo() {
        let mut aux = AuxReverb::new(48_000.0).unwrap();
        // Deterministic white-ish noise at about 0.1 RMS.
        let mut seed = 0x1234_5678_u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 - 0.5) * 0.346
        };
        let mut send_sq = 0.0_f64;
        let mut ret_sq = 0.0_f64;
        let mut cross = 0.0_f64;
        let mut ret_r_sq = 0.0_f64;
        let mut block = vec![0.0; 1024];
        for round in 0..96 {
            for sample in &mut block {
                *sample = noise();
            }
            let (left, right) = aux.process(&block, &block);
            if round >= 32 {
                for ((s, l), r) in block.iter().zip(left).zip(right) {
                    send_sq += f64::from(s * s);
                    ret_sq += f64::from(l * l);
                    ret_r_sq += f64::from(r * r);
                    cross += f64::from(l * r);
                }
            }
        }
        let gain_db = 10.0 * (ret_sq / send_sq).log10();
        assert!(
            (-9.0..=3.0).contains(&gain_db),
            "return is {gain_db:.1} dB vs send"
        );
        let correlation = cross / (ret_sq * ret_r_sq).sqrt();
        assert!(
            correlation < 0.9,
            "return is not stereo: correlation {correlation:.2}"
        );
    }
}
