//! Keeping only modules whose cables lead to an output or a measurement.

use super::{Order, Wall};
use crate::catalogue::Kind;
use crate::engine::tables::{heard, processing_order};
use crate::probe;
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
}
