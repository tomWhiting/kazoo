//! Cost heard work on the control side before it reaches the engine.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::unheard::is_sink;
use super::{LOAD_GUARD, Source, Wall, error};
use crate::SUB_BLOCK;
use crate::catalogue::Kind;
use crate::engine::tables;
use crate::patch::Patch;
use crate::protocol::{ErrorCode, WallError, What};

/// Ten names identify a waking set without overwhelming a console refusal.
pub const WAKE_NAMES: usize = 10;

pub(super) const KIND_BENCH_NS: &[(&str, u32)] = &[
    ("vco", 315_280),
    ("lfo", 190_800),
    ("noise", 122_460),
    ("vcf", 528_620),
    ("vca", 423_150),
    ("env", 299_770),
    ("clock", 349_550),
    ("seq", 439_570),
    ("sh", 448_740),
    ("quant", 440_970),
    ("slew", 466_330),
    ("mix", 447_710),
    ("out", 502_350),
    ("compare", 694_290),
    ("logic", 583_050),
    ("branch", 718_550),
    ("switch", 1_368_000),
    ("select", 590_900),
    ("divide", 422_790),
    ("math", 518_570),
    ("scope", 469_920),
    ("readout", 490_350),
    ("loop", 404_320),
    ("klon", 4_950_700),
    ("screamer", 6_504_800),
    ("fuzz", 2_310_000),
    ("amp", 5_187_200),
    ("fold", 3_523_200),
    ("crush", 612_630),
    ("ringmod", 1_496_800),
    ("comp", 1_455_000),
    ("tape", 1_067_000),
    ("bbd", 291_030),
    ("digital", 289_280),
    ("chorus", 348_070),
    ("flanger", 533_770),
    ("phaser", 484_020),
    ("trem", 243_690),
    ("shift", 783_240),
    ("granular", 371_160),
    ("pitch", 609_790),
    ("plate", 476_870),
    ("hall", 741_010),
    ("spring", 1_675_400),
    ("shimmer", 1_420_200),
    ("resonator", 304_400),
    ("eq", 282_360),
    ("vhs", 725_820),
    ("cassette", 779_760),
    ("reel", 868_550),
    ("vinyl", 371_960),
    ("genloss", 703_390),
    ("radio", 465_940),
    ("sampler12", 1_212_500),
    ("kick", 163_680),
    ("snare", 153_260),
    ("clap", 152_390),
    ("hat", 169_840),
    ("cymbal", 202_540),
    ("tom", 176_060),
    ("conga", 215_240),
    ("rim", 179_960),
    ("cowbell", 165_290),
    ("shaker", 405_470),
    ("membrane", 264_440),
    ("modal", 191_950),
    ("metal", 191_350),
    ("noiseperc", 172_790),
    ("euclid", 155_730),
    ("prob", 164_390),
    ("grids", 144_240),
    ("poly", 123_770),
    ("burst", 117_960),
    ("vocoder", 267_700),
    ("speak", 423_150),
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
pub(super) const fn can_wake(what: &What) -> bool {
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

impl Wall {
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

    fn module_share(&self, id: &str, kind: Kind) -> f32 {
        let rate = self.engine.sample_rate();
        self.rack
            .slot_of(id)
            .and_then(|slot| self.engine.cost(slot))
            .map_or_else(
                || kind_share(kind, rate),
                // A sub-block is small; timings need relative precision.
                |ns| ns / (SUB_BLOCK as f32 * 1e9 / rate as f32),
            )
    }

    fn woken(&self) -> Vec<(String, f32)> {
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
                    self.module_share(&module.id, module.kind),
                )
            })
            .collect()
    }

    pub(super) fn wake_room(&mut self) -> Result<(), WallError> {
        if !self.load_watch.on {
            return Ok(());
        }
        let woken = self.woken();
        if woken.is_empty() {
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
        let waking = woken.iter().map(|(_, share)| share).sum();
        if let Err(err) = self.render_room(units, waking) {
            self.unmade = true;
            return Err(err);
        }
        self.load_watch.waking = waking;
        Ok(())
    }

    pub(super) fn wake_refusal(&self, load: f32, projected: f32) -> WallError {
        let woken = self.woken();
        let n = woken.len();
        let ids = woken
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
                "this would wake {n} modules nobody heard ({ids}), taking the wall from {:.0}% to about {:.0}% of its time rendering, past the {:.0}% it keeps to; take something away first, or wake them a few at a time",
                load * 100.0,
                projected * 100.0,
                LOAD_GUARD * 100.0,
            ),
        )
    }

    fn load_projection(&self, incoming: &Patch) -> (usize, f32, f32) {
        let heard = heard_modules(incoming);
        let sum: f32 = heard
            .iter()
            .map(|&index| kind_share(incoming.modules()[index].kind, self.engine.sample_rate()))
            .sum();
        let current: f32 = heard_modules(&self.patch)
            .iter()
            .map(|&index| {
                let module = &self.patch.modules()[index];
                self.module_share(&module.id, module.kind)
            })
            .sum();
        let rest = (self.engine.shared().render_load() - current).max(0.0);
        (heard.len(), sum, rest + sum)
    }

    pub(super) fn load_room(&self, incoming: &Patch) -> Result<(), WallError> {
        if !self.load_watch.on {
            return Ok(());
        }
        let (n, sum, projected) = self.load_projection(incoming);
        if sum > 0.0 && projected > LOAD_GUARD {
            return Err(error(
                ErrorCode::Full,
                format!(
                    "the wall this would put up renders {n} modules, taking it to about {:.0}% of its time rendering, past the {:.0}% it keeps to, so it was not put up",
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
        let (_, sum, projected) = self.load_projection(incoming);
        if !self.load_watch.on || sum <= 0.0 || projected <= LOAD_GUARD {
            return Ok(None);
        }
        let name = source.kept().map_or("new", |kept| kept.name.as_str());
        let warning = format!(
            "Tom loaded \"{name}\": about {:.0}% of the wall's time, past the {:.0}% guard",
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
