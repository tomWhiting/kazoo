//! Lock-free state shared between the desk (UI thread) and the audio callback.
//!
//! Controls flow desk → callback; meters, clip lights and health counters flow
//! callback → desk. Everything is a relaxed atomic: each field is independently
//! valid, so a reader that sees a mix of old and new fields during an update
//! still sees only legal values, and nothing ever blocks the audio thread.
//!
//! Clip lights latch here rather than in the engine: the callback only ever
//! sets them, and the desk clears them when the engineer says so.
//!
//! Meters hand over losslessly: every callback folds its peak into an atomic
//! maximum and its energy into an atomic sum, and the desk drains both with a
//! swap each frame. Nothing a callback publishes between two desk reads is
//! ever dropped, however the two threads interleave.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use kazoo_core::Pan;

use crate::controls::{ChannelControls, MasterControls};
use crate::engine::{ChannelSnapshot, NAME_BYTES, StereoLevel};
use crate::eq::EqSettings;

/// Number of channel strips on the desk.
pub const DESK_CHANNELS: usize = 8;

const NAME_WORDS: usize = NAME_BYTES / 4;

/// An `f32` stored in an `AtomicU32`.
#[derive(Debug)]
struct AtomicF32(AtomicU32);

impl AtomicF32 {
    const fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// Fixed-point scale for accumulated energy (sum of squared samples), so it
/// can be summed with an integer atomic. Each callback's energy is rounded to
/// the nearest unit, so with 2^32 a 64-frame callback still registers signal
/// down to about −117 dBFS, far below the lowest meter mark (−60 dBFS). Full
/// scale accumulates for over 20 hours at 48 kHz before saturating; the desk
/// drains every frame.
const ENERGY_SCALE: f64 = 4_294_967_296.0;

/// Peak and RMS drained from a meter since the previous drain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterTake {
    /// Highest absolute sample value.
    pub peak: StereoLevel,
    /// RMS over every frame published.
    pub rms: StereoLevel,
    /// Frames published.
    pub frames: u64,
}

impl MeterTake {
    /// Nothing published.
    pub const SILENT: Self = Self {
        peak: StereoLevel::ZERO,
        rms: StereoLevel::ZERO,
        frames: 0,
    };
}

/// A meter the callback adds to and the desk drains.
#[derive(Debug)]
struct MeterCell {
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    energy_left: AtomicU64,
    energy_right: AtomicU64,
    frames: AtomicU64,
}

impl MeterCell {
    const fn new() -> Self {
        Self {
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            energy_left: AtomicU64::new(0),
            energy_right: AtomicU64::new(0),
            frames: AtomicU64::new(0),
        }
    }

    /// Fold in one callback's peak and RMS over `frames` frames. Lock-free:
    /// a maximum, two sums and a count.
    fn add(&self, peak: StereoLevel, rms: StereoLevel, frames: usize) {
        // Non-negative finite floats order exactly like their bit patterns.
        self.peak_left
            .fetch_max(peak_bits(peak.left), Ordering::Relaxed);
        self.peak_right
            .fetch_max(peak_bits(peak.right), Ordering::Relaxed);
        let n = frames as f64;
        add_saturating(&self.energy_left, energy_fixed(rms.left, n));
        add_saturating(&self.energy_right, energy_fixed(rms.right, n));
        add_saturating(&self.frames, frames as u64);
    }

    /// Drain everything added since the last drain.
    ///
    /// Energy and frame count are separate atomics, so a callback landing
    /// mid-drain can put its energy in this take and its frames in the next;
    /// the RMS of those two reads is slightly skewed, but nothing is lost.
    fn take(&self) -> MeterTake {
        let peak = StereoLevel {
            left: f32::from_bits(self.peak_left.swap(0, Ordering::Relaxed)),
            right: f32::from_bits(self.peak_right.swap(0, Ordering::Relaxed)),
        };
        let energy_left = self.energy_left.swap(0, Ordering::Relaxed);
        let energy_right = self.energy_right.swap(0, Ordering::Relaxed);
        let frames = self.frames.swap(0, Ordering::Relaxed);
        let rms = if frames == 0 {
            StereoLevel::ZERO
        } else {
            let denominator = ENERGY_SCALE * frames as f64;
            StereoLevel {
                left: kazoo_core::sanitize_sample((energy_left as f64 / denominator).sqrt() as f32),
                right: kazoo_core::sanitize_sample(
                    (energy_right as f64 / denominator).sqrt() as f32
                ),
            }
        };
        MeterTake { peak, rms, frames }
    }
}

