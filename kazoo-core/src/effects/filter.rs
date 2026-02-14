//! Biquad filter: LP, HP, BP, Notch, Peak, `LowShelf`, `HighShelf`.
//!
//! Implements Direct Form II Transposed structure with coefficient computation
//! from the Audio EQ Cookbook (Robert Bristow-Johnson).

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};
use std::f32::consts::{FRAC_1_SQRT_2, PI};

/// Filter topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peak,
    LowShelf,
    HighShelf,
}

/// Biquad coefficients (normalised so a0 = 1).
#[derive(Debug, Clone, Copy)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Default for Coefficients {
    fn default() -> Self {
        // Pass-through by default.
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

/// Standard Direct Form II Transposed biquad filter.
#[derive(Debug)]
pub struct BiquadFilter {
    filter_type: FilterType,
    sample_rate: f32,
    frequency: f32,
    q: f32,
    gain_db: f32,
    coeffs: Coefficients,
    // Direct Form II Transposed state variables.
    z1: f32,
    z2: f32,
}

impl BiquadFilter {
    const PARAM_FREQUENCY: usize = 0;
    const PARAM_Q: usize = 1;
    const PARAM_GAIN_DB: usize = 2;

    const FREQ_MIN: f32 = 20.0;
    const FREQ_MAX: f32 = 20_000.0;
    const FREQ_DEFAULT: f32 = 1000.0;

    const Q_MIN: f32 = 0.1;
    const Q_MAX: f32 = 30.0;
    const Q_DEFAULT: f32 = FRAC_1_SQRT_2;

    const GAIN_DB_MIN: f32 = -24.0;
    const GAIN_DB_MAX: f32 = 24.0;
    const GAIN_DB_DEFAULT: f32 = 0.0;

    /// Create a new biquad filter of the given type at the specified sample rate.
    #[must_use]
    pub fn new(filter_type: FilterType, sample_rate: f32) -> Self {
        let mut f = Self {
            filter_type,
            sample_rate: sample_rate.max(1.0),
            frequency: Self::FREQ_DEFAULT,
            q: Self::Q_DEFAULT,
            gain_db: Self::GAIN_DB_DEFAULT,
            coeffs: Coefficients::default(),
            z1: 0.0,
            z2: 0.0,
        };
        f.recalculate_coefficients();
        f
    }

    /// Recalculate filter coefficients from the current parameters.
    ///
    /// Uses the Audio EQ Cookbook formulas (Robert Bristow-Johnson).
    #[allow(clippy::many_single_char_names)]
    fn recalculate_coefficients(&mut self) {
        let sr = self.sample_rate;
        let nyquist = sr * 0.5;

        // Clamp frequency to just below Nyquist to avoid numerical instability.
        let freq = self
            .frequency
            .clamp(Self::FREQ_MIN, (nyquist - 1.0).max(Self::FREQ_MIN));
        let q = self.q.max(0.001);
        let a_linear = 10.0_f32.powf(self.gain_db / 40.0); // for Peak/Shelf

        let w0 = 2.0 * PI * freq / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let ap1 = a_linear + 1.0;
        let am1 = a_linear - 1.0;

        let (b0, b1, b2, a0, a1, a2) = match self.filter_type {
            FilterType::LowPass => {
                let b1 = 1.0 - cos_w0;
                let b0 = b1 / 2.0;
                let b2 = b0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::HighPass => {
                let b1_raw = 1.0 + cos_w0;
                let b0 = b1_raw / 2.0;
                let b1 = -(1.0 + cos_w0);
                let b2 = b0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::BandPass => {
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::Notch => {
                let b0 = 1.0;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::Peak => {
                let b0 = alpha.mul_add(a_linear, 1.0);
                let b1 = -2.0 * cos_w0;
                let b2 = (-alpha).mul_add(a_linear, 1.0);
                let a0 = (alpha / a_linear) + 1.0;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha / a_linear;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::LowShelf => {
                let two_sqrt_a_alpha = 2.0 * a_linear.sqrt() * alpha;
                let b0 = a_linear * (am1.mul_add(-cos_w0, ap1) + two_sqrt_a_alpha);
                let b1 = 2.0 * a_linear * ap1.mul_add(-cos_w0, am1);
                let b2 = a_linear * (am1.mul_add(-cos_w0, ap1) - two_sqrt_a_alpha);
                let a0 = am1.mul_add(cos_w0, ap1) + two_sqrt_a_alpha;
                let a1 = -2.0 * ap1.mul_add(cos_w0, am1);
                let a2 = am1.mul_add(cos_w0, ap1) - two_sqrt_a_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::HighShelf => {
                let two_sqrt_a_alpha = 2.0 * a_linear.sqrt() * alpha;
                let b0 = a_linear * (am1.mul_add(cos_w0, ap1) + two_sqrt_a_alpha);
                let b1 = -2.0 * a_linear * ap1.mul_add(cos_w0, am1);
                let b2 = a_linear * (am1.mul_add(cos_w0, ap1) - two_sqrt_a_alpha);
                let a0 = am1.mul_add(-cos_w0, ap1) + two_sqrt_a_alpha;
                let a1 = 2.0 * ap1.mul_add(-cos_w0, am1);
                let a2 = am1.mul_add(-cos_w0, ap1) - two_sqrt_a_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
        };

        // Normalise so a0 = 1 and guard against division by zero.
        let inv_a0 = if a0.abs() > f32::EPSILON {
            1.0 / a0
        } else {
            1.0
        };

        self.coeffs = Coefficients {
            b0: sanitize_sample(b0 * inv_a0),
            b1: sanitize_sample(b1 * inv_a0),
            b2: sanitize_sample(b2 * inv_a0),
            a1: sanitize_sample(a1 * inv_a0),
            a2: sanitize_sample(a2 * inv_a0),
        };
    }

    fn param_infos() -> [ParamInfo; 3] {
        [
            ParamInfo {
                name: "Frequency".into(),
                min: Self::FREQ_MIN,
                max: Self::FREQ_MAX,
                default: Self::FREQ_DEFAULT,
                unit: "Hz".into(),
            },
            ParamInfo {
                name: "Q".into(),
                min: Self::Q_MIN,
                max: Self::Q_MAX,
                default: Self::Q_DEFAULT,
                unit: String::new(),
            },
            ParamInfo {
                name: "Gain".into(),
                min: Self::GAIN_DB_MIN,
                max: Self::GAIN_DB_MAX,
                default: Self::GAIN_DB_DEFAULT,
                unit: "dB".into(),
            },
        ]
    }
}

impl Processor for BiquadFilter {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        let c = &self.coeffs;

        for i in 0..len {
            let x = sanitize_sample(input[i]);
            // Direct Form II Transposed:
            //   y[n] = b0*x[n] + z1
            //   z1   = b1*x[n] - a1*y[n] + z2
            //   z2   = b2*x[n] - a2*y[n]
            let y = c.b0.mul_add(x, self.z1);
            self.z1 = c.b1.mul_add(x, (-c.a1).mul_add(y, self.z2));
            self.z2 = c.b2.mul_add(x, -c.a2 * y);

            output[i] = sanitize_sample(y);
        }

        // Flush denormals from state.
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    fn name(&self) -> &'static str {
        match self.filter_type {
            FilterType::LowPass => "Biquad LP Filter",
            FilterType::HighPass => "Biquad HP Filter",
            FilterType::BandPass => "Biquad BP Filter",
            FilterType::Notch => "Biquad Notch Filter",
            FilterType::Peak => "Biquad Peak Filter",
            FilterType::LowShelf => "Biquad Low Shelf Filter",
            FilterType::HighShelf => "Biquad High Shelf Filter",
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
            Self::PARAM_FREQUENCY => Some(self.frequency),
            Self::PARAM_Q => Some(self.q),
            Self::PARAM_GAIN_DB => Some(self.gain_db),
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
            Self::PARAM_Q => self.q = clamped,
            Self::PARAM_GAIN_DB => self.gain_db = clamped,
            _ => unreachable!(),
        }

        self.recalculate_coefficients();
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recalculate_coefficients();
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowpass_attenuates_high_frequencies() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_Q, 0.707).unwrap();

        // Generate white-ish noise using a simple deterministic PRNG.
        let len = 4096;
        let mut rng_state: u32 = 0xDEAD_BEEF;
        let input: Vec<f32> = (0..len)
            .map(|_| {
                // xorshift32
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 17;
                rng_state ^= rng_state << 5;
                (rng_state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        let mut output = vec![0.0_f32; len];
        filter.process(&input, &mut output);

        // Measure energy of second half (after transient settles).
        let half = len / 2;
        let input_energy: f32 = input[half..].iter().map(|s| s * s).sum();
        let output_energy: f32 = output[half..].iter().map(|s| s * s).sum();

        // With a 1 kHz LP on white noise at 44.1 kHz, output energy should be
        // substantially lower than input (most energy is above 1 kHz).
        assert!(
            output_energy < input_energy * 0.5,
            "LP filter should attenuate high frequencies: input_e={input_energy}, output_e={output_energy}"
        );
    }

    #[test]
    fn highpass_attenuates_low_frequencies() {
        let mut filter = BiquadFilter::new(FilterType::HighPass, 44100.0);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 5000.0)
            .unwrap();

        // Low-frequency sine at 100 Hz.
        let len = 4096;
        let input: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * 100.0 * i as f32 / 44100.0).sin())
            .collect();

        let mut output = vec![0.0_f32; len];
        filter.process(&input, &mut output);

        let half = len / 2;
        let input_energy: f32 = input[half..].iter().map(|s| s * s).sum();
        let output_energy: f32 = output[half..].iter().map(|s| s * s).sum();

        assert!(
            output_energy < input_energy * 0.1,
            "HP 5kHz should strongly attenuate 100 Hz sine: in={input_energy}, out={output_energy}"
        );
    }

    #[test]
    fn filter_handles_nan_input() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, 0.0];
        let mut output = [0.0_f32; 5];
        filter.process(&input, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s} is not finite");
        }
    }

    #[test]
    fn filter_reset_clears_state() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        let input = [1.0; 64];
        let mut output = [0.0_f32; 64];
        filter.process(&input, &mut output);

        filter.reset();

        // After reset, state should be zeroed -- processing silence should yield silence.
        let silence = [0.0_f32; 64];
        let mut out2 = [0.0_f32; 64];
        filter.process(&silence, &mut out2);

        for (i, &s) in out2.iter().enumerate() {
            assert!(
                s.abs() < 1e-10,
                "after reset, output[{i}] should be ~0, got {s}"
            );
        }
    }

    #[test]
    fn filter_param_count_and_info() {
        let filter = BiquadFilter::new(FilterType::Peak, 44100.0);
        assert_eq!(filter.param_count(), 3);
        assert!(filter.param_info(0).is_some());
        assert!(filter.param_info(1).is_some());
        assert!(filter.param_info(2).is_some());
        assert!(filter.param_info(3).is_none());
    }

    #[test]
    fn filter_set_param_clamps() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 50_000.0)
            .unwrap();
        assert!(
            (filter.param_value(BiquadFilter::PARAM_FREQUENCY).unwrap() - 20_000.0).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn filter_empty_buffers_no_panic() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        filter.process(&[], &mut []);
    }

    #[test]
    fn filter_sample_rate_change() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        filter.set_sample_rate(96000.0);
        // Should not panic and state should be reset.
        let input = [0.5; 16];
        let mut output = [0.0_f32; 16];
        filter.process(&input, &mut output);
        for &s in &output {
            assert!(s.is_finite());
        }
    }
}
