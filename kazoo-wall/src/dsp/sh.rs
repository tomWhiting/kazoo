//! Sample and hold: on each rising trigger it samples its input, or its own
//! noise when the input is unplugged, and glides to it over `slew`.

use super::{Edge, Io, Module, Rng, Tick, bounded, seed};
use crate::SUB_BLOCK;

const SLEW: usize = 0;

const IN_SIGNAL: usize = 0;
const IN_TRIG: usize = 1;

#[derive(Debug)]
pub struct SampleHold {
    trig: Edge,
    rng: Rng,
    held: f32,
    out: f32,
}

impl SampleHold {
    pub fn new() -> Self {
        Self {
            trig: Edge::new(),
            rng: Rng::new(seed()),
            held: 0.0,
            out: 0.0,
        }
    }
}

impl Module for SampleHold {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        let slew = io.knob(SLEW);
        let coefficient = if slew > 0.0 {
            1.0 - (-1.0 / (slew * tick.sample_rate)).exp()
        } else {
            1.0
        };
        for frame in 0..SUB_BLOCK {
            if self.trig.rising(io.inputs[IN_TRIG][frame]) {
                self.held = if io.connected[IN_SIGNAL] {
                    bounded(io.inputs[IN_SIGNAL][frame])
                } else {
                    self.rng.bipolar()
                };
            }
            self.out = bounded((self.held - self.out).mul_add(coefficient, self.out));
            io.outputs[0][frame] = self.out;
        }
    }

    fn reset(&mut self) {
        self.trig.reset();
        self.held = 0.0;
        self.out = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::SH.spec();
        assert_eq!(spec.knobs[SLEW].name, "slew");
        assert_eq!(spec.inputs[IN_SIGNAL].name, "in");
        assert_eq!(spec.inputs[IN_TRIG].name, "trig");
    }

    #[test]
    fn it_holds_what_it_sampled() {
        let mut bench = Bench::new(Kind::SH);
        let out = bench.render_fed(
            0,
            256,
            Some("trig"),
            |frame| {
                if frame == 64 { 1.0 } else { 0.0 }
            },
        );
        // Unplugged input: noise was sampled, then held still.
        assert!(out[..64].iter().all(|s| *s == 0.0));
        assert!(out[65..].iter().all(|s| s.to_bits() == out[64].to_bits()));

        let mut bench = Bench::new(Kind::SH);
        bench.hold("in", 0.75).hold("trig", 1.0);
        let out = bench.render(0, 32);
        assert!((out[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn slew_glides_to_the_new_value() {
        let mut bench = Bench::new(Kind::SH);
        bench.knob("slew", 0.1).hold("in", 1.0).hold("trig", 1.0);
        let out = bench.render(0, 48_000);
        assert!(
            out[100] < 0.1 && out[47_000] > 0.99,
            "{} {}",
            out[100],
            out[47_000]
        );
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::SH, CV_LIMIT);
        survives_nonsense(Kind::SH);
    }
}
