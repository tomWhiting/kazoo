use super::{Engine, Ordering, Shared};
use kazoo_core::ipc::link::RequestError;

impl Engine {
    pub(super) fn ask_desk_tempo(&mut self, bpm: f64, seq: u64) {
        let answer = if bpm.is_finite() {
            self.hub
                .as_mut()
                .map_or(Err(RequestError::NotConnected), |hub| {
                    hub.request_tempo(bpm as f32)
                })
        } else {
            Err(RequestError::Invalid)
        };
        let status = match answer {
            Ok(()) => 1,
            Err(RequestError::Full) => 2,
            Err(RequestError::NotConnected) => 3,
            Err(RequestError::Invalid) => 4,
            Err(RequestError::Unsupported) => 5,
        };
        // A later go must not publish its result under the earlier go's number.
        self.shared.desk_tempo_seq.store(0, Ordering::Release);
        self.shared
            .desk_tempo_status
            .store(status, Ordering::Release);
        self.shared.desk_tempo_seq.store(seq, Ordering::Release);
    }
}

impl Shared {
    pub(crate) fn desk_tempo_reply(&self, seq: u64) -> Option<Result<(), RequestError>> {
        if self.desk_tempo_seq.load(Ordering::Acquire) != seq {
            return None;
        }
        let status = self.desk_tempo_status.load(Ordering::Acquire);
        if self.desk_tempo_seq.load(Ordering::Acquire) != seq {
            return None;
        }
        match status {
            1 => Some(Ok(())),
            2 => Some(Err(RequestError::Full)),
            3 => Some(Err(RequestError::NotConnected)),
            4 => Some(Err(RequestError::Invalid)),
            5 => Some(Err(RequestError::Unsupported)),
            _ => None,
        }
    }
}
