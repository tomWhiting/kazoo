use super::{BTreeSet, Command, Wall};
use crate::MAX_INPUTS;
use crate::catalogue::Jack;
use crate::engine::Line;

impl Wall {
    pub(super) fn place_lines(&mut self) {
        let present: BTreeSet<u32> = self.patch.cables().iter().map(|cable| cable.id).collect();
        let gone: Vec<u32> = self
            .sent_cables
            .keys()
            .filter(|id| !present.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some((slot, jack)) = self.sent_cables.remove(&id)
                && self.lined.remove(&id).is_some()
            {
                self.engine.send(Command::Unline { slot, jack });
            }
        }
        let rate = self.engine.sample_rate() as f32;
        for cable in self.patch.cables() {
            if self.lined.contains_key(&cable.id)
                || self.timing.delays.get(&cable.id).copied().unwrap_or(0) == 0
            {
                continue;
            }
            let Some(slot) = self.rack.slot_of(&cable.to_module) else {
                continue;
            };
            let jack = match cable.to_jack {
                Jack::Input(index) => index,
                Jack::Knob(index) => MAX_INPUTS + index,
            };
            let Some(line) = Line::new(rate) else {
                eprintln!(
                    "kazoo-wall: no memory for cable {} delay; it plays uncompensated",
                    cable.id
                );
                if !self.timing.uncompensated.contains(&cable.id) {
                    self.timing.uncompensated.push(cable.id);
                }
                continue;
            };
            self.line_tags = self.line_tags.wrapping_add(1).max(1);
            self.engine.send(Command::Line {
                slot,
                jack,
                line: Box::new(line),
            });
            self.lined.insert(cable.id, self.line_tags);
        }
    }
}
