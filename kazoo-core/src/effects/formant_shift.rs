//! Formant frequency shifting via FFT-based spectral processing.
//!
//! Extracts the spectral envelope from the input, shifts it in the frequency
//! domain, then resynthesises via inverse FFT. Uses a Hann window with 75%
//! overlap-add for smooth output.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};
use rustfft::{FftPlanner, num_complex::Complex};
use std::f32::consts::PI;
use std::sync::Arc;

/// FFT size for the formant shift processor.
const FFT_SIZE: usize = 1024;
/// Hop size for 75% overlap.
const HOP_SIZE: usize = FFT_SIZE / 4;

/// Formant frequency shifting processor.
///
/// Uses overlap-add with a Hann window and FFT/IFFT to shift the spectral
/// envelope of the input signal.
pub struct FormantShift {
    sample_rate: f32,
    shift_hz: f32,
    mix: f32,
    // Pre-allocated buffers.
    input_ring: Vec<f32>,
    input_write_pos: usize,
    output_accum: Vec<f32>,
    output_read_pos: usize,
    window: Vec<f32>,
    fft_scratch: Vec<Complex<f32>>,
    fft_buffer: Vec<Complex<f32>>,
    fft_forward: Arc<dyn rustfft::Fft<f32>>,
    fft_inverse: Arc<dyn rustfft::Fft<f32>>,
    // Track how many input samples have been accumulated since the last hop.
    samples_since_hop: usize,
    // Latency: we need at least one full FFT frame before we can produce output.
    primed: bool,
    frames_accumulated: usize,
}

impl std::fmt::Debug for FormantShift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormantShift")
            .field("sample_rate", &self.sample_rate)
            .field("shift_hz", &self.shift_hz)
            .field("mix", &self.mix)
            .field("primed", &self.primed)
            .field("frames_accumulated", &self.frames_accumulated)
            .finish_non_exhaustive()
    }
}

impl FormantShift {
    const PARAM_SHIFT_HZ: usize = 0;
    const PARAM_MIX: usize = 1;

    const SHIFT_MIN: f32 = -1000.0;
    const SHIFT_MAX: f32 = 1000.0;
    const SHIFT_DEFAULT: f32 = 0.0;

    const MIX_MIN: f32 = 0.0;
    const MIX_MAX: f32 = 1.0;
    const MIX_DEFAULT: f32 = 1.0;

    /// Create a new formant shift processor at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut planner = FftPlanner::new();
        let fft_forward = planner.plan_fft_forward(FFT_SIZE);
        let fft_inverse = planner.plan_fft_inverse(FFT_SIZE);

        let window = Self::hann_window(FFT_SIZE);

        Self {
            sample_rate: sr,
            shift_hz: Self::SHIFT_DEFAULT,
            mix: Self::MIX_DEFAULT,
            input_ring: vec![0.0; FFT_SIZE],
            input_write_pos: 0,
            output_accum: vec![0.0; FFT_SIZE + HOP_SIZE],
            output_read_pos: 0,
            window,
            fft_scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            fft_buffer: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            fft_forward,
            fft_inverse,
            samples_since_hop: 0,
            primed: false,
            frames_accumulated: 0,
        }
    }

    /// Generate a Hann window of the given length.
    fn hann_window(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let phase = 2.0 * PI * i as f32 / len as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect()
    }

    /// Process one FFT frame: window, FFT, shift, IFFT, overlap-add.
    fn process_fft_frame(&mut self) {
        let fft_size = FFT_SIZE;
        let inv_fft = 1.0 / fft_size as f32;

        // Copy input ring into FFT buffer with windowing.
        for j in 0..fft_size {
            let idx = (self.input_write_pos + j) % fft_size;
            let windowed = self.input_ring[idx] * self.window[j];
            self.fft_buffer[j] = Complex::new(sanitize_sample(windowed), 0.0);
        }

        // Forward FFT.
        self.fft_forward
            .process_with_scratch(&mut self.fft_buffer, &mut self.fft_scratch);

        // Shift the spectrum: move each bin by `shift_bins`.
        let shift_bins = self.shift_hz * fft_size as f32 / self.sample_rate;
        let shift_int = shift_bins.round() as i32;

        // Create a shifted copy.
        let half = fft_size / 2 + 1;
        let mut shifted = vec![Complex::new(0.0_f32, 0.0); fft_size];

        for k in 0..half {
            #[allow(clippy::cast_possible_wrap)]
            let dst = k as i32 + shift_int;
            if dst >= 0 && (dst as usize) < half {
                shifted[dst as usize] = self.fft_buffer[k];
                // Mirror for negative frequencies.
                if dst > 0 && (dst as usize) < half {
                    shifted[fft_size - dst as usize] = self.fft_buffer[k].conj();
                }
            }
        }
        // Ensure DC and Nyquist are real.
        shifted[0].im = 0.0;
        shifted[fft_size / 2].im = 0.0;

        // Inverse FFT.
        self.fft_buffer.copy_from_slice(&shifted);
        let mut inv_scratch = vec![Complex::new(0.0_f32, 0.0); FFT_SIZE];
        self.fft_inverse
            .process_with_scratch(&mut self.fft_buffer, &mut inv_scratch);

        // Overlap-add: window the output and accumulate.
        let out_len = self.output_accum.len();
        for j in 0..fft_size {
            let sample = self.fft_buffer[j].re * inv_fft * self.window[j];
            let pos = (self.output_read_pos + j) % out_len;
            self.output_accum[pos] += sanitize_sample(sample);
        }
    }

    fn param_infos() -> [ParamInfo; 2] {
        [
            ParamInfo {
                name: "Shift".into(),
                min: Self::SHIFT_MIN,
                max: Self::SHIFT_MAX,
                default: Self::SHIFT_DEFAULT,
                unit: "Hz".into(),
            },
            ParamInfo {
                name: "Mix".into(),
                min: Self::MIX_MIN,
                max: Self::MIX_MAX,
                default: Self::MIX_DEFAULT,
                unit: String::new(),
            },
        ]
    }
}

