//! Keeping every path in step: how late each module's signal is, and the
//! delay each cable needs so everything arriving at a module arrives
//! together.
//!
//! A module's *latency* is how long it holds its sound back (an effect's
//! oversampling filters); its *arrival* is how late its inputs are, against
//! the wall's sources. Walking the modules in render order, a module's
//! arrival is the latest of its sources' arrival plus latency, and each
//! cable into it is delayed by the difference, so a dry path meets its wet
//! twin in phase. Cables that close a feedback loop are left alone (they
//! already carry the loop's one-sub-block delay, and a loop has no "in
//! step"), and the `out` modules are brought level too, so every out meets
//! the master bus together. A cable needing more than the longest delay a
//! line holds keeps that longest delay and is listed as uncompensated.

use std::collections::{BTreeMap, HashMap};

use crate::catalogue::Kind;
use crate::engine::MAX_CABLE_DELAY;
use crate::patch::Patch;

/// Every module's latency and arrival, and every cable's delay, in frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timing {
    /// Latency by module id (modules with none are left out).
    pub latency: BTreeMap<String, u32>,
    /// Arrival by module id (modules whose inputs are on time are left
    /// out).
    pub arrival: BTreeMap<String, u32>,
    /// Delay by cable number (cables with none are left out).
    pub delays: BTreeMap<u32, u16>,
    /// Cables that needed more than [`MAX_CABLE_DELAY`].
    pub uncompensated: Vec<u32>,
    /// Modules whose latency follows a patched knob (filled in by the
    /// wall, which can ask the effects).
    pub unsteady: Vec<String>,
}

