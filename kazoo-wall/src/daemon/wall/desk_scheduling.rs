use super::{Change, Event, Wall, What};
use kazoo_core::ipc::link::RequestError;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct Pending {
    seq: u64,
    bpm: f64,
    due: Instant,
}

fn last_tempo(what: &What) -> Option<(f64, bool, f64)> {
    match what {
        What::Tempo {
            to,
            desk,
            after_beats,
            ..
        } => Some((*to, *desk, *after_beats)),
        What::Batch { changes, .. } | What::Chain { changes, .. } => {
            changes.iter().rev().find_map(last_tempo)
        }
        _ => None,
    }
}

/// Why the desk did not take a tempo, in words.
const fn refusal_words(error: &RequestError) -> &'static str {
    match error {
        RequestError::Full => "it had too many requests waiting",
        RequestError::NotConnected => "it had gone away (the wall keeps its own tempo off it)",
        RequestError::Invalid => "it refused that tempo",
        RequestError::Unsupported => "it is too old to take a tempo from the wall",
    }
}

impl Wall {
    pub(super) fn cancel_pending_desk_tempo(&mut self, change: &Change) {
        if last_tempo(&change.what).is_some()
            || matches!(change.what, What::Load { .. } | What::Go { .. })
            || self
                .pending_desk_tempo
                .as_ref()
                .is_some_and(|pending| change.undoes == Some(pending.seq))
        {
            self.pending_desk_tempo = None;
        }
    }

    pub(super) fn schedule_desk_tempo(&mut self, change: &Change, now: Instant, bpm: f64) {
        if let Some((target, true, after_beats)) = last_tempo(&change.what) {
            if after_beats > 0.0 {
                self.pending_desk_tempo = Some(Pending {
                    seq: change.seq,
                    bpm: target,
                    // The accepted change owns this deadline; later desk
                    // reports cannot change how long its beats lasted.
                    // after_beats is checked to 0..=256 and bpm clamped to
                    // 20..=300, so at most 768 s; anything else (never
                    // expected) is due at once rather than never.
                    due: now
                        + Duration::try_from_secs_f64(after_beats * 60.0 / bpm)
                            .unwrap_or(Duration::ZERO),
                });
            }
        }
    }

    pub(super) fn tick_desk_tempo(&mut self, now: Instant) {
        if self
            .pending_desk_tempo
            .as_ref()
            .is_none_or(|pending| now < pending.due)
        {
            return;
        }
        // Taking it before delivery makes both success and refusal final.
        let Some(pending) = self.pending_desk_tempo.take() else {
            return;
        };
        match self
            .desk
            .as_ref()
            .map_or(Err(RequestError::NotConnected), |link| {
                link.request_tempo(pending.bpm as f32)
            }) {
            Ok(()) => self.desk_tempo.asked(pending.bpm),
            Err(error) => {
                let summary = format!(
                    "the desk could not take the scheduled tempo of {} for change {}: {}",
                    crate::format::bpm(pending.bpm),
                    pending.seq,
                    refusal_words(&error)
                );
                eprintln!("kazoo-wall: {summary}");
                self.told.push(Event::Notice { summary });
            }
        }
    }
}
