use std::time::Instant;

use super::{Growth, Outcome, Wall, encode, error};
use crate::protocol::{ChainSaveResult, ChainsResult};
use crate::protocol::{ErrorCode, Event, Request, WallError, What};
use crate::store::SaveError;

fn chain_error(err: &SaveError) -> WallError {
    if let SaveError::Exists(name) = err {
        return error(
            ErrorCode::SaveExists,
            format!(
                "a chain named \"{name}\" is kept already; a console saves over it with \
                 :chain replace {name}, anyone can choose another name"
            ),
        );
    }
    let code = match err {
        SaveError::Unknown(_) => ErrorCode::UnknownSave,
        SaveError::Exists(_) => ErrorCode::SaveExists,
        SaveError::Broken { .. } => ErrorCode::BrokenSave,
        SaveError::TooLarge(_) => ErrorCode::Full,
        SaveError::Io(_) => ErrorCode::Internal,
    };
    error(code, format!("chain storage refused the request: {err}"))
}

pub(super) fn undo_growth(changes: &[What]) -> Growth {
    changes.iter().fold(Growth::NONE, |mut growth, change| {
        let more = match change {
            What::Chain { changes, .. } => undo_growth(changes),
            What::Remove { cables, .. } => Growth {
                modules: 1,
                cables: cables.len(),
                words: 1,
            },
            What::Unpatch { .. } => Growth {
                cables: 1,
                ..Growth::NONE
            },
            _ => Growth::NONE,
        };
        growth.modules = growth.modules.saturating_add(more.modules);
        growth.cables = growth.cables.saturating_add(more.cables);
        growth.words = growth.words.saturating_add(more.words);
        growth
    })
}

impl Wall {
    pub(super) fn mirror_chain(&mut self, changes: &[What]) {
        for change in changes {
            self.mirror(change);
        }
    }

    pub(super) fn collect_chain_change(
        &mut self,
        what: What,
        module: Option<&str>,
        cable: Option<u32>,
    ) -> Result<Outcome, WallError> {
        let batch = self
            .chain_batch
            .as_mut()
            .ok_or_else(|| error(ErrorCode::Internal, "chain batch was not started"))?;
        batch.push(what);
        Ok(Outcome {
            result: serde_json::json!({"module": module, "cable": cable}),
            change: None,
            event: None,
            pending: None,
        })
    }

    pub(super) fn chains(&self) -> Result<Outcome, WallError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| error(ErrorCode::Internal, "this wall has no chain storage"))?;
        let names = store
            .list_chains()
            .map_err(|err| error(ErrorCode::Internal, format!("could not list chains: {err}")))?;
        Ok(Outcome {
            result: encode(&ChainsResult { names })?,
            change: None,
            event: None,
            pending: None,
        })
    }

    pub(super) fn chain_save(
        &self,
        name: &str,
        modules: &[String],
        console: bool,
    ) -> Result<Outcome, WallError> {
        let (chain, outside) = self.patch.chain(name, modules)?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| error(ErrorCode::Internal, "this wall has no chain storage"))?;
        store
            .write_chain(&chain, console)
            .map_err(|err| chain_error(&err))?;
        let names = store.list_chains().map_err(|err| {
            error(
                ErrorCode::Internal,
                format!("chain saved, but could not list chains: {err}"),
            )
        })?;
        Ok(Outcome {
            result: encode(&ChainSaveResult {
                name: chain.name,
                modules: chain.modules.len(),
                cables: chain.cables.len(),
                outside,
            })?,
            change: None,
            event: Some(Event::Chains { names }),
            pending: None,
        })
    }

    pub(super) fn chain_add(
        &mut self,
        seat: &str,
        name: &str,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        let chain = self
            .store
            .as_ref()
            .ok_or_else(|| error(ErrorCode::Internal, "this wall has no chain storage"))?
            .read_chain(name)
            .map_err(|err| chain_error(&err))?;
        let requests = chain.requests(&self.patch)?;
        self.room_to_grow(Growth {
            modules: chain.modules.len(),
            cables: chain.cables.len(),
            words: 0,
        })?;
        let before = self.patch.clone();
        self.chain_batch = Some(Vec::with_capacity(requests.len()));
        let applied = self.apply_chain_requests(seat, &chain, &requests, now);
        let batch = self
            .chain_batch
            .take()
            .ok_or_else(|| error(ErrorCode::Internal, "chain batch was not started"))?;
        if let Err(err) = applied {
            self.patch = before;
            return Err(err);
        }
        self.changed(
            seat,
            What::Chain {
                name: chain.name,
                changes: batch,
            },
            None,
            None,
            None,
        )
    }

    fn apply_chain_requests(
        &mut self,
        seat: &str,
        chain: &crate::patch::Chain,
        requests: &[Request],
        now: Instant,
    ) -> Result<(), WallError> {
        for (index, request) in requests.iter().enumerate() {
            let outcome = self.answer((seat, true), request, now)?;
            if matches!(request, Request::Add { .. }) {
                let id = outcome.result["module"]
                    .as_str()
                    .ok_or_else(|| error(ErrorCode::Internal, "chain add returned no module"))?;
                self.patch.put_knobs_back(id, &chain.modules[index].knobs);
                let record = self.patch.find(id)?.record();
                let batch = self
                    .chain_batch
                    .as_mut()
                    .ok_or_else(|| error(ErrorCode::Internal, "chain batch disappeared"))?;
                let last = batch
                    .last_mut()
                    .ok_or_else(|| error(ErrorCode::Internal, "chain add returned no change"))?;
                *last = What::Add { module: record };
            }
        }
        Ok(())
    }
}
