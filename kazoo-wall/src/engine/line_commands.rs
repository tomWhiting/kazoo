use super::{Engine, Line, MAX_JACKS, Retired};
use std::sync::atomic::Ordering;

impl Engine {
    pub(super) fn reset_changed_taps(&mut self, old: &super::CableTable) {
        let source = |route: Option<super::Route>| route.map(|route| (route.slot, route.port));
        for slot in 0..old.slots().max(self.cables.slots()) {
            let Some((page, at)) = self.room.page_mut(slot) else {
                continue;
            };
            for jack in 0..MAX_JACKS {
                if source(old.route(slot, jack)) != source(self.cables.route(slot, jack)) {
                    page.taps[at * MAX_JACKS + jack] = super::lines::Tap::EMPTY;
                }
            }
        }
    }

    pub(super) fn place_line(&mut self, slot: usize, jack: usize, line: Box<Line>) {
        let Some((page, at)) = self.room.page_mut(slot).filter(|_| jack < MAX_JACKS) else {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            self.retire(Retired::Line(line));
            return;
        };
        if let Some(old) = page.lines[at * MAX_JACKS + jack].replace(line) {
            self.retire(Retired::Line(old));
        } else {
            self.shared.lines.fetch_add(1, Ordering::Release);
        }
    }

    pub(super) fn unline(&mut self, slot: usize, jack: usize) {
        let Some((page, at)) = self.room.page_mut(slot).filter(|_| jack < MAX_JACKS) else {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Some(line) = page.lines[at * MAX_JACKS + jack].take() {
            self.shared.lines.fetch_sub(1, Ordering::Release);
            self.retire(Retired::Line(line));
        }
    }
}
