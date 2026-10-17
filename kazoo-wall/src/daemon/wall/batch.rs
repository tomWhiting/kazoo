//! A seat's batch: several changes toward one idea, with a note, applied
//! in order as one change, all or none, and undone as one.

use std::collections::BTreeMap;
use std::time::Instant;

use kazoo_core::ipc::link::HubLink;

use super::{Growth, Outcome, Wall, encode, error};
use crate::engine::clamp_bpm;
use crate::protocol::batch::{at_step, validate};
use crate::protocol::{ErrorCode, Request, WallError, What};

/// `value` with its `@label` given the id that label's add made.
fn resolve(value: &mut String, labels: &BTreeMap<String, String>) -> Result<(), WallError> {
    let Some(reference) = value.strip_prefix('@') else {
        return Ok(());
    };
    let (label, suffix) = reference
        .split_once('.')
        .map_or((reference, None), |(label, port)| (label, Some(port)));
    let id = labels
        .get(label)
        .ok_or_else(|| error(ErrorCode::BadRequest, format!("unknown label @{label}")))?;
    *value = suffix.map_or_else(|| id.clone(), |port| format!("{id}.{port}"));
    Ok(())
}

/// `request` with every `@label` in it resolved (see [`resolve`]).
fn resolved(request: &Request, labels: &BTreeMap<String, String>) -> Result<Request, WallError> {
    let mut request = request.clone();
    match &mut request {
        Request::Turn { module, .. } | Request::Remove { module, .. } => resolve(module, labels)?,
        Request::Patch { from, to, .. } => {
            resolve(from, labels)?;
            resolve(to, labels)?;
        }
        Request::Unpatch { to: Some(to), .. } => resolve(to, labels)?,
        Request::Add {
            place: Some(place), ..
        } => {
            if let Some(before) = &mut place.before {
                resolve(before, labels)?;
            }
        }
        _ => {}
    }
    Ok(request)
}

impl Wall {
    /// An add: one with a label only inside a batch.
    pub(super) fn batch_add(
        &mut self,
        seat: &str,
        request: &Request,
    ) -> Result<Outcome, WallError> {
        let Request::Add {
            kind,
            name,
            place,
            alias,
            ..
        } = request
        else {
            return Err(error(
                ErrorCode::Internal,
                "batch add requires an add request",
            ));
        };
        if alias.is_some() && self.chain_batch.is_none() {
            return Err(error(
                ErrorCode::BadRequest,
                "an add label is only used in a batch",
            ));
        }
        self.add(seat, kind, name.as_deref(), place.as_ref())
    }

    /// A seat's batch (see [`Request::Batch`]): its steps applied in order
    /// as one change, all or none. Refused before anything is applied when
    /// its note or a step is not one a batch takes; refused as a whole, with
    /// the step and its reason, when a step is; then the patch, the file's
    /// reckoning and the engine are as before it, and the engine hears
    /// nothing of it.
    pub(super) fn batch(
        &mut self,
        seat: &str,
        (note, comments): (&str, &[Option<String>]),
        requests: &[Request],
        now: Instant,
    ) -> Result<Outcome, WallError> {
        let (note, comments) = validate(note, requests, comments)?;
        let note = note.to_string();
        // A desk move not yet heard by a tick is logged first, as before a
        // single tempo: the batch's tempo starts from what plays.
        if requests
            .iter()
            .any(|request| matches!(request, Request::Tempo { .. }))
        {
            self.hear_the_desk_first(now);
        }
        let before = self.patch.clone();
        let reckoning = (self.file_bytes, self.file_grown, self.file_measured_at);
        self.chain_batch = Some(Vec::with_capacity(requests.len()));
        let applied = self.batch_requests(seat, requests, now);
        let collected = self.chain_batch.take();
        let staged = std::mem::replace(&mut self.patch, before.clone());
        let made = collected
            .ok_or_else(|| error(ErrorCode::Internal, "batch collection disappeared"))
            .and_then(|changes| Ok((changes, encode(&applied?)?)))
            .and_then(|made| {
                self.engine
                    .begin_batch()
                    .map_err(|why| error(ErrorCode::Internal, why))?;
                Ok(made)
            });
        let (changes, steps) = match made {
            Ok(made) => made,
            Err(err) => {
                (self.file_bytes, self.file_grown, self.file_measured_at) = reckoning;
                return Err(err);
            }
        };
        // The wall before the batch, for the tree to grow a moment from
        // each step (KZ-T10c); taken when the batch is fed to the tree.
        self.batch_before = Some(before.to_file());
        // What a loop added reserved for its tape stays reckoned; the room
        // for the rest is what the batch made, as the single changes would
        // reckon it: a module per add, a cable per cable that is new.
        let outcome = self.room_to_grow(growth_of(&changes)).and_then(|()| {
            self.patch = staged;
            self.changed(
                seat,
                What::Batch {
                    note,
                    changes,
                    comments,
                },
                None,
                None,
                None,
            )
        });
        self.batch_before = None;
        match outcome {
            Ok(mut outcome) => {
                outcome.result["steps"] = steps;
                self.engine.finish_batch();
                Ok(outcome)
            }
            Err(err) => {
                self.engine.abort_batch();
                self.patch = before;
                (self.file_bytes, self.file_grown, self.file_measured_at) = reckoning;
                Err(err)
            }
        }
    }

