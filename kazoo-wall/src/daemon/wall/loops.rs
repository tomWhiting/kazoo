//! Loop requests use the wall's ordinary durable change and rollback path.
use super::{Outcome, Wall};
use crate::protocol::{LoopOp, WallError};
impl Wall {
    pub(super) fn loop_change(
        &mut self,
        seat: &str,
        module: &str,
        op: &LoopOp,
    ) -> Result<Outcome, WallError> {
        let id = self.patch.find(module)?.id.clone();
        let what = self.patch.loop_op(&id, op)?;
        self.changed(seat, what, None, Some(id), None)
    }
}

use super::{LOADER_SEAT, error};
use crate::daemon::loops::pins::{ReelRefs, SAVE_WAIT, rotation};
use crate::protocol::{Change, Event, TakeWhy, What};
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

#[derive(Debug)]
struct Saving {
    seat: String,
    name: String,
    overwrite: bool,
    asked: Instant,
    wall: u64,
    takes: Vec<(String, Option<u64>, String)>,
}

/// Request and reference bookkeeping lives beside the loop integration.
#[derive(Debug, Default)]
pub(super) struct Work {
    refs: Option<ReelRefs>,
    rotation: Option<SystemTime>,
    seen: u64,
    next_save: u64,
    saving: BTreeMap<u64, Saving>,
    awaiting_log: BTreeMap<u64, (String, u64)>,
    now: Option<Instant>,
    names: BTreeMap<(String, u64), String>,
}

impl Wall {
    pub(super) fn refresh_loop_refs(&mut self, sweep: bool) {
        let changes: Vec<Change> = self.log.iter().cloned().collect();
        let missing = self.loop_work.refs.is_none();
        let mut refs = self.loop_work.refs.take().unwrap_or_default();
        let rotation = self.store.as_ref().map(rotation).transpose();
        let result = rotation.and_then(|stamp| {
            let stamp = stamp.flatten();
            let reread = missing || self.loop_work.seen == 0 || stamp != self.loop_work.rotation;
            refs.refresh(&self.patch.to_file(), self.store.as_mut(), &changes, reread)?;
            self.loop_work.rotation = stamp;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.loop_work.refs = Some(refs.clone());
                self.loop_work.seen = self.revision();
                self.loops.queue_pin(refs, sweep);
            }
            Err(error) => self.loops.uncertain_refs(format!(
                "the wall's reel references cannot be read: {error}; no reels are reclaimed"
            )),
        }
    }

    pub(super) fn tick_loops(&mut self, now: Instant) {
        self.loop_work.now = Some(now);
        self.update_loop_refs();
        self.loops.poll();
        self.loops.checkpoint(now);
        while let Some(take) = self.loops.recordings.finished.pop_front() {
            let was = self
                .patch
                .loop_record(&take.module)
                .and_then(|record| record.tape.clone());
            if take.why == TakeWhy::Save {
                if let Some(saving) = self.loop_work.saving.values().find(|saving| {
                    saving
                        .takes
                        .iter()
                        .any(|(_, ticket, _)| *ticket == Some(take.ticket))
                }) {
                    self.loop_work
                        .names
                        .insert((take.module.clone(), take.mark.frame), saving.name.clone());
                }
            }
            let written = self.written_after_take(
                &take.module,
                &take.tape,
                was.as_ref(),
                take.mark.frame,
                take.written,
            );
            let what = What::Take {
                module: take.module.clone(),
                tape: Some(take.tape),
                was,
                frame: take.mark.frame,
                beat: take.mark.beat,
                written,
                why: take.why,
            };
            let paid = self.commit_own(LOADER_SEAT, what);
            if let Some((change, event)) = paid {
                if take.why == TakeWhy::Save {
                    self.loops.recordings.logged.insert(take.ticket);
                }
                self.loops
                    .recordings
                    .last
                    .insert(take.module, (now, take.mark.frame, change.at.clone()));
                self.told.push(Event::Change {
                    change: Box::new(change),
                });
                self.told.extend(event);
            } else {
                self.loop_work
                    .awaiting_log
                    .insert(take.ticket, (take.module, take.mark.frame));
            }
        }
        let paid: Vec<_> = self.loop_work.awaiting_log.iter().filter_map(|(ticket, (module, frame))| {
            self.log.iter().rev().find(|change| matches!(&change.what, What::Take { module: id, frame: at, .. } if id == module && at == frame)).map(|change| (*ticket, module.clone(), *frame, change.at.clone()))
        }).collect();
        for (ticket, module, frame, at) in paid {
            self.loop_work.awaiting_log.remove(&ticket);
            self.loops.recordings.logged.insert(ticket);
            self.loops.recordings.last.insert(module, (now, frame, at));
        }
    }

