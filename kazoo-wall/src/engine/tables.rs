//! The engine's routing tables: where each input reads from, and the order
//! modules render in. Both are built on the control side and swapped in
//! whole.
//!
//! Slots and delay lines are numbered in `u32`: the wall holds as many as
//! the machine can render, and the tables are as long as the wall is.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::dsp::Block;
use crate::{MAX_INPUTS, MAX_JACKS, MAX_KNOBS, MAX_OUTPUTS};

/// The longest delay a cable carries to keep its signal in step with the
/// others arriving at the same module, in frames (about 21 ms at 48 kHz).
pub const MAX_CABLE_DELAY: u16 = 1_024;

/// Where one input reads from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    /// The source module's slot.
    pub slot: u32,
    /// The source output.
    pub port: u8,
    /// The cable's attenuverter, -1 to 1.
    pub amount: f32,
    /// The cable's delay line: one per cable, kept for the cable's life so
    /// its history survives every table swap.
    pub line: u32,
    /// Who owns the line: a line that changes hands starts empty, as the
    /// new cable had nothing through it before it was plugged in.
    pub tag: u32,
    /// Frames the signal is held back so it arrives with the others (see
    /// [`crate::engine`]); at most [`MAX_CABLE_DELAY`].
    pub delay: u16,
    /// Whether the cable carries a gate: a change of delay then switches
    /// where the old and new signals agree instead of crossfading.
    pub gate: bool,
}

impl Route {
    /// A cable from `slot`'s output `port` at `amount`, on delay line
    /// `line` owned by `tag`, with no delay.
    #[must_use]
    pub const fn new(slot: u32, port: u8, amount: f32, line: u32, tag: u32) -> Self {
        Self {
            slot,
            port,
            amount,
            line,
            tag,
            delay: 0,
            gate: false,
        }
    }
}

/// For every slot and jack (see [`crate::MAX_JACKS`]), the cable plugged
/// into it; and for every slot's output, how many cables it feeds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CableTable {
    /// By slot, then jack: as long as the highest slot anything was
    /// plugged into.
    routes: Vec<[Option<Route>; MAX_JACKS]>,
    plugged: Vec<u32>,
    /// By slot, then output: the cables reading it. Kept by [`Self::set`];
    /// as long as the highest slot a cable came from.
    feeds: Vec<[u32; MAX_OUTPUTS]>,
    /// One past the highest delay line a cable plugged in has used.
    lines: usize,
}

/// Make room in `table` for it to be `len` long, growing it by doubling;
/// `false` when there is no memory for it (nothing changes then).
fn room_for<T>(table: &mut Vec<T>, len: usize) -> bool {
    // Amortised: `try_reserve` at least doubles the capacity.
    table.try_reserve(len.saturating_sub(table.len())).is_ok()
}

