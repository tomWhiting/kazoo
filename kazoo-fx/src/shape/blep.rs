//! Two-sample polynomial residuals for an edge between two sample instants.

#[derive(Debug, Clone, Copy)]
pub struct EdgeWave {
    high: bool,
    pending: f64,
}

impl EdgeWave {
    pub(crate) const fn new() -> Self {
        Self {
            high: false,
            pending: 0.0,
        }
    }

    /// Return the preceding sample, completing its correction with the new
    /// edge. The next correction stays pending until the following sample.
    pub(crate) fn step(&mut self, high: bool, fraction: f64, band_limited: bool) -> f64 {
        let mut previous = if self.high { 1.0 } else { -1.0 };
        previous += self.pending;
        self.pending = 0.0;
        if high != self.high && band_limited {
            let direction = if high { 1.0 } else { -1.0 };
            let before = 1.0 - fraction.clamp(0.0, 1.0);
            previous = (direction * before).mul_add(before, previous);
            self.pending = -direction * fraction * fraction;
        }
        self.high = high;
        previous
    }
}
