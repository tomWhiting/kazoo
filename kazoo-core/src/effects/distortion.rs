//! Distortion effects: soft clip, hard clip, waveshaping, and bitcrushing.
//!
//! All modes share parameters for drive, mix, and tone. A one-pole low-pass
//! filter after the distortion stage provides the tone control.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

/// Distortion algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistortionType {
    /// Soft clipping via `tanh`.
    SoftClip,
    /// Hard clipping via `clamp`.
    HardClip,
    /// Waveshaping transfer function.
    Waveshape,
    /// Bit-depth reduction.
    Bitcrush,
}

/// Distortion processor with selectable algorithm.
#[derive(Debug)]
pub struct Distortion {
    sample_rate: f32,
    mode: DistortionType,
    drive: f32,
    mix: f32,
    tone: f32,
    // One-pole LP state for tone control.
    tone_state: f32,
}

impl Distortion {
    const PARAM_DRIVE: usize = 0;
    const PARAM_MIX: usize = 1;
    const PARAM_TONE: usize = 2;

    const DRIVE_MIN: f32 = 0.0;
    const DRIVE_MAX: f32 = 1.0;
    const DRIVE_DEFAULT: f32 = 0.5;

    const MIX_MIN: f32 = 0.0;
    const MIX_MAX: f32 = 1.0;
    const MIX_DEFAULT: f32 = 1.0;

    const TONE_MIN: f32 = 0.0;
    const TONE_MAX: f32 = 1.0;
    const TONE_DEFAULT: f32 = 0.5;

    /// Create a new distortion processor of the given type.
    #[must_use]
    pub const fn new(mode: DistortionType, sample_rate: f32) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            mode,
            drive: Self::DRIVE_DEFAULT,
            mix: Self::MIX_DEFAULT,
            tone: Self::TONE_DEFAULT,
            tone_state: 0.0,
        }
    }

    /// Apply the selected distortion algorithm to a single sample.
    #[inline]
    fn distort(&self, x: f32) -> f32 {
        match self.mode {
            DistortionType::SoftClip => {
                let gain = self.drive.mul_add(20.0, 1.0);
                (gain * x).tanh()
            }
            DistortionType::HardClip => {
                let gain = self.drive.mul_add(20.0, 1.0);
                (gain * x).clamp(-1.0, 1.0)
            }
            DistortionType::Waveshape => {
                // Transfer function: x * (|x| + drive) / (x^2 + (drive - 1) * |x| + 1)
                let abs_x = x.abs();
                let numerator = x * (abs_x + self.drive);
                let denominator = x.mul_add(x, (self.drive - 1.0).mul_add(abs_x, 1.0));
                if denominator.abs() > f32::EPSILON {
                    numerator / denominator
                } else {
                    0.0
                }
            }
            DistortionType::Bitcrush => {
                // Quantize to fewer levels. At drive=0 full resolution; at drive=1, ~1 bit.
                let levels = (16.0 * (1.0 - self.drive)).exp2();
                if levels >= 1.0 {
                    (x * levels).round() / levels
                } else {
                    0.0
                }
            }
        }
    }

    /// Compute the one-pole LP coefficient for the tone control.
    ///
    /// tone=0 is dark (low cutoff), tone=1 is bright (wide open).
    fn tone_coefficient(&self) -> f32 {
        // Map tone [0,1] to a cutoff range of roughly [200 Hz, 20000 Hz].
        let cutoff = 200.0 * (100.0_f32).powf(self.tone);
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        let dt = 1.0 / self.sample_rate;
        dt / (rc + dt)
    }

    fn param_infos() -> [ParamInfo; 3] {
        [
            ParamInfo {
                name: "Drive".into(),
                min: Self::DRIVE_MIN,
                max: Self::DRIVE_MAX,
                default: Self::DRIVE_DEFAULT,
                unit: String::new(),
            },
            ParamInfo {
                name: "Mix".into(),
                min: Self::MIX_MIN,
                max: Self::MIX_MAX,
                default: Self::MIX_DEFAULT,
                unit: String::new(),
            },
            ParamInfo {
                name: "Tone".into(),
                min: Self::TONE_MIN,
                max: Self::TONE_MAX,
                default: Self::TONE_DEFAULT,
                unit: String::new(),
            },
        ]
    }
}

impl Processor for Distortion {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        let mix = self.mix;
        let alpha = self.tone_coefficient();