    /// Whether the wall follows a desk now.
    fn on_the_desk(&self) -> bool {
        self.desk.as_ref().is_some_and(HubLink::is_connected)
    }

    /// Log a desk move no tick has heard yet (see [`Self::sync_desk_tempo`]),
    /// when the wall follows a desk.
    fn hear_the_desk_first(&mut self, now: Instant) {
        if self.on_the_desk() {
            self.sync_desk_tempo(clamp_bpm(self.engine.shared().bpm()), now);
            self.finish_desk_tempo();
        }
    }

    /// Where a tempo step in a batch starts: what plays for the first on
    /// the desk (as a single tempo starts), else the tempo the steps before
    /// it left.
    pub(super) fn batch_tempo_from(&self, desk: bool) -> f64 {
        let earlier = self.chain_batch.as_ref().is_some_and(|changes| {
            changes
                .iter()
                .any(|change| matches!(change, What::Tempo { .. }))
        });
        if desk && !earlier {
            clamp_bpm(self.engine.shared().bpm())
        } else {
            self.patch.tempo
        }
    }

    /// Apply a batch's steps in order, each `@label` given the id its add
    /// made; each step's answer, or the first refusal with its step.
    fn batch_requests(
        &mut self,
        seat: &str,
        requests: &[Request],
        now: Instant,
    ) -> Result<Vec<serde_json::Value>, WallError> {
        let mut labels = BTreeMap::new();
        let mut results = Vec::with_capacity(requests.len());
        for (index, request) in requests.iter().enumerate() {
            let request = resolved(request, &labels).map_err(|err| at_step(err, index))?;
            let mut outcome = self
                .answer((seat, true), &request, now)
                .map_err(|err| at_step(err, index))?;
            if let Request::Add {
                alias: Some(label), ..
            } = request
            {
                let id = outcome.result["module"].as_str().ok_or_else(|| {
                    at_step(
                        error(ErrorCode::Internal, "batch add returned no id"),
                        index,
                    )
                })?;
                labels.insert(label.clone(), id.to_string());
                outcome.result["as"] = serde_json::Value::String(label);
            }
            results.push(outcome.result);
        }
        Ok(results)
    }

    /// Undo batch `seq` (`what`): its steps' inverses in reverse order, as
    /// one change, all or none. Its tempo is put back as a single tempo's
    /// undo puts it back: from the tempo now, asked of the desk when the
    /// wall follows one now, whichever it followed when the batch was made.
    pub(super) fn undo_batch_change(
        &mut self,
        seat: &str,
        seq: u64,
        what: &What,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        let What::Batch { changes, .. } = what else {
            return Err(error(ErrorCode::Internal, "a batch undo needs a batch"));
        };
        let desk = self.on_the_desk();
        let tempo = changes
            .iter()
            .any(|change| matches!(change, What::Tempo { .. }));
        if tempo {
            self.hear_the_desk_first(now);
        }
        let now = if desk {
            clamp_bpm(self.engine.shared().bpm())
        } else {
            self.patch.tempo
        };
        let before = self.patch.clone();
        let inverse = tempo_as_now(self.patch.undo(what)?, now, desk);
        let staged = std::mem::replace(&mut self.patch, before.clone());
        self.engine
            .begin_batch()
            .map_err(|why| error(ErrorCode::Internal, why))?;
        let result = self
            .room_to_grow(super::chains::undo_growth(changes))
            .and_then(|()| {
                self.patch = staged;
                self.changed(seat, inverse, Some(seq), None, None)
            });
        if result.is_ok() {
            self.engine.finish_batch();
        } else {
            self.engine.abort_batch();
            self.patch = before;
        }
        result
    }

