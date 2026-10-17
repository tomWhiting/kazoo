use super::{Outcome, Wall, encode};
use crate::engine::{Command, clamp_bpm};
use crate::format;
use crate::protocol::{Change, Event, LoadResult, WallError};
use kazoo_core::ipc::link::RequestError;
use std::time::{Duration, Instant};

const ANSWER_WITHIN: Duration = Duration::from_secs(1);

/// Where an off-desk go's tempo is delivered when its answer settles.
#[derive(Clone, Copy, Debug)]
pub(super) enum TempoDelivery {
    Send,
    /// `load_engine` sends the patch's tempo to the replacement engine.
    Reload,
}

/// A replacement's typed answer, retained until its tempo is settled.
#[derive(Debug)]
pub(super) struct Loaded {
    pub(super) outcome: Outcome,
    pub(super) result: Box<LoadResult>,
}

impl Loaded {
    /// Retain a committed replacement and its notes until the answer is ready.
    pub(super) fn new(
        change: Change,
        seat: &str,
        save: Option<String>,
        backup: String,
        notes: Vec<String>,
    ) -> Self {
        Self {
            result: Box::new(LoadResult {
                change: change.clone(),
                backup: backup.clone(),
                notes,
            }),
            outcome: Outcome {
                result: serde_json::Value::Null,
                event: Some(Event::Loaded {
                    seq: change.seq,
                    seat: seat.to_string(),
                    save,
                    backup,
                }),
                change: Some(change),
                pending: None,
            },
        }
    }
}

/// A tree read or replacement awaiting its answer to the seat.
#[derive(Debug)]
pub(super) enum TreeOutcome {
    Ready(Outcome),
    Loaded(Loaded),
}

/// A landed go waiting for the desk's tempo, tied to its wall and engine swap.
#[derive(Debug)]
pub(super) struct Waiting {
    loaded: Loaded,
    serial: u64,
    seq: u64,
    bpm: f64,
    started: Option<Instant>,
}

impl Wall {
    /// Encode a replacement once, announcing its change even if encoding fails.
    pub(super) fn answer_load(&mut self, mut loaded: Loaded) -> Result<Outcome, WallError> {
        match encode(&loaded.result) {
            Ok(result) => {
                loaded.outcome.result = result;
                Ok(loaded.outcome)
            }
            Err(error) => {
                self.announce_desk_go(&mut loaded.outcome);
                Err(error)
            }
        }
    }

    /// Hold a go's typed answer until its landing and the desk's response.
    pub(super) fn wait_for_desk_tempo(&mut self, ticket: u64, loaded: Loaded) {
        self.desk_go_waiting.insert(
            ticket,
            Waiting {
                loaded,
                serial: self.swaps_sent,
                seq: self.patch.wall,
                bpm: self.patch.tempo,
                started: None,
            },
        );
    }

    /// Answer landed asks, allowing one second for the desk to echo the tempo.
    pub(super) fn desk_go_answers(
        &mut self,
        now: Instant,
    ) -> Vec<(u64, Result<Outcome, WallError>)> {
        let mut replies = self.settled_desk_go_answers();
        for (ticket, mut waiting) in std::mem::take(&mut self.desk_go_waiting) {
            if self.engine.shared().unhushed() < waiting.serial {
                self.desk_go_waiting.insert(ticket, waiting);
                continue;
            }
            let started = *waiting.started.get_or_insert(now);
            let answer = self.engine.shared().desk_tempo_reply(waiting.seq);
            let bpm = clamp_bpm(self.engine.shared().bpm());
            let accepted = self.on_desk()
                && matches!(answer, Some(Ok(())))
                && (bpm - waiting.bpm).abs() <= 0.05;
            let refused = matches!(answer, Some(Err(_))) || !self.on_desk();
            if !accepted && !refused && now.saturating_duration_since(started) < ANSWER_WITHIN {
                self.desk_go_waiting.insert(ticket, waiting);
                continue;
            }
            if accepted {
                self.sync_desk_tempo_at(bpm, now);
                replies.push((ticket, self.answer_load(waiting.loaded)));
                continue;
            }
            let result = self.finish_desk_go(waiting, bpm, answer, now, TempoDelivery::Send);
            replies.push((ticket, result));
        }
        replies
    }

    /// Take replies settled during a rebuild or close, without acting on tree jobs.
    pub(crate) fn settled_desk_go_answers(&mut self) -> Vec<(u64, Result<Outcome, WallError>)> {
        std::mem::take(&mut self.desk_go_settled)
    }

    /// Settle every old-engine ask before rebuilding or closing the wall.
    pub(super) fn settle_desk_go(&mut self, now: Instant, tempo: TempoDelivery) {
        let bpm = clamp_bpm(self.engine.shared().bpm());
        for (ticket, mut waiting) in std::mem::take(&mut self.desk_go_waiting) {
            self.announce_desk_go(&mut waiting.loaded.outcome);
            let result = self.finish_desk_go(waiting, bpm, None, now, tempo);
            if let Err(error) = &result {
                eprintln!(
                    "kazoo-wall: the go happened but its answer failed: {}",
                    error.message
                );
            }
            self.desk_go_settled.push((ticket, result));
        }
    }

    /// Announce a settled go now while retaining its answer for the ticket.
    fn announce_desk_go(&mut self, outcome: &mut Outcome) {
        if let Some(change) = outcome.change.take() {
            self.told.push(Event::Change {
                change: Box::new(change),
            });
        }
        self.told.extend(outcome.event.take());
    }

