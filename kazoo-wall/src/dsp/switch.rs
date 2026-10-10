//! The sequential switch: one input sent to one of four outputs.
//!
//! Each rising `step` moves on to the next output, wrapping after
//! `positions` of them; a rising `reset` goes back to the first (and wins
//! over a step in the same frame). `select` moves the choice on from the
//! count by whole positions, so with nothing at `step` it picks the output
//! directly; its jack spans every position, so a CV from 0 up to 1 (a
//! clock's `beat`, a divider's `count`) picks each output in turn. Moving
//! between outputs crossfades over `fade`, so audio does not click, but a
//! new switch (or one reset) starts on its position at once. An unplugged
//! input is a steady 1.0, which makes each output a gate for its position.

use super::{Edge, Io, Module, Tick, bounded};
use crate::SUB_BLOCK;

const POSITIONS: usize = 0;
const SELECT: usize = 1;
const FADE: usize = 2;

const IN_SIGNAL: usize = 0;
const IN_STEP: usize = 1;
const IN_RESET: usize = 2;

/// The most positions a switch has.
pub const WAYS: usize = 4;

/// Gains for the positions of a switch: the chosen one moves toward 1 and
/// every other toward 0, at the same rate, so a change of position is a
/// crossfade and never a jump. The gains never sum past 1.
///
/// A new or reset crossfade has not settled anywhere: its first frame
/// lands on the chosen position at once, so a module built for a loaded
/// wall (or reset after a fault) plays its position from the first frame
/// rather than fading over from the first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossfade {
    gains: [f32; WAYS],
    /// Whether the gains have landed on a position yet.
    settled: bool,
}

impl Crossfade {
    /// A crossfade that lands wherever its first frame chooses.
    pub const fn new() -> Self {
        Self {
            gains: [0.0; WAYS],
            settled: false,
        }
    }

    /// The most a gain moves in a frame, for a fade of `seconds` at
    /// `sample_rate`; a fade too short to time is one frame.
    pub fn rate(seconds: f32, sample_rate: f32) -> f32 {
        let frames = seconds * sample_rate;
        if frames > 1.0 { 1.0 / frames } else { 1.0 }
    }

    /// Move every gain a frame toward `position`, by at most `rate`, and
    /// give the gains.
    pub fn advance(&mut self, position: usize, rate: f32) -> &[f32; WAYS] {
        let rate = if self.settled { rate } else { 1.0 };
        self.settled = true;
        for (index, gain) in self.gains.iter_mut().enumerate() {
            let goal = if index == position { 1.0 } else { 0.0 };
            *gain += (goal - *gain).clamp(-rate, rate);
        }
        &self.gains
    }

    /// Forget where it was: the next frame lands at once.
    pub const fn reset(&mut self) {
        *self = Self::new();
    }
}

#[derive(Debug)]
pub struct Switch {
    /// Steps since the last reset, counted round the positions.
    count: usize,
    step: Edge,
    reset: Edge,
    fade: Crossfade,
}

impl Switch {
    pub const fn new() -> Self {
        Self {
            count: 0,
            step: Edge::new(),
            reset: Edge::new(),
            fade: Crossfade::new(),
        }
    }
}

