//! Cost heard work on the control side before it reaches the engine.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::unheard::is_sink;
use super::{LOAD_GUARD, Source, Wall, error};
use crate::SUB_BLOCK;
use crate::catalogue::Kind;
use crate::engine::tables;
use crate::patch::Patch;
use crate::protocol::{ErrorCode, ModuleRecord, WallError, What};

/// Ten names identify a waking set without overwhelming a console refusal.
pub const WAKE_NAMES: usize = 10;

/// Below this sum of bench shares (about a dozen modules), the wall up now
/// is too small to say how busy the machine is: its render time is mostly
/// the engine's own work and timing noise.
pub(super) const BUSY_MIN_BENCH: f32 = 0.15;
/// The most the factor reads. A wall whose measure is eight times its bench
/// cost is already far past the guard; a larger factor says nothing more.
pub(super) const BUSY_MAX: f32 = 8.0;
/// From this factor up, the sentences say the machine is busy.
pub(super) const BUSY_SAID: f32 = 1.25;

/// Each kind's `wall/kind/<name>` bench midpoint, in nanoseconds per
/// 512-frame block: the kind with a `vco` feeding it and an `out` it feeds
/// (`benches/support/patches.rs`), so each row includes their cost and the
/// engine's own, and errs high. Taken at `42664d7` on 13 Oct 2026 by the
/// KZ-D03b review, one-minute load averages 12 to 38 on ten cores (each
/// row's load is in `briefs/studio/BENCH.md`). `speak` has no bench row (it
/// needs a phrase feed): it takes the `vca` row. Rows taken with the machine
/// near a load of 100 came out three to six times these, and costed walls
/// that render at 57% of real time at 251%.
/// Sidestep provisionally takes the ringmod row until `fx/sidestep` is
/// measured on a quiet machine.
pub(super) const KIND_BENCH_NS: &[(&str, u32)] = &[
    ("vco", 133_880),
    ("lfo", 72_895),
    ("noise", 39_575),
    ("vcf", 102_190),
    ("vca", 91_703),
    ("env", 67_967),
    ("clock", 71_175),
    ("seq", 99_326),
    ("sh", 69_456),
    ("quant", 97_784),
    ("slew", 71_237),
    ("mix", 73_098),
    ("out", 88_709),
    ("compare", 106_340),
    ("logic", 95_147),
    ("branch", 95_226),
    ("switch", 112_510),
    ("select", 101_270),
    ("divide", 88_433),
    ("math", 113_050),
    ("scope", 112_080),
    ("readout", 97_724),
    ("loop", 70_485),
    ("klon", 782_260),
    ("screamer", 1_551_300),
    ("fuzz", 370_870),
    ("amp", 1_209_300),
    ("fold", 836_620),
    ("crush", 279_640),
    ("ringmod", 379_420),
    ("sidestep", 379_420),
    ("comp", 592_430),
    // fx/octaves measured 2026-10-14; four 128-frame blocks at the steady midpoint.
    ("octaves", 4_143_200),
    ("tape", 491_630),
    ("bbd", 162_010),
    ("digital", 133_860),
    ("chorus", 233_340),
    ("flanger", 212_400),
    ("phaser", 159_110),
    ("trem", 117_200),
    ("shift", 344_140),
    ("granular", 153_700),
    ("pitch", 314_290),
    ("plate", 217_740),
    ("hall", 353_440),
    ("spring", 604_340),
    ("shimmer", 657_380),
    ("resonator", 160_690),
    ("eq", 120_420),
    ("vhs", 261_110),
    ("cassette", 385_510),
    ("reel", 534_100),
    ("vinyl", 208_800),
    ("genloss", 374_890),
    ("radio", 158_740),
    ("sampler12", 131_510),
    ("kick", 81_975),
    ("snare", 83_282),
    ("clap", 75_216),
    ("hat", 89_219),
    ("cymbal", 111_510),
    ("tom", 84_614),
    ("conga", 89_861),
    ("rim", 89_555),
    ("cowbell", 78_316),
    ("shaker", 79_641),
    ("membrane", 89_672),
    ("modal", 78_788),
    ("metal", 79_826),
    ("noiseperc", 85_272),
    ("euclid", 77_447),
    ("prob", 81_155),
    ("grids", 82_980),
    ("poly", 80_973),
    ("burst", 69_658),
    ("vocoder", 140_790),
    ("speak", 91_703),
];

