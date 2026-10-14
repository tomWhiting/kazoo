use std::collections::BTreeMap;
use std::time::Instant;

use super::{Growth, Outcome, Wall, encode, error};
use crate::protocol::batch::{at_step, validate};
use crate::protocol::{ErrorCode, Request, WallError, What};

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

fn resolved(request: &Request, labels: &BTreeMap<String, String>) -> Result<Request, WallError> {
    let mut request = request.clone();
    match &mut request {
        Request::Turn { module, .. } | Request::Remove { module } => resolve(module, labels)?,
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

    pub(super) fn batch(
        &mut self,
        seat: &str,
        note: &str,
        requests: &[Request],
        now: Instant,
    ) -> Result<Outcome, WallError> {
        let note = validate(note, requests)?.to_string();
        let before = self.patch.clone();
        let file_grown = self.file_grown;
        self.chain_batch = Some(Vec::with_capacity(requests.len()));
        let applied = self.batch_requests(seat, requests, now);
        let changes = self
            .chain_batch
            .take()
            .ok_or_else(|| error(ErrorCode::Internal, "batch collection disappeared"))?;
        let staged = std::mem::replace(&mut self.patch, before.clone());
        self.file_grown = file_grown;
        let steps = encode(&applied?)?;
        let growth = requests.iter().fold(Growth::NONE, |mut growth, request| {
            match request {
                Request::Add { .. } => growth.modules += 1,
                Request::Patch { .. } => growth.cables += 1,
                _ => {}
            }
            growth
        });
        self.engine
            .begin_batch()
            .map_err(|why| error(ErrorCode::Internal, why))?;
        let outcome = self.room_to_grow(growth).and_then(|()| {
            self.patch = staged;
            self.changed(seat, What::Batch { note, changes }, None, None, None)
        });
        match outcome {
            Ok(mut outcome) => {
                outcome.result["steps"] = steps;
                self.engine.finish_batch();
                Ok(outcome)
            }
            Err(err) => {
                self.engine.abort_batch();
                self.patch = before;
                self.file_grown = file_grown;
                Err(err)
            }
        }
    }

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

    pub(super) fn undo_batch_change(
        &mut self,
        seat: &str,
        seq: u64,
        what: &What,
    ) -> Result<Outcome, WallError> {
        let before = self.patch.clone();
        let inverse = self.patch.undo(what)?;
        let staged = std::mem::replace(&mut self.patch, before.clone());
        let growth = if let What::Batch { changes, .. } = what {
            super::chains::undo_growth(changes)
        } else {
            Growth::NONE
        };
        self.engine
            .begin_batch()
            .map_err(|why| error(ErrorCode::Internal, why))?;
        let result = self.room_to_grow(growth).and_then(|()| {
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

    pub(super) fn mirror_batch(&mut self, changes: &[What]) {
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
    }
}

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