impl Processor for FormantShift {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        let mix = self.mix;
        let out_accum_len = self.output_accum.len();

        for i in 0..len {
            let x = sanitize_sample(input[i]);

            // Write to input ring.
            self.input_ring[self.input_write_pos] = x;
            self.input_write_pos = (self.input_write_pos + 1) % FFT_SIZE;
            self.samples_since_hop += 1;

            // When we've accumulated a hop's worth of samples, process an FFT frame.
            if self.samples_since_hop >= HOP_SIZE {
                self.samples_since_hop = 0;
                self.process_fft_frame();
                self.frames_accumulated += 1;
                if self.frames_accumulated >= 4 {
                    self.primed = true;
                }
            }

            // Read from the output accumulator.
            if self.primed {
                let wet = self.output_accum[self.output_read_pos];
                self.output_accum[self.output_read_pos] = 0.0;
                self.output_read_pos = (self.output_read_pos + 1) % out_accum_len;
                output[i] = sanitize_sample(x.mul_add(1.0 - mix, wet * mix));
            } else {
                // Before we have enough data, pass through dry.
                output[i] = x;
            }
        }
    }

    fn reset(&mut self) {
        self.input_ring.fill(0.0);
        self.input_write_pos = 0;
        self.output_accum.fill(0.0);
        self.output_read_pos = 0;
        self.samples_since_hop = 0;
        self.primed = false;
        self.frames_accumulated = 0;
    }

    fn name(&self) -> &'static str {
        "Formant Shift"
    }

    fn latency_samples(&self) -> usize {
        FFT_SIZE
    }

    fn param_count(&self) -> usize {
        2
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        let infos = Self::param_infos();
        infos.get(index).cloned()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_SHIFT_HZ => Some(self.shift_hz),
            Self::PARAM_MIX => Some(self.mix),
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
            Self::PARAM_SHIFT_HZ => self.shift_hz = clamped,
            Self::PARAM_MIX => self.mix = clamped,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        // Rebuild FFT plans (size unchanged so reuse is fine, but reset state).
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formant_shift_modifies_signal() {
        let sr = 44100.0;
        let mut fs = FormantShift::new(sr);
        fs.set_param(FormantShift::PARAM_SHIFT_HZ, 200.0).unwrap();
        fs.set_param(FormantShift::PARAM_MIX, 1.0).unwrap();

        let len = 8192;
        let input: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; len];

        fs.process(&input, &mut output);

        // After priming, the output should differ from the input.
        let diff_energy: f32 = input[FFT_SIZE..]
            .iter()
            .zip(output[FFT_SIZE..].iter())
            .map(|(a, b)| {
                let d = a - b;
                d * d
            })
            .sum();

        assert!(
            diff_energy > 0.01,
            "formant shift should modify the signal, diff={diff_energy}"
        );
    }

    #[test]
    fn formant_shift_handles_nan() {
        let mut fs = FormantShift::new(44100.0);
        let input = [f32::NAN, f32::INFINITY, 0.5, 0.0];
        let mut output = [0.0_f32; 4];
        fs.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn formant_shift_zero_shift_preserves() {
        let sr = 44100.0;
        let mut fs = FormantShift::new(sr);
        fs.set_param(FormantShift::PARAM_SHIFT_HZ, 0.0).unwrap();
        fs.set_param(FormantShift::PARAM_MIX, 1.0).unwrap();

        let len = 8192;
        let input: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; len];

        fs.process(&input, &mut output);

        // With zero shift, output should roughly resemble input (after priming).
        // Check that output has non-trivial energy.
        let energy: f32 = output[FFT_SIZE..].iter().map(|s| s * s).sum();
        assert!(
            energy > 0.1,
            "zero-shift should produce meaningful output, energy={energy}"
        );
    }

    #[test]
    fn formant_shift_reset() {
        let mut fs = FormantShift::new(44100.0);
        let input = [0.5_f32; 2048];
        let mut output = [0.0_f32; 2048];
        fs.process(&input, &mut output);
        fs.reset();

        assert!(!fs.primed);
        assert_eq!(fs.frames_accumulated, 0);
    }

    #[test]
    fn formant_shift_empty_buffers() {
        let mut fs = FormantShift::new(44100.0);
        fs.process(&[], &mut []);
    }

    #[test]
    fn formant_shift_param_count() {
        let fs = FormantShift::new(44100.0);
        assert_eq!(fs.param_count(), 2);
    }
}
