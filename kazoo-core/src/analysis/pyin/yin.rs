//! The YIN half of pYIN: each frame's cumulative mean normalised difference
//! over the periods searched, and the parabolic refinement of each period.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{Grid, Settings, refuse};
use crate::Result;

/// Autocorrelation and energy terms under this magnitude are taken as zero.
const FLOOR: f64 = 1e-6;

/// The FFT plans and buffers one frame's difference function needs.
pub struct Yin {
    frame_length: usize,
    win_length: usize,
    forward: Arc<dyn RealToComplex<f64>>,
    inverse: Arc<dyn ComplexToReal<f64>>,
    frame: Vec<f64>,
    reversed: Vec<f64>,
    frame_fft: Vec<Complex<f64>>,
    reversed_fft: Vec<Complex<f64>>,
    acf: Vec<f64>,
    forward_scratch: Vec<Complex<f64>>,
    inverse_scratch: Vec<Complex<f64>>,
    energy: Vec<f64>,
    difference: Vec<f64>,
    mean: Vec<f64>,
}

impl Yin {
    pub fn new(settings: &Settings, grid: &Grid) -> Self {
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(settings.frame_length);
        let inverse = planner.plan_fft_inverse(settings.frame_length);
        let lags = settings.frame_length - settings.win_length;
        Self {
            frame_length: settings.frame_length,
            win_length: settings.win_length,
            frame: forward.make_input_vec(),
            reversed: forward.make_input_vec(),
            frame_fft: forward.make_output_vec(),
            reversed_fft: forward.make_output_vec(),
            acf: inverse.make_output_vec(),
            forward_scratch: forward.make_scratch_vec(),
            inverse_scratch: inverse.make_scratch_vec(),
            energy: vec![0.0; settings.frame_length],
            difference: vec![0.0; lags],
            mean: vec![0.0; grid.max_period],
            forward,
            inverse,
        }
    }

    /// The cumulative mean normalised difference of one frame (equation 8 of
    /// de Cheveigné and Kawahara's YIN) at each period from the grid's
    /// shortest to its longest, into `out`.
    pub fn difference(&mut self, frame: &[f64], grid: &Grid, out: &mut [f64]) -> Result<()> {
        let (length, window) = (self.frame_length, self.win_length);

        // Autocorrelation at each lag, by FFT: the frame against its first
        // window reversed (samples window down to 1), zero-padded.
        self.frame.copy_from_slice(frame);
        for (slot, &x) in self.reversed.iter_mut().zip(frame[1..=window].iter().rev()) {
            *slot = x;
        }
        self.reversed[window..].fill(0.0);
        self.forward
            .process_with_scratch(
                &mut self.frame,
                &mut self.frame_fft,
                &mut self.forward_scratch,
            )
            .or_else(|error| refuse(format!("pyin's forward FFT: {error}")))?;
        self.forward
            .process_with_scratch(
                &mut self.reversed,
                &mut self.reversed_fft,
                &mut self.forward_scratch,
            )
            .or_else(|error| refuse(format!("pyin's forward FFT: {error}")))?;
        for (x, &y) in self.frame_fft.iter_mut().zip(&self.reversed_fft) {
            *x *= y;
        }
        self.inverse
            .process_with_scratch(
                &mut self.frame_fft,
                &mut self.acf,
                &mut self.inverse_scratch,
            )
            .or_else(|error| refuse(format!("pyin's inverse FFT: {error}")))?;
        // Scaled as numpy's irfft is.
        let scale = length as f64;

        // Energy in the window at each lag, from the running sum of squares.
        let mut sum = 0.0;
        for (slot, &x) in self.energy.iter_mut().zip(frame) {
            sum += x.powi(2);
            *slot = sum;
        }
        let first = floored(self.energy[window] - self.energy[0]);

        // The difference function at each lag.
        for (lag, slot) in self.difference.iter_mut().enumerate() {
            let acf = floored(self.acf[window + lag] / scale);
            let energy = floored(self.energy[window + lag] - self.energy[lag]);
            // Kept apart, not fused, to round as the crate did.
            let twice = acf * 2.0;
            *slot = first + energy - twice;
        }

        // Normalised by its running mean from lag 1.
        let mut running = 0.0;
        for (lag, slot) in self.mean.iter_mut().enumerate() {
            running += self.difference[lag + 1];
            *slot = running / (lag + 1) as f64;
        }
        for (index, slot) in out.iter_mut().enumerate() {
            let period = grid.min_period + index;
            *slot = self.difference[period] / (self.mean[period - 1] + f64::MIN_POSITIVE);
        }
        Ok(())
    }
}

const fn floored(x: f64) -> f64 {
    if x.abs() >= FLOOR { x } else { 0.0 }
}

/// The offset of each period's parabola vertex, within a sample either way,
/// else zero; the first and last periods have no neighbours and stay zero.
pub fn parabolic_shifts(yin: &[f64], out: &mut [f64]) {
    out.fill(0.0);
    for (index, window) in yin.windows(3).enumerate() {
        let (before, here, after) = (window[0], window[1], window[2]);
        let doubled = here * 2.0;
        let a = (before + after - doubled) / 2.0;
        let b = (after - before) / 2.0;
        let width = a * 2.0;
        let shift = -b / (width + f64::MIN_POSITIVE);
        out[index + 1] = if shift.abs() <= 1.0 { shift } else { 0.0 };
    }
}
