use std::sync::atomic::Ordering;

use std::time::Duration;

use super::{Engine, Shared};

impl Engine {
    pub(super) fn measure_fixed(&mut self, elapsed: Duration, lasted: f32, weight: f32) {
        // The module loop is measured with the same CPU clock as the whole
        // callback, leaving its surrounding work counted once per wall.
        let fixed = elapsed.saturating_sub(self.module_cpu).as_secs_f32() / lasted;
        let averaged = weight.mul_add(fixed - self.fixed_load, self.fixed_load);
        if averaged.is_finite() {
            self.fixed_load = averaged;
            self.shared
                .fixed
                .store(averaged.to_bits(), Ordering::Relaxed);
        }
    }
}

impl Shared {
    pub(crate) fn fixed_load(&self) -> f32 {
        let fixed = f32::from_bits(self.fixed.load(Ordering::Relaxed));
        if fixed.is_finite() {
            fixed.max(0.0)
        } else {
            0.0
        }
    }

    #[cfg(test)]
    pub(crate) fn set_fixed_load(&self, fixed: f32) {
        self.fixed.store(fixed.to_bits(), Ordering::Relaxed);
    }
}
