//! Fingerprints: who touched what, and where the signal carried that touch.
//!
//! Every touch deposits the seat's dye into the module it touched: a turn
//! deposits the size of the change as a share of the knob's range, a patch
//! the cable's |amount| into the module it plugs into, an add or a restore a
//! whole unit. The dye flows downstream along the cables. A module's
//! fingerprint is the mix of its own dye and all the dye that reaches it,
//! each seat's share from 0 to 1; a cable carries its source's mix.
//!
//! How much of a module's dye reaches another follows the strongest path
//! between them: each cable keeps |amount| of it, and every hop halves it,
//! like dye thinning in water. Paths around a feedback loop only ever thin
//! the dye, so the result is always well defined. Fingerprints are computed
//! on the control side when a change lands and change only when something
//! changes: a touch, or a cable plugged or unplugged (which re-routes the
//! dye). Nothing fades with time, and nothing here judges or labels.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

use serde::{Deserialize, Serialize};

use crate::patch::Patch;
use crate::protocol::What;

/// The share of dye each hop passes on, before the cable's |amount|.
pub const HOP: f64 = 0.5;

/// The most seats whose dye a module keeps: the strongest, the faintest
/// dropped as more seats touch it.
///
/// Six is more hands than a module shows at a glance ("Tom 62%, Waffles
/// 30%, Cassio 8%, Vesper <1%"): past the sixth, a seat's share of a
/// module is a sliver of a sliver. And it keeps the dye, which otherwise
/// grows with every seat that ever touches the wall, inside the wall's
/// file (see `store::MAX_WALL_FILE`).
pub const MAX_DYE_SEATS: usize = 6;

/// Drop the faintest of `seats` (dye amounts by seat) beyond
/// [`MAX_DYE_SEATS`], ties going to the later name. Returns how many were
/// dropped.
pub fn trim_seats(seats: &mut BTreeMap<String, f64>) -> usize {
    trim_seats_but(seats, None)
}

/// As [`trim_seats`], never dropping `keep` (the seat dyeing it now).
fn trim_seats_but(seats: &mut BTreeMap<String, f64>, keep: Option<&str>) -> usize {
    let mut dropped = 0;
    while seats.len() > MAX_DYE_SEATS {
        let faintest = seats
            .iter()
            .filter(|(who, _)| Some(who.as_str()) != keep)
            .min_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(who, _)| who.clone());
        let Some(faintest) = faintest else {
            break;
        };
        seats.remove(&faintest);
        dropped += 1;
    }
    dropped
}

/// Shares are rounded to this, so the same wall always shows the same
/// numbers.
const PRECISION: f64 = 1.0e6;

/// Each seat's share, 0 to 1, summing to 1 (empty when nobody's dye is
/// there).
pub type Shares = BTreeMap<String, f64>;

/// Every module's and every cable's shares.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Fingerprints {
    /// Shares by module id; a module nobody's dye reaches is left out.
    #[serde(default)]
    pub modules: BTreeMap<String, Shares>,
    /// Shares by cable number (as a string, as JSON keys are); a cable
    /// carrying no dye is left out.
    #[serde(default)]
    pub cables: BTreeMap<String, Shares>,
}

impl Fingerprints {
    /// What differs from `before`, entry by entry. An entry that is gone
    /// (a module or cable removed, or no longer reached by any dye) appears
    /// with empty shares.
    #[must_use]
    pub fn changed_since(&self, before: &Self) -> Self {
        Self {
            modules: diff(&before.modules, &self.modules),
            cables: diff(&before.cables, &self.cables),
        }
    }

    /// Whether nothing is in it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty() && self.cables.is_empty()
    }
}

fn diff(
    before: &BTreeMap<String, Shares>,
    after: &BTreeMap<String, Shares>,
) -> BTreeMap<String, Shares> {
    let mut changed: BTreeMap<String, Shares> = after
        .iter()
        .filter(|(id, shares)| before.get(*id) != Some(*shares))
        .map(|(id, shares)| (id.clone(), shares.clone()))
        .collect();
    for id in before.keys() {
        if !after.contains_key(id) {
            changed.insert(id.clone(), Shares::new());
        }
    }
    changed
}