    /// Keep the desk's actual tempo on this wall, or play the patch off the desk.
    fn finish_desk_go(
        &mut self,
        mut waiting: Waiting,
        bpm: f64,
        answer: Option<Result<(), RequestError>>,
        now: Instant,
        tempo: TempoDelivery,
    ) -> Result<Outcome, WallError> {
        if self.on_desk() {
            waiting.loaded.result.notes.push(format!(
                "the desk kept {}; this moment was made at {}",
                format::bpm(bpm),
                format::bpm(waiting.bpm)
            ));
            if self.patch.wall == waiting.seq {
                self.desk_kept_tempo(bpm, now);
            }
        } else {
            if matches!(tempo, TempoDelivery::Send) {
                self.engine.send(Command::Tempo(self.patch.tempo));
            }
            waiting.loaded.result.notes.push(format!(
                "the desk is not linked; the wall plays {}",
                format::bpm(self.patch.tempo)
            ));
        }
        let reason = match answer {
            Some(Err(RequestError::Full)) => "the desk's request queue was full",
            Some(Err(RequestError::NotConnected)) => "the desk disconnected before the ask",
            Some(Err(RequestError::Invalid)) => "the desk refused the tempo",
            Some(Err(RequestError::Unsupported)) => "the desk cannot take tempo requests",
            Some(Ok(())) => "the desk did not confirm the tempo within 1 second",
            None => "the go's tempo ask ended before the desk confirmed it",
        };
        waiting.loaded.result.notes.push(reason.to_string());
        self.answer_load(waiting.loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, engine};
    use crate::protocol::What;

    fn refused(reason: RequestError, note: &str, connected: bool) {
        let dir = tempfile::tempdir().unwrap();
        let (link, (_audio, _stream)) = super::super::tests::a_desk(dir.path(), 120.0);
        let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let mut patch = crate::seed::seed().unwrap();
        patch.tempo = 96.0;
        patch.last_seq = 1;
        patch.wall = 1;
        let mut wall = Wall::new(patch, control, connected.then_some(link), Vec::new(), None);
        let change = wall.new_change(
            "Tom",
            What::Tempo {
                from: 120.0,
                to: 96.0,
                desk: true,
                after_beats: 0.0,
                glide_beats: 0.0,
            },
            None,
        );
        let loaded = LoadResult {
            change: change.clone(),
            backup: "before-go".into(),
            notes: Vec::new(),
        };
        let waiting = Waiting {
            loaded: Loaded {
                outcome: Outcome {
                    result: serde_json::Value::Null,
                    change: Some(change),
                    event: None,
                    pending: None,
                },
                result: Box::new(loaded),
            },
            serial: 1,
            seq: 1,
            bpm: 96.0,
            started: None,
        };
        let result = wall
            .finish_desk_go(
                waiting,
                120.0,
                Some(Err(reason)),
                Instant::now(),
                TempoDelivery::Send,
            )
            .unwrap();
        let loaded: LoadResult = serde_json::from_value(result.result).unwrap();
        assert!(
            loaded.notes.iter().any(|entry| entry == note),
            "{:?}",
            loaded.notes
        );
        if connected {
            assert_eq!(wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
            assert!(wall.log.iter().any(|change| change.seat == "desk"));
        } else {
            assert_eq!(wall.patch.tempo.to_bits(), 96.0_f64.to_bits());
            assert!(wall.log.is_empty());
        }
    }

    #[test]
    fn a_go_completion_does_not_decode_an_intermediate_result() {
        let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let mut patch = crate::seed::seed().unwrap();
        patch.tempo = 96.0;
        let mut wall = Wall::new(patch, control, None, Vec::new(), None);
        let waiting = Waiting {
            loaded: Loaded {
                result: Box::new(LoadResult {
                    change: wall.new_change(
                        "Tom",
                        What::Tempo {
                            from: 120.0,
                            to: 96.0,
                            desk: true,
                            after_beats: 0.0,
                            glide_beats: 0.0,
                        },
                        None,
                    ),
                    backup: "before-go".into(),
                    notes: vec!["typed note".into()],
                }),
                outcome: Outcome {
                    result: serde_json::Value::Null,
                    change: None,
                    event: None,
                    pending: None,
                },
            },
            serial: 1,
            seq: 0,
            bpm: 96.0,
            started: None,
        };
        let result = wall.finish_desk_go(
            waiting,
            120.0,
            Some(Err(RequestError::NotConnected)),
            Instant::now(),
            TempoDelivery::Send,
        );
        assert!(result.is_ok(), "{result:?}");
        let loaded: LoadResult = serde_json::from_value(result.unwrap().result).unwrap();
        assert!(loaded.notes.iter().any(|note| note == "typed note"));
    }

    #[test]
    fn a_full_go_ask_names_its_refusal() {
        refused(
            RequestError::Full,
            "the desk's request queue was full",
            false,
        );
        refused(
            RequestError::Full,
            "the desk's request queue was full",
            true,
        );
    }

    #[test]
    fn an_invalid_go_ask_names_its_refusal() {
        refused(RequestError::Invalid, "the desk refused the tempo", false);
        refused(RequestError::Invalid, "the desk refused the tempo", true);
    }

    #[test]
    fn an_unsupported_go_ask_names_its_refusal() {
        refused(
            RequestError::Unsupported,
            "the desk cannot take tempo requests",
            false,
        );
        refused(
            RequestError::Unsupported,
            "the desk cannot take tempo requests",
            true,
        );
    }

    #[test]
    fn a_disconnected_go_ask_names_its_refusal() {
        refused(
            RequestError::NotConnected,
            "the desk disconnected before the ask",
            false,
        );
        refused(
            RequestError::NotConnected,
            "the desk disconnected before the ask",
            true,
        );
    }
}
