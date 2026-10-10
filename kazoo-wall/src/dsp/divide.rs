//! The clock divider: of every `division` rising gates at its input it
//! passes the first on, for as long as that gate stays high.
//!
//! `count` says where in the cycle the latest gate fell, as a ramp from 0
//! up to just under 1 (`k / division` for the `k`th gate, from 0), and
//! holds it until the next gate. A rising `reset` makes the next gate the
//! first of a cycle again, and does nothing else: a gate being passed on
//! lasts until the input falls, and `count` stays where it is until that
//! next gate. A reset and a gate in the same frame pass that gate on.

use super::{Edge, Io, Module, Tick};
use crate::SUB_BLOCK;

const DIVISION: usize = 0;

const IN_GATE: usize = 0;
const IN_RESET: usize = 1;

const OUT_GATE: usize = 0;
const OUT_COUNT: usize = 1;

#[derive(Debug)]
pub struct Divide {
    /// Where in the cycle the next gate falls.
    next: u32,
    /// Whether the gate high at the input is being passed on.
    passing: bool,
    /// The `count` output.
    count: f32,
    gate: Edge,
    reset: Edge,
}

impl Divide {
    pub const fn new() -> Self {
        Self {
            next: 0,
            passing: false,
            count: 0.0,
            gate: Edge::new(),
            reset: Edge::new(),
        }
    }
}

