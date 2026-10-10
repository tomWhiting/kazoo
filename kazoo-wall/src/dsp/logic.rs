//! Gate logic: `and`, `or`, `xor` and `nand` of two gates.
//!
//! An input is high above `threshold` (0.5, the wall's gate level, unless
//! turned). An unplugged `b` reads as `a`, so with only `a` in, `and` and
//! `or` pass it through and `nand` is `not a`, an inverter. The normalling
//! runs one way only: an unplugged `a` reads low, as any unplugged input
//! does, so with only `b` in, `and` is low, `or` and `xor` follow `b`, and
//! `nand` is high.

use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;

const THRESHOLD: usize = 0;

const IN_A: usize = 0;
const IN_B: usize = 1;

const OUT_AND: usize = 0;
const OUT_OR: usize = 1;
const OUT_XOR: usize = 2;
const OUT_NAND: usize = 3;

#[derive(Debug)]
pub struct Logic;

impl Logic {
    pub const fn new() -> Self {
        Self
    }
}

/// 1.0 for high, 0.0 for low.
const fn level(high: bool) -> f32 {
    if high { 1.0 } else { 0.0 }
}

impl Module for Logic {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for frame in 0..SUB_BLOCK {
            let threshold = io.knob_at(THRESHOLD, frame);
            let a = finite(io.inputs[IN_A][frame]) > threshold;
            let b = if io.connected[IN_B] {
                finite(io.inputs[IN_B][frame]) > threshold
            } else {
                a
            };
            io.outputs[OUT_AND][frame] = level(a && b);
            io.outputs[OUT_OR][frame] = level(a || b);
            io.outputs[OUT_XOR][frame] = level(a != b);
            io.outputs[OUT_NAND][frame] = level(!(a && b));
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::LOGIC.spec();
        assert_eq!(spec.knobs[THRESHOLD].name, "threshold");
        assert_eq!(spec.inputs[IN_A].name, "a");
        assert_eq!(spec.inputs[IN_B].name, "b");
        assert_eq!(spec.outputs[OUT_AND].name, "and");
        assert_eq!(spec.outputs[OUT_OR].name, "or");
        assert_eq!(spec.outputs[OUT_XOR].name, "xor");
        assert_eq!(spec.outputs[OUT_NAND].name, "nand");
    }

    /// The four outputs, high or low, after a sub-block.
    fn read(bench: &mut Bench) -> [bool; 4] {
        bench.step();
        let mut highs = [false; 4];
        for (high, port) in highs.iter_mut().zip(&bench.outputs) {
            assert!(port.iter().all(|s| (*s > 0.5) == (port[0] > 0.5)));
            *high = port[0] > 0.5;
        }
        highs
    }

    /// The four outputs for `a` and `b` held a sub-block.
    fn outputs(bench: &mut Bench, a: f32, b: f32) -> [bool; 4] {
        bench.hold("a", a).hold("b", b);
        read(bench)
    }

    #[test]
    fn the_truth_table_holds() {
        let mut bench = Bench::new(Kind::LOGIC);
        // and, or, xor, nand.
        assert_eq!(outputs(&mut bench, 0.0, 0.0), [false, false, false, true]);
        assert_eq!(outputs(&mut bench, 1.0, 0.0), [false, true, true, true]);
        assert_eq!(outputs(&mut bench, 0.0, 1.0), [false, true, true, true]);
        assert_eq!(outputs(&mut bench, 1.0, 1.0), [true, true, false, false]);
        // High is above 0.5, not at it.
        assert_eq!(outputs(&mut bench, 0.5, 0.51), [false, true, true, true]);
    }

    #[test]
    fn an_unplugged_b_follows_a_so_nand_inverts() {
        let mut bench = Bench::new(Kind::LOGIC);
        bench.hold("a", 1.0);
        assert_eq!(read(&mut bench), [true, true, false, false]);
        bench.hold("a", 0.0);
        assert_eq!(read(&mut bench), [false, false, false, true]);
    }

    #[test]
    fn an_unplugged_a_reads_low_so_b_alone_only_reaches_or_and_xor() {
        let mut bench = Bench::new(Kind::LOGIC);
        bench.hold("b", 1.0);
        assert_eq!(read(&mut bench), [false, true, true, true]);
        bench.hold("b", 0.0);
        assert_eq!(read(&mut bench), [false, false, false, true]);
    }

    #[test]
    fn the_threshold_reads_any_level_as_a_gate() {
        let mut bench = Bench::new(Kind::LOGIC);
        bench.knob("threshold", 0.1);
        assert_eq!(outputs(&mut bench, 0.2, 0.05), [false, true, true, true]);
        // A cable of 1.0 in the threshold's jack moves it by half its
        // range, 0.45: up to 0.95.
        bench.knob("threshold", 0.5).hold("threshold", 1.0);
        assert_eq!(outputs(&mut bench, 0.9, 1.1), [false, true, true, true]);
    }

    #[test]
    fn nonsense_reads_as_low() {
        let mut bench = Bench::new(Kind::LOGIC);
        assert_eq!(
            outputs(&mut bench, f32::NAN, f32::INFINITY),
            [false, false, false, true]
        );
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::LOGIC, 1.0);
        survives_nonsense(Kind::LOGIC);
    }
}
