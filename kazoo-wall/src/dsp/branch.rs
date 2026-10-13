//! The Bernoulli gate: on each rising gate a coin is tossed, and the gate
//! goes on to output `a` or to output `b`.
//!
//! `chance` is the chance of `b`: 0 sends every gate to `a`, 1 every gate
//! to `b`. In `gate` mode the chosen output follows the incoming gate; in
//! `latch` mode it stays high until a toss picks the other side; in
//! `toggle` mode the toss decides whether to change sides (with `chance`)
//! or stay, and the chosen output follows the gate.
//!
//! The coin is a small generator seeded once, when the module is built,
//! like those of the wall's other random modules (`noise`, `sh`, `seq`); a
//! reset forgets the gate and the side but the coin rolls on.

use super::{Edge, Io, Module, Rng, Tick};
use crate::SUB_BLOCK;

const CHANCE: usize = 0;
const MODE: usize = 1;

const IN_GATE: usize = 0;

const OUT_A: usize = 0;
const OUT_B: usize = 1;

/// The `mode` knob's positions.
const LATCH: usize = 1;
const TOGGLE: usize = 2;

#[derive(Debug)]
pub struct Branch {
    rng: Rng,
    gate: Edge,
    /// Whether the latest toss went to `b`.
    to_b: bool,
    /// Whether there has been a toss since the start or a reset.
    tossed: bool,
}

impl Branch {
    pub const fn new() -> Self {
        Self::seeded(kazoo_fx::dsp::stream_seed(0, 0))
    }

    /// A Bernoulli gate whose coin is seeded with `seed`.
    pub const fn seeded(seed: u32) -> Self {
        Self {
            rng: Rng::new(seed),
            gate: Edge::new(),
            to_b: false,
            tossed: false,
        }
    }
}

/// 1.0 for high, 0.0 for low.
const fn level(high: bool) -> f32 {
    if high { 1.0 } else { 0.0 }
}

