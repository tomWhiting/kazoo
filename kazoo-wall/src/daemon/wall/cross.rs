//! Control-side plans keep processors and cable histories alive until silent.

mod fades;
#[cfg(test)]
mod tests;

pub(super) use fades::Fades;
use fades::{Deadline, Leaving};

use super::{
    Adapter, Builder, Cable, Command, HashMap, HashSet, Jack, Kind, MAX_INPUTS, Module, Patch,
    Route, Wall, carries_gate,
};
use crate::protocol::MAX_CROSS_BEATS;

/// A callback may take the table one buffer after it was sent.
const CROSS_MARGIN_SECONDS: f64 = 0.25;

#[derive(Debug)]
pub(super) struct CrossTiming {
    pub knobs: u32,
    pub knob: HashMap<(String, usize), u32>,
    pub fades: u32,
    pub tempo: u32,
}

impl CrossTiming {
    pub(super) fn go(frames: u32) -> Self {
        Self {
            knobs: frames,
            knob: HashMap::new(),
            fades: frames,
            tempo: frames,
        }
    }
}

#[derive(Debug)]
struct KnobMove {
    module: String,
    slot: usize,
    knob: usize,
    from: f32,
    to: f32,
    frames: u32,
}

#[derive(Debug)]
pub(super) struct CrossPlan {
    timing: CrossTiming,
    matched: Vec<(String, String, usize)>,
    born: Vec<String>,
    leaving: Vec<(String, usize, Kind)>,
    knobs: Vec<KnobMove>,
    matched_cables: Vec<(u32, u32, u32)>,
    cables: Vec<Leaving>,
    entering: Vec<(u32, u32)>,
    waiting: Vec<(u32, u64, u32)>,
    tempo: Option<(f64, u32)>,
    margin: u64,
    cable_count: usize,
}

fn speaks(module: &Module) -> bool {
    matches!(module.kind.spec().build, Builder::Adapted(Adapter::Speak))
}

const fn jack_index(jack: Jack) -> usize {
    match jack {
        Jack::Input(index) => index,
        Jack::Knob(index) => MAX_INPUTS + index,
    }
}

pub(super) fn cross_or_cut(
    asked: f64,
    room: bool,
    guard_on: bool,
    projected: f32,
) -> Option<&'static str> {
    if !asked.is_finite() || asked <= 0.0 {
        Some("cut")
    } else if !room {
        Some("memory")
    } else if guard_on && (!projected.is_finite() || projected > super::LOAD_GUARD) {
        Some("load")
    } else {
        None
    }
}

impl Wall {
    pub(super) fn plan_crossing(&self, incoming: &Patch, timing: CrossTiming) -> CrossPlan {
        let margin = (CROSS_MARGIN_SECONDS * f64::from(self.engine.sample_rate())).ceil() as u64;
        let mut plan = CrossPlan {
            timing,
            matched: Vec::new(),
            born: Vec::new(),
            leaving: Vec::new(),
            knobs: Vec::new(),
            matched_cables: Vec::new(),
            cables: Vec::new(),
            entering: Vec::new(),
            waiting: Vec::new(),
            tempo: None,
            margin,
            cable_count: 0,
        };
        self.match_modules(incoming, &mut plan);
        let kinds = self
            .patch
            .modules()
            .iter()
            .map(|module| (module.id.as_str(), module.kind))
            .collect();
        let mut taken = HashSet::new();
        for old in self.patch.cables() {
            let destination = plan
                .matched
                .iter()
                .find(|(id, _, _)| id == &old.to_module)
                .map(|(_, id, _)| id);
            let new = destination.and_then(|id| {
                incoming
                    .cables()
                    .iter()
                    .find(|new| &new.to_module == id && new.to_jack == old.to_jack)
            });
            let same = new.is_some_and(|new| {
                new.from_port == old.from_port
                    && plan
                        .matched
                        .iter()
                        .any(|(from, to, _)| from == &old.from_module && to == &new.from_module)
            });
            if let Some(new) = new.filter(|_| same) {
                taken.insert(new.id);
                plan.matched_cables
                    .push((old.id, new.id, plan.timing.fades));
                plan.cable_count += usize::from(old.amount.to_bits() != new.amount.to_bits());
                continue;
            }
            if !self.fades.waits(old.id) && plan.timing.fades > 0 {
                let frames = if new.is_some() {
                    plan.timing.fades / 2
                } else {
                    plan.timing.fades
                };
                if let Some(route) = self.leaving_route(old, frames, margin, &kinds) {
                    plan.cables.push(route);
                    plan.cable_count += 1;
                }
            }
            if let Some(new) = new {
                taken.insert(new.id);
                let after = u64::from(plan.timing.fades / 2) + margin;
                if plan.timing.fades > 0 && !self.fades.waits(old.id) {
                    plan.waiting.push((new.id, after, plan.timing.fades / 2));
                } else {
                    plan.entering.push((new.id, plan.timing.fades));
                }
                plan.cable_count += 1;
            }
        }
        for new in incoming
            .cables()
            .iter()
            .filter(|new| !taken.contains(&new.id))
        {
            plan.entering.push((new.id, plan.timing.fades));
            plan.cable_count += 1;
        }
        if !self.on_desk() && incoming.tempo.to_bits() != self.engine.shared().bpm().to_bits() {
            plan.tempo = Some((incoming.tempo, plan.timing.tempo));
        }
        plan
    }

