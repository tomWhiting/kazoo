use super::{Outcome, Source, Wall, encode};
use crate::engine::clamp_bpm;
use crate::format;
use crate::patch::Patch;
use crate::protocol::{LoadResult, WallError};
use kazoo_core::ipc::link::RequestError;
use std::time::{Duration, Instant};

const ANSWER_WITHIN: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(super) struct Waiting {
    outcome: Outcome,
    serial: u64,
    seq: u64,
    bpm: f64,
    started: Option<Instant>,
}

impl Wall {
    pub(super) fn tempo_after_go(
        &self,
        incoming: &Patch,
        source: &Source,
        notes: &mut Vec<String>,
    ) -> f64 {
        if matches!(source, Source::Moment { .. } | Source::Back { .. }) && self.on_desk() {
            if self.recorder.status().is_some() {
                notes.push(
                    "the recording under way goes on through the change (with its short fade)"
                        .to_string(),
                );
            }
            incoming.tempo
        } else {
            self.playing_after(incoming, notes)
        }
    }

    pub(super) fn wait_for_desk_tempo(&mut self, ticket: u64, outcome: Outcome) {
        let seq = self.patch.last_seq;
        self.desk_go_waiting.insert(
            ticket,
            Waiting {
                outcome,
                serial: self.swaps_sent,
                seq,
                bpm: self.patch.tempo,
                started: None,
            },
        );
    }

    pub(super) fn desk_go_answers(
        &mut self,
        now: Instant,
    ) -> Vec<(u64, Result<Outcome, WallError>)> {
        let mut replies = Vec::new();
        for (ticket, mut waiting) in std::mem::take(&mut self.desk_go_waiting) {
            if self.engine.shared().unhushed() < waiting.serial {
                self.desk_go_waiting.insert(ticket, waiting);
                continue;
            }
            let started = *waiting.started.get_or_insert(now);
            let answer = self.engine.shared().desk_tempo_reply(waiting.seq);
            let bpm = clamp_bpm(self.engine.shared().bpm());
            let accepted = matches!(answer, Some(Ok(()))) && (bpm - waiting.bpm).abs() <= 0.05;
            let refused = matches!(answer, Some(Err(_)));
            if !accepted && !refused && now.saturating_duration_since(started) < ANSWER_WITHIN {
                self.desk_go_waiting.insert(ticket, waiting);
                continue;
            }
            if accepted {
                self.sync_desk_tempo_at(bpm, now);
                replies.push((ticket, Ok(waiting.outcome)));
                continue;
            }
            let result = self.finish_desk_go(waiting, bpm, answer);
            replies.push((ticket, result));
        }
        replies
    }

    fn finish_desk_go(
        &mut self,
        mut waiting: Waiting,
        bpm: f64,
        answer: Option<Result<(), RequestError>>,
    ) -> Result<Outcome, WallError> {
        let mut loaded: LoadResult = serde_json::from_value(waiting.outcome.result.clone())
            .map_err(|err| super::error(crate::protocol::ErrorCode::Internal, err.to_string()))?;
        loaded.notes.push(format!(
            "the desk kept {}; this moment was made at {}",
            format::bpm(bpm),
            format::bpm(waiting.bpm)
        ));
        let reason = match answer {
            Some(Err(RequestError::Full)) => "the desk's request queue was full",
            Some(Err(RequestError::NotConnected)) => "the desk disconnected before the ask",
            Some(Err(RequestError::Invalid)) => "the desk refused the tempo",
            Some(Err(RequestError::Unsupported)) => "the desk cannot take tempo requests",
            Some(Ok(())) => "the desk did not confirm the tempo within 1 second",
            None => "another wall replaced this go before its tempo was asked",
        };
        loaded.notes.push(reason.to_string());
        waiting.outcome.result = encode(&loaded)?;
        if self.patch.last_seq == waiting.seq {
            self.desk_kept_tempo(bpm);
        }
        Ok(waiting.outcome)
    }
}
