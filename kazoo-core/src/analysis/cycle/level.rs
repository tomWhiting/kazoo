use super::{CycleTracker, Peak, lag_from};

impl CycleTracker {
    pub(crate) const fn periodic_level(&self) -> f32 {
        self.periodic_level
    }

    pub(super) fn periodic_level_at(&self, peak: Peak) -> f32 {
        let Some(lag) = lag_from(f64::from(peak.period)) else {
            return 0.0;
        };
        let mut current = 0.0_f64;
        let mut previous = 0.0_f64;
        let mut current_sum = 0.0_f64;
        let mut previous_sum = 0.0_f64;
        let newest = self.position.wrapping_sub(1);
        for offset in 0..peak.window {
            let index = newest.wrapping_sub(offset);
            let x = f64::from(self.history[index & self.mask]);
            let y = f64::from(self.history[index.wrapping_sub(lag) & self.mask]);
            current = x.mul_add(x, current);
            previous = y.mul_add(y, previous);
            current_sum += x;
            previous_sum += y;
        }
        if self.centred_windows() && peak.window > 0 {
            let count = peak.window as f64;
            current = (-current_sum / count)
                .mul_add(current_sum, current)
                .max(0.0);
            previous = (-previous_sum / count)
                .mul_add(previous_sum, previous)
                .max(0.0);
        }
        // A transient can raise the peak follower while a quiet hum supplies
        // the pitch. Both correlated windows must carry the opening level.
        (current.min(previous) / peak.window.max(1) as f64).sqrt() as f32
    }
}