impl Module for Divide {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for frame in 0..SUB_BLOCK {
            // A reset restarts the count only: a gate being passed on lasts
            // until the input falls, and `count` keeps saying where the
            // latest gate fell until the next one.
            if self.reset.rising(io.inputs[IN_RESET][frame]) {
                self.next = 0;
            }
            if self.gate.rising(io.inputs[IN_GATE][frame]) {
                // Held to 1..=32 and rounded: the cast is exact.
                let division = (io.knob_at(DIVISION, frame) as u32).max(1);
                let this = self.next % division;
                self.passing = this == 0;
                self.next = (this + 1) % division;
                // Both at most 32: exact.
                self.count = this as f32 / division as f32;
            } else if !self.gate.is_high() {
                self.passing = false;
            }
            io.outputs[OUT_GATE][frame] = if self.passing { 1.0 } else { 0.0 };
            io.outputs[OUT_COUNT][frame] = self.count;
        }
    }

    fn reset(&mut self) {
        self.next = 0;
        self.passing = false;
        self.count = 0.0;
        self.gate.reset();
        self.reset.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::DIVIDE.spec();
        assert_eq!(spec.knobs[DIVISION].name, "division");
        assert_eq!(spec.inputs[IN_GATE].name, "in");
        assert_eq!(spec.inputs[IN_RESET].name, "reset");
        assert_eq!(spec.outputs[OUT_GATE].name, "out");
        assert_eq!(spec.outputs[OUT_COUNT].name, "count");
    }

    /// Frames between gates: four sub-blocks, so renders of whole gaps end
    /// where they say.
    const GAP: usize = 128;

    /// A gate rising every [`GAP`] frames, high for 30.
    fn gates(frame: usize) -> f32 {
        if frame % GAP < 30 { 1.0 } else { 0.0 }
    }

    /// The gates (by number, from 0) passed on in `out`.
    fn passed(out: &[f32]) -> Vec<usize> {
        (0..out.len() / GAP)
            .filter(|gate| out[gate * GAP + 10] > 0.5)
            .collect()
    }

    #[test]
    fn it_passes_the_first_of_every_division() {
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 3.0);
        let [out, count, ..] = bench.render_all(10 * GAP, "in", gates);
        assert_eq!(passed(&out), [0, 3, 6, 9]);
        // A passed gate lasts as long as the gate it passes on.
        assert_eq!(out[..GAP].iter().filter(|s| **s > 0.5).count(), 30);
        for gate in 0..10 {
            let expected = (gate % 3) as f32 / 3.0;
            assert!((count[gate * GAP + 50] - expected).abs() < 1e-6, "{gate}");
        }
    }

    #[test]
    fn a_division_of_one_passes_everything() {
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 1.0);
        let out = bench.render_fed(OUT_GATE, 10 * GAP, Some("in"), gates);
        assert_eq!(out, (0..10 * GAP).map(gates).collect::<Vec<f32>>());
        let count = bench.render(OUT_COUNT, 32);
        assert!(count.iter().all(|s| s.abs() < f32::EPSILON));
    }

    #[test]
    fn reset_starts_the_cycle_again() {
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 4.0);
        // Two gates in, then a reset on its own: the count still says
        // where the latest gate fell (1 of 4) until the next gate, which
        // is passed on as the first of a cycle.
        bench.render_fed(OUT_GATE, 2 * GAP, Some("in"), gates);
        bench.hold("in", 0.0).hold("reset", 1.0).step();
        assert!(
            bench.outputs[OUT_COUNT]
                .iter()
                .all(|s| (s - 0.25).abs() < f32::EPSILON)
        );
        bench.hold("reset", 0.0).step();
        let [out, count, ..] = bench.render_all(5 * GAP, "in", gates);
        assert_eq!(passed(&out), [0, 4]);
        assert!(count[0].abs() < f32::EPSILON && (count[GAP] - 0.25).abs() < 1e-6);
        // A reset and a gate together: that gate is the first.
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 4.0);
        bench.render_fed(OUT_GATE, 2 * GAP, Some("in"), gates);
        bench.hold("reset", 1.0);
        let out = bench.render_fed(OUT_GATE, GAP, Some("in"), gates);
        assert_eq!(passed(&out), [0]);
    }

    /// Whether the gate output is high all through a sub-block of `input`
    /// (`Some(true)`), low all through (`Some(false)`), or neither.
    fn through(bench: &mut Bench, input: f32) -> Option<bool> {
        bench.hold("in", input).step();
        let out = &bench.outputs[OUT_GATE];
        let high = out[0] > 0.5;
        out.iter().all(|s| (*s > 0.5) == high).then_some(high)
    }

    #[test]
    fn a_reset_mid_gate_lets_the_passed_gate_finish() {
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 2.0);
        assert_eq!(through(&mut bench, 0.0), Some(false));
        assert_eq!(through(&mut bench, 1.0), Some(true), "the first is passed");
        bench.hold("reset", 1.0);
        assert_eq!(
            through(&mut bench, 1.0),
            Some(true),
            "a reset leaves it high"
        );
        assert_eq!(
            through(&mut bench, 0.0),
            Some(false),
            "it ends as the input falls"
        );
        // The reset restarted the count: the next gate is the first again.
        bench.hold("reset", 0.0);
        assert_eq!(through(&mut bench, 1.0), Some(true));
    }

    #[test]
    fn a_division_changed_mid_gate_counts_from_the_next_gate() {
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 4.0);
        assert_eq!(through(&mut bench, 1.0), Some(true));
        // Down to 1 through the jack (-15.5 per 1.0) while the gate is high:
        // the gate passed on goes on, and every gate from the next passes.
        bench.hold("division", -1.0);
        assert_eq!(through(&mut bench, 1.0), Some(true));
        assert_eq!(through(&mut bench, 0.0), Some(false));
        let out = bench.render_fed(OUT_GATE, 4 * GAP, Some("in"), gates);
        assert_eq!(passed(&out), [0, 1, 2, 3]);
        // Up to 3 mid-gate on a gate not passed on: it stays low.
        let mut bench = Bench::new(Kind::DIVIDE);
        bench.knob("division", 2.0);
        assert_eq!(through(&mut bench, 1.0), Some(true));
        assert_eq!(through(&mut bench, 0.0), Some(false));
        assert_eq!(through(&mut bench, 1.0), Some(false));
        bench.hold("division", 2.0 / 31.0);
        assert_eq!(through(&mut bench, 1.0), Some(false));
        assert_eq!(through(&mut bench, 0.0), Some(false));
        // That gate was 1 of 2, so the next is 0 of 3: passed.
        assert_eq!(through(&mut bench, 1.0), Some(true));
    }

    #[test]
    fn the_division_follows_its_jack_and_a_smaller_one_wraps_the_count() {
        let mut bench = Bench::new(Kind::DIVIDE);
        // Half the range (15.5) per 1.0: 2/31 moves it from 2 to 3.
        bench.knob("division", 2.0).hold("division", 2.0 / 31.0);
        let out = bench.render_fed(OUT_GATE, 7 * GAP, Some("in"), gates);
        assert_eq!(passed(&out), [0, 3, 6]);
        // Seven gates in, the next falls at 1 of 3; at a division of 2 that
        // is 1 of 2, then 0.
        bench.hold("division", 0.0);
        let out = bench.render_fed(OUT_GATE, 4 * GAP, Some("in"), gates);
        assert_eq!(passed(&out), [1, 3]);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::DIVIDE, 1.0);
        survives_nonsense(Kind::DIVIDE);
    }
}
