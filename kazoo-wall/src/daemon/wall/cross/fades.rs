use super::{HashMap, Kind, Route};
use crate::engine::Shared;

/// Pending offsets become absolute only after the engine acknowledges its bar.
#[derive(Debug, Clone, Copy)]
pub(super) struct Deadline {
    at: u64,
    landing: u64,
    floor: u64,
}

impl Deadline {
    pub(super) const fn at(at: u64) -> Self {
        Self::held(at, 0)
    }
    pub(super) const fn held(at: u64, landing: u64) -> Self {
        Self {
            at,
            landing,
            floor: 0,
        }
    }
    pub(super) const fn frame(self) -> u64 {
        self.at
    }
    pub(super) const fn due(self, now: u64) -> bool {
        self.landing == 0 && self.at <= now
    }
    pub(super) fn later(mut self, other: Self) -> Self {
        if self.landing == 0 && other.landing != 0 {
            return other.later(self);
        }
        if self.landing != 0 && other.landing == 0 {
            self.floor = self.floor.max(other.at);
        } else {
            self.at = self.at.max(other.at);
            self.floor = self.floor.max(other.floor);
        }
        self
    }
    fn resolve(&mut self, serial: u64, frame: u64) {
        if self.landing != 0 && self.landing <= serial {
            self.at = self.at.saturating_add(frame).max(self.floor);
            self.landing = 0;
        }
    }
}

#[derive(Debug, Clone)]
pub(in super::super) struct Leaving {
    pub(super) to_slot: usize,
    pub(super) jack: usize,
    pub(super) from_slot: usize,
    pub(super) route: Route,
    pub(super) gone_at: Deadline,
}

#[derive(Debug, Default)]
pub(in super::super) struct Fades {
    pub(super) cables: Vec<Leaving>,
    pub(super) modules: Vec<(usize, Kind, Deadline)>,
    pub(super) waiting: Vec<(u32, Deadline, u32)>,
    pub(super) glides: HashMap<u32, (u32, Deadline)>,
    end: u64,
}

impl Fades {
    pub(in super::super) fn glide_of(&self, cable: u32) -> u32 {
        self.glides.get(&cable).map_or(0, |&(frames, _)| frames)
    }
    pub(in super::super) fn waits(&self, cable: u32) -> bool {
        self.waiting.iter().any(|&(id, _, _)| id == cable)
    }
    pub(in super::super) fn lines(&self) -> impl Iterator<Item = u32> + '_ {
        self.cables.iter().map(|cable| cable.route.tag)
    }
    pub(in super::super) fn slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.modules.iter().map(|&(slot, _, _)| slot)
    }
    pub(in super::super) fn sinks(&self) -> impl Iterator<Item = usize> + '_ {
        self.modules
            .iter()
            .filter(|(_, kind, _)| super::super::unheard::is_sink(*kind))
            .map(|&(slot, _, _)| slot)
    }
    pub(in super::super) fn outs(&self) -> impl Iterator<Item = usize> + '_ {
        self.modules
            .iter()
            .filter(|(_, kind, _)| *kind == Kind::OUT)
            .map(|&(slot, _, _)| slot)
    }
    pub(in super::super) fn edges(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.cables
            .iter()
            .map(|cable| (cable.from_slot, cable.to_slot))
    }
    pub(in super::super) fn forget_slot(&mut self, slot: usize) {
        self.cables
            .retain(|cable| cable.from_slot != slot && cable.to_slot != slot);
    }
    pub(in super::super) fn is_empty(&self) -> bool {
        self.cables.is_empty()
            && self.modules.is_empty()
            && self.waiting.is_empty()
            && self.glides.is_empty()
    }
    pub(super) fn resolve(&mut self, shared: &Shared) {
        let serial = shared.unhushed();
        let frame = shared.landing_frame();
        for cable in &mut self.cables {
            cable.gone_at.resolve(serial, frame);
        }
        for (_, _, at) in &mut self.modules {
            at.resolve(serial, frame);
        }
        for (_, at, _) in &mut self.waiting {
            at.resolve(serial, frame);
        }
        for (_, at) in self.glides.values_mut() {
            at.resolve(serial, frame);
        }
    }
    pub(super) fn refresh_end(&mut self) {
        self.end = self
            .cables
            .iter()
            .map(|cable| cable.gone_at.frame())
            .chain(self.modules.iter().map(|&(_, _, at)| at.frame()))
            .chain(
                self.waiting
                    .iter()
                    .map(|&(_, at, frames)| at.frame().saturating_add(u64::from(frames))),
            )
            .chain(self.glides.values().map(|&(_, at)| at.frame()))
            .max()
            .unwrap_or(0);
    }
}
