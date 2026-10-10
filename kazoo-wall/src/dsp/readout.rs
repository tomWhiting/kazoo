//! Readout: measures its input (see [`super::measure::Meter`]) and hands
//! the engine a reading every [`super::measure::WINDOW_SECONDS`], passing
//! the input straight through to its output so it can sit inline. A rising
//! gate at `reset` clears the lowest and highest it has seen.
//!
//! Its `mode` knob says how the console shows the numbers (as pitch, as a
//! gate, or as audio; `auto` by what is plugged in): the module measures
//! all of them, so the knob only travels with the reading.

use super::measure::{Capture, Meter};
use super::{Edge, Io, Module, Tick, bounded};
use crate::{MAX_INPUTS, SUB_BLOCK};

const MODE: usize = 0;

const IN_SIGNAL: usize = 0;
const IN_RESET: usize = 1;

#[derive(Debug)]
pub struct Readout {
    meter: Meter,
    reset: Edge,
    capture: Capture,
    ready: bool,
}

impl Readout {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            meter: Meter::new(sample_rate),
            reset: Edge::new(),
            capture: Capture::NONE,
            ready: false,
        }
    }
}

impl Module for Readout {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let mode = io.knob(MODE);
        self.meter.set_connected(io.connected[IN_SIGNAL]);
        for frame in 0..SUB_BLOCK {
            if self.reset.rising(io.inputs[IN_RESET][frame]) {
                self.meter.clear_extremes();
            }
            let sample = io.inputs[IN_SIGNAL][frame];
            io.outputs[0][frame] = bounded(sample);
            if self.meter.feed(sample) {
                self.capture.inputs[0] = *self.meter.latest();
                self.capture.knobs[MODE] = mode;
                self.ready = true;
            }
        }
    }

    fn reset(&mut self) {
        self.meter.reset();
        self.reset.reset();
        self.capture = Capture::NONE;
        self.ready = false;
    }

    fn rewired(&mut self, moved: [bool; MAX_INPUTS]) {
        // What was measured is of the signal unplugged (a reset gate
        // re-plugged changes nothing measured).
        if moved[IN_SIGNAL] {
            self.meter.rewire();
            self.ready = false;
        }
    }

    fn capture(&mut self) -> Option<&Capture> {
        if !self.ready {
            return None;
        }
        self.ready = false;
        Some(&self.capture)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;
    use crate::dsp::measure::Reading;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::READOUT.spec();
        assert_eq!(spec.knobs[MODE].name, "mode");
        assert_eq!(spec.inputs[IN_SIGNAL].name, "in");
        assert_eq!(spec.inputs[IN_RESET].name, "reset");
        assert_eq!(spec.outputs[0].name, "out");
    }

    /// Run `bench` for `frames` with `signal` at its input (and `reset` at
    /// its reset), returning every reading it hands on.
    fn readings(
        bench: &mut Bench,
        frames: usize,
        signal: impl Fn(usize) -> f32,
        reset: impl Fn(usize) -> f32,
    ) -> Vec<(f32, Reading)> {
        let mut out = Vec::new();
        let mut frame = 0;
        while frame < frames {
            for offset in 0..SUB_BLOCK {
                bench.jack("in")[offset] = signal(frame + offset);
                bench.jack("reset")[offset] = reset(frame + offset);
            }
            bench.step();
            if let Some(capture) = bench.module.capture() {
                out.push((capture.knobs[MODE], capture.inputs[0]));
            }
            frame += SUB_BLOCK;
        }
        out
    }

    #[test]
    fn it_reads_ten_times_a_second_and_passes_its_input_through() {
        let mut bench = Bench::new(Kind::READOUT);
        bench.knob("mode", 3.0);
        let wave = |frame: usize| {
            (0.5 * (std::f64::consts::TAU * 440.0 * frame as f64 / f64::from(RATE)).sin()) as f32
        };
        let shots = readings(&mut bench, 48_000, wave, |_| 0.0);
        assert_eq!(shots.len(), 10);
        let (mode, reading) = shots[9];
        assert!((mode - 3.0).abs() < f32::EPSILON);
        assert!(reading.connected);
        assert!((reading.hz - 440.0).abs() < 0.05, "{reading:?}");
        assert!((bench.outputs[0][5] - wave(48_000 - SUB_BLOCK + 5)).abs() < 1e-6);
    }

    #[test]
    fn a_reset_gate_clears_the_extremes() {
        let mut bench = Bench::new(Kind::READOUT);
        let spike = |frame: usize| if frame == 100 { 2.0 } else { 0.1 };
        let shots = readings(&mut bench, 9_600, spike, |_| 0.0);
        assert!((shots[1].1.max - 2.0).abs() < f32::EPSILON);
        let shots = readings(
            &mut bench,
            9_600,
            |_| 0.1,
            |frame| {
                if (10..20).contains(&frame) { 1.0 } else { 0.0 }
            },
        );
        assert!(
            (shots[1].1.max - 0.1).abs() < f32::EPSILON,
            "{:?}",
            shots[1]
        );
    }

    #[test]
    fn non_numbers_are_counted_and_never_passed_on() {
        let mut bench = Bench::new(Kind::READOUT);
        let shots = readings(
            &mut bench,
            4_800,
            |frame| if frame % 4 == 0 { f32::NAN } else { 0.5 },
            |_| 0.0,
        );
        assert_eq!(shots[0].1.faults, 1_200);
        assert!((shots[0].1.mean - 0.5).abs() < f32::EPSILON);
        assert!(bench.outputs[0].iter().all(|s| s.is_finite()));
    }

    #[test]
    fn faults_add_up_until_the_reset_gate_clears_them() {
        let mut bench = Bench::new(Kind::READOUT);
        let shots = readings(
            &mut bench,
            9_600,
            |frame| if frame < 10 { f32::NAN } else { 0.5 },
            |_| 0.0,
        );
        assert_eq!((shots[1].1.faults, shots[1].1.faults_since), (0, 10));
        let shots = readings(
            &mut bench,
            4_800,
            |_| 0.5,
            |frame| {
                if frame == 100 { 1.0 } else { 0.0 }
            },
        );
        assert_eq!(shots[0].1.faults_since, 0);
    }

    #[test]
    fn re_plugged_it_hands_on_nothing_of_the_old_signal() {
        let mut bench = Bench::new(Kind::READOUT);
        readings(&mut bench, 7_200, |_| 0.75, |_| 0.0);
        // A reset gate re-plugged changes nothing measured.
        bench.module.rewired([false, true, false, false]);
        let kept = readings(&mut bench, 2_400, |_| 0.75, |_| 0.0);
        assert_eq!(kept.len(), 1, "the window under way carries on");
        bench.module.rewired([true, false, false, false]);
        let shots = readings(&mut bench, 4_800, |_| -0.5, |_| 0.0);
        assert_eq!(shots.len(), 1);
        assert!(
            (shots[0].1.mean + 0.5).abs() < 1e-6,
            "a whole window of the new"
        );
        assert!(
            (shots[0].1.max - 0.75).abs() < 1e-6,
            "the extremes are kept"
        );
    }

    #[test]
    fn a_reset_forgets_the_reading_under_way() {
        let mut bench = Bench::new(Kind::READOUT);
        bench.hold("in", 0.5);
        for _ in 0..200 {
            bench.step();
        }
        bench.module.reset();
        assert!(bench.module.capture().is_none());
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::READOUT, CV_LIMIT);
        survives_nonsense(Kind::READOUT);
    }
}
