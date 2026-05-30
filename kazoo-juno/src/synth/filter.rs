//! Simple procedural filters for the Juno signal path.

use std::f32::consts::TAU;

#[derive(Debug, Clone)]
pub struct DcBlockHighPass {
    sample_rate: f32,
    prev_input: f32,
    prev_output: f32,
}

impl DcBlockHighPass {
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            prev_input: 0.0,
            prev_output: 0.0,
        }
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub const fn reset(&mut self) {
        self.prev_input = 0.0;
        self.prev_output = 0.0;
    }

    pub fn process(&mut self, input: f32, amount: f32) -> f32 {
        let cutoff = amount.clamp(0.0, 1.0).powi(2).mul_add(900.0, 20.0);
        let rc = 1.0 / (TAU * cutoff);
        let dt = 1.0 / self.sample_rate.max(1.0);
        let alpha = rc / (rc + dt);
        let output = alpha * (self.prev_output + input - self.prev_input);
        self.prev_input = input;
        self.prev_output = output;
        output
    }
}

/// Two-pole state-variable low-pass filter.
#[derive(Debug, Clone)]
pub struct LowPassFilter {
    sample_rate: f32,
    ic1eq: f32,
    ic2eq: f32,
}

impl LowPassFilter {
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            ic1eq: 0.0,
            ic2eq: 0.0,
        }
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub const fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    pub fn process(&mut self, input: f32, cutoff_hz: f32, resonance: f32) -> f32 {
        let nyquist = self.sample_rate * 0.48;
        let cutoff = cutoff_hz.clamp(20.0, nyquist.max(20.0));
        let g = (std::f32::consts::PI * cutoff / self.sample_rate.max(1.0)).tan();
        let k = resonance.clamp(0.0, 0.96).mul_add(-1.85, 2.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
        self.ic1eq = 2.0_f32.mul_add(v1, -self.ic1eq);
        self.ic2eq = 2.0_f32.mul_add(v2, -self.ic2eq);
        v2.tanh()
    }
}
