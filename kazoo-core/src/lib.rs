//! Kazoo core audio engine library.
//!
//! Provides synthesis, analysis, effects processing, mixing, and audio I/O
//! for the Kazoo voice-driven synthesizer. This crate has zero UI dependencies —
//! any frontend consumes this API through [`engine::EngineHandle`].

pub mod analysis;
pub mod audio_transport;
pub mod effects;
pub mod engine;
pub mod io;
pub mod ipc;
pub mod mixer;
pub mod protocol;
pub mod synthesis;
pub mod transport;

use std::f32::consts::PI;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// All fallible operations in kazoo-core return this error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Audio device error: {0}")]
    AudioDevice(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("File I/O error: {0}")]
    FileIo(#[from] std::io::Error),

    #[error("Audio format error: {0}")]
    AudioFormat(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Engine not running")]
    EngineNotRunning,
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Core processor trait
// ---------------------------------------------------------------------------

/// Universal interface for anything that processes audio: synth engines,
/// effects, analysis stages, etc.
///
/// Every implementor **must** be `Send` so it can live in the output callback.
/// All internal buffers must be pre-allocated in `new()` or `set_sample_rate()`.
/// The `process` method must never allocate, lock, or perform I/O.
pub trait Processor: Send {
    /// Process a block of audio.
    ///
    /// For effects: `input` and `output` have the same length.
    /// For synthesis: `input` carries voice/analysis data, `output` receives
    /// generated audio. Lengths may differ.
    ///
    /// # Contracts
    /// - Must handle empty slices (no-op).
    /// - Must replace NaN/Inf in output with `0.0`.
    /// - Must not allocate or lock.
    fn process(&mut self, input: &[f32], output: &mut [f32]);

    /// Reset all internal state (delay lines, buffers, phase accumulators, etc.)
    /// to their initial values. Called on transport stop or mode switch.
    fn reset(&mut self);

    /// Latency in samples introduced by this processor. Used for delay
    /// compensation in the mixer.
    fn latency_samples(&self) -> usize {
        0
    }

    /// Human-readable name for UI display (e.g. "Biquad LP Filter").
    fn name(&self) -> &str;

    /// Number of user-controllable parameters this processor exposes.
    fn param_count(&self) -> usize {
        0
    }

    /// Metadata for the parameter at `index`. Returns `None` if out of range.
    fn param_info(&self, _index: usize) -> Option<ParamInfo> {
        None
    }

    /// Current value of the parameter at `index`. Returns `None` if out of range.
    fn param_value(&self, _index: usize) -> Option<f32> {
        None
    }

    /// Set the parameter at `index` to `value`.
    ///
    /// Implementations must clamp or reject out-of-range values via
    /// [`Error::Config`].
    fn set_param(&mut self, _index: usize, _value: f32) -> Result<()> {
        Err(Error::Config("no parameters".into()))
    }

    /// Called when the host sample rate changes. Implementations must
    /// recalculate coefficients, resize internal buffers, and reset state.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// Prepare internal buffers for the given maximum block size.
    ///
    /// Called once after construction and whenever the engine's buffer size
    /// changes. Implementations that hold scratch buffers sized to the block
    /// length should resize them here so that [`process`](Self::process) never
    /// allocates.
    ///
    /// The default implementation is a no-op — most processors use fixed-size
    /// internal buffers and do not need this.
    fn prepare(&mut self, _max_block_size: usize) {}

    /// Feed a detected pitch frequency (Hz) to this processor.
    ///
    /// Called by the output callback when the analysis thread detects a
    /// voiced pitch. Synthesis processors that track vocal pitch should
    /// override this to update their oscillator frequency.
    ///
    /// The default implementation is a no-op — effects and synths that do not
    /// use pitch tracking ignore this.
    fn set_pitch(&mut self, _frequency: f32) {}
}

// ---------------------------------------------------------------------------
// Decibel wrapper
// ---------------------------------------------------------------------------

/// Decibel value clamped to \[-100, +24\] dB.
///
/// Provides conversions to/from linear gain. `-100 dB` is treated as silence.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Db(f32);

impl Db {
    /// Minimum representable level (silence).
    pub const SILENCE: Self = Self(-100.0);
    /// Unity gain (0 dB).
    pub const UNITY: Self = Self(0.0);

    /// Create a new `Db` value, clamped to \[-100, +24\].
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self(clamp_finite(value, -100.0, 24.0))
    }

    /// Raw dB value.
    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }

    /// Convert to linear gain: `10^(dB/20)`.
    #[must_use]
    pub fn to_linear(self) -> f32 {
        if self.0 <= -100.0 {
            return 0.0;
        }
        10.0_f32.powf(self.0 / 20.0)
    }

    /// Create from a linear gain value.
    ///
    /// Values ≤ 0 map to [`Db::SILENCE`].
    #[must_use]
    pub fn from_linear(linear: f32) -> Self {
        if !linear.is_finite() || linear <= 0.0 {
            return Self::SILENCE;
        }
        let db = 20.0 * linear.log10();
        Self(clamp_finite(db, -100.0, 24.0))
    }
}