    fn match_modules(&self, incoming: &Patch, plan: &mut CrossPlan) {
        for new in incoming.modules() {
            let old = self.patch.modules().iter().find(|old| {
                old.birth.is_some()
                    && old.birth == new.birth
                    && old.kind == new.kind
                    && (!speaks(old)
                        || (old.id == new.id
                            && self.patch.speech.get(&old.id) == incoming.speech.get(&new.id)))
            });
            if let Some((old, slot)) =
                old.and_then(|old| self.rack.slot_of(&old.id).map(|slot| (old, slot)))
            {
                plan.matched.push((old.id.clone(), new.id.clone(), slot));
                for (knob, (&from, &to)) in old.knobs.iter().zip(&new.knobs).enumerate() {
                    if from.to_bits() != to.to_bits() {
                        let frames = plan
                            .timing
                            .knob
                            .get(&(new.id.clone(), knob))
                            .copied()
                            .unwrap_or(plan.timing.knobs);
                        plan.knobs.push(KnobMove {
                            module: new.id.clone(),
                            slot,
                            knob,
                            from,
                            to,
                            frames,
                        });
                    }
                }
            } else {
                plan.born.push(new.id.clone());
            }
        }
        for old in self.patch.modules() {
            if !plan.matched.iter().any(|(id, _, _)| id == &old.id) {
                if let Some(slot) = self.rack.slot_of(&old.id) {
                    plan.leaving.push((old.id.clone(), slot, old.kind));
                }
            }
        }
    }

    fn leaving_route(
        &self,
        cable: &Cable,
        frames: u32,
        margin: u64,
        kinds: &HashMap<&str, Kind>,
    ) -> Option<Leaving> {
        let from_slot = self.rack.slot_of(&cable.from_module)?;
        let to_slot = self.rack.slot_of(&cable.to_module)?;
        let slot = match u32::try_from(from_slot) {
            Ok(slot) => slot,
            Err(error) => {
                eprintln!("kazoo-wall: crossing source slot is out of range: {error}");
                return None;
            }
        };
        let port = match u8::try_from(cable.from_port) {
            Ok(port) => port,
            Err(error) => {
                eprintln!("kazoo-wall: crossing port is out of range: {error}");
                return None;
            }
        };
        Some(Leaving {
            to_slot,
            jack: jack_index(cable.to_jack),
            from_slot,
            route: Route {
                amount: 0.0,
                glide: frames,
                delay: self.timing.delays.get(&cable.id).copied().unwrap_or(0),
                gate: carries_gate(kinds, cable),
                ..Route::new(
                    slot,
                    port,
                    0.0,
                    self.lined
                        .get(&cable.id)
                        .or_else(|| self.cable_tags.get(&cable.id))
                        .copied()
                        .unwrap_or(0),
                )
            },
            gone_at: Deadline::at(u64::from(frames) + margin),
        })
    }

    pub(super) fn cross_arrival_note(&self, beats: f64, notes: &mut Vec<String>) {
        if beats.is_finite() {
            notes.push(format!(
                "lands on bar {:.0}",
                self.engine.landing_beat() / 4.0 + 1.0
            ));
        }
    }