    fn update_loop_refs(&mut self) {
        let stamp = self.store.as_ref().map(rotation).transpose();
        match stamp {
            Ok(stamp) => {
                let stamp = stamp.flatten();
                if self.loop_work.refs.is_none() || stamp != self.loop_work.rotation {
                    self.refresh_loop_refs(true);
                    self.loop_work.rotation = stamp;
                } else if self.loop_work.seen != self.revision() {
                    let file = self.patch.to_file();
                    if let Some(refs) = self.loop_work.refs.as_mut() {
                        let changes: Vec<_> = self
                            .log
                            .iter()
                            .filter(|change| change.seq > self.loop_work.seen)
                            .collect();
                        let mut failure = None;
                        for change in changes {
                            if let Err(error) = refs.add(change.seq, &change.what) {
                                failure = Some(error);
                                break;
                            }
                        }
                        if let Err(error) = refs.set_patch(&file) {
                            failure = Some(error);
                        }
                        if let Some(error) = failure {
                            self.loops.uncertain_refs(format!(
                                "reel references cannot be updated: {error}"
                            ));
                        } else {
                            self.loops.queue_pin(refs.clone(), false);
                            self.loop_work.seen = self.made;
                        }
                    }
                }
            }
            Err(error) => self.loops.uncertain_refs(format!(
                "reel log rotation cannot be read: {error}; no reels are reclaimed"
            )),
        }
    }

    fn written_after_take(
        &self,
        module: &str,
        tape: &crate::patch::TapeRecord,
        was: Option<&crate::patch::TapeRecord>,
        frame: u64,
        written: Vec<u16>,
    ) -> Vec<u16> {
        let Some(was) = was else {
            return written;
        };
        if was.frames != tape.frames
            || was.beats.to_bits() != tape.beats.to_bits()
            || was.anchor.to_bits() != tape.anchor.to_bits()
        {
            return written;
        }
        let prior = self.log.iter().rev().find_map(|change| match &change.what {
            What::Take {
                module: id,
                tape: Some(held),
                frame,
                beat,
                ..
            } if id == module && held.reel == was.reel => Some((*frame, *beat)),
            _ => None,
        });
        written
            .into_iter()
            .filter(|bar| {
                let bar = usize::from(*bar);
                let after = tape.bars.get(bar).copied().flatten();
                let before = was.bars.get(bar).copied().flatten();
                if after != before {
                    return true;
                }
                match (after, prior) {
                    (Some(pass), Some((prior_frame, beat))) if frame > prior_frame => {
                        let rel = beat - was.anchor;
                        let prior_pass = (rel / was.beats).floor() as i64;
                        let prior_bar = (rel.rem_euclid(was.beats) / 4.0).floor() as usize;
                        pass > prior_pass || (pass == prior_pass && bar >= prior_bar)
                    }
                    _ => false,
                }
            })
            .collect()
    }

    pub(super) fn named_take_summary(&self, what: &What) -> Option<String> {
        if let What::Take {
            module,
            frame,
            why: TakeWhy::Save,
            ..
        } = what
        {
            self.loop_work
                .names
                .get(&(module.clone(), *frame))
                .map(|name| format!("{LOADER_SEAT} {module} was taken for the save `{name}`"))
        } else {
            None
        }
    }

    pub(super) fn accept_take(&mut self, change: &Change) {
        if let What::Take {
            module,
            tape,
            frame,
            ..
        } = &change.what
        {
            let mut record = self.patch.loop_record(module).cloned().unwrap_or_default();
            record.tape.clone_from(tape);
            self.loop_work.names.remove(&(module.clone(), *frame));
            if let Err(error) = self.patch.set_loop(module, record) {
                self.loops.report(format!(
                    "{module}'s logged take cannot be installed: {}",
                    error.message
                ));
            }
        }
    }

