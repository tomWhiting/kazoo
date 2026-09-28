//! The engine's routing tables: where each input reads from, and the order
//! modules render in. Both are built on the control side and swapped in
//! whole.

use crate::{MAX_CABLES, MAX_JACKS, MAX_MODULES, MAX_OUTPUTS};

/// The longest delay a cable carries to keep its signal in step with the
/// others arriving at the same module, in frames (about 21 ms at 48 kHz).
pub const MAX_CABLE_DELAY: u16 = 1_024;

/// Where one input reads from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    /// The source module's slot.
    pub slot: u8,
    /// The source output.
    pub port: u8,
    /// The cable's attenuverter, -1 to 1.
    pub amount: f32,
    /// The cable's delay line, below [`MAX_CABLES`]: one per cable, kept
    /// for the cable's life so its history survives every table swap.
    pub line: u16,
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
    pub const fn new(slot: u8, port: u8, amount: f32, line: u16, tag: u32) -> Self {
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
/// into it.
#[derive(Debug, Clone, PartialEq)]
pub struct CableTable {
    /// By slot ([`MAX_MODULES`] of them), then jack.
    routes: Box<[[Option<Route>; MAX_JACKS]]>,
}

impl CableTable {
    /// A table with nothing plugged in (this allocates: build tables on
    /// the control side).
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: vec![[None; MAX_JACKS]; MAX_MODULES].into_boxed_slice(),
        }
    }

    /// Plug `slot`'s `jack` into `route`, or unplug it with `None`. Slots,
    /// ports and jacks out of range are ignored, and so is a non-finite
    /// amount; amounts are held to -1..1.
    pub fn set(&mut self, slot: usize, jack: usize, route: Option<Route>) {
        let valid = route.is_none_or(|route| {
            usize::from(route.slot) < MAX_MODULES
                && usize::from(route.port) < MAX_OUTPUTS
                && usize::from(route.line) < MAX_CABLES
                && route.amount.is_finite()
        });
        if slot >= MAX_MODULES || jack >= MAX_JACKS || !valid {
            return;
        }
        self.routes[slot][jack] = route.map(|route| Route {
            amount: route.amount.clamp(-1.0, 1.0),
            delay: route.delay.min(MAX_CABLE_DELAY),
            ..route
        });
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
}

impl Default for CableTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The order modules render in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    slots: [u8; MAX_MODULES],
    len: usize,
}

impl Order {
    /// Render `slots` in this order. Slots out of range, repeats, and
    /// anything past [`MAX_MODULES`] are left out.
    #[must_use]
    pub fn new(slots: &[usize]) -> Self {
        let mut order = Self {
            slots: [0; MAX_MODULES],
            len: 0,
        };
        let mut seen = [false; MAX_MODULES];
        for &slot in slots {
            if slot < MAX_MODULES && !seen[slot] && order.len < MAX_MODULES {
                seen[slot] = true;
                // Below MAX_MODULES (at most 256): fits a u8.
                order.slots[order.len] = slot as u8;
                order.len += 1;
            }
        }
        order
    }

    /// The slots, in render order.
    #[must_use]
    pub fn slots(&self) -> &[u8] {
        &self.slots[..self.len]
    }
}

impl Default for Order {
    fn default() -> Self {
        Self::new(&[])
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
#[must_use]
pub fn processing_order(slots: &[usize], edges: &[(usize, usize)]) -> Order {
    let mut pending: Vec<usize> = Vec::with_capacity(slots.len());
    for &slot in slots {
        if slot < MAX_MODULES && !pending.contains(&slot) {
            pending.push(slot);
        }
    }
    let mut placed = [false; MAX_MODULES];
    let mut order = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        // Sources not yet rendered, for each pending module; a cable from a
        // module to itself never waits.
        let waiting = |slot: usize| {
            edges
                .iter()
                .filter(|&&(from, to)| {
                    to == slot
                        && from != slot
                        && from < MAX_MODULES
                        && !placed[from]
                        && pending.contains(&from)
                })
                .count()
        };
        let mut best = 0;
        let mut best_waiting = usize::MAX;
        for (index, &slot) in pending.iter().enumerate() {
            let count = waiting(slot);
            if count < best_waiting {
                best = index;
                best_waiting = count;
                if count == 0 {
                    break;
                }
            }
        }
        let slot = pending.remove(best);
        placed[slot] = true;
        order.push(slot);
    }
    Order::new(&order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(order: &Order, slot: usize) -> usize {
        order
            .slots()
            .iter()
            .position(|s| usize::from(*s) == slot)
            .unwrap()
    }

    #[test]
    fn sources_render_before_what_they_feed() {
        // 3 → 1 → 2, added in the order 1, 2, 3.
        let order = processing_order(&[1, 2, 3], &[(3, 1), (1, 2)]);
        assert_eq!(order.slots(), &[3, 1, 2]);
    }

    #[test]
    fn cycles_are_broken_and_everything_renders_once() {
        // 0 → 1 → 2 → 0, and 3 feeding 1.
        let order = processing_order(&[0, 1, 2, 3], &[(0, 1), (1, 2), (2, 0), (3, 1)]);
        assert_eq!(order.slots().len(), 4);
        // Within the cycle, the earliest added module goes first: 0 then 1
        // (once 3 has rendered) then 2.
        assert!(position(&order, 0) < position(&order, 1));
        assert!(position(&order, 3) < position(&order, 1));
        assert!(position(&order, 1) < position(&order, 2));
    }

    #[test]
    fn self_patches_and_nonsense_are_harmless() {
        let order = processing_order(&[5, 5, 99, 6], &[(5, 5), (6, 5), (99, 5)]);
        assert_eq!(order.slots(), &[6, 5]);
        assert_eq!(Order::new(&[1, 1, 200, 2]).slots(), &[1, 2]);
    }

    #[test]
    fn cable_tables_hold_only_sensible_routes() {
        let mut table = CableTable::new();
        let route = Route::new(2, 1, 3.0, 5, 9);
        table.set(4, 1, Some(route));
        assert_eq!(table.route(4, 1).map(|r| r.amount), Some(1.0));
        // Delays are held to the most a line holds; lines must exist.
        table.set(
            4,
            3,
            Some(Route {
                delay: 9_999,
                ..route
            }),
        );
        assert_eq!(table.route(4, 3).map(|r| r.delay), Some(MAX_CABLE_DELAY));
        table.set(4, 6, Some(Route { line: 999, ..route }));
        assert_eq!(table.route(4, 6), None);
        table.set(
            4,
            2,
            Some(Route {
                amount: f32::NAN,
                ..route
            }),
        );
        assert_eq!(table.route(4, 2), None);
        table.set(200, 0, Some(route));
        table.set(4, 99, Some(route));
        table.set(4, 23, Some(route));
        assert!(table.route(4, 23).is_some());
        table.set(4, 23, None);
        table.set(4, 0, Some(Route { port: 9, ..route }));
        assert_eq!(table.route(4, 0), None);
        assert_eq!(table.route(200, 0), None);
        table.set(4, 1, None);
        table.set(4, 3, None);
        assert_eq!(table, CableTable::default());
    }
}
