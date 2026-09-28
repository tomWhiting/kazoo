//! Any `kazoo-fx` effect as a wall module.
//!
//! The effect is built and prepared for the stream's rate on the control
//! side, before the engine ever sees it. Its knobs are its parameters, in
//! order; each is handed over only when it changes (the effect smooths it).
//! `right` is mono-normalled: with nothing plugged into it, the effect hears
//! `left` on both sides.

use kazoo_fx::{Context, Effect, EffectKind};

use super::{Block, Io, Module, Tick, finite};
use crate::{MAX_KNOBS, SUB_BLOCK};

const IN_LEFT: usize = 0;
const IN_RIGHT: usize = 1;

/// Output bound: an effect's own caps keep it far inside this.
const LIMIT: f32 = 16.0;

/// A prepared effect and the knob values it holds.
#[derive(Debug)]
pub struct EffectModule {
    effect: Box<dyn Effect>,
    /// Values handed to the effect; NaN until the first block.
    applied: [f32; MAX_KNOBS],
    left: Block,
    right: Block,
}

impl EffectModule {
    /// Build `kind` and prepare it for `sample_rate` (this allocates).
    #[must_use]
    pub fn new(kind: &'static EffectKind, sample_rate: f32) -> Self {
        let mut effect = (kind.build)();
        effect.prepare(sample_rate);
        Self {
            effect,
            applied: [f32::NAN; MAX_KNOBS],
            left: [0.0; SUB_BLOCK],
            right: [0.0; SUB_BLOCK],
        }
    }
}

impl Module for EffectModule {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        for index in 0..io.spec.knobs.len().min(MAX_KNOBS) {
            let value = io.knob(index);
            // Unset values are NaN, which no knob value matches.
            if self.applied[index].to_bits() != value.to_bits() {
                self.effect.set_param(index, value);
                self.applied[index] = value;
            }
        }
        let stereo = io.connected[IN_RIGHT];
        for frame in 0..SUB_BLOCK {
            let left = finite(io.inputs[IN_LEFT][frame]).clamp(-LIMIT, LIMIT);
            self.left[frame] = left;
            self.right[frame] = if stereo {
                finite(io.inputs[IN_RIGHT][frame]).clamp(-LIMIT, LIMIT)
            } else {
                left
            };
        }
        let (first, rest) = io.outputs.split_at_mut(1);
        let context = Context { bpm: tick.bpm };
        self.effect.process(
            &context,
            [&self.left, &self.right],
            [&mut first[0], &mut rest[0]],
        );
        for port in io.outputs.iter_mut().take(2) {
            for sample in port.iter_mut() {
                *sample = finite(*sample).clamp(-LIMIT, LIMIT);
            }
        }
    }

    fn reset(&mut self) {
        self.effect.reset();
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Test-only effects: one that works, and some the registry must refuse.

    use kazoo_fx::{Context, Curve, Effect, EffectKind, ParamSpec};

    /// A gain in dB, with a soft or hard clip, silent until prepared.
    #[derive(Debug, Default)]
    pub struct TestGain {
        prepared: bool,
        gain: f32,
        hard: bool,
    }

    impl Effect for TestGain {
        fn prepare(&mut self, sample_rate: f32) {
            self.prepared = sample_rate > 0.0;
        }

        fn reset(&mut self) {}

        fn set_param(&mut self, index: usize, value: f32) {
            if value.is_nan() {
                return;
            }
            match index {
                0 => self.gain = (value / 20.0 * std::f32::consts::LN_10).exp(),
                1 => self.hard = value >= 0.5,
                _ => {}
            }
        }

        fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
            let [out_left, out_right] = output;
            for (outs, ins) in [(out_left, input[0]), (out_right, input[1])] {
                for (out, sample) in outs.iter_mut().zip(ins) {
                    let value = if self.prepared {
                        sample * self.gain
                    } else {
                        0.0
                    };
                    *out = if self.hard {
                        value.clamp(-0.5, 0.5)
                    } else {
                        value
                    };
                }
            }
        }
    }

    fn build() -> Box<dyn Effect> {
        Box::new(TestGain::default())
    }

    const GAIN: ParamSpec = ParamSpec {
        name: "gain",
        min: -24.0,
        max: 24.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    };

    const MODE: ParamSpec = ParamSpec {
        name: "mode",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["soft", "hard"],
        },
    };

    const fn kind(id: &'static str, params: &'static [ParamSpec]) -> EffectKind {
        EffectKind {
            id,
            name: "Test gain",
            description: "a gain for testing the adapter",
            params,
            build,
        }
    }

    /// Effects the wall takes.
    pub static KINDS: [EffectKind; 1] = [kind("testgain", &[GAIN, MODE])];

    /// An effect whose id ends in digits (as `kazoo-sampler`'s may).
    pub static DIGIT_KINDS: [EffectKind; 1] = [kind("sampler12", &[GAIN])];

    /// Effects the wall must refuse: a bad id, a taken name, a parameter
    /// named like an input, a parameter with no range.
    pub static BAD_KINDS: [EffectKind; 4] = [
        kind("Bad-Id", &[GAIN]),
        kind("vco", &[GAIN]),
        kind(
            "leftish",
            &[ParamSpec {
                name: "left",
                ..GAIN
            }],
        ),
        kind(
            "flat",
            &[ParamSpec {
                min: 1.0,
                max: 1.0,
                ..GAIN
            }],
        ),
    ];
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use crate::catalogue::Kind;

    fn kind() -> Kind {
        Kind::from_name("testgain").unwrap()
    }

    #[test]
    fn it_is_prepared_and_right_is_mono_normalled() {
        let mut bench = Bench::new(kind());
        bench.hold("left", 0.25);
        bench.step();
        assert!((bench.outputs[0][0] - 0.25).abs() < 1e-6);
        assert!((bench.outputs[1][0] - 0.25).abs() < 1e-6);
        bench.hold("right", -0.1);
        bench.step();
        assert!((bench.outputs[1][0] + 0.1).abs() < 1e-6);
    }

    #[test]
    fn knobs_and_their_jacks_reach_the_effect() {
        let mut bench = Bench::new(kind());
        bench.knob("gain", 20.0).hold("left", 0.1);
        bench.step();
        assert!(
            (bench.outputs[0][0] - 1.0).abs() < 1e-4,
            "{}",
            bench.outputs[0][0]
        );
        bench.knob("mode", 1.0);
        bench.step();
        assert!((bench.outputs[0][0] - 0.5).abs() < 1e-6);
        // A jack on gain: -1 takes it down half the range (24 dB).
        bench.knob("mode", 0.0).knob("gain", 0.0).hold("gain", -1.0);
        bench.step();
        // 0.1 at -24 dB.
        assert!((bench.outputs[0][0] - 0.006_309_57).abs() < 1e-5);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(kind(), 16.0);
        survives_nonsense(kind());
    }
}
