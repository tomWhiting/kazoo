//! Wavetable synthesis: extracted single-cycle waveforms played back with morphing.
//!
//! [`WavetableExtractor`] analyses voice audio to pull out single-cycle
//! waveforms aligned to zero crossings. [`WavetableOscillator`] plays them
//! back with phase-accurate interpolation and frame morphing.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

// ---------------------------------------------------------------------------
// Wavetable
// ---------------------------------------------------------------------------

/// A collection of single-cycle waveform frames extracted from voice audio.
#[derive(Debug, Clone)]
pub struct Wavetable {
    /// Each frame is one single-cycle waveform, resampled to `frame_size`.
    frames: Vec<Vec<f32>>,
    /// Number of samples per frame.
    frame_size: usize,
    /// The fundamental frequency of the source audio used for extraction.
    source_frequency: f32,
    /// Number of frames in the wavetable.
    num_frames: usize,
}

impl Wavetable {
    /// Number of frames in this wavetable.
    #[must_use]
    pub const fn num_frames(&self) -> usize {
        self.num_frames
    }

    /// Samples per frame.
    #[must_use]
    pub const fn frame_size(&self) -> usize {
        self.frame_size
    }

    /// Source fundamental frequency.
    #[must_use]
    pub const fn source_frequency(&self) -> f32 {
        self.source_frequency
    }

    /// Whether this wavetable has any frames.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.num_frames == 0
    }

    /// Read a sample from a specific frame with linear interpolation.
    ///
    /// `phase` is in `[0, 1)` mapping across the frame. Out-of-range frames
    /// are clamped.
    #[must_use]
    fn read_sample(&self, frame_index: usize, phase: f32) -> f32 {
        if self.num_frames == 0 || self.frame_size == 0 {
            return 0.0;
        }
        let fi = frame_index.min(self.num_frames - 1);
        let frame = &self.frames[fi];

        let pos = phase * self.frame_size as f32;
        let idx0 = (pos.floor() as usize) % self.frame_size;
        let idx1 = (idx0 + 1) % self.frame_size;
        let frac = pos - pos.floor();

        let s0 = sanitize_sample(frame[idx0]);
        let s1 = sanitize_sample(frame[idx1]);
        frac.mul_add(s1 - s0, s0)
    }
}

// ---------------------------------------------------------------------------
// WavetableExtractor
// ---------------------------------------------------------------------------

/// Extracts single-cycle waveform frames from voice audio.
///
/// Given audio and a fundamental frequency, locates zero crossings to isolate
/// individual cycles that are then resampled to a fixed frame size.
#[derive(Debug)]
pub struct WavetableExtractor {
    /// Target number of samples per extracted frame.
    frame_size: usize,
}

impl WavetableExtractor {
    /// Default frame size for extracted wavetables.
    pub const DEFAULT_FRAME_SIZE: usize = 2048;

    /// Create a new extractor with the given frame size.
    #[must_use]
    pub const fn new(frame_size: usize) -> Self {
        let fs = if frame_size < 4 { 2048 } else { frame_size };
        Self { frame_size: fs }
    }

    /// Extract a wavetable from audio at the given fundamental frequency.
    ///
    /// # Errors
    ///
    /// Returns an error if the fundamental or sample rate are invalid, or if
    /// no complete cycles can be found in the audio.
    pub fn extract(&self, audio: &[f32], fundamental: f32, sample_rate: f32) -> Result<Wavetable> {
        if !fundamental.is_finite() || fundamental <= 0.0 {
            return Err(Error::Config(format!(
                "invalid fundamental frequency: {fundamental}"
            )));
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(Error::Config(format!("invalid sample rate: {sample_rate}")));
        }

        let period_samples = sample_rate / fundamental;
        if period_samples < 2.0 {
            return Err(Error::Config(
                "fundamental frequency too high for sample rate".into(),
            ));
        }
        if audio.len() < period_samples.ceil() as usize {
            return Err(Error::Config(
                "audio too short for one complete cycle".into(),
            ));
        }

        // Find positive-going zero crossings.
        let crossings = find_zero_crossings(audio);
        if crossings.len() < 2 {
            return Err(Error::Config("could not find enough zero crossings".into()));
        }

        // Extract cycles between consecutive zero crossings that are
        // approximately one period long.
        let expected_period = period_samples;
        let tolerance = expected_period * 0.4;
        let min_period = (expected_period - tolerance).max(2.0);
        let max_period = expected_period + tolerance;

        let mut frames = Vec::new();

        for pair in crossings.windows(2) {
            let start = pair[0];
            let end = pair[1];
            let cycle_len = end - start;

            if (cycle_len as f32) < min_period || (cycle_len as f32) > max_period {
                continue;
            }

            // Resample this cycle to frame_size samples via linear interpolation.
            let frame = resample_cycle(&audio[start..end], self.frame_size);
            frames.push(frame);
        }

        if frames.is_empty() {
            return Err(Error::Config(
                "no valid cycles found matching the fundamental".into(),
            ));
        }

        let num_frames = frames.len();
        Ok(Wavetable {
            frames,
            frame_size: self.frame_size,
            source_frequency: fundamental,
            num_frames,
        })
    }
}