    pub(super) fn crossing_load(&self, plan: &CrossPlan, incoming: &Patch) -> f32 {
        let busy = self.busy();
        self.engine.shared().render_load()
            + plan
                .born
                .iter()
                .filter_map(|id| incoming.module(id))
                .map(|module| self.kind_share_now(module.kind, busy))
                .sum::<f32>()
    }

    pub(super) fn crossing_beats(
        &mut self,
        incoming: &Patch,
        asked: f64,
        notes: &mut Vec<String>,
    ) -> (f64, Option<Box<CrossPlan>>) {
        if !asked.is_finite() || asked <= 0.0 {
            return (0.0, None);
        }
        let asked = asked.min(MAX_CROSS_BEATS);
        let bpm = self.engine.shared().bpm();
        // max turns NaN into one frame; the cast is bounded to u32.
        let frames = (asked * 60.0 / bpm * f64::from(self.engine.sample_rate()))
            .max(1.0)
            .min(f64::from(u32::MAX)) as u32;
        let plan = self.plan_crossing(incoming, CrossTiming::go(frames));
        let projected = self.crossing_load(&plan, incoming);
        let slots = self.rack.slots_for(plan.born.len());
        let table = crate::engine::CableTable::with_slots(slots);
        let room = table.is_some() && self.engine.reserve_slots(slots);
        match cross_or_cut(asked, room, self.load_watch.on, projected) {
            Some("memory") => {
                notes.push("the machine has no memory to hold both walls at once, so the wall cut to the moment instead of crossing".into());
                (0.0, None)
            }
            Some("load") => {
                notes.push(format!("both walls at once would take the wall to about {:.0}% of its time rendering, past the {:.0}% it keeps to, so it cut to the moment instead of crossing", projected * 100.0, super::LOAD_GUARD * 100.0));
                (0.0, None)
            }
            Some(_) => (0.0, None),
            None => {
                self.spare_table = table;
                notes.push(format!("the wall crosses to it over {} ({:.2} s at {}): {} modules glide, {} fade in, {} fade out, {} cables crossfade", crate::format::beats(asked), f64::from(frames) / f64::from(self.engine.sample_rate()), crate::format::bpm(bpm), plan.matched.len(), plan.born.len(), plan.leaving.len(), plan.cable_count));
                (asked, Some(Box::new(plan)))
            }
        }
    }

    pub(super) fn cross(&mut self, mut plan: CrossPlan) {
        self.fades.resolve(self.engine.shared());
        let held = plan.timing.fades > 0;
        let start = if held {
            0
        } else {
            self.engine.shared().frames()
        };
        let serial = if held {
            self.engine.next_landing_serial()
        } else {
            0
        };
        if held {
            self.engine.begin_swap();
            self.restart_load_watch();
            self.crossings_sent += 1;
        }
        for (id, _, kind) in &plan.leaving {
            if matches!(kind.spec().build, Builder::Adapted(Adapter::Speak)) {
                self.speech.forget(id);
            }
            if *kind == Kind::LOOP {
                self.loops.forget(id);
            }
            self.rack.leave(id);
        }
        self.rack.reindex(&plan.matched);
        self.reserve_for(plan.born.len());
        for id in &plan.born {
            if let Some(module) = self.patch.module(id).cloned() {
                if self.insert(&module) {
                    self.put_back_words(id);
                }
            }
        }
        self.keep_crossing(&plan, start, serial);
        self.begin_fades(&mut plan, start, serial);
        self.fades.refresh_end();
        self.send_tables();
        for knob in plan.knobs {
            self.engine.send(Command::Knob {
                slot: knob.slot,
                knob: knob.knob,
                target: knob.to,
                glide_frames: knob.frames,
            });
            self.note_turned(
                &knob.module,
                knob.knob,
                f64::from(knob.from),
                f64::from(knob.to),
                f64::from(knob.frames),
            );
        }
        let settled_from = std::time::Instant::now();
        for (module, knob, settles) in std::mem::take(&mut self.turned) {
            self.settling.insert((module, knob), settled_from + settles);
        }
        if let Some((bpm, frames)) = plan.tempo {
            self.engine.send(Command::TempoGlide { bpm, frames });
        }
        if held {
            self.engine.end_cross();
        }
        if plan.timing.fades == 0 {
            for (_, slot, _) in plan.leaving {
                self.engine.send(Command::Remove { slot });
                self.rack.gone(slot);
            }
        }
    }

