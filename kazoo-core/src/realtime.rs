//! Real-time scheduling for the threads that carry audio between clocks.
//!
//! A device callback runs on a thread the operating system already treats
//! as real-time. The threads that carry audio between processes (the wall's
//! link to the desk, the desk's hub, a renderer on a timer) are ordinary
//! threads, and on a busy machine an ordinary thread can wait tens of
//! milliseconds for its turn, long enough for a block to reach the desk
//! after its time. [`RealTime::promote`] asks the operating system to
//! schedule the calling thread ahead of ordinary ones. The system calls live
//! in the `audio_thread_priority` crate, so this crate stays free of
//! `unsafe`. What the request means differs by platform:
//!
//! - macOS: the Mach time-constraint policy, with the period given, a
//!   computation budget of half of it and a deadline of all of it. A thread
//!   that overruns its budget is preempted, and one that keeps overrunning
//!   is put back among ordinary threads for a while by the kernel.
//! - Linux: `SCHED_FIFO` at priority 10, whatever the period, which needs
//!   `RLIMIT_RTPRIO` or `CAP_SYS_NICE`. The thread then outranks every
//!   ordinary thread, cpal's ALSA threads among them unless cpal promotes
//!   them too, until it sleeps.
//! - Windows: MMCSS's "Audio" task, whatever the period. It is reverted
//!   only by [`RealTime::demote`], never on drop.
//!
//! A thread that cannot be promoted carries on at its normal priority: it
//! still works, it is only more easily held up.

use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use audio_thread_priority::RtPriorityHandle;

/// The longest period [`RealTime::promote`] takes: a second.
///
/// The `audio_thread_priority` crate hands macOS the period in its clock's
/// ticks as a `u32`, which holds about four seconds on an Intel Mac, and
/// silently clamps longer ones; real-time work comes round far more often.
pub const LONGEST_PERIOD: Duration = Duration::from_secs(1);

/// Why the calling thread was left at its normal priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotRealTime(String);

impl fmt::Display for NotRealTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotRealTime {}

/// The calling thread's promotion to real-time scheduling.
///
/// It lasts until [`Self::demote`] or until the thread ends; dropping it
/// does not demote the thread (no backend reverts on drop), so a thread
/// keeps it on its own stack for as long as it means to stay promoted, and
/// hands it back before work that blocks or allocates.
///
/// It cannot leave the thread that promoted (it is neither `Send` nor
/// `Sync`): demoting names the thread it was made on, which another thread
/// could outlive.
///
/// ```compile_fail
/// use kazoo_core::realtime::RealTime;
/// fn send<T: Send>(_: T) {}
/// let promoted = RealTime::promote(std::time::Duration::from_millis(1)).unwrap();
/// send(promoted);
/// ```
pub struct RealTime {
    handle: RtPriorityHandle,
    /// Keeps it on its thread.
    on_this_thread: PhantomData<*const ()>,
}

impl fmt::Debug for RealTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealTime").finish_non_exhaustive()
    }
}

impl RealTime {
    /// Schedule the calling thread as real-time, for work that comes round
    /// every `period` and needs at most half of it. The period is counted
    /// in whole microseconds, and matters on macOS only (see the module
    /// documentation).
    ///
    /// # Errors
    ///
    /// [`NotRealTime`] if the period is under a microsecond or over
    /// [`LONGEST_PERIOD`], or the operating system refuses; the thread
    /// then keeps its normal priority.
    pub fn promote(period: Duration) -> Result<Self, NotRealTime> {
        let longest = LONGEST_PERIOD.as_micros();
        let micros = match u32::try_from(period.as_micros()) {
            Ok(micros) if micros >= 1 && u128::from(micros) <= longest => micros,
            Ok(_) | Err(_) => {
                return Err(NotRealTime(format!(
                    "{period:?} is not a period to schedule"
                )));
            }
        };
        // Frames at a million a second are microseconds.
        audio_thread_priority::promote_current_thread_to_real_time(micros, 1_000_000)
            .map(|handle| Self {
                handle,
                on_this_thread: PhantomData,
            })
            .map_err(|err| NotRealTime(err.to_string()))
    }

    /// Put the calling thread back to the scheduling it had before
    /// [`Self::promote`]. Must be called on the thread that promoted.
    ///
    /// # Errors
    ///
    /// [`NotRealTime`] if the operating system refuses; the thread may then
    /// still be real-time.
    pub fn demote(self) -> Result<(), NotRealTime> {
        audio_thread_priority::demote_current_thread_from_real_time(self.handle)
            .map_err(|err| NotRealTime(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_that_is_no_period_is_refused_before_the_system_is_asked() {
        let why = RealTime::promote(Duration::ZERO).unwrap_err().to_string();
        assert!(why.contains("not a period"), "{why}");
        assert!(RealTime::promote(Duration::from_nanos(999)).is_err());
        assert!(RealTime::promote(Duration::from_millis(1_001)).is_err());
        assert!(RealTime::promote(Duration::from_secs(3_601)).is_err());
    }

    #[test]
    fn a_thread_is_promoted_and_demoted() {
        let outcome = std::thread::spawn(|| {
            let promoted = RealTime::promote(Duration::from_micros(500))?;
            promoted.demote()
        })
        .join()
        .expect("the promoted thread ran to its end");
        // Any process may ask for the time-constraint policy on macOS;
        // elsewhere it depends on the process's limits, and a refusal must
        // say why.
        #[cfg(target_os = "macos")]
        assert_eq!(outcome, Ok(()));
        #[cfg(not(target_os = "macos"))]
        if let Err(why) = outcome {
            assert!(!why.to_string().is_empty());
        }
    }
}
