//! CV maths: two inputs, each scaled and offset, then summed, compared and
//! rectified.
//!
//! Each input is `scale × input + offset` (an unplugged input is 0, so its
//! offset alone makes a steady voltage). From those two, `a′` and `b′`:
//! `sum` is `a′ + b′`, `min` and `max` are the lesser and the larger of the
//! two, and `abs` is `|a′ + b′|`, a full-wave rectifier (of `a′` alone
//! when `b` is unplugged and its offset 0). With `scale_b` at -1 and
//! `offset_b` at 0, `sum` is a difference: `a′ − b`.

use super::{Io, Module, Tick, bounded};
use crate::SUB_BLOCK;

const SCALE_A: usize = 0;
const OFFSET_A: usize = 1;
const SCALE_B: usize = 2;
const OFFSET_B: usize = 3;

const IN_A: usize = 0;
const IN_B: usize = 1;

const OUT_SUM: usize = 0;
const OUT_MIN: usize = 1;
const OUT_MAX: usize = 2;
const OUT_ABS: usize = 3;

#[derive(Debug)]
pub struct Math;

impl Math {
    pub const fn new() -> Self {
        Self
    }
}

impl Module for Math {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for frame in 0..SUB_BLOCK {
            let a = bounded(io.inputs[IN_A][frame])
                .mul_add(io.knob_at(SCALE_A, frame), io.knob_at(OFFSET_A, frame));
            let b = bounded(io.inputs[IN_B][frame])
                .mul_add(io.knob_at(SCALE_B, frame), io.knob_at(OFFSET_B, frame));
            io.outputs[OUT_SUM][frame] = bounded(a + b);
            io.outputs[OUT_MIN][frame] = bounded(a.min(b));
            io.outputs[OUT_MAX][frame] = bounded(a.max(b));
            io.outputs[OUT_ABS][frame] = bounded((a + b).abs());
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::MATH.spec();
        assert_eq!(spec.knobs[SCALE_A].name, "scale_a");
        assert_eq!(spec.knobs[OFFSET_A].name, "offset_a");
        assert_eq!(spec.knobs[SCALE_B].name, "scale_b");
        assert_eq!(spec.knobs[OFFSET_B].name, "offset_b");
        assert_eq!(spec.inputs[IN_A].name, "a");
        assert_eq!(spec.inputs[IN_B].name, "b");
        assert_eq!(spec.outputs[OUT_SUM].name, "sum");
        assert_eq!(spec.outputs[OUT_MIN].name, "min");
        assert_eq!(spec.outputs[OUT_MAX].name, "max");
        assert_eq!(spec.outputs[OUT_ABS].name, "abs");
    }

    /// Every output after a sub-block of `a` and `b`, checked steady.
    fn outputs(bench: &mut Bench, a: f32, b: f32) -> [f32; 4] {
        bench.hold("a", a).hold("b", b).step();
        let mut values = [0.0; 4];
        for (value, port) in values.iter_mut().zip(&bench.outputs) {
            assert!(port.iter().all(|s| s.to_bits() == port[0].to_bits()));
            *value = port[0];
        }
        values
    }

    fn near(got: [f32; 4], expected: [f32; 4]) -> bool {
        got.iter().zip(expected).all(|(g, e)| (g - e).abs() < 1e-6)
    }

    #[test]
    fn it_sums_compares_and_rectifies() {
        let mut bench = Bench::new(Kind::MATH);
        // sum, min, max, abs.
        let got = outputs(&mut bench, 0.3, -0.5);
        assert!(near(got, [-0.2, -0.5, 0.3, 0.2]), "{got:?}");
        let got = outputs(&mut bench, -0.7, -0.1);
        assert!(near(got, [-0.8, -0.7, -0.1, 0.8]), "{got:?}");
    }

    #[test]
    fn scales_and_offsets_shape_each_input() {
        let mut bench = Bench::new(Kind::MATH);
        bench
            .knob("scale_a", 2.0)
            .knob("offset_a", 0.1)
            .knob("scale_b", -1.0)
            .knob("offset_b", -0.2);
        // a′ = 2 × 0.25 + 0.1 = 0.6; b′ = -0.5 - 0.2 = -0.7.
        let got = outputs(&mut bench, 0.25, 0.5);
        assert!(near(got, [-0.1, -0.7, 0.6, 0.1]), "{got:?}");
    }

    #[test]
    fn unplugged_inputs_are_their_offsets_and_abs_rectifies_a_alone() {
        let mut bench = Bench::new(Kind::MATH);
        bench.knob("offset_b", 0.4);
        assert!((bench.render(OUT_SUM, 32)[0] - 0.4).abs() < 1e-6);
        let mut bench = Bench::new(Kind::MATH);
        let rectified = bench.render_fed(OUT_ABS, 960, Some("a"), |frame| {
            (std::f32::consts::TAU * frame as f32 / 96.0).sin()
        });
        for (frame, value) in rectified.iter().enumerate() {
            let input = (std::f32::consts::TAU * frame as f32 / 96.0).sin();
            assert!((value - input.abs()).abs() < 1e-6, "frame {frame}");
        }
    }

    #[test]
    fn the_knobs_follow_their_jacks() {
        let mut bench = Bench::new(Kind::MATH);
        // Half of each range per 1.0: scale_a (-2..2) by 2, offset_b
        // (-1..1) by 1.
        bench.hold("scale_a", 0.5).hold("offset_b", -0.5);
        let got = outputs(&mut bench, 0.25, 0.0);
        assert!(near(got, [0.0, -0.5, 0.5, 0.0]), "{got:?}");
        // scale_b by 2 and offset_a by 1 per 1.0: scale_b from 1 to -1,
        // offset_a from 0 to 0.3; a′ = 0.55, b′ = -0.4.
        let mut bench = Bench::new(Kind::MATH);
        bench.hold("scale_b", -1.0).hold("offset_a", 0.3);
        let got = outputs(&mut bench, 0.25, 0.4);
        assert!(near(got, [0.15, -0.4, 0.55, 0.15]), "{got:?}");
    }

    #[test]
    fn large_and_nonsense_inputs_are_held() {
        let mut bench = Bench::new(Kind::MATH);
        bench.knob("scale_a", 2.0).knob("scale_b", 2.0);
        let got = outputs(&mut bench, 6.0, 6.0);
        assert!(
            near(got, [CV_LIMIT, CV_LIMIT, CV_LIMIT, CV_LIMIT]),
            "{got:?}"
        );
        let got = outputs(&mut bench, f32::NAN, f32::NEG_INFINITY);
        assert!(near(got, [0.0, 0.0, 0.0, 0.0]), "{got:?}");
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::MATH, CV_LIMIT);
        survives_nonsense(Kind::MATH);
    }
}
