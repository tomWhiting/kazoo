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

    // -----------------------------------------------------------------------
    // All filter types: response curve validation
    // -----------------------------------------------------------------------

    /// Measure energy ratio (output/input) over the second half of a block.
    fn energy_ratio(filter: &mut BiquadFilter, input: &[f32]) -> f32 {
        let mut output = vec![0.0_f32; input.len()];
        filter.process(input, &mut output);
        let half = input.len() / 2;
        let in_energy: f32 = input[half..].iter().map(|s| s * s).sum();
        let out_energy: f32 = output[half..].iter().map(|s| s * s).sum();
        if in_energy > 0.0 {
            out_energy / in_energy
        } else {
            0.0
        }
    }

    /// Generate a sine wave at the given frequency.
    fn sine_wave(freq_hz: f32, sample_rate: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq_hz * i as f32 / sample_rate).sin())
            .collect()
    }

    /// Generate white noise using a deterministic PRNG.
    fn white_noise(len: usize) -> Vec<f32> {
        let mut rng: u32 = 0xDEAD_BEEF;
        (0..len)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn bandpass_passes_center_attenuates_edges() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::BandPass, sr);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_Q, 5.0).unwrap();

        // 1 kHz sine (at center) should pass through well.
        let center = sine_wave(1000.0, sr, 4096);
        let center_ratio = energy_ratio(&mut filter, &center);
        filter.reset();

        // 100 Hz sine (far below) should be attenuated.
        let low = sine_wave(100.0, sr, 4096);
        let low_ratio = energy_ratio(&mut filter, &low);
        filter.reset();

        // 10 kHz sine (far above) should be attenuated.
        let high = sine_wave(10000.0, sr, 4096);
        let high_ratio = energy_ratio(&mut filter, &high);

        assert!(
            center_ratio > low_ratio * 5.0,
            "BP: center ({center_ratio}) should pass much more than low ({low_ratio})"
        );
        assert!(
            center_ratio > high_ratio * 5.0,
            "BP: center ({center_ratio}) should pass much more than high ({high_ratio})"
        );
    }

    #[test]
    fn notch_rejects_center_passes_edges() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::Notch, sr);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_Q, 5.0).unwrap();

        // 1 kHz should be strongly attenuated.
        let center = sine_wave(1000.0, sr, 4096);
        let center_ratio = energy_ratio(&mut filter, &center);
        filter.reset();

        // 100 Hz should pass through.
        let low = sine_wave(100.0, sr, 4096);
        let low_ratio = energy_ratio(&mut filter, &low);
        filter.reset();

        // 10 kHz should pass through.
        let high = sine_wave(10000.0, sr, 4096);
        let high_ratio = energy_ratio(&mut filter, &high);

        assert!(
            center_ratio < 0.1,
            "Notch: center frequency should be strongly rejected ({center_ratio})"
        );
        assert!(
            low_ratio > 0.5,
            "Notch: low frequency should pass ({low_ratio})"
        );
        assert!(
            high_ratio > 0.5,
            "Notch: high frequency should pass ({high_ratio})"
        );
    }

    #[test]
    fn peak_boosts_center_frequency() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::Peak, sr);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_Q, 2.0).unwrap();
        filter.set_param(BiquadFilter::PARAM_GAIN_DB, 12.0).unwrap();

        // 1 kHz sine should be boosted.
        let center = sine_wave(1000.0, sr, 4096);
        let center_ratio = energy_ratio(&mut filter, &center);
        filter.reset();

        // 100 Hz should be mostly unchanged.
        let low = sine_wave(100.0, sr, 4096);
        let low_ratio = energy_ratio(&mut filter, &low);

        // +12 dB boost = ~16x power at center.
        assert!(
            center_ratio > 4.0,
            "Peak +12dB: center should be significantly boosted ({center_ratio})"
        );
        assert!(
            low_ratio < center_ratio * 0.5,
            "Peak: off-center should be boosted less ({low_ratio} vs {center_ratio})"
        );
    }

    #[test]
    fn peak_cuts_center_frequency() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::Peak, sr);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_Q, 2.0).unwrap();
        filter
            .set_param(BiquadFilter::PARAM_GAIN_DB, -12.0)
            .unwrap();

        let center = sine_wave(1000.0, sr, 4096);
        let center_ratio = energy_ratio(&mut filter, &center);

        // -12 dB cut = ~1/16 power at center.
        assert!(
            center_ratio < 0.25,
            "Peak -12dB: center should be strongly cut ({center_ratio})"
        );
    }

    #[test]
    fn lowshelf_boosts_below_frequency() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::LowShelf, sr);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_GAIN_DB, 12.0).unwrap();

        // 100 Hz (below shelf) should be boosted.
        let low = sine_wave(100.0, sr, 4096);
        let low_ratio = energy_ratio(&mut filter, &low);
        filter.reset();

        // 10 kHz (above shelf) should be ~unity.
        let high = sine_wave(10000.0, sr, 4096);
        let high_ratio = energy_ratio(&mut filter, &high);

        assert!(
            low_ratio > 4.0,
            "LowShelf +12dB: bass should be boosted ({low_ratio})"
        );
        assert!(
            high_ratio < low_ratio * 0.5,
            "LowShelf: treble ({high_ratio}) should be boosted less than bass ({low_ratio})"
        );
    }

    #[test]
    fn highshelf_boosts_above_frequency() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::HighShelf, sr);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 1000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_GAIN_DB, 12.0).unwrap();

        // 10 kHz (above shelf) should be boosted.
        let high = sine_wave(10000.0, sr, 4096);
        let high_ratio = energy_ratio(&mut filter, &high);
        filter.reset();

        // 100 Hz (below shelf) should be ~unity.
        let low = sine_wave(100.0, sr, 4096);
        let low_ratio = energy_ratio(&mut filter, &low);

        assert!(
            high_ratio > 4.0,
            "HighShelf +12dB: treble should be boosted ({high_ratio})"
        );
        assert!(
            low_ratio < high_ratio * 0.5,
            "HighShelf: bass ({low_ratio}) should be boosted less than treble ({high_ratio})"
        );
    }

    // -----------------------------------------------------------------------
    // Edge cases and stability
    // -----------------------------------------------------------------------

    #[test]
    fn all_filter_types_handle_nan_input() {
        let bad_input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];

        for ft in [
            FilterType::LowPass,
            FilterType::HighPass,
            FilterType::BandPass,
            FilterType::Notch,
            FilterType::Peak,
            FilterType::LowShelf,
            FilterType::HighShelf,
        ] {
            let mut filter = BiquadFilter::new(ft, 44100.0);
            let mut output = [0.0_f32; 5];
            filter.process(&bad_input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(s.is_finite(), "{:?}: output[{i}] = {s} is not finite", ft);
            }
        }
    }

    #[test]
    fn all_filter_types_stable_with_noise() {
        let noise = white_noise(8192);

        for ft in [
            FilterType::LowPass,
            FilterType::HighPass,
            FilterType::BandPass,
            FilterType::Notch,
            FilterType::Peak,
            FilterType::LowShelf,
            FilterType::HighShelf,
        ] {
            let mut filter = BiquadFilter::new(ft, 44100.0);
            let mut output = vec![0.0_f32; 8192];
            filter.process(&noise, &mut output);

            // No sample should explode (stability check).
            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() < 100.0,
                    "{ft:?}: output[{i}] = {s} is unstable",
                );
            }
        }
    }

    #[test]
    fn frequency_at_nyquist_boundary() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::LowPass, sr);

        // Set frequency to Nyquist (22050) — should be clamped below.
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 22050.0)
            .unwrap();

        let input = white_noise(1024);
        let mut output = vec![0.0_f32; 1024];
        filter.process(&input, &mut output);

        // Should not produce NaN/Inf even at Nyquist boundary.
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn q_at_extremes() {
        let sr = 44100.0;

        // Q at minimum (0.1) — very broad filter.
        let mut filter = BiquadFilter::new(FilterType::BandPass, sr);
        filter.set_param(BiquadFilter::PARAM_Q, 0.1).unwrap();
        let noise = white_noise(2048);
        let mut output = vec![0.0_f32; 2048];
        filter.process(&noise, &mut output);
        for &s in &output {
            assert!(s.is_finite(), "Q=0.1: output is not finite");
        }

        // Q at maximum (30.0) — very narrow filter.
        filter.reset();
        filter.set_param(BiquadFilter::PARAM_Q, 30.0).unwrap();
        filter.process(&noise, &mut output);
        for &s in &output {
            assert!(s.is_finite(), "Q=30.0: output is not finite");
        }
    }

    #[test]
    fn denormal_flushing_after_silence() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);

        // Process a loud signal to set state.
        let loud = [1.0_f32; 64];
        let mut output = [0.0_f32; 64];
        filter.process(&loud, &mut output);

        // Process silence — state should converge to zero (denormals flushed).
        for _ in 0..100 {
            let silence = [0.0_f32; 64];
            filter.process(&silence, &mut output);
        }

        // After many blocks of silence, state should be flushed.
        // The denormal threshold is 1e-30.
        assert!(
            filter.z1.abs() < 1e-20,
            "z1 should be flushed: {}",
            filter.z1
        );
        assert!(
            filter.z2.abs() < 1e-20,
            "z2 should be flushed: {}",
            filter.z2
        );
    }

    #[test]
    fn zero_gain_peak_is_unity() {
        let sr = 44100.0;
        let mut filter = BiquadFilter::new(FilterType::Peak, sr);
        filter.set_param(BiquadFilter::PARAM_GAIN_DB, 0.0).unwrap();

        let input = sine_wave(1000.0, sr, 4096);
        let ratio = energy_ratio(&mut filter, &input);

        // 0 dB gain should be ~unity (ratio ≈ 1.0).
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "Peak at 0dB should be unity: ratio={ratio}"
        );
    }

    #[test]
    fn low_sample_rate_stability() {
        // 8 kHz: Nyquist is 4 kHz, default frequency 1000 Hz is valid.
        let mut filter = BiquadFilter::new(FilterType::LowPass, 8000.0);
        let noise = white_noise(1024);
        let mut output = vec![0.0_f32; 1024];
        filter.process(&noise, &mut output);

        for &s in &output {
            assert!(s.is_finite(), "8 kHz LP output is not finite");
        }
    }

    #[test]
    fn very_low_sample_rate_clamps_frequency() {
        // At sample rate 100 Hz, Nyquist is 50 Hz, which is below FREQ_MIN (20 Hz).
        // Frequency gets clamped to (Nyquist-1).max(FREQ_MIN) = max(49, 20) = 49 Hz.
        let mut filter = BiquadFilter::new(FilterType::LowPass, 100.0);
        let input = [0.5_f32; 64];
        let mut output = [0.0_f32; 64];
        filter.process(&input, &mut output);

        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn all_filter_types_name_not_empty() {
        for ft in [
            FilterType::LowPass,
            FilterType::HighPass,
            FilterType::BandPass,
            FilterType::Notch,
            FilterType::Peak,
            FilterType::LowShelf,
            FilterType::HighShelf,
        ] {
            let filter = BiquadFilter::new(ft, 44100.0);
            assert!(!filter.name().is_empty(), "{ft:?} has empty name");
        }
    }

    #[test]
    fn param_value_returns_correct_values() {
        let mut filter = BiquadFilter::new(FilterType::Peak, 44100.0);
        filter
            .set_param(BiquadFilter::PARAM_FREQUENCY, 5000.0)
            .unwrap();
        filter.set_param(BiquadFilter::PARAM_Q, 2.5).unwrap();
        filter.set_param(BiquadFilter::PARAM_GAIN_DB, -6.0).unwrap();

        assert!((filter.param_value(0).unwrap() - 5000.0).abs() < f32::EPSILON);
        assert!((filter.param_value(1).unwrap() - 2.5).abs() < f32::EPSILON);
        assert!((filter.param_value(2).unwrap() - (-6.0)).abs() < f32::EPSILON);
        assert!(filter.param_value(3).is_none());
    }

    #[test]
    fn set_param_invalid_index_returns_error() {
        let mut filter = BiquadFilter::new(FilterType::LowPass, 44100.0);
        assert!(filter.set_param(99, 0.0).is_err());
    }
}