const fn peak_bits(value: f32) -> u32 {
    if value.is_finite() && value > 0.0 {
        value.to_bits()
    } else {
        0
    }
}

fn energy_fixed(rms: f32, frames: f64) -> u64 {
    let rms = f64::from(rms);
    let energy = rms * rms * frames * ENERGY_SCALE;
    if energy.is_finite() && energy > 0.0 {
        // `as` saturates at u64::MAX.
        energy.round() as u64
    } else {
        0
    }
}

fn add_saturating(cell: &AtomicU64, value: u64) {
    if value == 0 {
        return;
    }
    // Lock-free: the only other writer is the desk's swap, so this retries at
    // most once in practice. Saturating, so a desk that stops draining can
    // never wrap a meter back to silence.
    let mut current = cell.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(value);
        match cell.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

const FLAG_MUTED: u32 = 0b01;
const FLAG_SOLOED: u32 = 0b10;

#[derive(Debug)]
struct SlotCell {
    trim_db: AtomicF32,
    eq_low_db: AtomicF32,
    eq_mid_db: AtomicF32,
    eq_high_db: AtomicF32,
    aux_send: AtomicF32,
    pan: AtomicF32,
    fader_db: AtomicF32,
    flags: AtomicU32,
    meter: MeterCell,
    clip_latched: AtomicBool,
    connected: AtomicBool,
    faulted: AtomicBool,
    underruns: AtomicU64,
    slips: AtomicU64,
    resyncs: AtomicU64,
    name: [AtomicU32; NAME_WORDS],
}

impl SlotCell {
    const fn new() -> Self {
        let d = ChannelControls::DEFAULT;
        Self {
            trim_db: AtomicF32::new(d.trim_db),
            eq_low_db: AtomicF32::new(d.eq.low_db),
            eq_mid_db: AtomicF32::new(d.eq.mid_db),
            eq_high_db: AtomicF32::new(d.eq.high_db),
            aux_send: AtomicF32::new(d.aux_send),
            pan: AtomicF32::new(0.0),
            fader_db: AtomicF32::new(d.fader_db),
            flags: AtomicU32::new(0),
            meter: MeterCell::new(),
            clip_latched: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            faulted: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            slips: AtomicU64::new(0),
            resyncs: AtomicU64::new(0),
            name: [const { AtomicU32::new(0) }; NAME_WORDS],
        }
    }
}

/// Runtime state of one strip as the desk sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelReadout {
    /// A source is attached.
    pub connected: bool,
    /// The strip stopped reading its source after a fault.
    pub faulted: bool,
    /// Source name bytes.
    pub name: [u8; NAME_BYTES],
    /// Clip light (latched until cleared).
    pub clip: bool,
    /// Empty-ring pops.
    pub underruns: u64,
    /// Timing corrections.
    pub slips: u64,
    /// Re-anchors to the studio clock.
    pub resyncs: u64,
}

/// Runtime state of the master bus as the desk sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterReadout {
    /// Master clip light (latched until cleared).
    pub clip: bool,
}

/// Shared desk ↔ callback state.
#[derive(Debug)]
pub struct SharedState {
    slots: [SlotCell; DESK_CHANNELS],
    master_fader_db: AtomicF32,
    master_aux_return: AtomicF32,
    master_meter: MeterCell,
    master_clip: AtomicBool,
    callback_frames: AtomicU32,
    stream_errors: AtomicU64,
    /// Engine calls the callback made that were refused (a bug if ever
    /// non-zero).
    engine_faults: AtomicU64,
    /// Next studio frame the callback will render.
    studio_frame: AtomicU64,
    /// How far ahead of the frame it renders next the callback schedules a
    /// transport change, in frames; 0 until the hub says.
    schedule_ahead: AtomicU32,
    tempo_bpm: AtomicF32,
    playing: AtomicBool,
    /// Bumped after every transport change, so the hub can tell instruments.
    transport_revision: AtomicU64,
    /// The desk's metronome click is audible.
    click: AtomicBool,
    /// Beat within the bar the metronome last struck; `u32::MAX` while
    /// stopped.
    beat: AtomicU32,
}

