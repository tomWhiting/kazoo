//! The slew limiter: its output follows its input no faster than 1.0 per
//! `rise` seconds going up and 1.0 per `fall` seconds going down.

use super::{Io, Module, Tick, bounded};
use crate::SUB_BLOCK;

const RISE: usize = 0;
const FALL: usize = 1;

const IN_SIGNAL: usize = 0;

#[derive(Debug)]
pub struct Slew {
    out: f32,
}

impl Slew {
    pub const fn new() -> Self {
        Self { out: 0.0 }
    }
}

/// Largest move per frame for a time of `seconds` per 1.0.
fn rate(seconds: f32, sample_rate: f32) -> f32 {
    if seconds > 0.0 {
        1.0 / (seconds * sample_rate)
    } else {
        f32::MAX
    }
}

impl Module for Slew {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        let up = rate(io.knob(RISE), tick.sample_rate);
        let down = rate(io.knob(FALL), tick.sample_rate);
        for frame in 0..SUB_BLOCK {
            let target = bounded(io.inputs[IN_SIGNAL][frame]);
            let delta = (target - self.out).clamp(-down, up);
            self.out = bounded(self.out + delta);
            io.outputs[0][frame] = self.out;
        }
    }

    fn reset(&mut self) {
        self.out = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::SLEW.spec();
        assert_eq!(spec.knobs[RISE].name, "rise");
        assert_eq!(spec.knobs[FALL].name, "fall");
        assert_eq!(spec.inputs[IN_SIGNAL].name, "in");
    }

    #[test]
    fn rise_and_fall_take_their_time() {
        let mut bench = Bench::new(Kind::SLEW);
        bench.knob("rise", 0.5).knob("fall", 0.1).hold("in", 1.0);
        let up = bench.render(0, (RATE * 0.6) as usize);
        let halfway = up.iter().position(|s| *s >= 0.5).unwrap();
        assert!(
            RATE.mul_add(-0.25, halfway as f32).abs() < 64.0,
            "{halfway}"
        );
        assert!((up[up.len() - 1] - 1.0).abs() < 1e-6);
        bench.hold("in", 0.0);
        let down = bench.render(0, (RATE * 0.2) as usize);
        let halfway = down.iter().position(|s| *s <= 0.5).unwrap();
        assert!(
            RATE.mul_add(-0.05, halfway as f32).abs() < 64.0,
            "{halfway}"
        );
    }

    #[test]
    fn zero_time_passes_straight_through() {
        let mut bench = Bench::new(Kind::SLEW);
        bench.knob("rise", 0.0).knob("fall", 0.0).hold("in", 0.7);
        assert!((bench.render(0, 32)[0] - 0.7).abs() < 1e-6);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::SLEW, CV_LIMIT);
        survives_nonsense(Kind::SLEW);
    }
}
