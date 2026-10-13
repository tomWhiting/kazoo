use std::sync::atomic::{AtomicU64, Ordering};

static STUCK_SHUTDOWNS: AtomicU64 = AtomicU64::new(0);

/// Process-wide count of link shutdown timeouts and undelivered completions.
#[must_use]
pub fn stuck_shutdowns() -> u64 {
    STUCK_SHUTDOWNS.load(Ordering::Relaxed)
}

pub(super) fn record_failure() {
    STUCK_SHUTDOWNS.fetch_add(1, Ordering::Relaxed);
}

/// The link thread's last act: tell the handle it has finished.
pub(super) fn report_finished(finished: &std::sync::mpsc::SyncSender<()>) {
    if finished.send(()).is_err() {
        record_failure();
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static LINK_CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
