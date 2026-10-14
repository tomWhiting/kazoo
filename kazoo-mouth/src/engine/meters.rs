use std::sync::atomic::{AtomicU32, Ordering};

use kazoo_core::sanitize_sample;

#[derive(Debug, Clone, Copy, Default)]
pub struct Rms {
    energy: f64,
    count: usize,
}

impl Rms {
    pub fn stereo(&mut self, left: &[f32], right: &[f32], window: usize, output: &AtomicU32) {
        for (&left, &right) in left.iter().zip(right) {
            let left = f64::from(sanitize_sample(left));
            let right = f64::from(sanitize_sample(right));
            self.energy = 0.5_f64.mul_add(right.mul_add(right, left * left), self.energy);
            self.count += 1;
            if self.count >= window.max(1) {
                let rms = (self.energy / self.count as f64).sqrt() as f32;
                output.store(rms.to_bits(), Ordering::Relaxed);
                self.energy = 0.0;
                self.count = 0;
            }
        }
    }
}
