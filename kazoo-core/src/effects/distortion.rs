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
}