impl Default for Db {
    fn default() -> Self {
        Self::UNITY
    }
}

// ---------------------------------------------------------------------------
// Pan wrapper
// ---------------------------------------------------------------------------

/// Stereo pan position in \[-1.0 (full left), +1.0 (full right)\].
///
/// Uses an equal-power pan law so that a centered signal plays at the same
/// perceived loudness through both speakers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pan(f32);

impl Pan {
    /// Center pan (equal to both channels).
    pub const CENTER: Self = Self(0.0);

    /// Create a new `Pan` value, clamped to \[-1, +1\].
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self(clamp_finite(value, -1.0, 1.0))
    }

    /// Raw pan value.
    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }

    /// Equal-power pan gains: `(left_gain, right_gain)`.
    ///
    /// At center both channels receive `≈ 0.707` (−3 dB).
    /// At hard left: `(1.0, 0.0)`. At hard right: `(0.0, 1.0)`.
    #[must_use]
    pub fn gains(self) -> (f32, f32) {
        // Map [-1, 1] → [0, π/2], then cos/sin gives equal-power curve.
        let angle = (self.0 + 1.0) * 0.25 * PI;
        (angle.cos(), angle.sin())
    }
}

impl Default for Pan {
    fn default() -> Self {
        Self::CENTER
    }
}

// ---------------------------------------------------------------------------
// Time position
// ---------------------------------------------------------------------------

/// A sample-accurate time position in the transport timeline.
#[derive(Debug, Clone, Copy)]
pub struct TimePosition {
    /// Absolute sample offset from the beginning.
    pub samples: u64,
    /// Sample rate this position is relative to.
    pub sample_rate: u32,
}

impl TimePosition {
    /// Position at the very beginning (sample 0) with default sample rate.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            samples: 0,
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }

    /// Create a position at a specific sample with the given rate.
    #[must_use]
    pub fn new(samples: u64, sample_rate: u32) -> Self {
        Self {
            samples,
            sample_rate: sample_rate.max(1),
        }
    }

    /// Time in seconds.
    #[must_use]
    pub fn seconds(&self) -> f64 {
        self.samples as f64 / f64::from(self.sample_rate.max(1))
    }

    /// Position expressed in beats at the given tempo.
    #[must_use]
    pub fn beats(&self, bpm: f64) -> f64 {
        if !bpm.is_finite() || bpm <= 0.0 {
            return 0.0;
        }
        self.seconds() * bpm / 60.0
    }

    /// Format as `MM:SS.mmm`.
    #[must_use]
    pub fn format_time(&self) -> String {
        let total_secs = self.seconds();
        let minutes = (total_secs / 60.0) as u32;
        let seconds = total_secs % 60.0;
        format!("{minutes:02}:{seconds:06.3}")
    }

    /// Format as `Bar.Beat.Tick` given tempo and time signature.
    ///
    /// Ticks are in 1/480 of a beat (standard MIDI resolution).
    #[must_use]
    pub fn format_bar_beat_tick(&self, bpm: f64, beats_per_bar: u8) -> String {
        let bpb = f64::from(beats_per_bar.max(1));
        let total_beats = self.beats(bpm);
        let bar = (total_beats / bpb) as u32 + 1;
        let beat_in_bar = (total_beats % bpb) as u32 + 1;
        let frac = total_beats.fract();
        let tick = (frac * 480.0) as u32;
        format!("{bar}.{beat_in_bar}.{tick:03}")
    }
}

