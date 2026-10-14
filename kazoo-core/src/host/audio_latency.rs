use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug)]
pub(super) struct LatencyReport(AtomicU64);

impl Default for LatencyReport {
    fn default() -> Self {
        Self(AtomicU64::new(u64::MAX))
    }
}

impl LatencyReport {
    pub(super) fn record(&self, duration: Option<Duration>) {
        let nanos = duration.map_or(u64::MAX, |time| {
            u64::try_from(time.as_nanos()).unwrap_or(u64::MAX)
        });
        self.0.store(nanos, Ordering::Relaxed);
    }

    pub(super) fn milliseconds(&self) -> Option<f64> {
        let nanos = self.0.load(Ordering::Relaxed);
        (nanos != u64::MAX).then_some(nanos as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_latency_preserves_unknown_and_zero() {
        let report = LatencyReport::default();
        assert_eq!(report.milliseconds(), None);
        report.record(Some(Duration::from_millis(5)));
        assert_eq!(report.milliseconds(), Some(5.0));
        report.record(Some(Duration::ZERO));
        assert_eq!(report.milliseconds(), Some(0.0));
        report.record(None);
        assert_eq!(report.milliseconds(), None);
    }
}