    fn begin_fades(&mut self, plan: &mut CrossPlan, start: u64, serial: u64) {
        if plan.timing.fades > 0 {
            for mut cable in std::mem::take(&mut plan.cables) {
                cable.gone_at = Deadline::held(start + cable.gone_at.frame(), serial);
                self.fades.cables.push(cable);
            }
            for &(id, after, glide) in &plan.waiting {
                let destination = self
                    .patch
                    .cables()
                    .iter()
                    .find(|cable| cable.id == id)
                    .and_then(|cable| {
                        self.rack
                            .slot_of(&cable.to_module)
                            .map(|slot| (slot, jack_index(cable.to_jack)))
                    });
                let held = destination
                    .and_then(|(slot, jack)| {
                        self.fades
                            .cables
                            .iter()
                            .filter(|cable| cable.to_slot == slot && cable.jack == jack)
                            .map(|cable| cable.gone_at)
                            .reduce(Deadline::later)
                    })
                    .unwrap_or_else(|| Deadline::held(start + after, serial));
                self.fades.waiting.push((
                    id,
                    held.later(Deadline::held(start + after, serial)),
                    glide,
                ));
            }
            for &(id, glide) in &plan.entering {
                let destination = self
                    .patch
                    .cables()
                    .iter()
                    .find(|cable| cable.id == id)
                    .and_then(|cable| {
                        self.rack
                            .slot_of(&cable.to_module)
                            .map(|slot| (slot, jack_index(cable.to_jack)))
                    });
                let held = destination.and_then(|(slot, jack)| {
                    self.fades
                        .cables
                        .iter()
                        .filter(|cable| cable.to_slot == slot && cable.jack == jack)
                        .map(|cable| cable.gone_at)
                        .reduce(Deadline::later)
                });
                if let Some(at) = held {
                    self.fades
                        .waiting
                        .push((id, at.later(Deadline::held(start, serial)), glide));
                } else {
                    self.fades.glides.insert(
                        id,
                        (
                            glide,
                            Deadline::held(start + u64::from(glide) + plan.margin, serial),
                        ),
                    );
                }
            }
            for &(_, slot, kind) in &plan.leaving {
                let at = self
                    .fades
                    .cables
                    .iter()
                    .filter(|cable| cable.from_slot == slot || cable.to_slot == slot)
                    .map(|cable| cable.gone_at)
                    .reduce(Deadline::later)
                    .unwrap_or_else(|| Deadline::held(start + plan.margin, serial));
                self.fades.modules.push((slot, kind, at));
            }
        } else {
            self.fades = Fades::default();
        }
    }

    fn keep_crossing(&mut self, plan: &CrossPlan, start: u64, serial: u64) {
        let same: HashSet<&str> = plan
            .matched
            .iter()
            .filter(|(old, new, _)| old == new)
            .map(|(_, id, _)| id.as_str())
            .collect();
        self.readings.retain(|id, _| same.contains(id.as_str()));
        self.wiring.retain(|id, _| same.contains(id.as_str()));
        self.fresh_from.retain(|id, _| same.contains(id.as_str()));
        self.settled.retain(|id, _| same.contains(id.as_str()));
        self.settling
            .retain(|(id, _), _| same.contains(id.as_str()));
        self.faults
            .unreported
            .retain(|id, _| same.contains(id.as_str()));
        self.faults
            .last_report
            .retain(|id, _| same.contains(id.as_str()));
        let old_lined = std::mem::take(&mut self.lined);
        let old_tags = std::mem::take(&mut self.cable_tags);
        let old_places = std::mem::take(&mut self.line_places);
        let old_sent = std::mem::take(&mut self.sent_cables);
        let old_waiting = std::mem::take(&mut self.fades.waiting);
        self.fades.glides.clear();
        for &(old, new, glide) in &plan.matched_cables {
            self.cable_tags
                .insert(new, old_tags.get(&old).copied().unwrap_or(0));
            if let Some(&tag) = old_lined.get(&old) {
                self.lined.insert(new, tag);
            }
            if let Some(&place) = old_places.get(&old) {
                self.line_places.insert(new, place);
            }
            if let Some(&place) = old_sent.get(&old) {
                self.sent_cables.insert(new, place);
            }
            if let Some(&(_, at, frames)) = old_waiting.iter().find(|&&(id, _, _)| id == old) {
                self.fades.waiting.push((new, at, frames));
            } else if glide > 0 {
                self.fades.glides.insert(
                    new,
                    (
                        glide,
                        Deadline::held(start + u64::from(glide) + plan.margin, serial),
                    ),
                );
            }
        }
    }

