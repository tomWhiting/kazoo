use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct InputNotices {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    seen: Vec<String>,
    pending: Vec<String>,
}

impl InputNotices {
    pub fn report(&self, message: String) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.seen.contains(&message) {
            state.seen.push(message.clone());
            state.pending.push(message);
        }
    }

    #[must_use]
    pub fn take(&self) -> Option<String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.pending.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut state.pending).join("; "))
        }
    }
}