impl Default for TimePosition {
    fn default() -> Self {
        Self::zero()
    }
}

// ---------------------------------------------------------------------------
// Parameter metadata
// ---------------------------------------------------------------------------

/// Describes one user-controllable parameter on a [`Processor`].
#[derive(Debug, Clone)]
pub struct ParamInfo {
    /// Display name (e.g. "Cutoff Frequency").
    pub name: String,
    /// Minimum allowed value.
    pub min: f32,
    /// Maximum allowed value.
    pub max: f32,
    /// Default value (used on reset).
    pub default: f32,
    /// Unit label for display (e.g. "Hz", "dB", "ms", "%", "").
    pub unit: String,
}

impl ParamInfo {
    /// Clamp `value` to this parameter's valid range.
    #[must_use]
    pub const fn clamp(&self, value: f32) -> f32 {
        clamp_finite(value, self.min, self.max)
    }

    /// Check whether `value` is within this parameter's valid range.
    #[must_use]
    pub fn in_range(&self, value: f32) -> bool {
        value.is_finite() && value >= self.min && value <= self.max
    }

    /// Normalize `value` to \[0, 1\] across this parameter's range.
    #[must_use]
    pub fn normalize(&self, value: f32) -> f32 {
        let range = self.max - self.min;
        if range <= 0.0 {
            return 0.0;
        }
        ((value - self.min) / range).clamp(0.0, 1.0)
    }

