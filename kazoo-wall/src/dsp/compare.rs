//! The comparator. In `above` mode its gate is high while the input is
//! above `threshold`; in `window` mode, while the input is within half of
//! `window` of the threshold (a window of 0 holds nothing).
//!
//! The threshold spans ±4, so it can sit on any pitch a sequence plays
//! (1.0 per octave). `hysteresis` keeps a noisy input from chattering: the
//! gate goes high only once the input is past the edge by half the
//! hysteresis, and low only once it is back past it by the other half. In
//! a window, half the hysteresis is held to a quarter of the window, so a
//! narrow window can still be entered. `inverse` is the gate the other way
//! up, and `trig` is a 5 ms trigger on every rising edge of the gate.

use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;

const THRESHOLD: usize = 0;
const WINDOW: usize = 1;
const HYSTERESIS: usize = 2;
const MODE: usize = 3;

/// The `mode` knob's position for a window comparator.
const WINDOW_MODE: f32 = 1.0;

const IN_SIGNAL: usize = 0;

const OUT_GATE: usize = 0;
const OUT_INVERSE: usize = 1;
const OUT_TRIG: usize = 2;

/// How long a trigger lasts.
const TRIG_SECONDS: f32 = 0.005;

#[derive(Debug)]
pub struct Compare {
    /// Whether the condition holds.
    high: bool,
    /// Frames the trigger stays high.
    trig_left: u32,
}

impl Compare {
    pub const fn new() -> Self {
        Self {
            high: false,
            trig_left: 0,
        }
    }
}

/// What a comparator compares against: its knobs at one frame.
#[derive(Debug, Clone, Copy)]
struct Edges {
    threshold: f32,
    /// The window's width; `None` in `above` mode.
    window: Option<f32>,
    hysteresis: f32,
}

/// Whether the gate is high for `input`, given whether it was (`high`).
///
/// Above, the input must rise above `threshold + h` to go high and fall
/// below `threshold - h` to go low, where `h` is half the hysteresis. In a
/// window, it must come within `half - h` of the threshold to go high and
/// leave `half + h` of it to go low, where `half` is half the window and
/// `h` at most a quarter of it.
fn holds(input: f32, high: bool, edges: Edges) -> bool {
    let h = edges.hysteresis * 0.5;
    let Some(window) = edges.window else {
        return if high {
            input >= edges.threshold - h
        } else {
            input > edges.threshold + h
        };
    };
    let half = window * 0.5;
    let h = h.min(half * 0.5);
    let distance = (input - edges.threshold).abs();
    if high {
        distance <= half + h
    } else {
        distance < half - h
    }
}

