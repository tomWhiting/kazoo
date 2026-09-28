//! The step sequencer: up to 16 steps of pitch, advanced by a clock.
//!
//! Each rising clock moves to the next step and sets the pitch; the step's
//! gate plays with the `chance` probability and lasts `gate` of the time
//! between the last two clocks. A rising `reset` makes the next clock play
//! step 1.

use super::{Edge, Io, Module, Rng, Tick, seed};
use crate::SUB_BLOCK;

const STEPS: usize = 0;
const FIRST_STEP: usize = 1;
const GATE: usize = 17;
const CHANCE: usize = 18;

/// Steps a sequence can hold.
const MAX_STEPS: usize = 16;

const IN_CLOCK: usize = 0;
const IN_RESET: usize = 1;

const OUT_PITCH: usize = 0;
const OUT_GATE: usize = 1;

/// The longest clock period measured, in seconds: slower clocks are timed
/// as this.
const LONGEST_PERIOD_SECONDS: f32 = 16.0;

#[derive(Debug)]
pub struct Seq {
    /// The step playing, or `None` before the first clock after a reset.
    step: Option<usize>,
    clock: Edge,
    reset: Edge,
    rng: Rng,
    /// Frames since the last clock.
    since_clock: u32,
    /// Frames between the last two clocks.
    period: u32,
    longest: u32,
    /// Frames the gate stays high.
    gate_left: u32,
    pitch: f32,
}

impl Seq {
    pub fn new(sample_rate: f32) -> Self {
        // Positive and bounded: the casts are exact enough.
        let longest = (sample_rate * LONGEST_PERIOD_SECONDS) as u32;
        Self {
            step: None,
            clock: Edge::new(),
            reset: Edge::new(),
            rng: Rng::new(seed()),
            since_clock: 0,
            period: (sample_rate * 0.25) as u32,
            longest,
            gate_left: 0,
            pitch: 0.0,
        }
    }

    /// Move to the next step. Returns whether its gate plays.
    fn advance(&mut self, io: &Io<'_>) -> bool {
        // Held to 1..=16 and rounded: the cast is exact.
        let steps = (io.knob(STEPS) as usize).clamp(1, MAX_STEPS);
        let next = self.step.map_or(0, |step| (step + 1) % steps);
        self.step = Some(next);
        self.pitch = io.knob(FIRST_STEP + next) / 12.0;
        let chance = io.knob(CHANCE);
        chance >= 1.0 || self.rng.unit() < chance
    }
}

impl Module for Seq {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let gate_knob = io.knob(GATE);
        for frame in 0..SUB_BLOCK {
            if self.reset.rising(io.inputs[IN_RESET][frame]) {
                self.step = None;
            }
            self.since_clock = self.since_clock.saturating_add(1);
            let mut gap = false;
            if self.clock.rising(io.inputs[IN_CLOCK][frame]) {
                self.period = self.since_clock.min(self.longest).max(2);
                self.since_clock = 0;
                if self.advance(&io) {
                    // A gate still high gets one low frame, so envelopes
                    // hear a new note.
                    gap = self.gate_left > 0;
                    // The period is at most 16 s of frames: exact in f32.
                    let length = (gate_knob * self.period as f32) as u32;
                    self.gate_left = length.clamp(1, self.period - 1);
                } else {
                    self.gate_left = 0;
                }
            }
            io.outputs[OUT_PITCH][frame] = self.pitch;
            io.outputs[OUT_GATE][frame] = if self.gate_left > 0 && !gap { 1.0 } else { 0.0 };
            if !gap {
                self.gate_left = self.gate_left.saturating_sub(1);
            }
        }
    }

    fn reset(&mut self) {
        self.step = None;
        self.clock.reset();
        self.reset.reset();
        self.since_clock = 0;
        self.gate_left = 0;
        self.pitch = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::SEQ.spec();
        assert_eq!(spec.knobs[STEPS].name, "steps");
        assert_eq!(spec.knobs[FIRST_STEP].name, "step1");
        assert_eq!(spec.knobs[FIRST_STEP + 15].name, "step16");
        assert_eq!(spec.knobs[GATE].name, "gate");
        assert_eq!(spec.knobs[CHANCE].name, "chance");
        assert_eq!(spec.inputs[IN_CLOCK].name, "clock");
        assert_eq!(spec.inputs[IN_RESET].name, "reset");
        assert_eq!(spec.outputs[OUT_PITCH].name, "pitch");
        assert_eq!(spec.outputs[OUT_GATE].name, "gate");
    }

    /// A clock rising every 1 000 frames, high for 100.
    fn clock(frame: usize) -> f32 {
        if frame % 1_000 < 100 { 1.0 } else { 0.0 }
    }

    #[test]
    fn it_steps_through_and_wraps() {
        let mut bench = Bench::new(Kind::SEQ);
        bench
            .knob("steps", 3.0)
            .knob("step1", 0.0)
            .knob("step2", 12.0)
            .knob("step3", -7.0);
        let pitch = bench.render_fed(0, 5_000, Some("clock"), clock);
        for (step, expected) in [0.0, 1.0, -7.0 / 12.0, 0.0, 1.0].iter().enumerate() {
            let at = step * 1_000 + 500;
            assert!(
                (pitch[at] - expected).abs() < 1e-6,
                "step {step}: {}",
                pitch[at]
            );
        }
    }

    #[test]
    fn gates_follow_the_clock_period_and_chance() {
        let mut bench = Bench::new(Kind::SEQ);
        bench.knob("gate", 0.5);
        let gate = bench.render_fed(1, 6_000, Some("clock"), clock);
        // After the first period is measured, each gate is 500 frames long.
        let high = gate[3_000..4_000].iter().filter(|s| **s > 0.5).count();
        assert_eq!(high, 500);

        let mut bench = Bench::new(Kind::SEQ);
        bench.knob("chance", 0.0);
        let gate = bench.render_fed(1, 6_000, Some("clock"), clock);
        assert!(gate.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn full_length_gates_still_retrigger() {
        let mut bench = Bench::new(Kind::SEQ);
        bench.knob("gate", 1.0);
        let gate = bench.render_fed(1, 5_000, Some("clock"), clock);
        let rises = gate.windows(2).filter(|w| w[0] < 0.5 && w[1] > 0.5).count();
        assert!(rises >= 4, "{rises}");
    }

    #[test]
    fn reset_goes_back_to_step_one() {
        let mut bench = Bench::new(Kind::SEQ);
        bench
            .knob("step1", 5.0)
            .knob("step2", 7.0)
            .knob("step3", 9.0);
        bench.render_fed(0, 2_500, Some("clock"), clock);
        bench.hold("reset", 1.0);
        bench.render(0, 32);
        bench.hold("reset", 0.0);
        let pitch = bench.render_fed(0, 1_600, Some("clock"), |frame| {
            if frame >= 1_000 { 1.0 } else { 0.0 }
        });
        assert!((pitch[1_500] - 5.0 / 12.0).abs() < 1e-6, "{}", pitch[1_500]);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::SEQ, 2.0);
        survives_nonsense(Kind::SEQ);
    }
}