/// The dye each seat has put straight into each module, by module id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dye {
    deposits: BTreeMap<String, BTreeMap<String, f64>>,
}

impl Dye {
    /// No dye anywhere.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            deposits: BTreeMap::new(),
        }
    }

    /// The dye as saved: by module id, then seat.
    #[must_use]
    pub const fn deposits(&self) -> &BTreeMap<String, BTreeMap<String, f64>> {
        &self.deposits
    }

    /// Dye as saved. Amounts that are not positive numbers are refused.
    ///
    /// # Errors
    ///
    /// A sentence naming the bad entry.
    pub fn from_deposits(
        deposits: BTreeMap<String, BTreeMap<String, f64>>,
    ) -> Result<Self, String> {
        for (module, seats) in &deposits {
            for (seat, amount) in seats {
                if !(amount.is_finite() && *amount > 0.0) {
                    return Err(format!(
                        "the dye of {seat} on {module} is not a positive number"
                    ));
                }
            }
        }
        Ok(Self { deposits })
    }

    /// Put `amount` of `seat`'s dye into `module`, keeping the
    /// [`MAX_DYE_SEATS`] strongest seats there (never `seat` itself among
    /// those dropped). Nothing for amounts that are not positive.
    pub fn deposit(&mut self, module: &str, seat: &str, amount: f64) {
        if !(amount.is_finite() && amount > 0.0) {
            return;
        }
        let seats = self.deposits.entry(module.to_string()).or_default();
        *seats.entry(seat.to_string()).or_insert(0.0) += amount;
        trim_seats_but(seats, Some(seat));
    }

    /// Forget all dye in `module` (it was taken away).
    pub fn forget(&mut self, module: &str) {
        self.deposits.remove(module);
    }

    /// Record the dye a change leaves, done by `seat` on `patch` (already
    /// changed).
    pub fn touch(&mut self, seat: &str, what: &What, patch: &Patch) {
        match what {
            What::Chain { changes, .. } => {
                for change in changes {
                    self.touch(seat, change, patch);
                }
            }
            What::Turn {
                module,
                knob,
                from,
                to,
                ..
            } => {
                let range = patch
                    .module(module)
                    .and_then(|m| {
                        let spec = m.kind.spec();
                        spec.knob_index(knob).map(|index| &spec.knobs[index])
                    })
                    .map(|knob| f64::from(knob.max) - f64::from(knob.min));
                if let Some(range) = range.filter(|range| *range > 0.0) {
                    self.deposit(module, seat, ((to - from).abs() / range).min(1.0));
                }
            }
            What::Patch { cable, .. } => {
                if let Some((module, _)) = cable.to.split_once('.') {
                    self.deposit(module, seat, cable.amount.abs().min(1.0));
                }
            }
            What::Add { module } | What::Restore { module, .. } => {
                self.deposit(&module.id, seat, 1.0);
            }
            // New words are a whole touch on the speaker.
            What::Take { module, .. }
            | What::Erase { module, .. }
            | What::LoopFrom { module, .. }
            | What::Speak { module, .. } => self.deposit(module, seat, 1.0),
            What::Remove { module, .. } => self.forget(&module.id),
            // Unplugging re-routes the dye but leaves none; the tempo and
            // a recording are no module's.
            // A change this build does not know leaves nothing it can see.
            // A load brings its wall's own dye with it, untouched.
            What::Unpatch { .. }
            | What::Tempo { .. }
            | What::Record { .. }
            | What::Migrate { .. }
            | What::Load { .. }
            | What::Go { .. }
            | What::Abandoned { .. }
            | What::Resumed { .. }
            | What::SetAside { .. }
            | What::Unknown => {}
        }
    }

    /// Where the dye is on `patch`: each module's and cable's shares.
    ///
    /// The strongest path from each module whose dye there is to every
    /// module it reaches is found once per such module (Dijkstra's search
    /// on products: every hop keeps at most half, so a path only thins as
    /// it goes on, and the strongest is settled first): about
    /// `S·E·log V` for `S` dyed modules, `E` cables and `V` modules, and
    /// memory as the wall is large, so a wall of any size is fingerprinted
    /// on every change.
    #[must_use]
    pub fn flow(&self, patch: &Patch) -> Fingerprints {
        let modules = patch.modules();
        let count = modules.len();
        let index: HashMap<&str, usize> = modules
            .iter()
            .enumerate()
            .map(|(at, module)| (module.id.as_str(), at))
            .collect();
        // feeds[u]: the modules u feeds, each by its strongest cable.
        let mut feeds: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); count];
        for cable in patch.cables() {
            if let (Some(&u), Some(&v)) = (
                index.get(cable.from_module.as_str()),
                index.get(cable.to_module.as_str()),
            ) {
                let carried = HOP * f64::from(cable.amount.abs());
                if u != v && carried > 0.0 {
                    let best = feeds[u].entry(v).or_insert(0.0);
                    *best = best.max(carried);
                }
            }
        }
        let mut shares_of: Vec<Shares> = vec![Shares::new(); count];
        // How much of the source's dye reaches each module, and which
        // modules it reached, cleared after each source.
        let mut reach = vec![0.0_f64; count];
        let mut reached = vec![false; count];
        let mut touched = Vec::new();
        let mut queue = BinaryHeap::new();
        for (u, source) in modules.iter().enumerate() {
            let Some(deposits) = self.deposits.get(&source.id) else {
                continue;
            };
            reach[u] = 1.0;
            reached[u] = true;
            touched.push(u);
            queue.push(Strength(1.0, u));
            while let Some(Strength(strength, v)) = queue.pop() {
                // A module is settled at its strongest; weaker entries for
                // it that were queued before are passed over.
                if strength < reach[v] {
                    continue;
                }
                for (&w, &carried) in &feeds[v] {
                    let onward = strength * carried;
                    if onward > reach[w] {
                        if !reached[w] {
                            reached[w] = true;
                            touched.push(w);
                        }
                        reach[w] = onward;
                        queue.push(Strength(onward, w));
                    }
                }
            }
            for &v in &touched {
                let weight = reach[v];
                if weight > 0.0 {
                    for (seat, amount) in deposits {
                        *shares_of[v].entry(seat.clone()).or_insert(0.0) += amount * weight;
                    }
                }
                reach[v] = 0.0;
                reached[v] = false;
            }
            touched.clear();
        }
        for shares in &mut shares_of {
            normalise(shares);
        }
        let mut prints = Fingerprints::default();
        for (module, shares) in modules.iter().zip(&shares_of) {
            if !shares.is_empty() {
                prints.modules.insert(module.id.clone(), shares.clone());
            }
        }
        for cable in patch.cables() {
            let Some(&u) = index.get(cable.from_module.as_str()) else {
                continue;
            };
            // The cable carries its source's mix, whatever it plugs into.
            if !shares_of[u].is_empty() {
                prints
                    .cables
                    .insert(cable.id.to_string(), shares_of[u].clone());
            }
        }
        prints
    }
}

/// How much of a source's dye reaches a module (by its place in the wall),
/// ordered by the amount for the search in [`Dye::flow`]: strongest first.
#[derive(Debug, Clone, Copy)]
struct Strength(f64, usize);

impl PartialEq for Strength {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Strength {}

impl PartialOrd for Strength {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Strength {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .total_cmp(&other.0)
            .then_with(|| self.1.cmp(&other.1))
    }
}

/// Scale `shares` to sum to 1, rounded; drop seats whose share rounds to 0.
fn normalise(shares: &mut Shares) {
    let total: f64 = shares.values().sum();
    if !(total.is_finite() && total > 0.0) {
        shares.clear();
        return;
    }
    for share in shares.values_mut() {
        *share = (*share / total * PRECISION).round() / PRECISION;
    }
    shares.retain(|_, share| *share > 0.0);
}

#[cfg(test)]
mod tests;
