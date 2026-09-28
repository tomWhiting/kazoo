//! Joining background worker threads from `Drop`.

use std::thread::{self, JoinHandle};

/// Wait for a worker to finish while its owner is dropped.
///
/// A panic inside the worker is not swallowed: it is re-raised here, on the
/// owner's thread. If the owner is already unwinding from another panic, a
/// second panic would abort the process, so the worker's panic is reported
/// on stderr instead.
pub fn join_worker(name: &str, handle: JoinHandle<()>) {
    if let Err(payload) = handle.join() {
        if thread::panicking() {
            eprintln!("kazoo-mix: the {name} thread panicked during shutdown");
        } else {
            std::panic::resume_unwind(payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_worker_joins_quietly() {
        join_worker("test", thread::spawn(|| {}));
    }

    #[test]
    #[should_panic(expected = "worker failed")]
    fn a_worker_panic_reaches_the_owner() {
        join_worker("test", thread::spawn(|| panic!("worker failed")));
    }
}
