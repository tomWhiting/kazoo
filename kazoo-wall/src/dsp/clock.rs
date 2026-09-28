//! The clock: gates on the wall's beat at a chosen division, with swing.
//!
//! It reads the song position every frame, so it lands on the same frames as
//! the desk's beats while the wall follows the desk. Swing delays every
//! second pulse; a rising `reset` makes the current frame the start of the
//! count.

use super::{Edge, Io, Module, Tick};
use crate::SUB_BLOCK;
use crate::catalogue::CLOCK_DIVISIONS;

const DIVISION: usize = 0;
const SWING: usize = 1;
const WIDTH: usize = 2;

const IN_RESET: usize = 0;

const OUT_GATE: usize = 0;
const OUT_BEAT: usize = 1;

#[derive(Debug)]
pub struct Clock {
    /// Song position the count starts from.
    origin: f64,
    reset: Edge,
}

impl Clock {
    pub const fn new() -> Self {
        Self {
            origin: 0.0,
            reset: Edge::new(),
        }
    }
}

/// The division's length in beats, for a division knob value.
fn division_beats(knob: f32) -> f64 {
    // Held to 0..=6 and rounded by the knob: the cast is exact.
    let index = knob.max(0.0) as usize;
    CLOCK_DIVISIONS[index.min(CLOCK_DIVISIONS.len() - 1)]
}

/// Whether the gate is high `position` beats into a swung pair of
/// divisions `length` beats each.
fn gate(position: f64, length: f64, swing: f64, width: f64) -> bool {
    let pulse = width * length;
    if position < length {
        return position < pulse;
    }
    // The second pulse of the pair lands late by up to 3/8 of a division,
    // and ends before the next pair starts.
    let second = length * swing.mul_add(0.5, 1.0);
    position >= second && position < (second + pulse).min(2.0 * length)
}

impl Module for Clock {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        let length = division_beats(io.knob(DIVISION));
        let swing = f64::from(io.knob(SWING));
        let width = f64::from(io.knob(WIDTH));
        for frame in 0..SUB_BLOCK {
            let beat = tick.beat_at(frame);
            if self.reset.rising(io.inputs[IN_RESET][frame]) {
                self.origin = beat;
            }
            let since = beat - self.origin;
            let position = since.rem_euclid(2.0 * length);
            let high = gate(position, length, swing, width);
            io.outputs[OUT_GATE][frame] = if high { 1.0 } else { 0.0 };
            // A ramp in 0..1: single precision is plenty.
            io.outputs[OUT_BEAT][frame] = (since / length).rem_euclid(1.0) as f32;
        }
    }

    fn reset(&mut self) {
        self.origin = 0.0;
        self.reset.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::CLOCK.spec();
        assert_eq!(spec.knobs[DIVISION].name, "division");
        assert_eq!(spec.knobs[SWING].name, "swing");
        assert_eq!(spec.knobs[WIDTH].name, "width");
        assert_eq!(spec.inputs[IN_RESET].name, "reset");
        assert_eq!(spec.outputs[OUT_GATE].name, "out");
        assert_eq!(spec.outputs[OUT_BEAT].name, "beat");
    }

    fn rises(samples: &[f32]) -> Vec<usize> {
        samples
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.5 && w[1] >= 0.5)
            .map(|(i, _)| i + 1)
            .collect()
    }

    #[test]
    fn it_ticks_on_the_beat() {
        let mut bench = Bench::new(Kind::CLOCK);
        // Quarter notes at 120 BPM: every 24 000 frames.
        bench.knob("division", 2.0);
        bench.bpm = 120.0;
        bench.beat = 0.25;
        let samples = bench.render(0, (RATE * 4.0) as usize);
        let rises = rises(&samples);
        assert_eq!(rises.len(), 8, "{rises:?}");
        // The song position is summed in floating point: a frame either
        // way over a beat is the grid's rounding.
        for pair in rises.windows(2) {
            assert!((pair[1] - pair[0]).abs_diff(24_000) <= 1, "{rises:?}");
        }
    }

    #[test]
    fn swing_delays_every_second_pulse() {
        let mut bench = Bench::new(Kind::CLOCK);
        bench
            .knob("division", 2.0)
            .knob("swing", 0.5)
            .knob("width", 0.1);
        bench.beat = 0.5;
        let samples = bench.render(0, (RATE * 3.0) as usize);
        let rises = rises(&samples);
        let gaps: Vec<usize> = rises.windows(2).map(|w| w[1] - w[0]).collect();
        // Long, short, long, short: 1.25 and 0.75 beats.
        let near = |target: usize| gaps.iter().any(|gap| gap.abs_diff(target) <= 1);
        assert!(near(30_000) && near(18_000), "{gaps:?}");
    }

    #[test]
    fn the_beat_output_ramps_and_reset_restarts_it() {
        let mut bench = Bench::new(Kind::CLOCK);
        bench.knob("division", 2.0);
        bench.beat = 0.0;
        let ramp = bench.render(1, 12_000);
        assert!(ramp[0].abs() < 1e-6 && (ramp[11_999] - 0.5).abs() < 1e-3);
        let fed = bench.render_fed(
            1,
            64,
            Some("reset"),
            |frame| {
                if frame >= 32 { 1.0 } else { 0.0 }
            },
        );
        assert!(fed[32].abs() < 1e-6, "{}", fed[32]);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::CLOCK, 1.0);
        survives_nonsense(Kind::CLOCK);
    }
}