/// The timing of `patch`, rendered in `order` (module ids), with each
/// module's latency from `latency`.
#[must_use]
pub fn timing(patch: &Patch, order: &[String], latency: impl Fn(&str) -> u32) -> Timing {
    let position: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let forward = |from: &str, to: &str| match (position.get(from), position.get(to)) {
        (Some(from), Some(to)) => from < to,
        _ => false,
    };
    let mut timing = Timing::default();
    let mut arrival: HashMap<&str, u32> = HashMap::new();
    let mut ready: HashMap<&str, u32> = HashMap::new();
    for id in order {
        let late = patch
            .cables()
            .iter()
            .filter(|cable| cable.to_module == *id && forward(&cable.from_module, id))
            .map(|cable| ready.get(cable.from_module.as_str()).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
        arrival.insert(id, late);
        ready.insert(id, late.saturating_add(latency(id)));
    }
    // Every out meets the master bus with the latest of them.
    let is_out = |id: &str| patch.module(id).is_some_and(|m| m.kind == Kind::OUT);
    let master = order
        .iter()
        .filter(|id| is_out(id))
        .map(|id| arrival.get(id.as_str()).copied().unwrap_or(0))
        .max()
        .unwrap_or(0);
    for cable in patch.cables() {
        let (from, to) = (cable.from_module.as_str(), cable.to_module.as_str());
        if !forward(from, to) {
            continue;
        }
        let lead = if is_out(to) {
            master.saturating_sub(arrival.get(to).copied().unwrap_or(0))
        } else {
            0
        };
        let wanted = arrival
            .get(to)
            .copied()
            .unwrap_or(0)
            .saturating_sub(ready.get(from).copied().unwrap_or(0))
            .saturating_add(lead);
        if wanted > u32::from(MAX_CABLE_DELAY) {
            timing.uncompensated.push(cable.id);
        }
        // Held to MAX_CABLE_DELAY, which fits a u16.
        let delay = wanted.min(u32::from(MAX_CABLE_DELAY)) as u16;
        if delay > 0 {
            timing.delays.insert(cable.id, delay);
        }
    }
    for id in order {
        let own = latency(id);
        if own > 0 {
            timing.latency.insert(id.clone(), own);
        }
        // An out is heard with the latest of them: its cables wait for that.
        let late = if is_out(id) {
            master
        } else {
            arrival.get(id.as_str()).copied().unwrap_or(0)
        };
        if late > 0 {
            timing.arrival.insert(id.clone(), late);
        }
    }
    timing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(modules: &[&str], cables: &[(&str, &str)]) -> Patch {
        let mut patch = Patch::empty(120.0);
        for kind in modules {
            patch.add(kind, None, None).unwrap();
        }
        for (from, to) in cables {
            patch.plug(from, to, None, None).unwrap();
        }
        patch
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    /// vca1 is the only module with latency: 40 frames.
    fn vca_latency(id: &str) -> u32 {
        if id == "vca1" { 40 } else { 0 }
    }

    #[test]
    fn a_dry_path_waits_for_its_wet_twin() {
        // vco1 → vca1 (40 frames) → mix1.a, and vco1 → mix1.b dry.
        let wall = patch(
            &["vco", "vca", "mix"],
            &[
                ("vco1.out", "vca1.in"),
                ("vca1.out", "mix1.a"),
                ("vco1.out", "mix1.b"),
            ],
        );
        let timing = timing(&wall, &ids(&["vco1", "vca1", "mix1"]), vca_latency);
        assert_eq!(timing.latency, BTreeMap::from([("vca1".to_string(), 40)]));
        assert_eq!(timing.arrival, BTreeMap::from([("mix1".to_string(), 40)]));
        // Only the dry cable (3) is held back.
        assert_eq!(timing.delays, BTreeMap::from([(3, 40)]));
        assert!(timing.uncompensated.is_empty());
    }

    #[test]
    fn latencies_add_along_a_chain_and_fan_out_is_aligned() {
        // vco1 → vca1 → vca2 → mix1.a, vco1 → mix1.b, vca1 → mix1.c.
        let wall = patch(
            &["vco", "vca", "vca", "mix"],
            &[
                ("vco1.out", "vca1.in"),
                ("vca1.out", "vca2.in"),
                ("vca2.out", "mix1.a"),
                ("vco1.out", "mix1.b"),
                ("vca1.out", "mix1.c"),
            ],
        );
        let latency = |id: &str| match id {
            "vca1" => 40,
            "vca2" => 25,
            _ => 0,
        };
        let timing = timing(&wall, &ids(&["vco1", "vca1", "vca2", "mix1"]), latency);
        assert_eq!(timing.arrival["mix1"], 65);
        assert_eq!(timing.delays, BTreeMap::from([(4, 65), (5, 25)]));
    }

    #[test]
    fn feedback_cables_are_left_alone() {
        // vco1 → vca1 → vco1.fm closes a loop; vca1 renders after vco1.
        let wall = patch(
            &["vco", "vca"],
            &[("vco1.out", "vca1.in"), ("vca1.out", "vco1.fm")],
        );
        let timing = timing(&wall, &ids(&["vco1", "vca1"]), vca_latency);
        assert!(timing.delays.is_empty());
        assert!(!timing.arrival.contains_key("vco1"));
    }

    #[test]
    fn outs_meet_the_master_together() {
        // vco1 → vca1 (40) → out1, and vco2 → out2 straight.
        let wall = patch(
            &["vco", "vca", "out", "vco", "out"],
            &[
                ("vco1.out", "vca1.in"),
                ("vca1.out", "out1.left"),
                ("vco2.out", "out2.left"),
            ],
        );
        let timing = timing(
            &wall,
            &ids(&["vco1", "vca1", "out1", "vco2", "out2"]),
            vca_latency,
        );
        assert_eq!(timing.delays, BTreeMap::from([(3, 40)]));
        // Both outs are heard 40 frames late: that is when out2 arrives.
        assert_eq!(timing.arrival.get("out1"), Some(&40));
        assert_eq!(timing.arrival.get("out2"), Some(&40));
    }

    #[test]
    fn knob_cables_are_timed_like_any_other() {
        // lfo1 → vca1 (40) → vcf1.cutoff, and lfo1 → vcf1.in straight.
        let wall = patch(
            &["lfo", "vca", "vcf"],
            &[
                ("lfo1.out", "vca1.in"),
                ("vca1.out", "vcf1.cutoff"),
                ("lfo1.out", "vcf1.in"),
            ],
        );
        let timing = timing(&wall, &ids(&["lfo1", "vca1", "vcf1"]), vca_latency);
        assert_eq!(timing.arrival["vcf1"], 40);
        assert_eq!(timing.delays, BTreeMap::from([(3, 40)]));
    }

    #[test]
    fn modules_not_in_the_order_are_left_out() {
        // vca1 is not rendered (not in the rack): its cables are not timed,
        // and nothing waits for it.
        let wall = patch(
            &["vco", "vca", "mix"],
            &[
                ("vco1.out", "vca1.in"),
                ("vca1.out", "mix1.a"),
                ("vco1.out", "mix1.b"),
            ],
        );
        let timing = timing(&wall, &ids(&["vco1", "mix1"]), vca_latency);
        assert_eq!(timing, Timing::default());
    }

    #[test]
    fn an_out_past_the_cap_is_capped_and_said_too() {
        // vco1 → vca1 (5 000) → out1, vco2 → out2: out2's cable needs 5 000.
        let wall = patch(
            &["vco", "vca", "out", "vco", "out"],
            &[
                ("vco1.out", "vca1.in"),
                ("vca1.out", "out1.left"),
                ("vco2.out", "out2.left"),
            ],
        );
        let timing = timing(
            &wall,
            &ids(&["vco1", "vca1", "out1", "vco2", "out2"]),
            |id| if id == "vca1" { 5_000 } else { 0 },
        );
        assert_eq!(timing.delays, BTreeMap::from([(3, MAX_CABLE_DELAY)]));
        assert_eq!(timing.uncompensated, vec![3]);
    }

    #[test]
    fn a_delay_past_the_longest_line_is_capped_and_said() {
        let wall = patch(
            &["vco", "vca", "mix"],
            &[
                ("vco1.out", "vca1.in"),
                ("vca1.out", "mix1.a"),
                ("vco1.out", "mix1.b"),
            ],
        );
        let timing = timing(&wall, &ids(&["vco1", "vca1", "mix1"]), |id| {
            if id == "vca1" { 5_000 } else { 0 }
        });
        assert_eq!(timing.delays[&3], MAX_CABLE_DELAY);
        assert_eq!(timing.uncompensated, vec![3]);
    }

    #[test]
    fn nothing_is_delayed_without_latency() {
        let wall = crate::seed::seed().unwrap();
        let order: Vec<String> = wall.modules().iter().map(|m| m.id.clone()).collect();
        assert_eq!(timing(&wall, &order, |_| 0), Timing::default());
    }
}