fn bench_costs() -> &'static HashMap<Kind, u32> {
    static COSTS: OnceLock<HashMap<Kind, u32>> = OnceLock::new();
    COSTS.get_or_init(|| {
        KIND_BENCH_NS
            .iter()
            .filter_map(|&(name, ns)| Kind::from_name(name).map(|kind| (kind, ns)))
            .collect()
    })
}

pub(super) fn kind_share(kind: Kind, rate: u32) -> f32 {
    let costs = bench_costs();
    let ns = costs
        .get(&kind)
        .copied()
        .unwrap_or_else(|| costs.values().copied().max().unwrap_or(0));
    // Rates and block lengths are small; nanoseconds need relative precision.
    ns as f32 / (512.0 * 1e9 / rate as f32)
}

/// Whether `what` can make a module heard: a cable or a module put on the
/// wall, a module brought back, a chain, or an undo that plugs back the
/// cables it had replaced (an unplugging or a removal that replugs). Every
/// other change (a knob, a tempo, a plain unplugging or removal) only ever
/// takes work away, and is never costed.
pub(super) fn can_wake(what: &What) -> bool {
    match what {
        What::Patch { .. } | What::Add { .. } | What::Restore { .. } | What::Chain { .. } => true,
        What::Unpatch { replugged, .. } => replugged.is_some(),
        What::Remove { replugged, .. } => !replugged.is_empty(),
        _ => false,
    }
}

fn heard_modules(patch: &Patch) -> Vec<usize> {
    let positions: HashMap<&str, usize> = patch
        .modules()
        .iter()
        .enumerate()
        .map(|(index, module)| (module.id.as_str(), index))
        .collect();
    let slots: Vec<usize> = (0..patch.modules().len()).collect();
    let edges: Vec<(usize, usize)> = patch
        .cables()
        .iter()
        .filter_map(|cable| {
            Some((
                *positions.get(cable.from_module.as_str())?,
                *positions.get(cable.to_module.as_str())?,
            ))
        })
        .collect();
    let sinks: Vec<usize> = patch
        .modules()
        .iter()
        .enumerate()
        .filter_map(|(index, module)| is_sink(module.kind).then_some(index))
        .collect();
    tables::heard(&slots, &edges, &sinks)
}

/// The modules `what` takes off the wall: a removal's, or every removal's
/// in a chain. Each still renders until the change is made.
fn taken_away(what: &What) -> Vec<&ModuleRecord> {
    match what {
        What::Remove { module, .. } => vec![module],
        What::Chain { changes, .. } => changes.iter().flat_map(taken_away).collect(),
        _ => Vec::new(),
    }
}

/// What a change would wake, costed once: the factor it was costed at, and
/// each module it would wake with its share of the wall's time.
pub(super) struct Waking {
    busy: f32,
    woken: Vec<(String, f32)>,
}

impl Waking {
    /// The share of the wall's time the woken modules would take.
    pub(super) fn share(&self) -> f32 {
        self.woken.iter().map(|(_, share)| share).sum()
    }

    /// The refusal of this wake, which would take the wall from `load` to
    /// `projected`.
    pub(super) fn refusal(&self, load: f32, projected: f32) -> WallError {
        let clause = busy_clause(self.busy);
        let n = self.woken.len();
        let ids = self
            .woken
            .iter()
            .take(WAKE_NAMES)
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let ids = if n > WAKE_NAMES {
            format!("{ids}, and {} more", n - WAKE_NAMES)
        } else {
            ids
        };
        error(
            ErrorCode::Full,
            format!(
                "this would wake {n} modules nobody heard ({ids}), taking the wall from {:.0}% to about {:.0}% of its time rendering, past the {:.0}% it keeps to{clause}; take something away first, or wake them a few at a time",
                load * 100.0,
                projected * 100.0,
                LOAD_GUARD * 100.0,
            ),
        )
    }
}

