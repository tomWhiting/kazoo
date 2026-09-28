//! The mixer: four inputs, each with its own level, summed.

use super::{Io, Module, Tick, bounded};
use crate::SUB_BLOCK;

/// Inputs, and the knob for each, in the same order.
const CHANNELS: usize = 4;

#[derive(Debug)]
pub struct Mix;

impl Mix {
    pub const fn new() -> Self {
        Self
    }
}

impl Module for Mix {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for frame in 0..SUB_BLOCK {
            let mut sum = 0.0_f32;
            for (channel, input) in io.inputs.iter().enumerate().take(CHANNELS) {
                sum = bounded(input[frame]).mul_add(io.knob_at(channel, frame), sum);
            }
            io.outputs[0][frame] = bounded(sum);
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;

    #[test]
    fn knobs_and_inputs_pair_up() {
        let spec = Kind::MIX.spec();
        for (index, name) in ["a", "b", "c", "d"].iter().enumerate() {
            assert_eq!(spec.knobs[index].name, format!("level_{name}"));
            assert_eq!(spec.inputs[index].name, *name);
        }
    }

    #[test]
    fn it_sums_with_levels() {
        let mut bench = Bench::new(Kind::MIX);
        bench
            .knob("level_a", 1.0)
            .knob("level_b", 0.5)
            .knob("level_c", 0.0)
            .hold("a", 0.2)
            .hold("b", 0.4)
            .hold("c", 1.0);
        let out = bench.render(0, 32);
        assert!((out[0] - 0.4).abs() < 1e-6, "{}", out[0]);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::MIX, CV_LIMIT);
        survives_nonsense(Kind::MIX);
    }
}
