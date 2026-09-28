//! The slow oscillator: sine, triangle, saw, square, random steps and smooth
//! random, morphing continuously, free-running or locked to the beat.

use std::f32::consts::TAU;

use super::{Edge, Io, Module, Rng, Tick, finite, seed};
use crate::SUB_BLOCK;
use crate::catalogue::{SYNC_DIVISIONS, sync_beats};

const RATE: usize = 0;
const SHAPE: usize = 1;
const DEPTH: usize = 2;
const OFFSET: usize = 3;
const SYNC: usize = 4;

const IN_RESET: usize = 0;

/// Output bound: offset plus depth reach ±2 at most.
const LIMIT: f32 = 2.0;

#[derive(Debug)]
pub struct Lfo {
    phase: f64,
    reset: Edge,
    rng: Rng,
    /// Random values: the one held this cycle, and the one before, for the
    /// smooth shape to glide from.
    held: f32,
    previous: f32,
}

impl Lfo {
    pub fn new() -> Self {
        let mut rng = Rng::new(seed());
        let held = rng.bipolar();
        Self {
            phase: 0.0,
            reset: Edge::default(),
            rng,
            held,
            previous: 0.0,
        }
    }

    fn new_cycle(&mut self) {
        self.previous = self.held;
        self.held = self.rng.bipolar();
    }
}

impl Module for Lfo {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        let shape = io.knob(SHAPE);
        let depth = io.knob(DEPTH);
        let offset = io.knob(OFFSET);
        let synced = sync_beats(&SYNC_DIVISIONS, io.knob(SYNC));
        let inverse_rate = 1.0 / f64::from(tick.sample_rate);
        for frame in 0..SUB_BLOCK {
            if self.reset.rising(io.inputs[IN_RESET][frame]) && synced.is_none() {
                self.phase = 0.0;
                self.new_cycle();
            }
            let wrapped = if let Some(beats) = synced {
                let phase = (tick.beat_at(frame) / beats).rem_euclid(1.0);
                let wrapped = phase < self.phase;
                self.phase = phase;
                wrapped
            } else {
                let hz = io.knob_at(RATE, frame);
                self.phase = f64::from(hz).mul_add(inverse_rate, self.phase);
                let wrapped = self.phase >= 1.0;
                self.phase = self.phase.rem_euclid(1.0);
                wrapped
            };
            if wrapped {
                self.new_cycle();
            }
            // Phase is 0 to 1: single precision is plenty for the wave.
            let value = self.wave(self.phase as f32, shape);
            io.outputs[0][frame] = finite(depth.mul_add(value, offset)).clamp(-LIMIT, LIMIT);
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.reset.reset();
        self.previous = 0.0;
        self.held = self.rng.bipolar();
    }
}

impl Lfo {
    fn wave(&self, phase: f32, shape: f32) -> f32 {
        let at = |index: u32| match index {
            0 => (phase * TAU).sin(),
            1 => 4.0f32.mul_add(-(phase - 0.5).abs(), 1.0),
            2 => phase.mul_add(2.0, -1.0),
            3 => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            4 => self.held,
            _ => {
                // Cosine ease from the last random value to this one.
                let t = 0.5f32.mul_add(-(phase * std::f32::consts::PI).cos(), 0.5);
                (self.held - self.previous).mul_add(t, self.previous)
            }
        };
        let low = shape.floor().clamp(0.0, 5.0);
        let t = shape - low;
        // Floored and held to 0..=5: the cast is exact.
        let index = low as u32;
        if t <= 0.0 || index >= 5 {
            at(index)
        } else {
            (at(index + 1) - at(index)).mul_add(t, at(index))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE as TEST_RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_indices_match_the_catalogue() {
        let spec = Kind::LFO.spec();
        for (index, name) in [
            (RATE, "rate"),
            (SHAPE, "shape"),
            (DEPTH, "depth"),
            (OFFSET, "offset"),
            (SYNC, "sync"),
        ] {
            assert_eq!(spec.knobs[index].name, name);
        }
        assert_eq!(spec.inputs[IN_RESET].name, "reset");
    }

    /// Cycles counted by upward zero crossings over `seconds`.
    fn cycles(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count()
    }

    #[test]
    fn the_rate_is_accurate() {
        let mut bench = Bench::new(Kind::LFO);
        bench.knob("rate", 4.0);
        // Ten seconds: forty cycles, give or take the first crossing.
        let samples = bench.render(0, (TEST_RATE * 10.0) as usize);
        let count = cycles(&samples);
        assert!((39..=41).contains(&count), "{count}");

        let mut bench = Bench::new(Kind::LFO);
        bench.knob("rate", 30.0);
        let hz = super::super::testing::frequency(&bench.render(0, 48_000));
        assert!((hz - 30.0).abs() < 0.05, "{hz}");
    }

    #[test]
    fn sync_locks_to_the_beat() {
        let mut bench = Bench::new(Kind::LFO);
        // 1/4 at 120 BPM: two cycles a second, whatever the rate knob says.
        bench.knob("sync", 3.0).knob("rate", 0.01);
        bench.bpm = 120.0;
        let samples = bench.render(0, (TEST_RATE * 5.0) as usize);
        let count = cycles(&samples);
        assert!((9..=11).contains(&count), "{count}");
    }

    #[test]
    fn depth_and_offset_shape_the_output() {
        let mut bench = Bench::new(Kind::LFO);
        bench
            .knob("rate", 5.0)
            .knob("depth", 0.25)
            .knob("offset", 0.5);
        let samples = bench.render(0, 48_000);
        let min = samples.iter().copied().fold(f32::MAX, f32::min);
        let max = samples.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            (max - 0.75).abs() < 0.01 && (min - 0.25).abs() < 0.01,
            "{min} {max}"
        );
    }

    #[test]
    fn random_steps_hold_through_a_cycle() {
        let mut bench = Bench::new(Kind::LFO);
        bench.knob("rate", 1.0).knob("shape", 4.0);
        let samples = bench.render(0, 48_000 * 3);
        let changes = samples
            .windows(2)
            .filter(|w| w[0].to_bits() != w[1].to_bits())
            .count();
        assert!((1..=4).contains(&changes), "{changes}");
    }

    #[test]
    fn reset_restarts_the_cycle() {
        let mut bench = Bench::new(Kind::LFO);
        bench.knob("rate", 0.5).knob("shape", 2.0);
        bench.render(0, 24_000);
        let fed = bench.render_fed(
            0,
            64,
            Some("reset"),
            |frame| {
                if frame >= 16 { 1.0 } else { 0.0 }
            },
        );
        // Just after the reset the saw is back at its bottom.
        assert!(fed[17] < -0.99, "{}", fed[17]);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::LFO, 2.0);
        survives_nonsense(Kind::LFO);
    }
}