/// Slowest tempo the transport accepts.
pub const MIN_BPM: f32 = 20.0;

/// Fastest tempo the transport accepts.
pub const MAX_BPM: f32 = 300.0;

/// Tempo the desk starts at.
pub const DEFAULT_BPM: f32 = 120.0;

/// The studio transport: one tempo and play state for every instrument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    /// Tempo in beats per minute.
    pub bpm: f32,
    /// Whether the studio is playing.
    pub playing: bool,
    /// Changes whenever tempo or play state changes.
    pub revision: u64,
}

/// A tempo clamped to [`MIN_BPM`]..=[`MAX_BPM`]; non-finite tempos keep
/// `current`.
#[must_use]
pub const fn clamp_bpm(bpm: f32, current: f32) -> f32 {
    if bpm.is_finite() {
        bpm.clamp(MIN_BPM, MAX_BPM)
    } else {
        current
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedState {
    /// Fresh state: default controls, silent meters, clip lights off.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [const { SlotCell::new() }; DESK_CHANNELS],
            master_fader_db: AtomicF32::new(MasterControls::DEFAULT.fader_db),
            master_aux_return: AtomicF32::new(MasterControls::DEFAULT.aux_return),
            master_meter: MeterCell::new(),
            master_clip: AtomicBool::new(false),
            callback_frames: AtomicU32::new(0),
            stream_errors: AtomicU64::new(0),
            engine_faults: AtomicU64::new(0),
            studio_frame: AtomicU64::new(0),
            schedule_ahead: AtomicU32::new(0),
            tempo_bpm: AtomicF32::new(DEFAULT_BPM),
            playing: AtomicBool::new(false),
            transport_revision: AtomicU64::new(0),
            click: AtomicBool::new(false),
            beat: AtomicU32::new(u32::MAX),
        }
    }

    /// Whether the metronome click is audible.
    #[must_use]
    pub fn click(&self) -> bool {
        self.click.load(Ordering::Relaxed)
    }

    /// Switch the metronome click on or off.
    pub fn set_click(&self, audible: bool) {
        self.click.store(audible, Ordering::Relaxed);
    }

    /// Record the beat within the bar the metronome last struck.
    pub fn set_beat(&self, beat: Option<u32>) {
        self.beat.store(beat.unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    /// Beat within the bar (0 = beat one) the metronome last struck, or
    /// `None` while stopped.
    #[must_use]
    pub fn beat(&self) -> Option<u32> {
        match self.beat.load(Ordering::Relaxed) {
            u32::MAX => None,
            beat => Some(beat),
        }
    }

    /// How far ahead the callback schedules transport changes, in frames:
    /// enough for the slowest instrument to hear of a change before it has
    /// rendered that frame.
    #[must_use]
    pub fn schedule_ahead(&self) -> u32 {
        self.schedule_ahead.load(Ordering::Relaxed)
    }

    /// Set how far ahead transport changes are scheduled.
    pub fn set_schedule_ahead(&self, frames: u32) {
        self.schedule_ahead.store(frames, Ordering::Relaxed);
    }

    /// Record the next studio frame the callback will render.
    pub fn set_studio_frame(&self, frame: u64) {
        self.studio_frame.store(frame, Ordering::Release);
    }

    /// Next studio frame the callback will render: the desk's clock.
    #[must_use]
    pub fn studio_frame(&self) -> u64 {
        self.studio_frame.load(Ordering::Acquire)
    }

    /// The current transport.
    #[must_use]
    pub fn transport(&self) -> Transport {
        // Revision first: a change landing mid-read shows up as a newer
        // revision on the next read, so it is never missed.
        let revision = self.transport_revision.load(Ordering::Acquire);
        Transport {
            bpm: self.tempo_bpm.load(),
            playing: self.playing.load(Ordering::Acquire),
            revision,
        }
    }

    /// Set the tempo, clamped to the accepted range; non-finite tempos are
    /// ignored. Returns the tempo now in force.
    pub fn set_tempo(&self, bpm: f32) -> f32 {
        let current = self.tempo_bpm.load();
        let bpm = clamp_bpm(bpm, current);
        self.tempo_bpm.store(bpm);
        self.transport_revision.fetch_add(1, Ordering::AcqRel);
        bpm
    }

    /// Start or stop the studio.
    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Release);
        self.transport_revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Controls for a strip; out-of-range slots read as defaults.
    #[must_use]
    pub fn channel_controls(&self, slot: usize) -> ChannelControls {
        let Some(cell) = self.slots.get(slot) else {
            return ChannelControls::DEFAULT;
        };
        let flags = cell.flags.load(Ordering::Relaxed);
        ChannelControls {
            trim_db: cell.trim_db.load(),
            eq: EqSettings {
                low_db: cell.eq_low_db.load(),
                mid_db: cell.eq_mid_db.load(),
                high_db: cell.eq_high_db.load(),
            },
            aux_send: cell.aux_send.load(),
            pan: Pan::new(cell.pan.load()),
            fader_db: cell.fader_db.load(),
            muted: flags & FLAG_MUTED != 0,
            soloed: flags & FLAG_SOLOED != 0,
        }
        .sanitized()
    }

    /// Store controls for a strip (sanitised). Out-of-range slots are ignored.
    pub fn store_channel_controls(&self, slot: usize, controls: ChannelControls) {
        let Some(cell) = self.slots.get(slot) else {
            return;
        };
        let controls = controls.sanitized();
        cell.trim_db.store(controls.trim_db);
        cell.eq_low_db.store(controls.eq.low_db);
        cell.eq_mid_db.store(controls.eq.mid_db);
        cell.eq_high_db.store(controls.eq.high_db);
        cell.aux_send.store(controls.aux_send);
        cell.pan.store(controls.pan.value());
        cell.fader_db.store(controls.fader_db);
        let mut flags = 0;
        if controls.muted {
            flags |= FLAG_MUTED;
        }
        if controls.soloed {
            flags |= FLAG_SOLOED;
        }
        cell.flags.store(flags, Ordering::Relaxed);
    }

    /// Master controls.
    #[must_use]
    pub fn master_controls(&self) -> MasterControls {
        MasterControls {
            fader_db: self.master_fader_db.load(),
            aux_return: self.master_aux_return.load(),
        }
        .sanitized()
    }

    /// Store master controls (sanitised).
    pub fn store_master_controls(&self, controls: MasterControls) {
        let controls = controls.sanitized();
        self.master_fader_db.store(controls.fader_db);
        self.master_aux_return.store(controls.aux_return);
    }

    /// Publish one callback's worth of a strip's runtime state: its meters
    /// (peak and RMS over `frames` frames) are folded into the strip's meter,
    /// and a clipped snapshot latches the clip light. Nothing here ever clears
    /// either.
    pub fn publish_channel(&self, slot: usize, snapshot: &ChannelSnapshot, frames: usize) {
        let Some(cell) = self.slots.get(slot) else {
            return;
        };
        cell.meter.add(snapshot.peak, snapshot.rms, frames);
        if snapshot.clipped {
            cell.clip_latched.store(true, Ordering::Relaxed);
        }
        cell.connected.store(snapshot.connected, Ordering::Relaxed);
        cell.faulted.store(snapshot.faulted, Ordering::Relaxed);
        cell.underruns.store(snapshot.underruns, Ordering::Relaxed);
        cell.slips.store(snapshot.slips, Ordering::Relaxed);
        cell.resyncs.store(snapshot.resyncs, Ordering::Relaxed);
        for (word, chunk) in cell.name.iter().zip(snapshot.name.chunks_exact(4)) {
            let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
            word.store(u32::from_le_bytes(bytes), Ordering::Relaxed);
        }
    }

    /// Publish one callback's master meters (over `frames` frames) and clip.
    pub fn publish_master(
        &self,
        peak: StereoLevel,
        rms: StereoLevel,
        frames: usize,
        clipped: bool,
    ) {
        self.master_meter.add(peak, rms, frames);
        if clipped {
            self.master_clip.store(true, Ordering::Relaxed);
        }
    }

    /// Read a strip's runtime state; out-of-range slots read as empty.
    #[must_use]
    pub fn channel_readout(&self, slot: usize) -> ChannelReadout {
        let Some(cell) = self.slots.get(slot) else {
            return ChannelReadout {
                connected: false,
                faulted: false,
                name: [0; NAME_BYTES],
                clip: false,
                underruns: 0,
                slips: 0,
                resyncs: 0,
            };
        };
        let mut name = [0_u8; NAME_BYTES];
        for (chunk, word) in name.chunks_exact_mut(4).zip(cell.name.iter()) {
            chunk.copy_from_slice(&word.load(Ordering::Relaxed).to_le_bytes());
        }
        ChannelReadout {
            connected: cell.connected.load(Ordering::Relaxed),
            faulted: cell.faulted.load(Ordering::Relaxed),
            name,
            clip: cell.clip_latched.load(Ordering::Relaxed),
            underruns: cell.underruns.load(Ordering::Relaxed),
            slips: cell.slips.load(Ordering::Relaxed),
            resyncs: cell.resyncs.load(Ordering::Relaxed),
        }
    }

    /// Read master bus state.
    #[must_use]
    pub fn master_readout(&self) -> MasterReadout {
        MasterReadout {
            clip: self.master_clip.load(Ordering::Relaxed),
        }
    }

    /// Drain a strip's meter: peak and RMS of everything published since the
    /// last drain. Out-of-range slots read as silent.
    pub fn take_channel_meters(&self, slot: usize) -> MeterTake {
        self.slots
            .get(slot)
            .map_or(MeterTake::SILENT, |cell| cell.meter.take())
    }

    /// Drain the master meter.
    pub fn take_master_meters(&self) -> MeterTake {
        self.master_meter.take()
    }

    /// Turn every clip light off.
    pub fn clear_clips(&self) {
        for cell in &self.slots {
            cell.clip_latched.store(false, Ordering::Relaxed);
        }
        self.master_clip.store(false, Ordering::Relaxed);
    }

    /// Record the frame count of the latest device callback.
    pub fn set_callback_frames(&self, frames: u32) {
        self.callback_frames.store(frames, Ordering::Relaxed);
    }

    /// Frames in the latest device callback, or `None` before the first one.
    #[must_use]
    pub fn callback_frames(&self) -> Option<u32> {
        match self.callback_frames.load(Ordering::Relaxed) {
            0 => None,
            frames => Some(frames),
        }
    }

    /// Record an audio stream error.
    pub fn note_stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Audio stream errors since start.
    #[must_use]
    pub fn stream_errors(&self) -> u64 {
        self.stream_errors.load(Ordering::Relaxed)
    }

    /// Count an engine call the callback made that the engine refused.
    pub fn note_engine_fault(&self) {
        self.engine_faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Engine calls refused since start.
    #[must_use]
    pub fn engine_faults(&self) -> u64 {
        self.engine_faults.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{Step, StripControl};
    use crate::engine::short_name;
    use crate::test_support::assert_float_eq;
    use kazoo_core::protocol::ChannelId;

    #[test]
    fn controls_round_trip_through_atomics() {
        let state = SharedState::new();
        let mut controls = ChannelControls::DEFAULT;
        for control in StripControl::ALL {
            controls.adjust(control, 2, Step::Coarse);
        }
        controls.muted = true;
        controls.soloed = true;
        state.store_channel_controls(3, controls);
        assert_eq!(state.channel_controls(3), controls);
        assert_eq!(state.channel_controls(2), ChannelControls::DEFAULT);
    }

    #[test]
    fn out_of_range_slots_are_harmless() {
        let state = SharedState::new();
        state.store_channel_controls(99, ChannelControls::DEFAULT);
        state.publish_channel(99, &ChannelSnapshot::EMPTY, 64);
        assert_eq!(state.channel_controls(99), ChannelControls::DEFAULT);
        assert!(!state.channel_readout(99).connected);
    }

    #[test]
    fn stored_controls_are_sanitised() {
        let state = SharedState::new();
        state.store_channel_controls(
            0,
            ChannelControls {
                trim_db: f32::NAN,
                fader_db: 1_000.0,
                ..ChannelControls::DEFAULT
            },
        );
        let controls = state.channel_controls(0);
        assert_float_eq(controls.trim_db, 0.0);
        assert!(controls.fader_db <= crate::controls::FADER_MAX_DB);
    }

    #[test]
    fn clip_latches_until_cleared() {
        let state = SharedState::new();
        let clipped = ChannelSnapshot {
            clipped: true,
            ..ChannelSnapshot::EMPTY
        };
        state.publish_channel(1, &clipped, 64);
        state.publish_channel(1, &ChannelSnapshot::EMPTY, 64);
        assert!(state.channel_readout(1).clip);
        state.publish_master(StereoLevel::ZERO, StereoLevel::ZERO, 64, true);
        state.publish_master(StereoLevel::ZERO, StereoLevel::ZERO, 64, false);
        assert!(state.master_readout().clip);
        state.clear_clips();
        assert!(!state.channel_readout(1).clip);
        assert!(!state.master_readout().clip);
    }

    #[test]
    fn readout_carries_name_meters_and_health() {
        let state = SharedState::new();
        let snapshot = ChannelSnapshot {
            id: ChannelId(0),
            connected: true,
            name: short_name("808"),
            peak: StereoLevel {
                left: 0.5,
                right: 0.25,
            },
            rms: StereoLevel {
                left: 0.2,
                right: 0.1,
            },
            clipped: false,
            underruns: 3,
            slips: 2,
            resyncs: 1,
            faulted: false,
        };
        state.publish_channel(0, &snapshot, 100);
        let readout = state.channel_readout(0);
        assert!(readout.connected);
        assert_eq!(readout.name, short_name("808"));
        let meters = state.take_channel_meters(0);
        assert_eq!(meters.peak, snapshot.peak);
        assert!((meters.rms.left - 0.2).abs() < 1e-5);
        assert!((meters.rms.right - 0.1).abs() < 1e-5);
        assert_eq!(meters.frames, 100);
        assert_eq!(
            (readout.underruns, readout.slips, readout.resyncs),
            (3, 2, 1)
        );
    }

    #[test]
    fn master_controls_round_trip() {
        let state = SharedState::new();
        let master = MasterControls {
            fader_db: -4.5,
            aux_return: 0.3,
        };
        state.store_master_controls(master);
        assert_eq!(state.master_controls(), master);
    }

    #[test]
    fn callback_frames_and_stream_errors() {
        let state = SharedState::new();
        assert_eq!(state.callback_frames(), None);
        state.set_callback_frames(512);
        assert_eq!(state.callback_frames(), Some(512));
        state.note_stream_error();
        assert_eq!(state.stream_errors(), 1);
    }

    fn level(left: f32, right: f32) -> StereoLevel {
        StereoLevel { left, right }
    }

    fn snapshot_with(peak: StereoLevel, rms: StereoLevel) -> ChannelSnapshot {
        ChannelSnapshot {
            peak,
            rms,
            ..ChannelSnapshot::EMPTY
        }
    }

    #[test]
    fn meters_keep_the_loudest_peak_until_drained() {
        let state = SharedState::new();
        state.publish_channel(2, &snapshot_with(level(0.9, 0.1), level(0.3, 0.05)), 64);
        state.publish_channel(2, &snapshot_with(level(0.2, 0.4), level(0.1, 0.2)), 64);
        let take = state.take_channel_meters(2);
        assert_eq!(take.peak, level(0.9, 0.4));
        assert_eq!(take.frames, 128);
        // Drained: the next take starts from silence.
        assert_eq!(state.take_channel_meters(2), MeterTake::SILENT);
    }

    #[test]
    fn meter_rms_is_energy_weighted_across_callbacks() {
        let state = SharedState::new();
        state.publish_master(level(1.0, 1.0), level(1.0, 0.0), 300, false);
        state.publish_master(StereoLevel::ZERO, StereoLevel::ZERO, 100, false);
        let take = state.take_master_meters();
        assert!((take.rms.left - 0.75_f32.sqrt()).abs() < 1e-5, "{take:?}");
        assert_float_eq(take.rms.right, 0.0);
        assert_eq!(take.frames, 400);
    }

    #[test]
    fn meters_ignore_garbage_and_saturate_instead_of_wrapping() {
        let state = SharedState::new();
        state.publish_master(
            level(f32::NAN, -1.0),
            level(f32::INFINITY, f32::NAN),
            10,
            false,
        );
        let take = state.take_master_meters();
        assert_eq!(take.peak, StereoLevel::ZERO);
        assert_eq!(take.rms, StereoLevel::ZERO);

        for _ in 0..4 {
            state.publish_master(level(1.0, 1.0), level(1e6, 1e6), usize::MAX, false);
        }
        let take = state.take_master_meters();
        assert!(take.rms.left.is_finite() && take.rms.left > 0.0);
        assert_eq!(take.frames, u64::MAX);
    }

    #[test]
    fn out_of_range_meters_are_silent() {
        let state = SharedState::new();
        assert_eq!(state.take_channel_meters(99), MeterTake::SILENT);
    }
}