impl Module for Compare {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        // At least 8 kHz: positive, and well inside u32.
        let trig_frames = ((TRIG_SECONDS * tick.sample_rate) as u32).max(1);
        for frame in 0..SUB_BLOCK {
            let input = finite(io.inputs[IN_SIGNAL][frame]);
            let edges = Edges {
                threshold: io.knob_at(THRESHOLD, frame),
                window: (io.knob_at(MODE, frame) >= WINDOW_MODE).then(|| io.knob_at(WINDOW, frame)),
                hysteresis: io.knob_at(HYSTERESIS, frame),
            };
            let high = holds(input, self.high, edges);
            // A trigger still high when the gate rises again gets one low
            // frame, so whatever listens hears a new edge.
            let mut gap = false;
            if high && !self.high {
                gap = self.trig_left > 0;
                self.trig_left = trig_frames;
            }
            self.high = high;
            io.outputs[OUT_GATE][frame] = if high { 1.0 } else { 0.0 };
            io.outputs[OUT_INVERSE][frame] = if high { 0.0 } else { 1.0 };
            io.outputs[OUT_TRIG][frame] = if self.trig_left > 0 && !gap { 1.0 } else { 0.0 };
            if !gap {
                self.trig_left = self.trig_left.saturating_sub(1);
            }
        }
    }

    fn reset(&mut self) {
        self.high = false;
        self.trig_left = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::COMPARE.spec();
        assert_eq!(spec.knobs[THRESHOLD].name, "threshold");
        assert_eq!(spec.knobs[WINDOW].name, "window");
        assert_eq!(spec.knobs[HYSTERESIS].name, "hysteresis");
        assert_eq!(spec.knobs[MODE].name, "mode");
        assert_eq!(spec.knobs[MODE].labels, ["above", "window"]);
        assert_eq!(spec.inputs[IN_SIGNAL].name, "in");
        assert_eq!(spec.outputs[OUT_GATE].name, "gate");
        assert_eq!(spec.outputs[OUT_INVERSE].name, "inverse");
        assert_eq!(spec.outputs[OUT_TRIG].name, "trig");
    }

    /// Whether the gate is high at the end of a sub-block of `input`.
    fn gate_for(bench: &mut Bench, input: f32) -> bool {
        bench.hold("in", input);
        bench.render(OUT_GATE, 32)[31] > 0.5
    }

    /// Whether every sample is high (`true`) or every one low.
    fn all(samples: &[f32], high: bool) -> bool {
        samples.iter().all(|s| (*s > 0.5) == high)
    }

    #[test]
    fn it_is_high_above_the_threshold_and_inverse_is_not() {
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("threshold", 0.3).knob("hysteresis", 0.0);
        assert!(!gate_for(&mut bench, 0.2));
        assert!(gate_for(&mut bench, 0.31));
        assert!(all(&bench.outputs[OUT_INVERSE], false));
        assert!(gate_for(&mut bench, 0.3), "at the edge it holds");
        assert!(!gate_for(&mut bench, 0.29));
        assert!(all(&bench.outputs[OUT_INVERSE], true));
        // Unplugged, the input reads 0: below a positive threshold.
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("threshold", 0.1);
        assert!(all(&bench.render(OUT_GATE, 32), false));
        assert!(all(&bench.render(OUT_INVERSE, 32), true));
    }

    #[test]
    fn the_threshold_reaches_pitches_a_sequence_plays() {
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("threshold", 1.5).knob("hysteresis", 0.0);
        assert!(!gate_for(&mut bench, 1.4));
        assert!(gate_for(&mut bench, 1.6));
        bench.knob("threshold", -1.9);
        assert!(!gate_for(&mut bench, -2.0));
        assert!(gate_for(&mut bench, -1.8));
        // Held to 4.
        bench.knob("threshold", 9.0);
        assert!(!gate_for(&mut bench, 3.9));
        assert!(gate_for(&mut bench, 4.1));
    }

    #[test]
    fn hysteresis_keeps_a_noisy_signal_from_chattering() {
        // A slow ramp through the threshold with noise ±0.02 on it.
        let noisy = |frame: usize| {
            let ramp = frame as f32 / 4_800.0 - 0.5;
            let noise = if frame % 2 == 0 { 0.02 } else { -0.02 };
            ramp + noise
        };
        let rises = |hysteresis: f32| {
            let mut bench = Bench::new(Kind::COMPARE);
            bench.knob("hysteresis", hysteresis);
            let gate = bench.render_fed(OUT_GATE, 4_800, Some("in"), noisy);
            gate.windows(2).filter(|w| w[0] < 0.5 && w[1] > 0.5).count()
        };
        assert!(rises(0.0) > 10, "{}", rises(0.0));
        assert_eq!(rises(0.1), 1);
        // And it goes high only once past the threshold by half of it.
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("hysteresis", 0.2);
        assert!(!gate_for(&mut bench, 0.09));
        assert!(gate_for(&mut bench, 0.11));
        assert!(gate_for(&mut bench, -0.09));
        assert!(!gate_for(&mut bench, -0.11));
    }

    #[test]
    fn a_window_holds_the_gate_high_near_the_threshold() {
        let mut bench = Bench::new(Kind::COMPARE);
        bench
            .knob("mode", 1.0)
            .knob("threshold", 0.5)
            .knob("window", 0.4)
            .knob("hysteresis", 0.0);
        assert!(!gate_for(&mut bench, 0.2));
        assert!(gate_for(&mut bench, 0.4));
        assert!(gate_for(&mut bench, 0.69));
        assert!(!gate_for(&mut bench, 0.71));
        assert!(!gate_for(&mut bench, -0.5));
        // With hysteresis it is entered later and left later.
        bench.knob("hysteresis", 0.1);
        assert!(!gate_for(&mut bench, 0.68));
        assert!(gate_for(&mut bench, 0.64));
        assert!(gate_for(&mut bench, 0.74));
        assert!(!gate_for(&mut bench, 0.76));
        // Hysteresis past the window still leaves the middle of it open.
        bench.knob("window", 0.1).knob("hysteresis", 0.5);
        assert!(gate_for(&mut bench, 0.5));
        assert!(gate_for(&mut bench, 0.57));
        assert!(!gate_for(&mut bench, 0.58));
        // A window of 0 holds nothing.
        bench.knob("window", 0.0);
        assert!(!gate_for(&mut bench, 0.5));
    }

    #[test]
    fn a_window_works_below_zero_too() {
        let mut bench = Bench::new(Kind::COMPARE);
        bench
            .knob("mode", 1.0)
            .knob("threshold", -2.0)
            .knob("window", 1.0)
            .knob("hysteresis", 0.0);
        assert!(!gate_for(&mut bench, -2.6));
        assert!(gate_for(&mut bench, -2.4));
        assert!(gate_for(&mut bench, -1.6));
        assert!(!gate_for(&mut bench, -1.4));
        assert!(!gate_for(&mut bench, 2.0), "not its mirror image");
    }

    #[test]
    fn trig_is_short_and_fires_on_each_rise() {
        let mut bench = Bench::new(Kind::COMPARE);
        let trig = bench.render_fed(OUT_TRIG, 9_600, Some("in"), |frame| {
            if (100..4_000).contains(&frame) || frame >= 5_000 {
                1.0
            } else {
                -1.0
            }
        });
        let high: Vec<usize> = (0..trig.len()).filter(|i| trig[*i] > 0.5).collect();
        let frames = (TRIG_SECONDS * RATE) as usize;
        assert_eq!(high.len(), 2 * frames);
        assert_eq!(high[0], 100);
        assert_eq!(high[frames], 5_000);
        // Rises closer together than a trigger still each make an edge.
        let mut bench = Bench::new(Kind::COMPARE);
        let trig = bench.render_fed(OUT_TRIG, 960, Some("in"), |frame| {
            if frame % 40 < 20 { 1.0 } else { -1.0 }
        });
        let rises = trig.windows(2).filter(|w| w[0] < 0.5 && w[1] > 0.5).count() + 1;
        assert_eq!(rises, 960 / 40);
    }

    #[test]
    fn a_trigger_lasts_5_ms_at_any_rate() {
        for rate in [44_100.0_f32, 96_000.0] {
            let mut bench = Bench::at(Kind::COMPARE, rate);
            let trig = bench.render_fed(OUT_TRIG, 9_600, Some("in"), |frame| {
                if frame >= 64 { 1.0 } else { -1.0 }
            });
            let high = trig.iter().filter(|s| **s > 0.5).count();
            assert_eq!(high, (TRIG_SECONDS * rate) as usize, "{rate}");
        }
    }

    #[test]
    fn the_threshold_follows_its_jack_at_audio_rate() {
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("hysteresis", 0.0).hold("in", 0.5);
        // A cable of 0.25 moves the threshold by a quarter of its half
        // range (4): to 1.0.
        let jack = bench.jack("threshold");
        for (frame, sample) in jack.iter_mut().enumerate() {
            *sample = if frame < 16 { 0.0 } else { 0.25 };
        }
        let gate = bench.render(OUT_GATE, 32);
        assert!(all(&gate[..16], true));
        assert!(all(&gate[16..], false));
    }

    #[test]
    fn window_hysteresis_and_mode_follow_their_jacks() {
        // Mode: a cable of 1.0 moves it half its range, 0.5, which rounds
        // up to the window.
        let mut bench = Bench::new(Kind::COMPARE);
        bench
            .knob("threshold", 0.0)
            .knob("window", 1.0)
            .knob("hysteresis", 0.0);
        assert!(!gate_for(&mut bench, -0.3), "above mode");
        bench.hold("mode", 1.0);
        assert!(gate_for(&mut bench, -0.3), "window mode");
        // Window: a cable of 0.5 adds half of half its range (4): from 1.0
        // to 3.0, so 1.4 is inside.
        assert!(!gate_for(&mut bench, 1.4));
        bench.hold("window", 0.5);
        assert!(gate_for(&mut bench, 1.4));
        // Hysteresis: a cable of 0.2 adds 0.2 of half its range (1): 0.2.
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("hysteresis", 0.0).hold("hysteresis", 0.2);
        assert!(!gate_for(&mut bench, 0.09));
        assert!(gate_for(&mut bench, 0.11));
        assert!(gate_for(&mut bench, -0.09));
        assert!(!gate_for(&mut bench, -0.11));
    }

    #[test]
    fn nonsense_reads_as_zero() {
        let mut bench = Bench::new(Kind::COMPARE);
        bench.knob("threshold", -0.5);
        assert!(gate_for(&mut bench, f32::NAN));
        bench.knob("threshold", 0.5);
        assert!(!gate_for(&mut bench, f32::INFINITY));
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::COMPARE, 1.0);
        survives_nonsense(Kind::COMPARE);
    }
}
