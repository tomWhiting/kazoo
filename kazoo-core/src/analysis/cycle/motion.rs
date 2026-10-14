use super::{CENTS_PER_OCTAVE, CycleTracker, STEP_CENTS};

impl CycleTracker {
    pub(super) fn centred_windows(&self) -> bool {
        // Guitar and bass have bounded instrument ranges. Their partial
        // cycles need window-local DC rejection; the full-range voice and
        // line profiles keep the original measurement path.
        self.settings.max_hz <= 1400.0
    }

    pub(super) fn instrument_period(&self, period: f32) -> f32 {
        if self.trend_count < 2 {
            return period;
        }
        // NSDF readings describe the middle of two correlated windows.
        // Resample that measurement clock one period behind the callback,
        // rather than alternating held readings and glide extrapolation.
        let target = self.time - f64::from(period);
        for pair in self.trend[..self.trend_count].windows(2) {
            let [newer, older] = pair else {
                return period;
            };
            if target >= older.centre {
                let span = newer.centre - older.centre;
                let change = newer.log_period - older.log_period;
                if span <= 0.0 || change.abs() * CENTS_PER_OCTAVE > STEP_CENTS {
                    return period;
                }
                let measured = (change / span)
                    .mul_add(target - newer.centre, newer.log_period)
                    .exp2() as f32;
                return if (self.min_period..=self.max_period).contains(&measured) {
                    measured
                } else {
                    period
                };
            }
        }
        self.trend[self.trend_count - 1].log_period.exp2() as f32
    }
}