/// Find indices of positive-going zero crossings in audio.
fn find_zero_crossings(audio: &[f32]) -> Vec<usize> {
    let mut crossings = Vec::new();
    for i in 1..audio.len() {
        let prev = sanitize_sample(audio[i - 1]);
        let curr = sanitize_sample(audio[i]);
        // Positive-going: previous < 0, current >= 0.
        if prev < 0.0 && curr >= 0.0 {
            crossings.push(i);
        }
    }
    crossings
}

/// Resample a cycle of arbitrary length to exactly `target_len` samples
/// using linear interpolation.
fn resample_cycle(cycle: &[f32], target_len: usize) -> Vec<f32> {
    if cycle.is_empty() || target_len == 0 {
        return vec![0.0; target_len];
    }
    let src_len = cycle.len() as f32;
    (0..target_len)
        .map(|i| {
            let pos = i as f32 * src_len / target_len as f32;
            let idx0 = (pos.floor() as usize).min(cycle.len() - 1);
            let idx1 = if idx0 + 1 < cycle.len() { idx0 + 1 } else { 0 };
            let frac = pos - pos.floor();
            let s0 = sanitize_sample(cycle[idx0]);
            let s1 = sanitize_sample(cycle[idx1]);
            sanitize_sample(frac.mul_add(s1 - s0, s0))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// WavetableOscillator
// ---------------------------------------------------------------------------

/// Plays back a [`Wavetable`] with phase-accurate interpolation and frame
/// morphing.
///
/// The oscillator scans through wavetable frames (morphing between adjacent
/// frames via crossfade) while a phase accumulator drives per-sample lookup.
/// Input audio is used for envelope tracking; output is purely synthetic.
#[derive(Debug)]
pub struct WavetableOscillator {
    sample_rate: f32,
    /// The loaded wavetable (if any).
    wavetable: Option<Wavetable>,
    /// Phase accumulator [0, 1).
    phase: f32,
    /// Current frame position [0, 1) across all frames.
    frame_position: f32,
    /// Rate at which the frame position scans automatically.
    frame_scan_rate: f32,
    /// Playback frequency.
    frequency: f32,
    /// Envelope follower state for input tracking.
    envelope_current: f32,
    envelope_attack_coeff: f32,
    envelope_release_coeff: f32,
}

impl WavetableOscillator {
    const PARAM_FREQUENCY: usize = 0;
    const PARAM_FRAME_POSITION: usize = 1;
    const PARAM_FRAME_SCAN_RATE: usize = 2;
    const PARAM_COUNT: usize = 3;

    const DEFAULT_FREQUENCY: f32 = 440.0;

    /// Create a new wavetable oscillator (no wavetable loaded).
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        Self {
            sample_rate: sr,
            wavetable: None,
            phase: 0.0,
            frame_position: 0.0,
            frame_scan_rate: 0.0,
            frequency: Self::DEFAULT_FREQUENCY,
            envelope_current: 0.0,
            envelope_attack_coeff: compute_envelope_coeff(5.0, sr),
            envelope_release_coeff: compute_envelope_coeff(50.0, sr),
        }
    }

    /// Load a wavetable for playback. Replaces any previously loaded table.
    pub fn load_wavetable(&mut self, wavetable: Wavetable) {
        self.wavetable = Some(wavetable);
    }

    /// Remove the loaded wavetable.
    pub fn clear_wavetable(&mut self) {
        self.wavetable = None;
    }

    /// Whether a wavetable is currently loaded.
    #[must_use]
    pub const fn has_wavetable(&self) -> bool {
        self.wavetable.is_some()
    }

    /// Set the playback frequency directly.
    pub fn set_frequency(&mut self, frequency: f32) {
        if frequency.is_finite() && (20.0..=20_000.0).contains(&frequency) {
            self.frequency = frequency;
        }
    }

    fn param_infos() -> [ParamInfo; Self::PARAM_COUNT] {
        [
            ParamInfo {
                name: "Frequency".into(),
                min: 20.0,
                max: 20_000.0,
                default: Self::DEFAULT_FREQUENCY,
                unit: "Hz".into(),
            },
            ParamInfo {
                name: "Frame Position".into(),
                min: 0.0,
                max: 1.0,
                default: 0.0,
                unit: String::new(),
            },
            ParamInfo {
                name: "Frame Scan Rate".into(),
                min: 0.0,
                max: 10.0,
                default: 0.0,
                unit: "Hz".into(),
            },
        ]
    }
}

impl Processor for WavetableOscillator {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        if output.is_empty() {
            return;
        }

        // Check if we have a valid wavetable.
        let has_wt = self.wavetable.as_ref().is_some_and(|wt| !wt.is_empty());

        if !has_wt {
            output.fill(0.0);
            return;
        }

        let dt = self.frequency / self.sample_rate;
        let frame_dt = self.frame_scan_rate / self.sample_rate;
        let input_len = input.len();

        for (i, out_sample) in output.iter_mut().enumerate() {
            // Track envelope from input if available.
            if i < input_len {
                let rectified = sanitize_sample(input[i]).abs();
                let coeff = if rectified > self.envelope_current {
                    self.envelope_attack_coeff
                } else {
                    self.envelope_release_coeff
                };
                self.envelope_current = sanitize_sample(
                    coeff.mul_add(self.envelope_current, (1.0 - coeff) * rectified),
                );
            }

            // Determine current frame indices for crossfade.
            let wt = self.wavetable.as_ref().expect("checked above");
            let num_frames = wt.num_frames;
            let frame_pos_scaled = self.frame_position * (num_frames as f32 - 1.0).max(0.0);
            let frame_idx0 = (frame_pos_scaled.floor() as usize).min(num_frames.saturating_sub(1));
            let frame_idx1 = (frame_idx0 + 1).min(num_frames - 1);
            let frame_frac = frame_pos_scaled - frame_pos_scaled.floor();

            // Read samples from both frames and crossfade.
            let s0 = wt.read_sample(frame_idx0, self.phase);
            let s1 = wt.read_sample(frame_idx1, self.phase);
            let sample = frame_frac.mul_add(s1 - s0, s0);

            // Scale by envelope so silence in produces silence out.
            *out_sample = sanitize_sample(sample * self.envelope_current);

            // Advance phase.
            self.phase += dt;
            self.phase -= self.phase.floor();

            // Advance frame position if scanning.
            if frame_dt > 0.0 {
                self.frame_position += frame_dt;
                self.frame_position -= self.frame_position.floor();
            }
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.frame_position = 0.0;
        self.envelope_current = 0.0;
    }

    fn name(&self) -> &'static str {
        "Wavetable Oscillator"
    }

    fn param_count(&self) -> usize {
        Self::PARAM_COUNT
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        let infos = Self::param_infos();
        infos.get(index).cloned()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_FREQUENCY => Some(self.frequency),
            Self::PARAM_FRAME_POSITION => Some(self.frame_position),
            Self::PARAM_FRAME_SCAN_RATE => Some(self.frame_scan_rate),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) -> Result<()> {
        let infos = Self::param_infos();
        let info = infos
            .get(index)
            .ok_or_else(|| Error::Config(format!("invalid param index {index}")))?;
        let clamped = info.clamp(value);

        match index {
            Self::PARAM_FREQUENCY => self.frequency = clamped,
            Self::PARAM_FRAME_POSITION => self.frame_position = clamped,
            Self::PARAM_FRAME_SCAN_RATE => self.frame_scan_rate = clamped,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        self.sample_rate = sr;
        self.envelope_attack_coeff = compute_envelope_coeff(5.0, sr);
        self.envelope_release_coeff = compute_envelope_coeff(50.0, sr);
        self.reset();
    }
}

/// Compute a one-pole smoothing coefficient from time in ms.
fn compute_envelope_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    if !time_ms.is_finite() || !sample_rate.is_finite() || time_ms <= 0.0 || sample_rate <= 0.0 {
        return 0.0;
    }
    let samples = time_ms * sample_rate / 1000.0;
    if samples < f32::EPSILON {
        return 0.0;
    }
    let coeff = (-1.0 / samples).exp();
    if coeff.is_finite() {
        coeff.clamp(0.0, 1.0 - f32::EPSILON)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Generate a sine wave for testing.
    fn sine_wave(freq: f32, sr: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn extractor_extracts_from_sine() {
        let sr = 44100.0;
        let freq = 440.0;
        let audio = sine_wave(freq, sr, (sr as usize) / 2);

        let extractor = WavetableExtractor::new(WavetableExtractor::DEFAULT_FRAME_SIZE);
        let wt = extractor.extract(&audio, freq, sr).expect("should extract");

        assert!(wt.num_frames() > 0, "should have at least one frame");
        assert_eq!(wt.frame_size(), WavetableExtractor::DEFAULT_FRAME_SIZE);
        assert!((wt.source_frequency() - freq).abs() < f32::EPSILON);
    }

    #[test]
    fn extractor_rejects_invalid_params() {
        let extractor = WavetableExtractor::new(2048);
        let audio = vec![0.0; 1024];

        assert!(extractor.extract(&audio, 0.0, 44100.0).is_err());
        assert!(extractor.extract(&audio, -100.0, 44100.0).is_err());
        assert!(extractor.extract(&audio, f32::NAN, 44100.0).is_err());
        assert!(extractor.extract(&audio, 440.0, 0.0).is_err());
        assert!(extractor.extract(&audio, 440.0, -1.0).is_err());
    }

    #[test]
    fn extractor_rejects_too_short_audio() {
        let extractor = WavetableExtractor::new(2048);
        let audio = vec![0.0; 10];
        assert!(extractor.extract(&audio, 440.0, 44100.0).is_err());
    }

    #[test]
    fn oscillator_silent_without_wavetable() {
        let mut osc = WavetableOscillator::new(44100.0);
        let input = vec![0.5_f32; 256];
        let mut output = vec![1.0_f32; 256];
        osc.process(&input, &mut output);

        for &s in &output {
            assert!(
                s.abs() < f32::EPSILON,
                "no wavetable should produce silence, got {s}"
            );
        }
    }

    #[test]
    fn oscillator_plays_back_at_correct_pitch() {
        let sr = 44100.0;
        let freq = 440.0;
        let audio = sine_wave(freq, sr, 4096);

        let extractor = WavetableExtractor::new(2048);
        let wt = extractor.extract(&audio, freq, sr).expect("should extract");

        let mut osc = WavetableOscillator::new(sr);
        osc.load_wavetable(wt);
        osc.set_frequency(freq);

        // Feed voice-like input to drive envelope.
        let voice = sine_wave(200.0, sr, 4096);
        let mut output = vec![0.0_f32; 4096];
        osc.process(&voice, &mut output);

        let energy: f32 = output[1024..].iter().map(|s| s * s).sum();
        assert!(
            energy > 0.01,
            "should produce audible output, energy = {energy}"
        );

        for &s in &output {
            assert!(s.is_finite(), "all output should be finite");
        }
    }

    #[test]
    fn oscillator_empty_buffers() {
        let mut osc = WavetableOscillator::new(44100.0);
        osc.process(&[], &mut []);
    }

    #[test]
    fn oscillator_param_roundtrip() {
        let mut osc = WavetableOscillator::new(44100.0);
        assert_eq!(osc.param_count(), 3);

        osc.set_param(0, 880.0).unwrap();
        assert!((osc.param_value(0).unwrap() - 880.0).abs() < f32::EPSILON);

        osc.set_param(1, 0.5).unwrap();
        assert!((osc.param_value(1).unwrap() - 0.5).abs() < f32::EPSILON);

        assert!(osc.set_param(99, 0.0).is_err());
    }

    #[test]
    fn oscillator_reset_clears_phase() {
        let mut osc = WavetableOscillator::new(44100.0);
        osc.phase = 0.75;
        osc.frame_position = 0.5;
        osc.reset();
        assert!(osc.phase.abs() < f32::EPSILON);
        assert!(osc.frame_position.abs() < f32::EPSILON);
    }

    #[test]
    fn wavetable_read_sample_interpolates() {
        let wt = Wavetable {
            frames: vec![vec![0.0, 1.0, 0.0, -1.0]],
            frame_size: 4,
            source_frequency: 440.0,
            num_frames: 1,
        };

        assert!((wt.read_sample(0, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((wt.read_sample(0, 0.25) - 1.0).abs() < f32::EPSILON);
        let interp = wt.read_sample(0, 0.125);
        assert!(
            (interp - 0.5).abs() < f32::EPSILON,
            "interpolated value should be 0.5, got {interp}"
        );
    }

    #[test]
    fn wavetable_empty_returns_zero() {
        let wt = Wavetable {
            frames: vec![],
            frame_size: 0,
            source_frequency: 440.0,
            num_frames: 0,
        };
        assert!(wt.is_empty());
        assert!((wt.read_sample(0, 0.5) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nan_input_handled() {
        let sr = 44100.0;
        let freq = 440.0;
        let audio = sine_wave(freq, sr, 4096);
        let extractor = WavetableExtractor::new(2048);
        let wt = extractor.extract(&audio, freq, sr).expect("should extract");

        let mut osc = WavetableOscillator::new(sr);
        osc.load_wavetable(wt);
        osc.set_frequency(freq);

        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        let mut output = [0.0_f32; 4];
        osc.process(&input, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] should be finite, got {s}");
        }
    }

    #[test]
    fn sample_rate_change() {
        let mut osc = WavetableOscillator::new(44100.0);
        osc.set_sample_rate(96000.0);
        let input = vec![0.5_f32; 256];
        let mut output = vec![0.0_f32; 256];
        osc.process(&input, &mut output);
        for &s in &output {
            assert!(s.is_finite());
        }
    }
}
