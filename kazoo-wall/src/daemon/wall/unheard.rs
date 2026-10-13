//! Keeping only modules whose cables lead to an output, a measurement or
//! a loop.

use super::{Order, Wall, error};
use crate::catalogue::Kind;
use crate::engine::tables::{heard, processing_order};
use crate::probe;
use crate::protocol::{ErrorCode, WallError};
use std::collections::HashSet;

/// An output, a measurement or a loop hears every module that feeds it.
/// A loop records whether or not anything listens to it yet: arming a
/// loop and patching its output later is the usual order.
pub fn is_sink(kind: Kind) -> bool {
    kind == Kind::OUT || probe::measures(kind) || kind == Kind::LOOP
}

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
            .filter(|module| is_sink(module.kind))
            .filter_map(|module| self.rack.slot_of(&module.id))
            .collect();
        let kept = heard(slots, edges, &sinks);
        let heard: HashSet<usize> = kept.iter().copied().collect();
        self.heard_units = kept.len() + edges.iter().filter(|(_, to)| heard.contains(to)).count();
        let unheard: Vec<String> = slots
            .iter()
            .filter(|slot| !heard.contains(slot))
            .filter_map(|slot| self.rack.id_at(*slot).map(str::to_string))
            .collect();
        let was = std::mem::replace(&mut self.unheard, unheard);
        self.renew_lines_into_heard_again(&was);
        processing_order(&kept, edges, outs)
    }

    /// Give every cable into a module heard again (in `was`, the modules
    /// unheard before, and not unheard now) a fresh tag on its delay line.
    /// Its line was not written while the module was out of the order, so
    /// its history is from before; the engine empties a line whose tag is
    /// new, and the cable starts as a new cable does, silent for its delay,
    /// never replaying what it held (a stale gate there would strike).
    fn renew_lines_into_heard_again(&mut self, was: &[String]) {
        if was.is_empty() {
            return;
        }
        let still: HashSet<&str> = self.unheard.iter().map(String::as_str).collect();
        let again: HashSet<&str> = was
            .iter()
            .map(String::as_str)
            .filter(|id| !still.contains(id))
            .collect();
        if again.is_empty() {
            return;
        }
        for cable in self.patch.cables() {
            if !again.contains(cable.to_module.as_str()) {
                continue;
            }
            if let Some((_, tag)) = self.lines.get_mut(&cable.id) {
                self.line_tags = self.line_tags.wrapping_add(1).max(1);
                *tag = self.line_tags;
            }
        }
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
                    "{module} reaches no out, scope, readout or loop, so it is not rendered and has \
                     taken none of the words given it since; patch it toward one and it takes \
                     words again"
                ),
            )
        } else {
            err
        }
    }
}