fn busy_clause(busy: f32) -> String {
    if busy >= BUSY_SAID {
        format!(
            " (the machine is busy: the wall up now renders at {busy:.1}× its quiet cost, and the new modules are costed the same)"
        )
    } else {
        String::new()
    }
}

impl Wall {
    /// How many times its bench cost the wall up now takes to render: its
    /// render load over the sum of the bench shares of the modules the
    /// engine renders now, held to 1 to [`BUSY_MAX`]; 1 for a wall too small
    /// to say (below [`BUSY_MIN_BENCH`]) or a load not measured.
    pub(super) fn busy(&self) -> f32 {
        self.busy_beside(&[])
    }

    /// [`Self::busy`], with `taken` (modules a staged change takes off the
    /// wall, still rendering until it is made) counted as rendered now.
    ///
    /// The bench is the modules the engine renders now (placed in a slot,
    /// and not culled), never the heard modules of `self.patch`: a change
    /// that may wake modules is costed with the patch already staged, and
    /// the modules it would wake render nothing yet. Counted, they would
    /// lower the factor, and the wake would be costed the less the busier
    /// the machine. With nothing staged the two are the same modules.
    fn busy_beside(&self, taken: &[&ModuleRecord]) -> f32 {
        let rate = self.engine.sample_rate();
        let unheard: HashSet<&str> = self.unheard.iter().map(String::as_str).collect();
        let on_wall = self
            .patch
            .modules()
            .iter()
            .map(|module| (module.id.as_str(), Some(module.kind)));
        // A module taken away was placed with a kind the catalogue knows.
        let going = taken
            .iter()
            .filter(|module| self.patch.module(&module.id).is_none())
            .map(|module| (module.id.as_str(), Kind::from_name(&module.kind)));
        let bench: f32 = on_wall
            .chain(going)
            .filter(|(id, _)| self.rack.slot_of(id).is_some() && !unheard.contains(id))
            .filter_map(|(_, kind)| kind)
            .map(|kind| kind_share(kind, rate))
            .sum();
        let load = self.engine.shared().render_load();
        if bench < BUSY_MIN_BENCH || !load.is_finite() {
            return 1.0;
        }
        // Both measures cover the whole render. A module's own timing omits
        // the source, sink and engine work included in its bench row. The
        // bench errs high deliberately, so quiet work keeps its bench cost.
        (load / bench).clamp(1.0, BUSY_MAX)
    }

    pub(super) fn kind_share_now(&self, kind: Kind, busy: f32) -> f32 {
        kind_share(kind, self.engine.sample_rate()) * busy
    }

    pub(super) fn note_addition(&mut self, what: &What) {
        if matches!(
            what,
            What::Add { .. }
                | What::Restore { .. }
                | What::Chain { .. }
                | What::Patch { replaced: None, .. }
        ) {
            self.load_watch.added = Some(self.engine.shared().frames());
        }
    }

    pub(super) fn module_share(&self, id: &str, kind: Kind, busy: f32) -> f32 {
        let rate = self.engine.sample_rate();
        self.rack
            .slot_of(id)
            .and_then(|slot| self.engine.cost(slot))
            .map_or_else(
                || self.kind_share_now(kind, busy),
                // A sub-block is small; timings need relative precision.
                |ns| {
                    let share = ns / (SUB_BLOCK as f32 * 1e9 / rate as f32);
                    // Culled modules retain a timing from when they were
                    // last heard, so their cost needs the current factor.
                    if self.unheard.iter().any(|unheard| unheard == id) {
                        share * busy
                    } else {
                        share
                    }
                },
            )
    }

    /// The modules the staged patch would wake, each costed once at the
    /// factor of the wall the engine renders now (with `taken`, the
    /// modules the change takes away, as in [`Self::busy_beside`]).
    pub(super) fn costed_wake(&self, taken: &[&ModuleRecord]) -> Waking {
        let busy = self.busy_beside(taken);
        Waking {
            busy,
            woken: self.woken(busy),
        }
    }

