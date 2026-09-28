//! The ADSR envelope, with analogue-style exponential segments.
//!
//! A rising gate starts the attack from wherever the envelope is, so fast
//! retriggers do not click; a falling gate starts the release.

use super::{Edge, Io, Module, Tick};
use crate::SUB_BLOCK;

const ATTACK: usize = 0;
const DECAY: usize = 1;
const SUSTAIN: usize = 2;
const RELEASE: usize = 3;

const IN_GATE: usize = 0;

/// The attack aims past 1.0 so its exponential curve arrives in finite time.
const ATTACK_TARGET: f32 = 1.2;

/// Segments are specified to fall to within this fraction of their target.
const SETTLE: f32 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

#[derive(Debug)]
pub struct Env {
    stage: Stage,
    level: f32,
    gate: Edge,
}

impl Env {
    pub const fn new() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            gate: Edge::new(),
        }
    }
}

/// Per-sample coefficient for an exponential segment of `seconds`.
fn coefficient(seconds: f32, sample_rate: f32) -> f32 {
    let frames = (seconds * sample_rate).max(1.0);
    1.0 - SETTLE.powf(1.0 / frames)
}

impl Module for Env {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        let rate = tick.sample_rate;
        // The attack is timed to reach 1.0, not its overshoot target.
        let attack = coefficient(io.knob(ATTACK), rate) * 0.3;
        let decay = coefficient(io.knob(DECAY), rate);
        let sustain = io.knob(SUSTAIN);
        let release = coefficient(io.knob(RELEASE), rate);
        for frame in 0..SUB_BLOCK {
            let rose = self.gate.rising(io.inputs[IN_GATE][frame]);
            if rose {
                self.stage = Stage::Attack;
            } else if !self.gate.is_high() && matches!(self.stage, Stage::Attack | Stage::Decay) {
                self.stage = Stage::Release;
            }
            match self.stage {
                Stage::Idle => self.level = 0.0,
                Stage::Attack => {
                    self.level = (ATTACK_TARGET - self.level).mul_add(attack, self.level);
                    if self.level >= 1.0 {
                        self.level = 1.0;
                        self.stage = Stage::Decay;
                    }
                }
                Stage::Decay => self.level = (sustain - self.level).mul_add(decay, self.level),
                Stage::Release => {
                    self.level = self.level.mul_add(-release, self.level);
                    if self.level < 1.0e-5 {
                        self.level = 0.0;
                        self.stage = Stage::Idle;
                    }
                }
            }
            if !self.level.is_finite() {
                self.level = 0.0;
                self.stage = Stage::Idle;
            }
            self.level = self.level.clamp(0.0, 1.0);
            io.outputs[0][frame] = self.level;
        }
    }

    fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
        self.gate.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_indices_match_the_catalogue() {
        let spec = Kind::ENV.spec();
        for (index, name) in [
            (ATTACK, "attack"),
            (DECAY, "decay"),
            (SUSTAIN, "sustain"),
            (RELEASE, "release"),
        ] {
            assert_eq!(spec.knobs[index].name, name);
        }
        assert_eq!(spec.inputs[IN_GATE].name, "gate");
    }

    #[test]
    fn it_rises_holds_at_sustain_and_releases() {
        let mut bench = Bench::new(Kind::ENV);
        bench
            .knob("attack", 0.01)
            .knob("decay", 0.05)
            .knob("sustain", 0.5)
            .knob("release", 0.1);
        let gate_frames = (RATE * 0.5) as usize;
        let samples = bench.render_fed(0, 48_000, Some("gate"), |frame| {
            if frame < gate_frames { 1.0 } else { 0.0 }
        });
        // The attack reaches the top within roughly its time.
        let peak_at = samples.iter().position(|s| *s >= 1.0).unwrap();
        assert!(peak_at < (RATE * 0.02) as usize, "{peak_at}");
        // Held at sustain before the gate falls.
        assert!((samples[gate_frames - 10] - 0.5).abs() < 0.01);
        // Released well after the gate falls.
        assert!(samples[47_000] < 0.001);
        assert!(samples.iter().all(|s| (0.0..=1.0).contains(s)));
    }

    #[test]
    fn no_gate_means_silence() {
        let mut bench = Bench::new(Kind::ENV);
        assert!(bench.render(0, 4_800).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::ENV, 1.0);
        survives_nonsense(Kind::ENV);
    }
}