impl CableTable {
    /// A table with nothing plugged in. Plugging in allocates: build
    /// tables on the control side.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            routes: Vec::new(),
            plugged: Vec::new(),
            feeds: Vec::new(),
            lines: 0,
        }
    }

    /// A table with nothing plugged in and room for `slots` slots, or
    /// `None` when there is no memory for it (control side only). Build a
    /// wall's table this way, sized to its rack, so plugging in never has
    /// to grow it.
    #[must_use]
    pub fn with_slots(slots: usize) -> Option<Self> {
        let mut table = Self::new();
        let sized = table.routes.try_reserve_exact(slots).is_ok()
            && table.plugged.try_reserve_exact(slots).is_ok()
            && table.feeds.try_reserve_exact(slots).is_ok();
        if !sized {
            return None;
        }
        table.routes.resize(slots, [None; MAX_JACKS]);
        table.plugged.resize(slots, 0);
        table.feeds.resize(slots, [0; MAX_OUTPUTS]);
        Some(table)
    }

    /// Plug `slot`'s `jack` into `route`, or unplug it with `None`; amounts
    /// are held to -1..1. Returns whether the table now holds what was
    /// asked: `false` for a port or jack out of range, a non-finite amount,
    /// or a slot past the table there was no memory to reach (the table
    /// grows by doubling past its room; see [`Self::with_slots`]). A table
    /// that refuses is left as it was.
    #[must_use]
    pub fn set(&mut self, slot: usize, jack: usize, route: Option<Route>) -> bool {
        let valid = route
            .is_none_or(|route| usize::from(route.port) < MAX_OUTPUTS && route.amount.is_finite());
        if jack >= MAX_JACKS || !valid {
            return false;
        }
        let new = route.map(|route| Route {
            amount: route.amount.clamp(-1.0, 1.0),
            delay: route.delay.min(MAX_CABLE_DELAY),
            ..route
        });
        if let Some(new) = new {
            // Room for both before either grows, so a table that refuses is
            // left as it was. (u32 into usize: lossless.)
            let (routes, feeds) = (
                slot.saturating_add(1),
                (new.slot as usize).saturating_add(1),
            );
            if !(room_for(&mut self.routes, routes)
                && room_for(&mut self.plugged, routes)
                && room_for(&mut self.feeds, feeds))
            {
                return false;
            }
            if self.routes.len() < routes {
                self.routes.resize(routes, [None; MAX_JACKS]);
                self.plugged.resize(routes, 0);
            }
            if self.feeds.len() < feeds {
                self.feeds.resize(feeds, [0; MAX_OUTPUTS]);
            }
        }
        let Some(jacks) = self.routes.get_mut(slot) else {
            // Unplugging where nothing was ever plugged in: nothing to do.
            return true;
        };
        let old = std::mem::replace(&mut jacks[jack], new);
        if new.is_some() {
            self.plugged[slot] |= 1 << jack;
        } else {
            self.plugged[slot] &= !(1 << jack);
        }
        if let Some(old) = old {
            // u32 into usize: lossless; the old cable was counted when it
            // went in, so its source's count is there.
            if let Some(count) = self
                .feeds
                .get_mut(old.slot as usize)
                .and_then(|ports| ports.get_mut(usize::from(old.port)))
            {
                *count = count.saturating_sub(1);
            }
        }
        if let Some(new) = new {
            if let Some(count) = self
                .feeds
                .get_mut(new.slot as usize)
                .and_then(|ports| ports.get_mut(usize::from(new.port)))
            {
                *count = count.saturating_add(1);
            }
            self.lines = self.lines.max(new.line as usize + 1);
        }
        true
    }

    /// One bit per plugged jack in `slot`; zero past the table.
    #[must_use]
    pub fn plugged(&self, slot: usize) -> u32 {
        self.plugged.get(slot).copied().unwrap_or(0)
    }

    /// Whether `slot`'s output `port` feeds any cable.
    #[must_use]
    pub fn feeds(&self, slot: usize, port: usize) -> bool {
        self.feeds
            .get(slot)
            .and_then(|ports| ports.get(port))
            .is_some_and(|count| *count > 0)
    }

    /// Slots the table reaches.
    #[must_use]
    pub fn slots(&self) -> usize {
        self.routes.len()
    }

    /// The cable into `slot`'s `jack`.
    #[must_use]
    pub fn route(&self, slot: usize, jack: usize) -> Option<Route> {
        self.routes
            .get(slot)
            .and_then(|jacks| jacks.get(jack))
            .copied()
            .flatten()
    }

    /// Delay lines the engine needs to carry this table: one past the
    /// highest line a cable plugged in has used.
    #[must_use]
    pub const fn lines(&self) -> usize {
        self.lines
    }
}

/// The order modules render in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Order {
    slots: Box<[u32]>,
    outs: Box<[u32]>,
}