impl Module for Branch {
    fn set_seed(&mut self, seed: u64) {
        self.rng = Rng::new(kazoo_fx::dsp::stream_seed(seed, 0));
    }

    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        // Clamped and rounded: the cast is exact.
        let mode = io.knob(MODE) as usize;
        for frame in 0..SUB_BLOCK {
            if self.gate.rising(io.inputs[IN_GATE][frame]) {
                let heads = self.rng.unit() < io.knob_at(CHANCE, frame);
                self.to_b = if mode == TOGGLE {
                    self.to_b != heads
                } else {
                    heads
                };
                self.tossed = true;
            }
            let on = if mode == LATCH {
                self.tossed
            } else {
                self.gate.is_high()
            };
            io.outputs[OUT_A][frame] = level(on && !self.to_b);
            io.outputs[OUT_B][frame] = level(on && self.to_b);
        }
    }

    fn reset(&mut self) {
        self.gate.reset();
        self.to_b = false;
        self.tossed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::BRANCH.spec();
        assert_eq!(spec.knobs[CHANCE].name, "chance");
        assert_eq!(spec.knobs[MODE].name, "mode");
        assert_eq!(spec.knobs[MODE].labels[LATCH], "latch");
        assert_eq!(spec.knobs[MODE].labels[TOGGLE], "toggle");
        assert_eq!(spec.inputs[IN_GATE].name, "in");
        assert_eq!(spec.outputs[OUT_A].name, "a");
        assert_eq!(spec.outputs[OUT_B].name, "b");
    }

    /// A gate rising every 128 frames (four sub-blocks), high for 40.
    fn gates(frame: usize) -> f32 {
        if frame % 128 < 40 { 1.0 } else { 0.0 }
    }

    /// Where each of `count` gates went: `true` for `b`, read in the
    /// middle of each gate. Panics if a gate went both ways or nowhere.
    fn sides(bench: &mut Bench, count: usize) -> Vec<bool> {
        let [a, b, ..] = bench.render_all(count * 128, "in", gates);
        let mut sides = Vec::with_capacity(count);
        for gate in 0..count {
            let at = gate * 128 + 20;
            let (on_a, on_b) = (a[at] > 0.5, b[at] > 0.5);
            assert!(on_a != on_b, "gate {gate}: a {on_a}, b {on_b}");
            sides.push(on_b);
        }
        sides
    }

    #[test]
    fn chance_sends_every_gate_one_way_at_the_ends() {
        let mut bench = Bench::new(Kind::BRANCH);
        bench.knob("chance", 0.0);
        assert!(sides(&mut bench, 200).iter().all(|b| !b));
        bench.knob("chance", 1.0);
        assert!(sides(&mut bench, 200).iter().all(|b| *b));
    }

    #[test]
    fn chance_is_the_share_that_goes_to_b() {
        for chance in [0.25_f32, 0.5, 0.8] {
            let mut bench = Bench::new(Kind::BRANCH);
            bench.knob("chance", chance);
            let sides = sides(&mut bench, 4_000);
            let share = sides.iter().filter(|b| **b).count() as f32 / 4_000.0;
            assert!((share - chance).abs() < 0.03, "{chance}: {share}");
        }
    }

    #[test]
    fn in_gate_mode_the_chosen_output_follows_the_gate() {
        let mut bench = Bench::new(Kind::BRANCH);
        bench.knob("chance", 1.0);
        let b = bench.render_fed(OUT_B, 1_000, Some("in"), gates);
        assert_eq!(b, (0..1_000).map(gates).collect::<Vec<f32>>());
    }

    #[test]
    fn latch_holds_the_side_until_the_coin_changes_it() {
        let mut bench = Bench::new(Kind::BRANCH);
        bench.knob("mode", 1.0).knob("chance", 0.5);
        // Before any gate, both are low.
        assert!(bench.render(OUT_A, 64).iter().all(|s| *s < 0.5));
        assert!(bench.outputs[OUT_B].iter().all(|s| *s < 0.5));
        let frames = 20_000;
        let [a, b, ..] = bench.render_all(frames, "in", gates);
        // Between gates, exactly one side is high, and it changes only as
        // a gate rises.
        for frame in 0..frames {
            assert!((a[frame] > 0.5) != (b[frame] > 0.5), "frame {frame}");
            if frame > 0 && frame % 128 != 0 {
                assert!((a[frame] > 0.5) == (a[frame - 1] > 0.5), "frame {frame}");
            }
        }
        assert!(a.iter().any(|s| *s > 0.5) && b.iter().any(|s| *s > 0.5));
    }

    #[test]
    fn toggle_changes_sides_with_the_chance() {
        let mut bench = Bench::new(Kind::BRANCH);
        bench.knob("mode", 2.0).knob("chance", 1.0);
        let flips = sides(&mut bench, 50);
        for (gate, side) in flips.iter().enumerate() {
            assert_eq!(*side, gate % 2 == 0, "gate {gate}");
        }
        bench.knob("chance", 0.0);
        let stays = sides(&mut bench, 50);
        assert!(stays.iter().all(|b| *b == stays[0]));
    }

    #[test]
    fn the_same_seed_tosses_the_same_coins_and_a_reset_rolls_on() {
        let tosses = |seed: u32, count: usize| {
            let mut bench = Bench::new(Kind::BRANCH);
            bench.module = Box::new(Branch::seeded(seed));
            sides(&mut bench, count)
        };
        let first = tosses(0x1234_5678, 637);
        assert_eq!(first, tosses(0x1234_5678, 637));
        assert_ne!(first, tosses(0x8765_4321, 637));
        // A reset forgets the gate and the side, not the coin: the tosses
        // carry on from where they were, as noise and sh do.
        let mut bench = Bench::new(Kind::BRANCH);
        bench.module = Box::new(Branch::seeded(0x1234_5678));
        sides(&mut bench, 137);
        bench.module.reset();
        assert_eq!(sides(&mut bench, 500), first[137..]);
    }

    #[test]
    fn the_mode_follows_its_jack() {
        // Half the range (1) per 1.0: a cable of 2.0 turns gate to toggle.
        let mut bench = Bench::new(Kind::BRANCH);
        bench.knob("chance", 1.0).hold("mode", 2.0);
        let flips = sides(&mut bench, 20);
        for (gate, side) in flips.iter().enumerate() {
            assert_eq!(*side, gate % 2 == 0, "gate {gate}");
        }
        // And 1.0 to latch: the side stays high between gates.
        let mut bench = Bench::new(Kind::BRANCH);
        bench.knob("chance", 1.0).hold("mode", 1.0);
        let [a, b, ..] = bench.render_all(4 * 128, "in", gates);
        assert!(a.iter().all(|s| *s < 0.5) && b.iter().all(|s| *s > 0.5));
    }

    #[test]
    fn modules_built_one_after_another_toss_differently() {
        let mut one = Bench::seeded(
            Kind::BRANCH,
            super::super::testing::RATE,
            crate::chance::module_seed(0x5EED, "branch1"),
        );
        let mut two = Bench::seeded(
            Kind::BRANCH,
            super::super::testing::RATE,
            crate::chance::module_seed(0x5EED, "branch2"),
        );
        assert_ne!(sides(&mut one, 500), sides(&mut two, 500));
    }

    #[test]
    fn the_chance_follows_its_jack() {
        let mut bench = Bench::new(Kind::BRANCH);
        // A cable of 1.0 moves the chance by half its range: 0 to 0.5.
        bench.knob("chance", 0.0).hold("chance", 1.0);
        let sides = sides(&mut bench, 2_000);
        let share = sides.iter().filter(|b| **b).count() as f32 / 2_000.0;
        assert!((share - 0.5).abs() < 0.05, "{share}");
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::BRANCH, 1.0);
        survives_nonsense(Kind::BRANCH);
    }
}