impl Module for Switch {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        // Clamped and rounded: the cast is exact.
        let positions = (io.knob(POSITIONS) as usize).clamp(2, WAYS);
        let rate = Crossfade::rate(io.knob(FADE), tick.sample_rate);
        for frame in 0..SUB_BLOCK {
            let reset = self.reset.rising(io.inputs[IN_RESET][frame]);
            let step = self.step.rising(io.inputs[IN_STEP][frame]);
            if reset {
                self.count = 0;
            } else if step {
                self.count = (self.count + 1) % positions;
            }
            // Clamped and rounded: the cast is exact.
            let select = io.knob_at(SELECT, frame) as usize;
            let gains = self.fade.advance((self.count + select) % positions, rate);
            let source = if io.connected[IN_SIGNAL] {
                bounded(io.inputs[IN_SIGNAL][frame])
            } else {
                1.0
            };
            for (output, gain) in io.outputs.iter_mut().zip(gains) {
                output[frame] = source * gain;
            }
        }
    }

    fn reset(&mut self) {
        self.count = 0;
        self.step.reset();
        self.reset.reset();
        self.fade.reset();
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
        let spec = Kind::SWITCH.spec();
        assert_eq!(spec.knobs[POSITIONS].name, "positions");
        assert_eq!(spec.knobs[SELECT].name, "select");
        assert_eq!(spec.knobs[FADE].name, "fade");
        assert_eq!(spec.inputs[IN_SIGNAL].name, "in");
        assert_eq!(spec.inputs[IN_STEP].name, "step");
        assert_eq!(spec.inputs[IN_RESET].name, "reset");
        for (index, port) in spec.outputs.iter().enumerate() {
            assert_eq!(port.name, format!("out{}", index + 1));
        }
        assert_eq!(spec.outputs.len(), WAYS);
    }

    /// A step rising every 1 024 frames (32 sub-blocks), high for 100.
    fn steps(frame: usize) -> f32 {
        if frame % 1_024 < 100 { 1.0 } else { 0.0 }
    }

    /// Which output carries the input at the end of each 1 024 frames of
    /// `outputs`.
    fn positions(outputs: &[Vec<f32>; WAYS]) -> Vec<usize> {
        (0..outputs[0].len() / 1_024)
            .map(|block| {
                let at = block * 1_024 + 1_023;
                let on: Vec<usize> = (0..WAYS).filter(|way| outputs[*way][at] > 0.99).collect();
                assert_eq!(on.len(), 1, "at {at}: {on:?}");
                assert!((0..WAYS).all(|way| on[0] == way || outputs[way][at].abs() < 1e-6));
                on[0]
            })
            .collect()
    }

    #[test]
    fn steps_move_round_the_positions_and_reset_goes_back() {
        let mut bench = Bench::new(Kind::SWITCH);
        bench.hold("in", 1.0);
        let outputs = bench.render_all(6_144, "step", steps);
        assert_eq!(positions(&outputs), [1, 2, 3, 0, 1, 2]);
        // Fewer positions: the count (at 2) wraps round them.
        bench.knob("positions", 2.0);
        let outputs = bench.render_all(4_096, "step", steps);
        assert_eq!(positions(&outputs), [1, 0, 1, 0]);
        // A reset and a step together: the reset wins.
        bench.knob("positions", 4.0);
        bench.hold("reset", 0.0);
        bench.render_all(2_048, "step", steps);
        bench.hold("reset", 1.0);
        let outputs = bench.render_all(1_024, "step", steps);
        assert_eq!(positions(&outputs), [0]);
        bench.hold("reset", 0.0);
        let outputs = bench.render_all(2_048, "step", steps);
        assert_eq!(positions(&outputs), [1, 2]);
    }

    #[test]
    fn select_moves_the_choice_on_and_its_jack_spans_every_output() {
        let mut bench = Bench::new(Kind::SWITCH);
        bench.hold("in", 1.0).knob("select", 2.0);
        let outputs = bench.render_all(1_024, "step", |_| 0.0);
        assert_eq!(positions(&outputs), [2]);
        let outputs = bench.render_all(2_048, "step", steps);
        assert_eq!(positions(&outputs), [3, 0]);
        // Each quarter of 1.0 in the jack moves select one position.
        let mut bench = Bench::new(Kind::SWITCH);
        bench.hold("in", 1.0);
        for (cv, expected) in [(0.0, 0), (0.25, 1), (0.6, 2), (0.99, 3), (1.5, 3)] {
            bench.hold("select", cv);
            let outputs = bench.render_all(1_024, "step", |_| 0.0);
            assert_eq!(positions(&outputs), [expected], "{cv}");
        }
        // With fewer positions the choice wraps round them.
        bench.knob("positions", 2.0).hold("select", 0.75);
        let outputs = bench.render_all(1_024, "step", |_| 0.0);
        assert_eq!(positions(&outputs), [1]);
    }

    #[test]
    fn a_ramp_from_0_to_1_in_the_select_jack_visits_every_output() {
        let mut bench = Bench::new(Kind::SWITCH);
        bench.hold("in", 1.0).knob("fade", 0.001);
        // A ramp a little shy of 1, so each frame `positions` reads (the
        // last of each quarter) sits clear of the next step's snap.
        let outputs = bench.render_all(4_096, "select", |frame| frame as f32 * 0.995 / 4_096.0);
        assert_eq!(positions(&outputs), [0, 1, 2, 3]);
    }

    #[test]
    fn a_new_or_reset_switch_starts_on_its_position_at_once() {
        let mut bench = Bench::new(Kind::SWITCH);
        bench.hold("in", 1.0).knob("select", 3.0);
        let [_, _, _, out4] = bench.render_all(32, "step", |_| 0.0);
        assert!((out4[0] - 1.0).abs() < f32::EPSILON, "{}", out4[0]);
        bench.module.reset();
        bench.knob("select", 2.0);
        let [_, _, out3, out4] = bench.render_all(32, "step", |_| 0.0);
        assert!((out3[0] - 1.0).abs() < f32::EPSILON && out4[0].abs() < f32::EPSILON);
    }

    /// The largest move of any output from one frame to the next.
    fn largest_move(outputs: &[Vec<f32>; WAYS]) -> f32 {
        outputs
            .iter()
            .flat_map(|port| port.windows(2).map(|w| (w[1] - w[0]).abs()))
            .fold(0.0_f32, f32::max)
    }

    #[test]
    fn a_choice_changed_mid_fade_fades_on_from_where_it_is() {
        // Unplugged, the outputs are the gains themselves.
        let mut bench = Bench::new(Kind::SWITCH);
        bench.knob("fade", 0.01);
        bench.render_all(1_024, "step", |_| 0.0);
        bench.knob("select", 1.0);
        let mut outputs = bench.render_all(128, "step", |_| 0.0);
        assert!((outputs[1][127] - 128.0 / 480.0).abs() < 1e-4);
        bench.knob("select", 2.0);
        let rest = bench.render_all(1_024, "step", |_| 0.0);
        for (port, more) in outputs.iter_mut().zip(rest) {
            port.extend(more);
        }
        assert!(largest_move(&outputs) <= 1.0 / 480.0 + 1e-6);
        for frame in 0..outputs[0].len() {
            let sum: f32 = outputs.iter().map(|port| port[frame]).sum();
            assert!(sum <= 1.0 + 1e-5, "frame {frame}: {sum}");
        }
        let last = outputs[0].len() - 1;
        assert!((outputs[2][last] - 1.0).abs() < 1e-6);
        assert!([0, 1, 3].iter().all(|way| outputs[*way][last].abs() < 1e-6));
    }

    #[test]
    fn positions_turned_below_the_choice_fade_it_back_into_range() {
        let mut bench = Bench::new(Kind::SWITCH);
        bench.knob("fade", 0.01).knob("select", 3.0);
        bench.render_all(1_024, "step", |_| 0.0);
        // Three past the first, round two positions: the second.
        bench.knob("positions", 2.0);
        let outputs = bench.render_all(1_024, "step", |_| 0.0);
        assert!(largest_move(&outputs) <= 1.0 / 480.0 + 1e-6);
        assert_eq!(positions(&outputs), [1]);
    }

    /// Frames a move from the first output to the second takes, at `rate`
    /// with `fade` seconds on the knob and `jack` in its jack.
    fn fade_frames(rate: f32, fade: f32, jack: f32) -> usize {
        let mut bench = Bench::at(Kind::SWITCH, rate);
        bench.knob("fade", fade).hold("fade", jack);
        bench.render_all(1_024, "step", |_| 0.0);
        bench.knob("select", 1.0);
        let [_, out2, ..] = bench.render_all(1_024, "step", |_| 0.0);
        out2.iter().position(|gain| *gain >= 1.0 - 1e-6).unwrap() + 1
    }

    #[test]
    fn the_fade_takes_its_time_at_any_rate_and_follows_its_jack() {
        for rate in [44_100.0_f32, 48_000.0, 96_000.0] {
            let frames = fade_frames(rate, 0.01, 0.0);
            assert!(
                frames.abs_diff((rate * 0.01) as usize) <= 1,
                "{rate}: {frames}"
            );
        }
        // A cable of 0.02 adds 0.02 of half the range (0.4995): about 11 ms.
        let frames = fade_frames(RATE, 0.001, 0.02);
        assert!(frames.abs_diff(528) <= 1, "{frames}");
    }

    #[test]
    fn positions_follow_their_jack() {
        // A cable of -1.0 moves positions down half its range (1): to 3.
        let mut bench = Bench::new(Kind::SWITCH);
        bench.hold("in", 1.0).hold("positions", -1.0);
        let outputs = bench.render_all(4_096, "step", steps);
        assert_eq!(positions(&outputs), [1, 2, 0, 1]);
    }

    #[test]
    fn unplugged_the_outputs_are_gates_for_their_positions() {
        let mut bench = Bench::new(Kind::SWITCH);
        let outputs = bench.render_all(4_096, "step", steps);
        assert_eq!(positions(&outputs), [1, 2, 3, 0]);
    }

    #[test]
    fn moving_between_outputs_crossfades_without_a_click() {
        // A loud tone, stepped on at its peak.
        let tone = |frame: usize| 0.9 * (std::f32::consts::TAU * 440.0 * frame as f32 / RATE).sin();
        let largest_tone_step = 0.9 * std::f32::consts::TAU * 440.0 / RATE;
        let peak = (RATE / 440.0 / 4.0) as usize + 4_800;
        let mut bench = Bench::new(Kind::SWITCH);
        bench.knob("fade", 0.005);
        let mut frame = 0;
        let mut outputs: [Vec<f32>; WAYS] = Default::default();
        while frame < 9_600 {
            for (n, sample) in bench.jack("in").iter_mut().enumerate() {
                *sample = tone(frame + n);
            }
            for (n, sample) in bench.jack("step").iter_mut().enumerate() {
                *sample = if frame + n >= peak { 1.0 } else { 0.0 };
            }
            bench.step();
            for (port, samples) in outputs.iter_mut().zip(&bench.outputs) {
                port.extend_from_slice(samples);
            }
            frame += crate::SUB_BLOCK;
        }
        let fade_frames = (0.005 * RATE) as usize;
        for (way, samples) in outputs.iter().enumerate().take(2) {
            let jump = samples
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max);
            // The fade adds at most the tone's level over the fade's length
            // to each frame's move.
            let limit = 0.9f32.mul_add(1.0 / fade_frames as f32, largest_tone_step) + 1e-4;
            assert!(jump <= limit, "out{}: {jump} > {limit}", way + 1);
        }
        // Halfway through, both carry some; once through, only the second.
        let half = peak + fade_frames / 2;
        assert!(outputs[0][half].abs() > 0.1 && outputs[1][half].abs() > 0.1);
        let after = peak + fade_frames + 1;
        assert!(outputs[0][after..].iter().all(|s| s.abs() < 1e-6));
        assert!(
            (outputs[1][after..]
                .iter()
                .fold(0.0_f32, |m, s| m.max(s.abs()))
                - 0.9)
                .abs()
                < 0.01
        );
    }

    #[test]
    fn the_gains_never_sum_past_one_however_fast_the_steps() {
        let mut fade = Crossfade::new();
        for frame in 0..10_000 {
            let gains = fade.advance((frame / 7) % WAYS, 1.0 / 240.0);
            let sum: f32 = gains.iter().sum();
            assert!(sum <= 1.0 + 1e-6 && gains.iter().all(|g| (0.0..=1.0).contains(g)));
        }
        assert!((Crossfade::rate(0.0, RATE) - 1.0).abs() < f32::EPSILON);
        assert!((Crossfade::rate(f32::NAN, RATE) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::SWITCH, CV_LIMIT);
        survives_nonsense(Kind::SWITCH);
    }
}