    fn woken(&self, busy: f32) -> Vec<(String, f32)> {
        let heard: HashSet<usize> = heard_modules(&self.patch).into_iter().collect();
        let unheard: HashSet<&str> = self.unheard.iter().map(String::as_str).collect();
        self.patch
            .modules()
            .iter()
            .enumerate()
            .filter(|(index, module)| {
                heard.contains(index)
                    && (self.rack.slot_of(&module.id).is_none()
                        || unheard.contains(module.id.as_str()))
            })
            .map(|(_, module)| {
                (
                    module.id.clone(),
                    self.module_share(&module.id, module.kind, busy),
                )
            })
            .collect()
    }

    /// Make sure the machine has time to render what the staged change
    /// `what` would wake (see [`Self::render_room`]).
    pub(super) fn wake_room(&mut self, what: &What) -> Result<(), WallError> {
        if !self.load_watch.on {
            return Ok(());
        }
        let waking = self.costed_wake(&taken_away(what));
        if waking.woken.is_empty() {
            return Ok(());
        }
        let units = self
            .patch
            .modules()
            .iter()
            .filter(|module| self.rack.slot_of(&module.id).is_none())
            .count()
            + self
                .patch
                .cables()
                .iter()
                .filter(|cable| !self.lines.contains_key(&cable.id))
                .count();
        if let Err(err) = self.render_room(units, Some(&waking)) {
            self.unmade = true;
            return Err(err);
        }
        self.load_watch.waking = waking.share();
        Ok(())
    }

    fn load_projection(&self, incoming: &Patch) -> (usize, f32, f32, f32) {
        let busy = self.busy();
        let heard = heard_modules(incoming);
        let sum: f32 = heard
            .iter()
            .map(|&index| self.kind_share_now(incoming.modules()[index].kind, busy))
            .sum();
        let current: f32 = heard_modules(&self.patch)
            .iter()
            .map(|&index| {
                let module = &self.patch.modules()[index];
                self.module_share(&module.id, module.kind, busy)
            })
            .sum();
        let rest = (self.engine.shared().render_load() - current).max(0.0);
        (heard.len(), sum, rest + sum, busy)
    }

    pub(super) fn load_room(&self, incoming: &Patch) -> Result<(), WallError> {
        if !self.load_watch.on {
            return Ok(());
        }
        let (n, sum, projected, busy) = self.load_projection(incoming);
        if sum > 0.0 && projected > LOAD_GUARD {
            let clause = busy_clause(busy);
            return Err(error(
                ErrorCode::Full,
                format!(
                    "the wall this would put up renders {n} modules, taking it to about {:.0}% of its time rendering, past the {:.0}% it keeps to{clause}, so it was not put up",
                    projected * 100.0,
                    LOAD_GUARD * 100.0,
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn load_room_as(
        &self,
        console: bool,
        incoming: &Patch,
        source: &Source,
        notes: &mut Vec<String>,
    ) -> Result<Option<String>, WallError> {
        if !console {
            self.load_room(incoming)?;
            return Ok(None);
        }
        let (_, sum, projected, busy) = self.load_projection(incoming);
        if !self.load_watch.on || sum <= 0.0 || projected <= LOAD_GUARD {
            return Ok(None);
        }
        let name = source.kept().map_or("new", |kept| kept.name.as_str());
        let clause = busy_clause(busy);
        let warning = format!(
            "Tom loaded \"{name}\": about {:.0}% of the wall's time{clause}, past the {:.0}% guard",
            projected * 100.0,
            LOAD_GUARD * 100.0
        );
        notes.push(warning.clone());
        Ok(Some(warning))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_bench_cost() {
        for kind in Kind::all() {
            if crate::dsp::effect::testing::KINDS
                .iter()
                .any(|test| test.id == kind.name())
            {
                continue;
            }
            assert!(
                KIND_BENCH_NS
                    .iter()
                    .any(|&(name, ns)| name == kind.name() && ns > 0),
                "{} has no bench cost",
                kind.name()
            );
        }
        for &(name, ns) in KIND_BENCH_NS {
            assert!(Kind::from_name(name).is_some(), "unknown kind {name}");
            assert!(ns > 0);
        }
    }
}