    /// Map a \[0, 1\] normalized value back to this parameter's range.
    #[must_use]
    pub fn denormalize(&self, normalized: f32) -> f32 {
        let n = normalized.clamp(0.0, 1.0);
        self.min + n * (self.max - self.min)
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default audio sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

/// Default audio buffer size in samples.
pub const DEFAULT_BUFFER_SIZE: usize = 128;

/// Maximum number of mixer tracks.
pub const MAX_TRACKS: usize = 16;

/// FFT size used for spectrum display.
pub const SPECTRUM_FFT_SIZE: usize = 2048;

/// Maximum number of effects per track.
pub const MAX_EFFECTS_PER_TRACK: usize = 8;

/// Maximum number of synth layers per track.
pub const MAX_SYNTH_LAYERS: usize = 4;

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Clamp `value` to `[min, max]`, treating NaN/Inf as `min`.
#[inline]
#[must_use]
const fn clamp_finite(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

/// Sanitize a single audio sample: replace NaN/Inf with `0.0`.
#[inline]
#[must_use]
pub const fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

/// Sanitize an entire audio buffer in-place, replacing NaN/Inf with `0.0`.
pub fn sanitize_buffer(buffer: &mut [f32]) {
    for sample in buffer.iter_mut() {
        if !sample.is_finite() {
            *sample = 0.0;
        }
    }
}

/// Absolute level below which the soft limiter passes audio through untouched.
const SOFT_LIMIT_KNEE: f32 = 0.9;
/// Width of the soft-knee region above [`SOFT_LIMIT_KNEE`].
const SOFT_LIMIT_KNEE_WIDTH: f32 = 1.0 - SOFT_LIMIT_KNEE;

/// Apply a soft limiter to a single audio sample.
///
/// Signals within ±[`SOFT_LIMIT_KNEE`] (0.9) pass through unchanged — no
/// colouration, no distortion. Above that threshold a `tanh`-shaped knee
/// smoothly compresses the signal so it never exceeds ±1.0, preventing the
/// harsh artifacts of hard clipping at the DAC.
///
/// This should be the last processing step before sending audio to the DAC.
#[inline]
#[must_use]
pub fn soft_limit(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let abs = sample.abs();
    if abs <= SOFT_LIMIT_KNEE {
        sample
    } else {
        // Excess above the knee mapped through tanh for smooth compression.
        let excess = (abs - SOFT_LIMIT_KNEE) / SOFT_LIMIT_KNEE_WIDTH;
        let compressed = SOFT_LIMIT_KNEE_WIDTH.mul_add(excess.tanh(), SOFT_LIMIT_KNEE);
        sample.signum() * compressed
    }
}

/// Apply a soft limiter to an entire audio buffer in-place.
///
/// See [`soft_limit`] for details on the limiting curve. Signals below ±0.9
/// pass through unchanged; signals above are smoothly compressed.
pub fn soft_limit_buffer(buffer: &mut [f32]) {
    for sample in buffer.iter_mut() {
        *sample = soft_limit(*sample);
    }
}

/// Convert a frequency in Hz to the nearest MIDI note number.
///
/// Returns `None` if the frequency is outside the MIDI range (roughly 8–13kHz).
#[must_use]
pub fn frequency_to_midi_note(frequency: f32) -> Option<u8> {
    if !frequency.is_finite() || frequency <= 0.0 {
        return None;
    }
    let midi_float = 12.0f32.mul_add((frequency / 440.0).log2(), 69.0);
    let note = midi_float.round();
    if (0.0..=127.0).contains(&note) {
        Some(note as u8)
    } else {
        None
    }
}

/// Convert a MIDI note number to frequency in Hz.
#[must_use]
pub fn midi_note_to_frequency(note: u8) -> f32 {
    440.0 * ((f32::from(note) - 69.0) / 12.0).exp2()
}

/// MIDI note name (e.g. `"A4"`, `"C#3"`).
///
/// Uses the standard MIDI convention where MIDI note 0 is C-1, MIDI note 12
/// is C0, MIDI note 60 is C4, and MIDI note 69 is A4.
#[must_use]
pub fn midi_note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(note / 12) - 1;
    let name = NAMES[note as usize % 12];
    format!("{name}{octave}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Db tests --

    #[test]
    fn db_unity_is_zero() {
        assert!((Db::UNITY.value() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_silence_is_negative_100() {
        assert!((Db::SILENCE.value() - (-100.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_linear_unity() {
        let linear = Db::UNITY.to_linear();
        assert!(
            (linear - 1.0).abs() < 1e-6,
            "unity dB should be linear 1.0, got {linear}"
        );
    }

    #[test]
    fn db_to_linear_silence() {
        let linear = Db::SILENCE.to_linear();
        assert!(
            linear.abs() < f32::EPSILON,
            "silence dB should be linear 0.0, got {linear}"
        );
    }

    #[test]
    fn db_to_linear_plus6() {
        let db = Db::new(6.0);
        let linear = db.to_linear();
        // +6 dB ≈ 1.995
        assert!(
            (linear - 1.995_262).abs() < 0.001,
            "+6 dB should be ~1.995, got {linear}"
        );
    }

    #[test]
    fn db_to_linear_minus20() {
        let db = Db::new(-20.0);
        let linear = db.to_linear();
        assert!(
            (linear - 0.1).abs() < 1e-6,
            "-20 dB should be linear 0.1, got {linear}"
        );
    }

    #[test]
    fn db_from_linear_roundtrip() {
        for db_val in [-60.0, -20.0, -6.0, 0.0, 6.0, 12.0, 20.0] {
            let original = Db::new(db_val);
            let linear = original.to_linear();
            let recovered = Db::from_linear(linear);
            assert!(
                (original.value() - recovered.value()).abs() < 0.01,
                "roundtrip failed for {db_val} dB: got {recovered:?}"
            );
        }
    }

    #[test]
    fn db_from_linear_zero_is_silence() {
        assert_eq!(Db::from_linear(0.0), Db::SILENCE);
    }

    #[test]
    fn db_from_linear_negative_is_silence() {
        assert_eq!(Db::from_linear(-1.0), Db::SILENCE);
    }

    #[test]
    fn db_clamps_high() {
        let db = Db::new(100.0);
        assert!((db.value() - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_clamps_low() {
        let db = Db::new(-200.0);
        assert!((db.value() - (-100.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn db_handles_nan() {
        let db = Db::new(f32::NAN);
        assert!(db.value().is_finite(), "NaN should be clamped to finite");
    }

    #[test]
    fn db_handles_infinity() {
        let db = Db::new(f32::INFINITY);
        assert!(db.value().is_finite());
        let db2 = Db::new(f32::NEG_INFINITY);
        assert!(db2.value().is_finite());
    }

    #[test]
    fn db_from_linear_nan() {
        let db = Db::from_linear(f32::NAN);
        assert_eq!(db, Db::SILENCE);
    }

    #[test]
    fn db_from_linear_infinity() {
        let db = Db::from_linear(f32::INFINITY);
        assert_eq!(db, Db::SILENCE);
    }

    // -- Pan tests --

    #[test]
    fn pan_center_gains() {
        let (l, r) = Pan::CENTER.gains();
        // At center, both should be cos(π/4) = sin(π/4) ≈ 0.707
        let expected = (PI / 4.0).cos();
        assert!(
            (l - expected).abs() < 1e-6,
            "center left gain should be ~0.707, got {l}"
        );
        assert!(
            (r - expected).abs() < 1e-6,
            "center right gain should be ~0.707, got {r}"
        );
    }

    #[test]
    fn pan_hard_left() {
        let (l, r) = Pan::new(-1.0).gains();
        assert!((l - 1.0).abs() < 1e-6, "hard left L should be 1.0, got {l}");
        assert!(r.abs() < 1e-6, "hard left R should be 0.0, got {r}");
    }

    #[test]
    fn pan_hard_right() {
        let (l, r) = Pan::new(1.0).gains();
        assert!(l.abs() < 1e-6, "hard right L should be 0.0, got {l}");
        assert!(
            (r - 1.0).abs() < 1e-6,
            "hard right R should be 1.0, got {r}"
        );
    }

    #[test]
    fn pan_equal_power_sum() {
        // For any pan position, l² + r² should ≈ 1.0 (equal power)
        for i in 0..=20 {
            let p = Pan::new(-1.0 + (i as f32) * 0.1);
            let (l, r) = p.gains();
            let power = l * l + r * r;
            assert!(
                (power - 1.0).abs() < 1e-5,
                "equal-power sum at pan={}: l²+r² = {power}",
                p.value()
            );
        }
    }

    #[test]
    fn pan_clamps() {
        assert!((Pan::new(-5.0).value() - (-1.0)).abs() < f32::EPSILON);
        assert!((Pan::new(5.0).value() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pan_handles_nan() {
        let pan = Pan::new(f32::NAN);
        assert!(pan.value().is_finite());
    }

    // -- TimePosition tests --

    #[test]
    fn time_position_zero() {
        let tp = TimePosition::zero();
        assert_eq!(tp.samples, 0);
        assert_eq!(tp.sample_rate, DEFAULT_SAMPLE_RATE);
        assert!((tp.seconds() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_position_one_second() {
        let tp = TimePosition::new(44_100, 44_100);
        assert!((tp.seconds() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn time_position_beats() {
        // At 120 BPM, 1 second = 2 beats
        let tp = TimePosition::new(44_100, 44_100);
        let beats = tp.beats(120.0);
        assert!(
            (beats - 2.0).abs() < 1e-10,
            "1 sec at 120 BPM should be 2 beats, got {beats}"
        );
    }

    #[test]
    fn time_position_beats_zero_bpm() {
        let tp = TimePosition::new(44_100, 44_100);
        assert!((tp.beats(0.0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_position_beats_nan_bpm() {
        let tp = TimePosition::new(44_100, 44_100);
        assert!((tp.beats(f64::NAN) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_position_beats_inf_bpm() {
        let tp = TimePosition::new(44_100, 44_100);
        assert!((tp.beats(f64::INFINITY) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_position_format_time() {
        let tp = TimePosition::new(44_100 * 65, 44_100); // 1 min 5 sec
        let formatted = tp.format_time();
        assert_eq!(formatted, "01:05.000");
    }

    #[test]
    fn time_position_format_bar_beat_tick() {
        // 2 beats at 120 BPM, 4/4 → bar 1, beat 3, tick 000
        let tp = TimePosition::new(44_100, 44_100); // 1 second = 2 beats
        let formatted = tp.format_bar_beat_tick(120.0, 4);
        assert_eq!(formatted, "1.3.000");
    }

    #[test]
    fn time_position_sample_rate_zero_safe() {
        // sample_rate of 0 shouldn't divide by zero
        let tp = TimePosition::new(100, 0);
        assert!(tp.seconds().is_finite());
    }

    // -- ParamInfo tests --

    #[test]
    fn param_info_clamp() {
        let info = ParamInfo {
            name: "test".into(),
            min: 0.0,
            max: 100.0,
            default: 50.0,
            unit: "%".into(),
        };
        assert!((info.clamp(-10.0) - 0.0).abs() < f32::EPSILON);
        assert!((info.clamp(200.0) - 100.0).abs() < f32::EPSILON);
        assert!((info.clamp(50.0) - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn param_info_in_range() {
        let info = ParamInfo {
            name: "test".into(),
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            unit: "Hz".into(),
        };
        assert!(info.in_range(440.0));
        assert!(!info.in_range(10.0));
        assert!(!info.in_range(f32::NAN));
    }

    #[test]
    fn param_info_normalize_denormalize_roundtrip() {
        let info = ParamInfo {
            name: "freq".into(),
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            unit: "Hz".into(),
        };
        let value = 5000.0;
        let normalized = info.normalize(value);
        let recovered = info.denormalize(normalized);
        assert!(
            (value - recovered).abs() < 0.01,
            "roundtrip: {value} → {normalized} → {recovered}"
        );
    }

    #[test]
    fn param_info_normalize_boundaries() {
        let info = ParamInfo {
            name: "test".into(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            unit: "".into(),
        };
        assert!((info.normalize(0.0) - 0.0).abs() < f32::EPSILON);
        assert!((info.normalize(1.0) - 1.0).abs() < f32::EPSILON);
        assert!((info.normalize(0.5) - 0.5).abs() < f32::EPSILON);
    }

    // -- MIDI conversion tests --

    #[test]
    fn frequency_to_midi_a4() {
        let note = frequency_to_midi_note(440.0);
        assert_eq!(note, Some(69), "A4 = 440 Hz should be MIDI 69");
    }

    #[test]
    fn frequency_to_midi_middle_c() {
        let note = frequency_to_midi_note(261.63);
        assert_eq!(note, Some(60), "middle C ≈ 261.63 Hz should be MIDI 60");
    }

    #[test]
    fn frequency_to_midi_zero() {
        assert_eq!(frequency_to_midi_note(0.0), None);
    }

    #[test]
    fn frequency_to_midi_negative() {
        assert_eq!(frequency_to_midi_note(-100.0), None);
    }

    #[test]
    fn frequency_to_midi_nan() {
        assert_eq!(frequency_to_midi_note(f32::NAN), None);
    }

    #[test]
    fn midi_note_to_frequency_a4() {
        let freq = midi_note_to_frequency(69);
        assert!(
            (freq - 440.0).abs() < 0.01,
            "MIDI 69 should be 440 Hz, got {freq}"
        );
    }

    #[test]
    fn midi_note_name_a4() {
        assert_eq!(midi_note_name(69), "A4");
    }

    #[test]
    fn midi_note_name_middle_c() {
        assert_eq!(midi_note_name(60), "C4");
    }

    #[test]
    fn midi_note_name_c0() {
        // MIDI note 12 = C0 (octave = 12/12 - 1 = 0)
        assert_eq!(midi_note_name(12), "C0");
    }

    #[test]
    fn midi_note_name_c_minus_1() {
        // MIDI note 0 = C-1 (octave = 0/12 - 1 = -1)
        assert_eq!(midi_note_name(0), "C-1");
    }

    #[test]
    fn midi_note_name_b_minus_1() {
        // MIDI note 11 = B-1
        assert_eq!(midi_note_name(11), "B-1");
    }

    // -- Utility tests --

    #[test]
    fn sanitize_sample_normal() {
        assert!((sanitize_sample(0.5) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn sanitize_sample_nan() {
        assert!((sanitize_sample(f32::NAN) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sanitize_sample_inf() {
        assert!((sanitize_sample(f32::INFINITY) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sanitize_buffer_cleans_nan() {
        let mut buf = [1.0, f32::NAN, -0.5, f32::INFINITY, 0.0];
        sanitize_buffer(&mut buf);
        assert!((buf[0] - 1.0).abs() < f32::EPSILON);
        assert!((buf[1] - 0.0).abs() < f32::EPSILON);
        assert!((buf[2] - (-0.5)).abs() < f32::EPSILON);
        assert!((buf[3] - 0.0).abs() < f32::EPSILON);
        assert!((buf[4] - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clamp_finite_normal() {
        assert!((clamp_finite(5.0, 0.0, 10.0) - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clamp_finite_nan() {
        assert!((clamp_finite(f32::NAN, 0.0, 10.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clamp_finite_inf() {
        assert!((clamp_finite(f32::INFINITY, 0.0, 10.0) - 0.0).abs() < f32::EPSILON);
    }

    // -- Soft limiter tests --

    #[test]
    fn soft_limit_passes_small_signals_unchanged() {
        // Signals below the knee (±0.9) should pass through bit-exact.
        for val in [0.0, 0.1, 0.5, 0.7, 0.89, -0.1, -0.5, -0.89] {
            let output = soft_limit(val);
            assert!(
                (output - val).abs() < f32::EPSILON,
                "signal {val} should pass through unchanged, got {output}"
            );
        }
    }

    #[test]
    fn soft_limit_transparent_at_knee_boundary() {
        // At exactly 0.9 the output should still be 0.9 (knee start).
        let output = soft_limit(0.9);
        assert!(
            (output - 0.9).abs() < f32::EPSILON,
            "knee boundary should be exact, got {output}"
        );
    }

    #[test]
    fn soft_limit_compresses_above_knee() {
        // A signal at 1.0 should be compressed below 1.0 but above 0.9.
        let output = soft_limit(1.0);
        assert!(output > 0.9, "1.0 should stay above knee, got {output}");
        assert!(output < 1.0, "1.0 should be compressed, got {output}");
    }

    #[test]
    fn soft_limit_compresses_hot_signals() {
        // A signal at 1.0 is just above the knee — clearly compressed but below 1.0.
        let at_one = soft_limit(1.0);
        assert!(at_one < 1.0, "1.0 should be compressed, got {at_one}");
        assert!(
            at_one > 0.9,
            "1.0 should stay above knee start, got {at_one}"
        );

        // A signal at 2.0 saturates the tanh, reaching approximately 1.0.
        let at_two = soft_limit(2.0);
        assert!(
            at_two <= 1.0,
            "hot signal should not exceed unity, got {at_two}"
        );
        assert!(at_two > 0.99, "2.0 should be near unity, got {at_two}");
    }

    #[test]
    fn soft_limit_never_exceeds_unity() {
        for val in [1.0, 2.0, 5.0, 10.0, 100.0, 1000.0] {
            let pos = soft_limit(val);
            let neg = soft_limit(-val);
            assert!(
                pos <= 1.0 && pos >= 0.0,
                "positive {val} -> {pos} should be in [0, 1]"
            );
            assert!(
                neg >= -1.0 && neg <= 0.0,
                "negative {val} -> {neg} should be in [-1, 0]"
            );
        }
    }

    #[test]
    fn soft_limit_handles_nan_inf() {
        assert!((soft_limit(f32::NAN) - 0.0).abs() < f32::EPSILON);
        assert!((soft_limit(f32::INFINITY) - 0.0).abs() < f32::EPSILON);
        assert!((soft_limit(f32::NEG_INFINITY) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn soft_limit_preserves_zero() {
        assert!((soft_limit(0.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn soft_limit_preserves_sign() {
        assert!(soft_limit(0.5) > 0.0);
        assert!(soft_limit(-0.5) < 0.0);
    }

    #[test]
    fn soft_limit_monotonic() {
        // The limiter must be monotonically increasing — louder in = louder out.
        let mut prev = soft_limit(0.0);
        for i in 1..=200 {
            let val = i as f32 * 0.05;
            let out = soft_limit(val);
            assert!(
                out >= prev,
                "not monotonic: soft_limit({prev_val}) = {prev} > soft_limit({val}) = {out}",
                prev_val = (i - 1) as f32 * 0.05,
            );
            prev = out;
        }
    }

    #[test]
    fn soft_limit_buffer_limits_all_samples() {
        let mut buf = [0.5, 2.0, -3.0, f32::NAN, 0.0];
        soft_limit_buffer(&mut buf);
        for (i, &s) in buf.iter().enumerate() {
            assert!(
                s.is_finite() && s >= -1.0 && s <= 1.0,
                "sample {i} = {s} should be in [-1, 1]"
            );
        }
        // 0.5 is below knee — should be unchanged.
        assert!(
            (buf[0] - 0.5).abs() < f32::EPSILON,
            "0.5 should pass through unchanged"
        );
    }
}
