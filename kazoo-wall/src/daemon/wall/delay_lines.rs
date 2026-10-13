use super::{BTreeMap, BTreeSet, CableTable, Command, Route, Wall};
use crate::engine::Line;

impl Wall {
    pub(super) fn place_lines(
        &mut self,
        routed: &BTreeMap<u32, (usize, usize)>,
        heard_before: &BTreeSet<usize>,
    ) {
        let gone: Vec<u32> = self
            .line_places
            .iter()
            .filter(|&(id, destination)| routed.get(id) != Some(destination))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some((slot, jack)) = self.line_places.remove(&id) {
                self.lined.remove(&id);
                self.engine.send(Command::Unline { slot, jack });
            }
        }
        let rate = self.engine.sample_rate() as f32;
        #[cfg(test)]
        let allocate = self.line_allocator;
        #[cfg(not(test))]
        let allocate = make_line;
        for (&id, &(slot, jack)) in routed {
            if self.lined.contains_key(&id)
                || self.timing.delays.get(&id).copied().unwrap_or(0) == 0
            {
                continue;
            }
            let joining =
                self.sent_cables.get(&id) == Some(&(slot, jack)) && heard_before.contains(&slot);
            let tag = self.line_tags.wrapping_add(1).max(1);
            let Some(line) = allocate(rate, tag, joining) else {
                eprintln!("kazoo-wall: no memory for cable {id} delay; it plays uncompensated");
                self.timing.delays.remove(&id);
                if !self.timing.uncompensated.contains(&id) {
                    self.timing.uncompensated.push(id);
                }
                continue;
            };
            self.line_tags = tag;
            self.engine.send(Command::Line {
                slot,
                jack,
                line: Box::new(line),
            });
            self.lined.insert(id, tag);
            self.line_places.insert(id, (slot, jack));
        }
    }

    pub(super) fn apply_line_delays(
        &self,
        table: &mut CableTable,
        routed: &BTreeMap<u32, (usize, usize)>,
    ) {
        for (&id, &(slot, jack)) in routed {
            let Some(route) = table.route(slot, jack) else {
                eprintln!("kazoo-wall: cable {id} is missing from the validated table");
                continue;
            };
            // The endpoints and amount were accepted already, so changing
            // only the tag and bounded delay cannot grow or invalidate it.
            let route = Route {
                tag: self.lined.get(&id).copied().unwrap_or(0),
                delay: self.timing.delays.get(&id).copied().unwrap_or(0),
                ..route
            };
            if !table.set(slot, jack, Some(route)) {
                eprintln!("kazoo-wall: cable {id} could not update its delay in the table");
            }
        }
    }
}

pub(super) fn make_line(rate: f32, tag: u32, joining: bool) -> Option<Line> {
    if joining {
        Line::joining(rate, tag)
    } else {
        Line::new(rate)
    }
}
