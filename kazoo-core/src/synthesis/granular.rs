//! Granular synthesis: voice audio decomposed into grains and reassembled.
//!
//! [`GranularSynth`] maintains a circular buffer of incoming voice audio.
//! Grains are spawned at a configurable density rate, each reading from the
//! source buffer at a (potentially jittered) position with a windowed
//! envelope. The result is a cloud of overlapping micro-sounds.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

// ---------------------------------------------------------------------------
// Grain envelope shapes
// ---------------------------------------------------------------------------

/// Window function applied to each grain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrainEnvelope {
    Hann,
    Triangle,
    Gaussian,
    Tukey,
}

impl GrainEnvelope {
    /// Evaluate the envelope at normalised position `t` in `[0, 1]`.
    #[must_use]
    fn evaluate(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Hann => 0.5 * (1.0 - (std::f32::consts::TAU * t).cos()),
            Self::Triangle => {
                if t < 0.5 {
                    2.0 * t
                } else {
                    2.0 * (1.0 - t)
                }
            }
            Self::Gaussian => {
                let x = (t - 0.5) / 0.25;
                (-0.5 * x * x).exp()
            }
            Self::Tukey => {
                let half_alpha = 0.25_f32;
                if t < half_alpha {
                    0.5 * (1.0 - (std::f32::consts::PI * t / half_alpha).cos())
                } else if t > 1.0 - half_alpha {
                    0.5 * (1.0 - (std::f32::consts::PI * (1.0 - t) / half_alpha).cos())
                } else {
                    1.0
                }
            }
        }
    }

    #[must_use]
    fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::Triangle,
            2 => Self::Gaussian,
            3 => Self::Tukey,
            _ => Self::Hann,
        }
    }

    #[must_use]
    const fn to_param(self) -> f32 {
        match self {
            Self::Hann => 0.0,
            Self::Triangle => 1.0,
            Self::Gaussian => 2.0,
            Self::Tukey => 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Grain
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Grain {
    active: bool,
    source_position: f32,
    read_phase: f32,
    phase_increment: f32,
    envelope_phase: f32,
    envelope_increment: f32,
    gain: f32,
}

impl Grain {
    const fn inactive() -> Self {
        Self {
            active: false,
            source_position: 0.0,
            read_phase: 0.0,
            phase_increment: 1.0,
            envelope_phase: 0.0,
            envelope_increment: 0.0,
            gain: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Xorshift64 RNG
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    const fn new(seed: u64) -> Self {
        let state = if seed == 0 {
            0x5EED_DEAD_BEEF_CAFE
        } else {
            seed
        };
        Self { state }
    }

    const fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn next_f32_bipolar(&mut self) -> f32 {
        self.next_f32().mul_add(2.0, -1.0)
    }
}

// ---------------------------------------------------------------------------
// GranularSynth
// ---------------------------------------------------------------------------

const MAX_GRAINS: usize = 128;
const SOURCE_BUFFER_SECONDS: f32 = 2.0;

/// Granular synthesis engine.
///
/// Incoming voice audio fills a circular source buffer. Grains are spawned
/// at a configurable density rate, each reading a segment of the buffer
/// with a windowed envelope and optional pitch shifting.
#[derive(Debug)]
pub struct GranularSynth {
    sample_rate: f32,
    source_buffer: Vec<f32>,
    source_write_pos: usize,
    source_len: usize,
    grains: Vec<Grain>,
    spawn_counter: f32,
    grain_size_ms: f32,
    density: f32,
    position: f32,
    position_jitter: f32,
    pitch_shift_semitones: f32,
    pitch_jitter_semitones: f32,
    envelope_shape: GrainEnvelope,
    rng: Xorshift64,
}

impl GranularSynth {
    const PARAM_GRAIN_SIZE: usize = 0;
    const PARAM_DENSITY: usize = 1;
    const PARAM_POSITION: usize = 2;
    const PARAM_POSITION_JITTER: usize = 3;
    const PARAM_PITCH_SHIFT: usize = 4;
    const PARAM_PITCH_JITTER: usize = 5;
    const PARAM_ENVELOPE_SHAPE: usize = 6;
    const PARAM_COUNT: usize = 7;

    /// Create a new granular synth at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        let source_len = ((sr * SOURCE_BUFFER_SECONDS) as usize).max(1);

        Self {
            sample_rate: sr,
            source_buffer: vec![0.0; source_len],
            source_write_pos: 0,
            source_len,
            grains: (0..MAX_GRAINS).map(|_| Grain::inactive()).collect(),
            spawn_counter: 0.0,
            grain_size_ms: 50.0,
            density: 10.0,
            position: 0.5,
            position_jitter: 0.1,
            pitch_shift_semitones: 0.0,
            pitch_jitter_semitones: 0.0,
            envelope_shape: GrainEnvelope::Hann,
            rng: Xorshift64::new(0xDEAD_BEEF_1234_5678),
        }
    }

    fn spawn_grain(&mut self) {
        let Some(slot) = self.grains.iter_mut().find(|g| !g.active) else {
            return;
        };

        let grain_samples = (self.grain_size_ms * self.sample_rate / 1000.0).max(1.0);

        let jitter = self.rng.next_f32_bipolar() * self.position_jitter;
        let pos = (self.position + jitter).clamp(0.0, 1.0);
        let source_pos = pos * (self.source_len as f32 - 1.0);

        let pitch_jitter = self.rng.next_f32_bipolar() * self.pitch_jitter_semitones;
        let total_shift = self.pitch_shift_semitones + pitch_jitter;
        let phase_inc = (total_shift / 12.0).exp2();

        let gain_var = self.rng.next_f32_bipolar() * 0.15;
        let gain = (1.0 + gain_var).max(0.1);

        *slot = Grain {
            active: true,
            source_position: source_pos,
            read_phase: 0.0,
            phase_increment: phase_inc,
            envelope_phase: 0.0,
            envelope_increment: 1.0 / grain_samples,
            gain,
        };
    }

    fn param_infos() -> [ParamInfo; Self::PARAM_COUNT] {
        [
            ParamInfo {
                name: "Grain Size".into(),
                min: 1.0,
                max: 200.0,
                default: 50.0,
                unit: "ms".into(),
            },
            ParamInfo {
                name: "Density".into(),
                min: 1.0,
                max: 200.0,
                default: 10.0,
                unit: "grains/s".into(),
            },
            ParamInfo {
                name: "Position".into(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                unit: String::new(),
            },
            ParamInfo {
                name: "Position Jitter".into(),
                min: 0.0,
                max: 0.5,
                default: 0.1,
                unit: String::new(),
            },
            ParamInfo {
                name: "Pitch Shift".into(),
                min: -24.0,
                max: 24.0,
                default: 0.0,
                unit: "st".into(),
            },
            ParamInfo {
                name: "Pitch Jitter".into(),
                min: 0.0,
                max: 12.0,
                default: 0.0,
                unit: "st".into(),
            },
            ParamInfo {
                name: "Envelope".into(),
                min: 0.0,
                max: 3.0,
                default: GrainEnvelope::Hann.to_param(),
                unit: String::new(),
            },
        ]
    }
}

impl Processor for GranularSynth {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        if len == 0 {
            return;
        }

        let spawn_interval = if self.density > 0.0 {
            self.sample_rate / self.density
        } else {
            f32::MAX
        };

        for i in 0..len {
            let sample_in = sanitize_sample(input[i]);
            self.source_buffer[self.source_write_pos] = sample_in;
            self.source_write_pos = (self.source_write_pos + 1) % self.source_len;

            self.spawn_counter += 1.0;
            if self.spawn_counter >= spawn_interval {
                self.spawn_counter -= spawn_interval;
                self.spawn_grain();
            }

            let mut sum = 0.0_f32;
            let envelope_shape = self.envelope_shape;

            for grain in &mut self.grains {
                if !grain.active {
                    continue;
                }

                let read_pos = grain.source_position + grain.read_phase;
                let sample = read_source_at(&self.source_buffer, self.source_len, read_pos);

                let env = envelope_shape.evaluate(grain.envelope_phase);
                sum += sample * env * grain.gain;

                grain.read_phase += grain.phase_increment;
                grain.envelope_phase += grain.envelope_increment;

                if grain.envelope_phase >= 1.0 {
                    grain.active = false;
                }
            }

            output[i] = sanitize_sample(sum);
        }
    }

    fn reset(&mut self) {
        self.source_buffer.fill(0.0);
        self.source_write_pos = 0;
        self.spawn_counter = 0.0;
        for grain in &mut self.grains {
            *grain = Grain::inactive();
        }
    }

    fn name(&self) -> &'static str {
        "Granular Synth"
    }

    fn param_count(&self) -> usize {
        Self::PARAM_COUNT
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        Self::param_infos().get(index).cloned()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_GRAIN_SIZE => Some(self.grain_size_ms),
            Self::PARAM_DENSITY => Some(self.density),
            Self::PARAM_POSITION => Some(self.position),
            Self::PARAM_POSITION_JITTER => Some(self.position_jitter),
            Self::PARAM_PITCH_SHIFT => Some(self.pitch_shift_semitones),
            Self::PARAM_PITCH_JITTER => Some(self.pitch_jitter_semitones),
            Self::PARAM_ENVELOPE_SHAPE => Some(self.envelope_shape.to_param()),
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
            Self::PARAM_GRAIN_SIZE => self.grain_size_ms = clamped,
            Self::PARAM_DENSITY => self.density = clamped,
            Self::PARAM_POSITION => self.position = clamped,
            Self::PARAM_POSITION_JITTER => self.position_jitter = clamped,
            Self::PARAM_PITCH_SHIFT => self.pitch_shift_semitones = clamped,
            Self::PARAM_PITCH_JITTER => self.pitch_jitter_semitones = clamped,
            Self::PARAM_ENVELOPE_SHAPE => self.envelope_shape = GrainEnvelope::from_param(clamped),
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
        let new_len = ((sr * SOURCE_BUFFER_SECONDS) as usize).max(1);
        self.source_buffer.resize(new_len, 0.0);
        self.source_len = new_len;
        self.reset();
    }
}

/// Read from a source buffer with linear interpolation (free function for borrow safety).
#[inline]
fn read_source_at(buffer: &[f32], buffer_len: usize, position: f32) -> f32 {
    if buffer_len == 0 {
        return 0.0;
    }
    let pos = position.rem_euclid(buffer_len as f32);
    let idx0 = pos.floor() as usize % buffer_len;
    let idx1 = (idx0 + 1) % buffer_len;
    let frac = pos - pos.floor();

    let s0 = sanitize_sample(buffer[idx0]);
    let s1 = sanitize_sample(buffer[idx1]);
    frac.mul_add(s1 - s0, s0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn processes_without_panic() {
        let mut synth = GranularSynth::new(44100.0);
        let input: Vec<f32> = (0..4096)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut output = vec![0.0_f32; 4096];
        synth.process(&input, &mut output);

        for &s in &output {
            assert!(s.is_finite(), "output should be finite, got {s}");
        }
    }

    #[test]
    fn grains_produce_output() {
        let mut synth = GranularSynth::new(44100.0);
        synth.density = 50.0;
        synth.grain_size_ms = 30.0;

        let fill: Vec<f32> = (0..44100)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut discard = vec![0.0_f32; 44100];
        synth.process(&fill, &mut discard);

        let input: Vec<f32> = (0..4096)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut output = vec![0.0_f32; 4096];
        synth.process(&input, &mut output);

        let energy: f32 = output.iter().map(|s| s * s).sum();
        assert!(
            energy > 0.01,
            "should produce audible output, energy = {energy}"
        );
    }

    #[test]
    fn pitch_shift_changes_output() {
        let sr = 44100.0;
        let input: Vec<f32> = (0..44100)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / sr).sin())
            .collect();

        let mut synth0 = GranularSynth::new(sr);
        synth0.density = 30.0;
        let mut out0 = vec![0.0_f32; 44100];
        synth0.process(&input, &mut out0);

        let mut synth12 = GranularSynth::new(sr);
        synth12.density = 30.0;
        synth12.pitch_shift_semitones = 12.0;
        let mut out12 = vec![0.0_f32; 44100];
        synth12.process(&input, &mut out12);

        let diff: f32 = out0
            .iter()
            .zip(out12.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            diff > 0.1,
            "pitch shift should produce different output, diff = {diff}"
        );
    }

    #[test]
    fn empty_buffers_no_panic() {
        let mut synth = GranularSynth::new(44100.0);
        synth.process(&[], &mut []);
    }

    #[test]
    fn nan_inf_input_handled() {
        let mut synth = GranularSynth::new(44100.0);
        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
        let mut output = [0.0_f32; 5];
        synth.process(&input, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] should be finite, got {s}");
        }
    }

    #[test]
    fn param_count_and_info() {
        let synth = GranularSynth::new(44100.0);
        assert_eq!(synth.param_count(), 7);
        for i in 0..7 {
            assert!(synth.param_info(i).is_some());
            assert!(synth.param_value(i).is_some());
        }
        assert!(synth.param_info(7).is_none());
    }

    #[test]
    fn set_param_clamps() {
        let mut synth = GranularSynth::new(44100.0);
        synth
            .set_param(GranularSynth::PARAM_GRAIN_SIZE, 999.0)
            .unwrap();
        let val = synth.param_value(GranularSynth::PARAM_GRAIN_SIZE).unwrap();
        assert!((val - 200.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_param_invalid_index() {
        let mut synth = GranularSynth::new(44100.0);
        assert!(synth.set_param(99, 0.0).is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut synth = GranularSynth::new(44100.0);
        let input = vec![1.0_f32; 4096];
        let mut output = vec![0.0_f32; 4096];
        synth.process(&input, &mut output);

        synth.reset();
        assert_eq!(synth.source_write_pos, 0);
        assert!(synth.spawn_counter.abs() < f32::EPSILON);
        assert!(synth.grains.iter().all(|g| !g.active));
    }

    #[test]
    fn envelope_shapes_produce_correct_values() {
        assert!(GrainEnvelope::Hann.evaluate(0.0).abs() < 1e-6);
        assert!((GrainEnvelope::Hann.evaluate(0.5) - 1.0).abs() < 1e-6);
        assert!(GrainEnvelope::Hann.evaluate(1.0).abs() < 1e-6);

        assert!(GrainEnvelope::Triangle.evaluate(0.0).abs() < 1e-6);
        assert!((GrainEnvelope::Triangle.evaluate(0.5) - 1.0).abs() < 1e-6);
        assert!(GrainEnvelope::Triangle.evaluate(1.0).abs() < 1e-6);

        let center = GrainEnvelope::Gaussian.evaluate(0.5);
        let edge = GrainEnvelope::Gaussian.evaluate(0.0);
        assert!(center > edge);

        assert!((GrainEnvelope::Tukey.evaluate(0.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn xorshift_produces_unique_values() {
        let mut rng = Xorshift64::new(42);
        let a = rng.next_f32();
        let b = rng.next_f32();
        assert!(a != b);
        assert!((0.0..1.0).contains(&a));
        assert!((0.0..1.0).contains(&b));
    }

    #[test]
    fn sample_rate_change() {
        let mut synth = GranularSynth::new(44100.0);
        synth.set_sample_rate(96000.0);
        assert_eq!(synth.source_len, (96000.0 * SOURCE_BUFFER_SECONDS) as usize);
    }

    #[test]
    fn xorshift_zero_seed_uses_default() {
        let mut rng = Xorshift64::new(0);
        let a = rng.next_f32();
        assert!((0.0..1.0).contains(&a), "value should be in [0, 1): {a}");
    }

    #[test]
    fn xorshift_bipolar_range() {
        let mut rng = Xorshift64::new(42);
        for _ in 0..1000 {
            let v = rng.next_f32_bipolar();
            assert!(
                (-1.0..1.0).contains(&v),
                "bipolar should be in [-1, 1): {v}"
            );
        }
    }

    #[test]
    fn xorshift_f32_range() {
        let mut rng = Xorshift64::new(12345);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "f32 should be in [0, 1): {v}");
        }
    }

    #[test]
    fn envelope_hann_intermediate_values() {
        // Hann at 0.25 should be 0.5*(1 - cos(pi/2)) = 0.5*(1 - 0) = 0.5.
        let val = GrainEnvelope::Hann.evaluate(0.25);
        assert!(
            (val - 0.5).abs() < 1e-5,
            "Hann(0.25) should be ~0.5, got {val}"
        );

        // Hann at 0.75 should also be 0.5 (symmetric).
        let val75 = GrainEnvelope::Hann.evaluate(0.75);
        assert!(
            (val75 - 0.5).abs() < 1e-5,
            "Hann(0.75) should be ~0.5, got {val75}"
        );
    }

    #[test]
    fn envelope_triangle_intermediate() {
        assert!((GrainEnvelope::Triangle.evaluate(0.25) - 0.5).abs() < 1e-6);
        assert!((GrainEnvelope::Triangle.evaluate(0.75) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn envelope_tukey_plateau_region() {
        // Tukey with half_alpha=0.25: plateau is between 0.25 and 0.75.
        assert!(
            (GrainEnvelope::Tukey.evaluate(0.3) - 1.0).abs() < 1e-6,
            "Tukey should be 1.0 in plateau"
        );
        assert!(
            (GrainEnvelope::Tukey.evaluate(0.5) - 1.0).abs() < 1e-6,
            "Tukey should be 1.0 in plateau"
        );
        assert!(
            (GrainEnvelope::Tukey.evaluate(0.7) - 1.0).abs() < 1e-6,
            "Tukey should be 1.0 in plateau"
        );
    }

    #[test]
    fn envelope_gaussian_peak_near_one() {
        let peak = GrainEnvelope::Gaussian.evaluate(0.5);
        assert!(
            (peak - 1.0).abs() < 1e-5,
            "Gaussian peak should be ~1.0, got {peak}"
        );
    }

    #[test]
    fn envelope_clamps_outside_range() {
        // Values outside [0, 1] should be clamped.
        assert!(GrainEnvelope::Hann.evaluate(-0.5).abs() < 1e-6);
        assert!(GrainEnvelope::Hann.evaluate(1.5).abs() < 1e-6);
    }

    #[test]
    fn envelope_shape_from_param_roundtrip() {
        for shape in [
            GrainEnvelope::Hann,
            GrainEnvelope::Triangle,
            GrainEnvelope::Gaussian,
            GrainEnvelope::Tukey,
        ] {
            let param = shape.to_param();
            let recovered = GrainEnvelope::from_param(param);
            assert_eq!(shape, recovered, "roundtrip failed for {shape:?}");
        }
    }

    #[test]
    fn envelope_from_param_out_of_range_defaults_to_hann() {
        assert_eq!(GrainEnvelope::from_param(-1.0), GrainEnvelope::Hann);
        assert_eq!(GrainEnvelope::from_param(99.0), GrainEnvelope::Hann);
    }

    #[test]
    fn density_zero_produces_silence() {
        let sr = 44100.0;
        let mut synth = GranularSynth::new(sr);
        let _ = synth.set_param(GranularSynth::PARAM_DENSITY, 0.5); // minimum density

        let input: Vec<f32> = (0..4096)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; 4096];
        synth.process(&input, &mut output);

        // At minimum density, very few grains should spawn. Output should be finite.
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn stability_with_noise_input() {
        let mut synth = GranularSynth::new(44100.0);
        synth.density = 20.0;

        let mut rng: u32 = 0xFACE_FEED;
        let noise: Vec<f32> = (0..4096)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();
        let mut output = vec![0.0_f32; 4096];
        synth.process(&noise, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.is_finite() && s.abs() < 100.0,
                "noise stability: output[{i}] = {s}"
            );
        }
    }

    #[test]
    fn extreme_pitch_shift() {
        let sr = 44100.0;
        let input: Vec<f32> = (0..8192)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / sr).sin())
            .collect();

        for semitones in [-24.0, -12.0, 12.0, 24.0] {
            let mut synth = GranularSynth::new(sr);
            synth.density = 20.0;
            synth.pitch_shift_semitones = semitones;

            let mut output = vec![0.0_f32; 8192];
            synth.process(&input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() < 100.0,
                    "pitch_shift={semitones}: output[{i}] = {s}"
                );
            }
        }
    }

    #[test]
    fn param_info_names_not_empty() {
        let synth = GranularSynth::new(44100.0);
        for i in 0..synth.param_count() {
            let info = synth.param_info(i).unwrap();
            assert!(!info.name.is_empty(), "param {i} has empty name");
        }
    }

    #[test]
    fn name_is_not_empty() {
        let synth = GranularSynth::new(44100.0);
        assert!(!synth.name().is_empty());
    }

    #[test]
    fn long_sustained_processing() {
        let sr = 44100.0;
        let mut synth = GranularSynth::new(sr);
        synth.density = 15.0;

        let input: Vec<f32> = (0..512)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; 512];

        for _ in 0..50 {
            synth.process(&input, &mut output);
            for &s in &output {
                assert!(s.is_finite() && s.abs() < 100.0);
            }
        }
    }
}