    pub(super) fn check_take_undo(&self, what: &What) -> Result<(), WallError> {
        if let What::Take {
            was: Some(tape), ..
        } = what
        {
            let id = tape.reel.parse().map_err(|error| {
                super::error(
                    crate::protocol::ErrorCode::Internal,
                    format!("invalid take reel: {error}"),
                )
            })?;
            if let Some(at) = self.loops.recordings.pruned.get(&id) {
                return Err(error(
                    crate::protocol::ErrorCode::NotAllowed,
                    format!("that checkpoint's reel was pruned for space at {at}"),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn note_loop_restart(&mut self) {
        let frame = self.engine.shared().frames();
        for module in self.loops.recording_modules() {
            let (taken, at) = self
                .loops
                .recordings
                .last
                .get(&module)
                .map_or((0, "no take"), |(_, frame, at)| (*frame, at.as_str()));
            let lost = frame.saturating_sub(taken) / u64::from(self.engine.sample_rate().max(1));
            let pass = self.loops.gauges(&module).map_or(0, |gauges| {
                gauges.pass.load(std::sync::atomic::Ordering::Relaxed)
            });
            self.loops.report(format!("{module} was recording at pass {pass} when the audio restarted; up to {lost} s since its last take at {at} were lost; its last take is put back"));
        }
        for (at, frame, _) in self.loops.recordings.last.values_mut() {
            *at = Instant::now();
            *frame = 0;
        }
    }

    pub(super) fn begin_loop_save(
        &mut self,
        seat: &str,
        name: &str,
        overwrite: bool,
        now: Instant,
    ) -> Option<Outcome> {
        self.loops.collect();
        let modules = self.loops.recording_modules();
        if modules.is_empty() {
            return None;
        }
        let mut takes = Vec::new();
        for module in modules {
            let at = self
                .loops
                .recordings
                .last
                .get(&module)
                .map_or_else(|| "no previous take".to_string(), |(_, _, at)| at.clone());
            let ticket = self.loops.request_take(&module, TakeWhy::Save, None);
            takes.push((module, ticket, at));
        }
        self.loop_work.next_save = self.loop_work.next_save.wrapping_add(1);
        let ticket = (1_u64 << 63) | self.loop_work.next_save;
        self.loop_work.saving.insert(
            ticket,
            Saving {
                seat: seat.to_string(),
                name: name.to_string(),
                overwrite,
                asked: now,
                wall: self.patch.wall,
                takes,
            },
        );
        Some(Outcome {
            result: serde_json::Value::Null,
            change: None,
            event: None,
            pending: Some(ticket),
        })
    }

    /// Complete named saves after all their loop takes are logged, or at five seconds.
    pub fn saved(&mut self) -> Vec<(u64, Result<Outcome, WallError>)> {
        let now = self.loop_work.now.unwrap_or_else(Instant::now);
        let ready: Vec<_> = self
            .loop_work
            .saving
            .iter()
            .filter(|(_, saving)| {
                saving.wall != self.patch.wall
                    || now.saturating_duration_since(saving.asked) >= SAVE_WAIT
                    || saving.takes.iter().all(|(_, ticket, _)| {
                        ticket.is_some_and(|ticket| self.loops.recordings.logged.contains(&ticket))
                    })
            })
            .map(|(ticket, _)| *ticket)
            .collect();
        let mut answers = Vec::new();
        for ticket in ready {
            let Some(saving) = self.loop_work.saving.remove(&ticket) else {
                continue;
            };
            if saving.wall != self.patch.wall {
                answers.push((ticket, Err(error(crate::protocol::ErrorCode::WallReplaced, "the wall was replaced while its loops were being saved; the save was not written"))));
                continue;
            }
            let notes: Vec<_> = saving.takes.iter().filter(|(_, ticket, _)| ticket.is_none_or(|ticket| !self.loops.recordings.logged.contains(&ticket)))
                .map(|(module, _, at)| {
                    let at = self.loops.recordings.last.get(module).map_or(at.as_str(), |(_, _, at)| at.as_str());
                    let at = at.get(11..19).unwrap_or(at);
                    format!("{module}'s newest recording was not ready in 5 s; the save holds its take from {at}")
                }).collect();
            let answer = self
                .write_loop_save(&saving.seat, &saving.name, saving.overwrite)
                .and_then(|mut outcome| {
                    let mut saved: crate::protocol::SaveResult =
                        serde_json::from_value(outcome.result).map_err(|err| {
                            error(
                                crate::protocol::ErrorCode::Internal,
                                format!("the loop save result could not be read: {err}"),
                            )
                        })?;
                    saved.notes.extend(notes);
                    outcome.result = super::encode(&saved)?;
                    Ok(outcome)
                });
            for (_, ticket, _) in saving.takes {
                if let Some(ticket) = ticket {
                    if !self.loops.recordings.logged.remove(&ticket) {
                        self.loops.cancel_take(ticket);
                    }
                }
            }
            answers.push((ticket, answer));
        }
        answers
    }
}
