//! Process-wide counts of desk links that did not stop cleanly, kept here
//! because the [`super::HubLink`] that saw it is gone by the time anyone
//! could read its status.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

static STUCK_SHUTDOWNS: AtomicU64 = AtomicU64::new(0);
static UNDELIVERED_COMPLETIONS: AtomicU64 = AtomicU64::new(0);

/// How many desk link shutdowns in this process left the thread running.
///
/// A shutdown gives up after 500 ms. Each stuck link counts once, however
/// it ends.
#[must_use]
pub fn stuck_shutdowns() -> u64 {
    STUCK_SHUTDOWNS.load(Ordering::Relaxed)
}

/// How many desk link threads in this process finished with nobody to tell.
///
/// The handle had already been dropped, which happens only after a shutdown
/// gave up on the thread, so this counts stuck links that did end later,
/// not further stuck links.
#[must_use]
pub fn undelivered_completions() -> u64 {
    UNDELIVERED_COMPLETIONS.load(Ordering::Relaxed)
}

pub(super) fn record_stuck() {
    STUCK_SHUTDOWNS.fetch_add(1, Ordering::Relaxed);
}

/// The link thread's last act: tell the handle it has finished.
pub(super) fn report_finished(finished: &SyncSender<()>) {
    if finished.send(()).is_err() {
        UNDELIVERED_COMPLETIONS.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static LINK_CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