    /// Ask the desk for a logged batch's last tempo on the desk, if it has
    /// one.
    ///
    /// # Errors
    ///
    /// `slow_down` when the desk cannot take it.
    pub(super) fn ask_batch_tempo(&mut self, what: &What) -> Result<(), WallError> {
        let What::Batch { changes, .. } = what else {
            return Ok(());
        };
        let tempo = changes.iter().rev().find_map(|change| match change {
            What::Tempo { to, desk: true, .. } => Some(*to),
            _ => None,
        });
        if let Some(bpm) = tempo {
            let result = self.desk.as_ref().map_or(
                Err(kazoo_core::ipc::link::RequestError::NotConnected),
                |link| link.request_tempo(bpm as f32),
            );
            result.map_err(|err| {
                error(
                    ErrorCode::SlowDown,
                    format!("the desk refused the batch tempo: {err:?}"),
                )
            })?;
            self.desk_tempo.asked(bpm);
        }
        Ok(())
    }

    /// Mirror a batch's changes into the engine in order, each module
    /// added as its add made it, so a later turn of it glides as asked.
    pub(super) fn mirror_batch(&mut self, changes: &[What]) {
        let earlier = self.motion_bpm;
        self.motion_bpm = Some(self.engine.shared().bpm());
        for change in changes {
            if let What::Add { module } | What::Restore { module, .. } = change {
                if let Some(mut added) = self.patch.module(&module.id).cloned() {
                    for (spec, knob) in added.kind.spec().knobs.iter().zip(&mut added.knobs) {
                        if let Some(value) = module.knobs.get(spec.name) {
                            *knob = spec.clamp(*value as f32);
                        }
                    }
                    if self.insert(&added) {
                        self.put_back_words(&added.id);
                    }
                    self.send_tables();
                }
            } else {
                self.mirror(change);
            }
        }
        self.motion_bpm = earlier;
    }
}

/// The room a batch's changes take, as the single changes reckon theirs:
/// a module for each add, a cable for each cable that replaced none.
fn growth_of(changes: &[What]) -> Growth {
    changes.iter().fold(Growth::NONE, |mut growth, change| {
        match change {
            What::Add { .. } => growth.modules += 1,
            What::Patch { replaced: None, .. } => growth.cables += 1,
            _ => {}
        }
        growth
    })
}

/// A batch undo's inverse with its tempo as the wall is now: the first
/// tempo step from `now`, and every tempo step on the desk or off it as
/// `desk` says.
fn tempo_as_now(inverse: What, now: f64, desk: bool) -> What {
    let What::Batch { note, changes, .. } = inverse else {
        return inverse;
    };
    let mut first = true;
    let changes = changes
        .into_iter()
        .map(|change| match change {
            What::Tempo { from, to, .. } => {
                let from = if first { now } else { from };
                first = false;
                What::Tempo {
                    from,
                    to,
                    desk,
                    after_beats: 0.0,
                    glide_beats: 0.0,
                }
            }
            other => other,
        })
        .collect();
    // The steps' comments said why each went on; undone, none does.
    What::Batch {
        note,
        changes,
        comments: Vec::new(),
    }
}

/// Stamp `birth` on the record of module `id` removed in batch `what`
/// (added and taken away in the same batch, so the patch no longer holds
/// it); whether there was one.
pub(super) fn stamp_removed_birth(what: &mut What, id: &str, birth: &crate::tree::Birth) -> bool {
    match what {
        What::Batch { changes, .. } => {
            let mut removed = false;
            for change in changes {
                removed |= stamp_removed_birth(change, id, birth);
            }
            removed
        }
        What::Remove { module, .. } if module.id == id => {
            module.birth = Some(birth.to_string());
            true
        }
        _ => false,
    }
}
