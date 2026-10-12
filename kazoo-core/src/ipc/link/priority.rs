use std::sync::atomic::Ordering;

use super::{HubLink, LinkState, NotRealTime, RealTime};

pub(super) const NOT_ASKED: u8 = 0;
const GRANTED: u8 = 1;
const REFUSED: u8 = 2;
static GRANTED_RESULT: Result<(), NotRealTime> = Ok(());

pub(super) fn outcome(state: &LinkState) -> Option<&Result<(), NotRealTime>> {
    match state.real_time.load(Ordering::Acquire) {
        GRANTED => Some(&GRANTED_RESULT),
        REFUSED => state.real_time_refusal.get(),
        _ => None,
    }
}

pub(super) fn promote(
    state: &LinkState,
    request: impl FnOnce() -> Result<RealTime, NotRealTime>,
) -> Option<RealTime> {
    match request() {
        Ok(priority) => {
            state.real_time.store(GRANTED, Ordering::Release);
            Some(priority)
        }
        Err(why) => {
            state.real_time_refusal.get_or_init(|| Err(why));
            state.real_time.store(REFUSED, Ordering::Release);
            None
        }
    }
}

impl HubLink {
    pub(crate) fn report_start_refusal(&self, reason: String) {
        match self.state.last_refusal.lock() {
            Ok(mut refusal) => *refusal = Some(reason),
            Err(poisoned) => *poisoned.into_inner() = Some(reason),
        }
    }
}