        for i in 0..len {
            let x = sanitize_sample(input[i]);
            let distorted = self.distort(x);

            // One-pole low-pass tone filter.
            self.tone_state += alpha * (distorted - self.tone_state);
            if self.tone_state.abs() < 1e-30 {
                self.tone_state = 0.0;
            }

            let wet = self.tone_state;
            output[i] = sanitize_sample(x.mul_add(1.0 - mix, wet * mix));
        }
    }

    fn reset(&mut self) {
        self.tone_state = 0.0;
    }

    fn name(&self) -> &'static str {
        match self.mode {
            DistortionType::SoftClip => "Distortion (Soft Clip)",
            DistortionType::HardClip => "Distortion (Hard Clip)",
            DistortionType::Waveshape => "Distortion (Waveshape)",
            DistortionType::Bitcrush => "Distortion (Bitcrush)",
        }
    }

    fn param_count(&self) -> usize {
        3
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        let infos = Self::param_infos();
        infos.get(index).cloned()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_DRIVE => Some(self.drive),
            Self::PARAM_MIX => Some(self.mix),
            Self::PARAM_TONE => Some(self.tone),
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
            Self::PARAM_DRIVE => self.drive = clamped,
            Self::PARAM_MIX => self.mix = clamped,
            Self::PARAM_TONE => self.tone = clamped,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.tone_state = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_sine(len: usize, freq: f32, sr: f32) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn soft_clip_reduces_peaks() {
        let sr = 44100.0;
        let mut dist = Distortion::new(DistortionType::SoftClip, sr);
        dist.set_param(Distortion::PARAM_DRIVE, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_TONE, 1.0).unwrap();

        let input = generate_sine(2048, 440.0, sr);
        let mut output = vec![0.0_f32; 2048];
        dist.process(&input, &mut output);

        // Soft-clipping should limit the peak to near 1.0 (tanh saturates).
        let peak: f32 = output.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(peak <= 1.001, "soft clip peak should be <= 1.0, got {peak}");
    }

    #[test]
    fn hard_clip_clamps() {
        let sr = 44100.0;
        let mut dist = Distortion::new(DistortionType::HardClip, sr);
        dist.set_param(Distortion::PARAM_DRIVE, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_TONE, 1.0).unwrap();

        let input = generate_sine(2048, 440.0, sr);
        let mut output = vec![0.0_f32; 2048];
        dist.process(&input, &mut output);

        // Hard clip should strictly clamp to [-1, 1].
        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.abs() <= 1.001,
                "hard clip output[{i}] = {s} exceeds [-1, 1]"
            );
        }
    }

    #[test]
    fn bitcrush_quantizes() {
        let sr = 44100.0;
        let mut dist = Distortion::new(DistortionType::Bitcrush, sr);
        dist.set_param(Distortion::PARAM_DRIVE, 0.8).unwrap();
        dist.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_TONE, 1.0).unwrap();

        let input = generate_sine(2048, 440.0, sr);
        let mut output = vec![0.0_f32; 2048];
        dist.process(&input, &mut output);

        // With heavy bitcrushing, the signal should be quantized (fewer unique values).
        let mut unique: Vec<i32> = output[256..]
            .iter()
            .map(|&s| (s * 10000.0).round() as i32)
            .collect();
        unique.sort_unstable();
        unique.dedup();

        // A pure sine at this resolution has ~1792 unique values; crushed should have fewer.
        assert!(
            unique.len() < 500,
            "bitcrushed signal should have few unique values, got {}",
            unique.len()
        );
    }

    #[test]
    fn waveshape_modifies_signal() {
        let sr = 44100.0;
        let mut dist = Distortion::new(DistortionType::Waveshape, sr);
        dist.set_param(Distortion::PARAM_DRIVE, 0.5).unwrap();
        dist.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_TONE, 1.0).unwrap();

        let input = generate_sine(2048, 440.0, sr);
        let mut output = vec![0.0_f32; 2048];
        dist.process(&input, &mut output);

        let diff_energy: f32 = input
            .iter()
            .zip(output.iter())
            .skip(256)
            .map(|(a, b)| {
                let d = a - b;
                d * d
            })
            .sum();

        assert!(
            diff_energy > 0.001,
            "waveshaper should modify signal, diff={diff_energy}"
        );
    }

    #[test]
    fn distortion_handles_nan() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        let input = [f32::NAN, f32::INFINITY, 0.5, 0.0];
        let mut output = [0.0_f32; 4];
        dist.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn distortion_empty_buffers() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        dist.process(&[], &mut []);
    }

    #[test]
    fn distortion_param_count() {
        let dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        assert_eq!(dist.param_count(), 3);
    }

    #[test]
    fn distortion_fully_dry_passes_input() {
        for mode in [
            DistortionType::SoftClip,
            DistortionType::HardClip,
            DistortionType::Waveshape,
            DistortionType::Bitcrush,
        ] {
            let mut dist = Distortion::new(mode, 44100.0);
            dist.set_param(Distortion::PARAM_MIX, 0.0).unwrap();

            let input = [0.5, -0.3, 0.8, -0.1, 0.0];
            let mut output = [0.0_f32; 5];
            dist.process(&input, &mut output);

            for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
                assert!(
                    (inp - out).abs() < 1e-6,
                    "{mode:?} dry pass: [{i}] expected {inp}, got {out}"
                );
            }
        }
    }

    #[test]
    fn distortion_zero_drive_soft_clip_near_unity() {
        let sr = 44100.0;
        let mut dist = Distortion::new(DistortionType::SoftClip, sr);
        dist.set_param(Distortion::PARAM_DRIVE, 0.0).unwrap();
        dist.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist.set_param(Distortion::PARAM_TONE, 1.0).unwrap(); // bright (wide open)

        // At drive=0, gain = 0*20 + 1 = 1, so tanh(1*x) ≈ x for small x.
        let input = generate_sine(2048, 440.0, sr);
        let mut output = vec![0.0_f32; 2048];
        dist.process(&input, &mut output);

        // Compare second half (after transient).
        let diff_energy: f32 = input[1024..]
            .iter()
            .zip(output[1024..].iter())
            .map(|(a, b)| {
                let d = a - b;
                d * d
            })
            .sum();
        let in_energy: f32 = input[1024..].iter().map(|s| s * s).sum();

        // The difference should be small relative to input.
        assert!(
            diff_energy / in_energy < 0.1,
            "zero drive soft clip should be near unity: diff/energy = {}",
            diff_energy / in_energy
        );
    }

    #[test]
    fn distortion_max_drive_all_modes_bounded() {
        let sr = 44100.0;
        let input = generate_sine(2048, 440.0, sr);

        for mode in [
            DistortionType::SoftClip,
            DistortionType::HardClip,
            DistortionType::Waveshape,
            DistortionType::Bitcrush,
        ] {
            let mut dist = Distortion::new(mode, sr);
            dist.set_param(Distortion::PARAM_DRIVE, 1.0).unwrap();
            dist.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
            dist.set_param(Distortion::PARAM_TONE, 1.0).unwrap();

            let mut output = vec![0.0_f32; 2048];
            dist.process(&input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() <= 1.1,
                    "{mode:?} max drive: output[{i}] = {s} exceeds bounds"
                );
            }
        }
    }

    #[test]
    fn distortion_tone_dark_vs_bright() {
        let sr = 44100.0;

        // Generate a signal with both low and high frequency content.
        let input: Vec<f32> = (0..4096)
            .map(|i| {
                let t = i as f32 / sr;
                // 200 Hz + 8 kHz mixed.
                (2.0 * PI * 200.0 * t).sin() * 0.5 + (2.0 * PI * 8000.0 * t).sin() * 0.5
            })
            .collect();

        // Dark tone (tone=0).
        let mut dist_dark = Distortion::new(DistortionType::SoftClip, sr);
        dist_dark.set_param(Distortion::PARAM_DRIVE, 0.5).unwrap();
        dist_dark.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist_dark.set_param(Distortion::PARAM_TONE, 0.0).unwrap();

        let mut output_dark = vec![0.0_f32; 4096];
        dist_dark.process(&input, &mut output_dark);

        // Bright tone (tone=1).
        let mut dist_bright = Distortion::new(DistortionType::SoftClip, sr);
        dist_bright.set_param(Distortion::PARAM_DRIVE, 0.5).unwrap();
        dist_bright.set_param(Distortion::PARAM_MIX, 1.0).unwrap();
        dist_bright.set_param(Distortion::PARAM_TONE, 1.0).unwrap();

        let mut output_bright = vec![0.0_f32; 4096];
        dist_bright.process(&input, &mut output_bright);

        // The dark output should have less high-frequency energy.
        // Measure this by looking at sample-to-sample differences (a proxy for HF).
        let hf_dark: f32 = output_dark
            .windows(2)
            .skip(1024)
            .map(|w| {
                let d = w[1] - w[0];
                d * d
            })
            .sum();
        let hf_bright: f32 = output_bright
            .windows(2)
            .skip(1024)
            .map(|w| {
                let d = w[1] - w[0];
                d * d
            })
            .sum();

        assert!(
            hf_bright > hf_dark * 1.5,
            "bright ({hf_bright}) should have more HF content than dark ({hf_dark})"
        );
    }

    #[test]
    fn distortion_sample_rate_change() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        let input = [0.5_f32; 64];
        let mut output = [0.0_f32; 64];
        dist.process(&input, &mut output);

        dist.set_sample_rate(96000.0);

        // After SR change, tone_state is reset — should not produce artifacts.
        let mut out2 = [0.0_f32; 64];
        dist.process(&input, &mut out2);
        for &s in &out2 {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn distortion_stability_with_noise_all_modes() {
        let mut rng: u32 = 0xDEAD_C0DE;
        let noise: Vec<f32> = (0..4096)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        for mode in [
            DistortionType::SoftClip,
            DistortionType::HardClip,
            DistortionType::Waveshape,
            DistortionType::Bitcrush,
        ] {
            let mut dist = Distortion::new(mode, 44100.0);
            dist.set_param(Distortion::PARAM_DRIVE, 0.8).unwrap();
            let mut output = vec![0.0_f32; 4096];
            dist.process(&noise, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() < 100.0,
                    "{mode:?} noise: output[{i}] = {s}"
                );
            }
        }
    }

    #[test]
    fn distortion_all_mode_names_not_empty() {
        for mode in [
            DistortionType::SoftClip,
            DistortionType::HardClip,
            DistortionType::Waveshape,
            DistortionType::Bitcrush,
        ] {
            let dist = Distortion::new(mode, 44100.0);
            assert!(!dist.name().is_empty(), "{mode:?} has empty name");
        }
    }

    #[test]
    fn distortion_all_param_info_names() {
        let dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        for i in 0..dist.param_count() {
            let info = dist.param_info(i).unwrap();
            assert!(!info.name.is_empty(), "param {i} has empty name");
        }
    }

    #[test]
    fn distortion_invalid_param_index() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        assert!(dist.set_param(99, 0.0).is_err());
        assert!(dist.param_value(99).is_none());
        assert!(dist.param_info(99).is_none());
    }

    #[test]
    fn distortion_param_clamping() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);

        // Drive above max (1.0) should clamp.
        dist.set_param(Distortion::PARAM_DRIVE, 5.0).unwrap();
        assert!((dist.param_value(Distortion::PARAM_DRIVE).unwrap() - 1.0).abs() < f32::EPSILON);

        // Mix below min (0.0) should clamp.
        dist.set_param(Distortion::PARAM_MIX, -1.0).unwrap();
        assert!(dist.param_value(Distortion::PARAM_MIX).unwrap().abs() < f32::EPSILON);
    }

    #[test]
    fn distortion_param_values_roundtrip() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        dist.set_param(Distortion::PARAM_DRIVE, 0.7).unwrap();
        dist.set_param(Distortion::PARAM_MIX, 0.3).unwrap();
        dist.set_param(Distortion::PARAM_TONE, 0.8).unwrap();

        assert!((dist.param_value(0).unwrap() - 0.7).abs() < f32::EPSILON);
        assert!((dist.param_value(1).unwrap() - 0.3).abs() < f32::EPSILON);
        assert!((dist.param_value(2).unwrap() - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn distortion_reset_clears_tone_state() {
        let mut dist = Distortion::new(DistortionType::SoftClip, 44100.0);
        let input = generate_sine(512, 440.0, 44100.0);
        let mut output = vec![0.0_f32; 512];
        dist.process(&input, &mut output);

        dist.reset();

        // After reset, processing silence should yield silence (tone_state = 0).
        let silence = [0.0_f32; 64];
        let mut out2 = [0.0_f32; 64];
        dist.process(&silence, &mut out2);

        for (i, &s) in out2.iter().enumerate() {
            assert!(
                s.abs() < 1e-10,
                "after reset, output[{i}] should be ~0, got {s}"
            );
        }
    }
}