    pub(super) fn cable_tag(&self, id: u32) -> u32 {
        self.lined
            .get(&id)
            .or_else(|| self.cable_tags.get(&id))
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn heard_before(&self) -> super::BTreeSet<usize> {
        self.rack
            .index
            .iter()
            .filter(|(id, _)| !self.unheard.contains(id))
            .map(|(_, slot)| *slot)
            .collect()
    }

    pub(super) fn overlay_fades(&self, table: &mut crate::engine::CableTable) {
        for cable in &self.fades.cables {
            if table.route(cable.to_slot, cable.jack).is_none()
                && !table.set(cable.to_slot, cable.jack, Some(cable.route))
            {
                eprintln!("kazoo-wall: leaving cable could not fit in the crossing table");
            }
        }
    }

    pub(super) fn assign_lines(&mut self) {
        let active: HashSet<u32> = self.patch.cables().iter().map(|cable| cable.id).collect();
        self.cable_tags.retain(|id, _| active.contains(id));
        let reserved: HashSet<u32> = self.fades.lines().collect();
        for cable in self.patch.cables() {
            if !self.cable_tags.contains_key(&cable.id)
                && !self.fades.waits(cable.id)
                && self.fades.glide_of(cable.id) > 0
            {
                loop {
                    self.line_tags = self.line_tags.wrapping_add(1).max(1);
                    if !reserved.contains(&self.line_tags) {
                        break;
                    }
                }
                self.cable_tags.insert(cable.id, self.line_tags);
            }
        }
    }

    pub(super) fn cross_tick(&mut self) {
        if self.fades.is_empty() {
            return;
        }
        self.fades.resolve(self.engine.shared());
        let now = self.engine.shared().frames();
        let before = (
            self.fades.cables.len(),
            self.fades.waiting.len(),
            self.fades.glides.len(),
            self.fades.modules.len(),
        );
        let expired: Vec<(usize, usize)> = self
            .fades
            .cables
            .iter()
            .filter(|cable| cable.gone_at.due(now))
            .map(|cable| (cable.to_slot, cable.jack))
            .collect();
        self.fades.cables.retain(|cable| !cable.gone_at.due(now));
        let ids: HashSet<u32> = self.patch.cables().iter().map(|cable| cable.id).collect();
        let waiting = std::mem::take(&mut self.fades.waiting);
        for (id, at, frames) in waiting {
            if !ids.contains(&id) {
                continue;
            }
            if at.due(now) {
                self.fades.glides.insert(
                    id,
                    (
                        frames,
                        Deadline::at(
                            now + u64::from(frames)
                                + (CROSS_MARGIN_SECONDS * f64::from(self.engine.sample_rate()))
                                    .ceil() as u64,
                        ),
                    ),
                );
            } else {
                self.fades.waiting.push((id, at, frames));
            }
        }
        self.fades
            .glides
            .retain(|id, (_, at)| ids.contains(id) && !at.due(now));
        let mut due = Vec::new();
        self.fades.modules.retain(|&(slot, _, at)| {
            if at.due(now)
                && !self
                    .fades
                    .cables
                    .iter()
                    .any(|cable| cable.from_slot == slot || cable.to_slot == slot)
            {
                due.push(slot);
                false
            } else {
                true
            }
        });
        let after = (
            self.fades.cables.len(),
            self.fades.waiting.len(),
            self.fades.glides.len(),
            self.fades.modules.len(),
        );
        if before != after || !due.is_empty() {
            self.send_tables();
            for (slot, jack) in expired {
                if !self
                    .line_places
                    .values()
                    .any(|&destination| destination == (slot, jack))
                {
                    self.engine.send(Command::Unline { slot, jack });
                }
            }
            for slot in due {
                self.engine.send(Command::Remove { slot });
                self.rack.gone(slot);
            }
        }
        self.fades.refresh_end();
        if self.fades.is_empty() {
            self.restart_load_watch();
            self.retime_now();
        }
    }
}