impl Order {
    /// Render `slots` in this order (this allocates: build orders on the
    /// control side). Repeats, and slots past `u32::MAX`, are left out.
    /// Keep only `outs` present in the order, in render order.
    #[must_use]
    pub fn new(slots: &[usize], outs: &[usize]) -> Self {
        let mut seen = HashSet::with_capacity(slots.len());
        let mut kept = Vec::with_capacity(slots.len());
        for &slot in slots {
            let Ok(slot) = u32::try_from(slot) else {
                continue;
            };
            if seen.insert(slot) {
                kept.push(slot);
            }
        }
        let outs: HashSet<usize> = outs.iter().copied().collect();
        let outs = kept
            .iter()
            .copied()
            .filter(|slot| outs.contains(&(*slot as usize)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots: kept.into_boxed_slice(),
            outs,
        }
    }

    /// The slots, in render order.
    #[must_use]
    pub const fn slots(&self) -> &[u32] {
        &self.slots
    }

    /// The out slots, in render order.
    #[must_use]
    pub const fn outs(&self) -> &[u32] {
        &self.outs
    }
}

/// The render order for modules in `slots` (preferred order: the order they
/// were added) joined by `edges` (source slot → destination slot).
///
/// Every module renders after the modules feeding it, so it hears their
/// output from this sub-block. Where cables form a cycle, the cycle is
/// broken at the module with the fewest unrendered sources (the earliest
/// added on a tie): the cables into it from later in the order are its
/// back-edges, and it hears them one sub-block late. Feedback patches are
/// therefore always legal.
///
/// Each module's unrendered sources are counted once and counted down as
/// its sources render, and the modules wait in a queue ordered by that
/// count and then by when they were added, so the next one to render is
/// always at its front: O((V + E) log V) for V modules and E cables.
#[must_use]
pub fn processing_order(slots: &[usize], edges: &[(usize, usize)], outs: &[usize]) -> Order {
    // Each slot's place in `pending`; repeats and slots an order cannot
    // hold are left out.
    let mut index: HashMap<usize, usize> = HashMap::with_capacity(slots.len());
    let mut pending: Vec<usize> = Vec::with_capacity(slots.len());
    for &slot in slots {
        if u32::try_from(slot).is_ok() && !index.contains_key(&slot) {
            index.insert(slot, pending.len());
            pending.push(slot);
        }
    }
    // Sources not yet rendered, for each pending module, and the modules
    // each one feeds; a cable from a module to itself never waits, and
    // cables from modules not in the order never hold anything back.
    let mut waiting = vec![0_usize; pending.len()];
    let mut feeds: Vec<Vec<usize>> = vec![Vec::new(); pending.len()];
    for &(from, to) in edges {
        if from == to {
            continue;
        }
        if let (Some(&from), Some(&to)) = (index.get(&from), index.get(&to)) {
            waiting[to] += 1;
            feeds[from].push(to);
        }
    }
    // The modules still to render, fewest unrendered sources first, the
    // earliest added on a tie.
    let mut queue: BTreeSet<(usize, usize)> = waiting
        .iter()
        .enumerate()
        .map(|(module, &count)| (count, module))
        .collect();
    let mut order = Vec::with_capacity(pending.len());
    while let Some((_, chosen)) = queue.pop_first() {
        for &fed in &feeds[chosen] {
            // Only a module still in the queue is waiting on this one.
            if queue.remove(&(waiting[fed], fed)) {
                waiting[fed] = waiting[fed].saturating_sub(1);
                queue.insert((waiting[fed], fed));
            }
        }
        order.push(pending[chosen]);
    }
    Order::new(&order, outs)
}

// Unplugged buffers retain zero until a gather writes them again.
pub(super) fn clear_jacks(
    mut mask: u32,
    inputs: &mut [Block; MAX_INPUTS],
    knob_cv: &mut [Block; MAX_KNOBS],
) {
    while mask != 0 {
        let jack = mask.trailing_zeros() as usize;
        if jack < MAX_INPUTS {
            inputs[jack].fill(0.0);
        } else {
            knob_cv[jack - MAX_INPUTS].fill(0.0);
        }
        mask &= mask - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(order: &Order, slot: usize) -> usize {
        order
            .slots()
            .iter()
            .position(|s| *s as usize == slot)
            .unwrap()
    }

    /// A slot no order can hold.
    const BEYOND: usize = u32::MAX as usize + 1;

    #[test]
    fn sources_render_before_what_they_feed() {
        // 3 → 1 → 2, added in the order 1, 2, 3.
        let order = processing_order(&[1, 2, 3], &[(3, 1), (1, 2)], &[]);
        assert_eq!(order.slots(), &[3, 1, 2]);
    }

    #[test]
    fn cycles_are_broken_and_everything_renders_once() {
        // 0 → 1 → 2 → 0, and 3 feeding 1.
        let order = processing_order(&[0, 1, 2, 3], &[(0, 1), (1, 2), (2, 0), (3, 1)], &[]);
        assert_eq!(order.slots().len(), 4);
        // Within the cycle, the earliest added module goes first: 0 then 1
        // (once 3 has rendered) then 2.
        assert!(position(&order, 0) < position(&order, 1));
        assert!(position(&order, 3) < position(&order, 1));
        assert!(position(&order, 1) < position(&order, 2));
    }

    #[test]
    fn self_patches_and_nonsense_are_harmless() {
        let order = processing_order(&[5, 5, BEYOND, 6], &[(5, 5), (6, 5), (BEYOND, 5)], &[]);
        assert_eq!(order.slots(), &[6, 5]);
        assert_eq!(Order::new(&[1, 1, BEYOND, 2], &[]).slots(), &[1, 2]);
        // A source that is not on the wall holds nothing back.
        let order = processing_order(&[5, 6], &[(99, 5), (6, 5)], &[]);
        assert_eq!(order.slots(), &[6, 5]);
    }

    #[test]
    fn orders_reach_far_past_a_byte() {
        // A chain of a thousand modules, added last to first: each renders
        // after the one feeding it, whatever its slot.
        let slots: Vec<usize> = (0..1_000).rev().collect();
        let edges: Vec<(usize, usize)> = (0..999).map(|slot| (slot, slot + 1)).collect();
        let order = processing_order(&slots, &edges, &[]);
        let expected: Vec<u32> = (0..1_000).collect();
        assert_eq!(order.slots(), expected.as_slice());
    }

    #[test]
    fn cable_tables_hold_only_sensible_routes() {
        let mut table = CableTable::new();
        assert_eq!(table.lines(), 0);
        let route = Route::new(2, 1, 3.0, 5, 9);
        assert!(table.set(4, 1, Some(route)));
        assert_eq!(table.route(4, 1).map(|r| r.amount), Some(1.0));
        assert_eq!(table.lines(), 6);
        // Delays are held to the most a line holds.
        assert!(table.set(
            4,
            3,
            Some(Route {
                delay: 9_999,
                ..route
            }),
        ));
        assert_eq!(table.route(4, 3).map(|r| r.delay), Some(MAX_CABLE_DELAY));
        assert!(!table.set(
            4,
            2,
            Some(Route {
                amount: f32::NAN,
                ..route
            }),
        ));
        assert_eq!(table.route(4, 2), None);
        assert!(!table.set(4, 99, Some(route)));
        assert!(table.set(4, 23, Some(route)));
        assert!(table.route(4, 23).is_some());
        assert!(table.set(4, 23, None));
        assert!(!table.set(4, 0, Some(Route { port: 9, ..route })));
        assert_eq!(table.route(4, 0), None);
        // Unplugging where nothing ever was plugged in changes nothing.
        assert!(table.set(9_000, 0, None));
        assert_eq!(table.route(9_000, 0), None);
        assert!(table.set(4, 1, None));
        assert!(table.set(4, 3, None));
        assert!((0..MAX_JACKS).all(|jack| table.route(4, jack).is_none()));
    }

    #[test]
    fn cable_tables_reach_any_slot_and_line() {
        let mut table = CableTable::new();
        let far = Route::new(70_000, 0, 0.5, 100_000, 1);
        assert!(table.set(5_000, 0, Some(far)));
        assert_eq!(table.route(5_000, 0), Some(far));
        assert_eq!(table.lines(), 100_001);
        assert_eq!(table.route(5_001, 0), None);
        // The source far past the table's slots is counted as feeding it.
        assert!(table.feeds(70_000, 0));
        assert!(!table.feeds(70_000, 1));
        assert!(table.set(5_000, 0, None));
        assert!(!table.feeds(70_000, 0));
    }

    #[test]
    fn a_table_sized_to_the_rack_never_grows_and_one_too_big_is_refused() {
        let mut table = CableTable::with_slots(300).unwrap();
        assert_eq!(table.slots(), 300);
        for slot in 0..300 {
            assert!(table.set(slot, 0, Some(Route::new(0, 0, 1.0, 0, 1))));
        }
        assert_eq!(table.slots(), 300);
        assert!(CableTable::with_slots(usize::MAX).is_none());
    }
}
