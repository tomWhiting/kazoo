//! Topology-preserving state-variable low-pass, following Zavalishin ch. 4.

use crate::drive::kit::flush64;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Svf {
    state: [f64; 2],
    cutoff: f64,
    g: f64,
}

impl Svf {
    pub(crate) const fn new() -> Self {
        Self {
            state: [0.0; 2],
            cutoff: 0.0,
            g: 0.0,
        }
    }

    pub(crate) fn lowpass(&mut self, input: f64, cutoff: f64, resonance: f64, rate: f64) -> f64 {
        let cutoff = cutoff.clamp(1.0, rate * 0.45);
        if (cutoff - self.cutoff).abs() > self.cutoff * 0.001 {
            self.cutoff = cutoff;
            self.g = (PI * cutoff / rate).tan();
        }
        let k = 2.0 - 1.98 * resonance.clamp(0.0, 1.0);
        let a = 1.0 / (1.0 + self.g * (self.g + k));
        let v1 = a * (self.state[0] + self.g * (input - self.state[1]));
        let v2 = self.state[1] + self.g * v1;
        self.state = [2.0 * v1 - self.state[0], 2.0 * v2 - self.state[1]];
        for state in &mut self.state {
            flush64(state);
        }
        if v2.is_finite() {
            v2
        } else {
            self.state = [0.0; 2];
            0.0
        }
    }
}
