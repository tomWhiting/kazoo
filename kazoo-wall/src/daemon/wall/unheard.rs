//! Keeping only modules whose cables lead to an output or a measurement.

use super::{Order, Wall, error};
use crate::catalogue::Kind;
use crate::engine::tables::{heard, processing_order};
use crate::probe;
use crate::protocol::{ErrorCode, WallError};
use std::collections::HashSet;

impl Wall {
    pub(super) fn heard_order(
        &mut self,
        slots: &[usize],
        edges: &[(usize, usize)],
        outs: &[usize],
    ) -> Order {
        let sinks: Vec<usize> = self
            .patch
            .modules()
            .iter()
            .filter(|module| module.kind == Kind::OUT || probe::measures(module.kind))
            .filter_map(|module| self.rack.slot_of(&module.id))
            .collect();
        let kept = heard(slots, edges, &sinks);
        let heard: HashSet<usize> = kept.iter().copied().collect();
        self.unheard = slots
            .iter()
            .filter(|slot| !heard.contains(slot))
            .filter_map(|slot| self.rack.id_at(*slot).map(str::to_string))
            .collect();
        processing_order(&kept, edges, outs)
    }

    /// A speaker's player refusing words (`err`), said plainly when the
    /// speaker is unheard: it is out of the render order, so it takes no
    /// phrase until a cable path leads from it to an out, a scope or a
    /// readout, and waiting never helps.
    pub(super) fn unheard_refusal(&self, module: &str, err: WallError) -> WallError {
        if err.code == ErrorCode::SlowDown && self.unheard.iter().any(|id| id == module) {
            error(
                ErrorCode::NotAllowed,
                format!(
                    "{module} reaches no out, scope or readout, so it is not rendered and has \
                     taken none of the words given it since; patch it toward one and it takes \
                     words again"
                ),
            )
        } else {
            err
        }
    }
}
