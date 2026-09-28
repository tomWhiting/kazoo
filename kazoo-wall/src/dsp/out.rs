//! The output: level and pan into the master bus.
//!
//! With only one input plugged in it is mono and `pan` places it
//! (equal-power); with both it is stereo and `pan` is a balance. It writes
//! the left and right feed to its first two output buffers, which the engine
//! sums into the master bus; they are not patchable.

use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;

const LEVEL: usize = 0;
const PAN: usize = 1;

const IN_LEFT: usize = 0;
const IN_RIGHT: usize = 1;

/// The master feed buffers.
pub const FEED_LEFT: usize = 0;
pub const FEED_RIGHT: usize = 1;

#[derive(Debug)]
pub struct Out;

impl Out {
    pub const fn new() -> Self {
        Self
    }
}

/// Equal-power gains for `pan` (-1 left, +1 right).
fn gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

impl Module for Out {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let stereo = io.connected[IN_LEFT] && io.connected[IN_RIGHT];
        let pan_moves = io.knob_moves(PAN);
        let mut mono_gains = gains(io.knob_at(PAN, 0));
        for frame in 0..SUB_BLOCK {
            let level = io.knob_at(LEVEL, frame);
            let pan = io.knob_at(PAN, frame);
            let left = finite(io.inputs[IN_LEFT][frame]).clamp(-16.0, 16.0);
            let right = finite(io.inputs[IN_RIGHT][frame]).clamp(-16.0, 16.0);
            let (l, r) = if stereo {
                // Balance: the far side fades, the near side stays.
                let l_gain = (1.0 - pan).min(1.0);
                let r_gain = (1.0 + pan).min(1.0);
                (left * l_gain, right * r_gain)
            } else {
                let mono = left + right;
                if pan_moves {
                    mono_gains = gains(pan);
                }
                let (l_gain, r_gain) = mono_gains;
                (mono * l_gain, mono * r_gain)
            };
            io.outputs[FEED_LEFT][frame] = l * level;
            io.outputs[FEED_RIGHT][frame] = r * level;
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
        let spec = Kind::OUT.spec();
        assert_eq!(spec.knobs[LEVEL].name, "level");
        assert_eq!(spec.knobs[PAN].name, "pan");
        assert_eq!(spec.inputs[IN_LEFT].name, "left");
        assert_eq!(spec.inputs[IN_RIGHT].name, "right");
    }

    #[test]
    fn mono_pans_and_stereo_balances() {
        let mut bench = Bench::new(Kind::OUT);
        bench.knob("level", 1.0).knob("pan", 0.0).hold("left", 1.0);
        bench.step();
        let (l, r) = (bench.outputs[FEED_LEFT][0], bench.outputs[FEED_RIGHT][0]);
        assert!((l - r).abs() < 1e-6 && (l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);

        let mut bench = Bench::new(Kind::OUT);
        bench.knob("level", 1.0).knob("pan", 1.0).hold("left", 1.0);
        bench.step();
        assert!(bench.outputs[FEED_LEFT][0].abs() < 1e-6);

        let mut bench = Bench::new(Kind::OUT);
        bench
            .knob("level", 0.5)
            .knob("pan", -1.0)
            .hold("left", 1.0)
            .hold("right", 1.0);
        bench.step();
        assert!((bench.outputs[FEED_LEFT][0] - 0.5).abs() < 1e-6);
        assert!(bench.outputs[FEED_RIGHT][0].abs() < 1e-6);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::OUT, 32.0);
        survives_nonsense(Kind::OUT);
    }
}
