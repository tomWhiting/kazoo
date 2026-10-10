//! The selector: one of four inputs, chosen by `select`, sent to one
//! output.
//!
//! The four inputs take every input a module has, so the choice is the
//! `select` knob and its jack. The jack spans every input: a CV from 0 up
//! to 1 picks in1, in2, in3 and in4 in turn (each quarter moves it one),
//! so a clock's `beat` sweeps them across a division and a divider's
//! `count` (at a division of 4) steps through them gate by gate, going
//! back to in1 on the divider's reset. Moving between inputs crossfades
//! over `fade`, so audio does not click, but a new selector (or one reset)
//! starts on its input at once: a wall loaded with a selector on in4 plays
//! in4 from its first frame.

use super::switch::Crossfade;
use super::{Io, Module, Tick, bounded};
use crate::SUB_BLOCK;

const SELECT: usize = 0;
const FADE: usize = 1;

#[derive(Debug)]
pub struct Select {
    fade: Crossfade,
}

impl Select {
    pub const fn new() -> Self {
        Self {
            fade: Crossfade::new(),
        }
    }
}

impl Module for Select {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        let rate = Crossfade::rate(io.knob(FADE), tick.sample_rate);
        for frame in 0..SUB_BLOCK {
            // Clamped and rounded: the cast is exact.
            let position = io.knob_at(SELECT, frame) as usize;
            let gains = self.fade.advance(position, rate);
            let mut sum = 0.0_f32;
            for (input, gain) in io.inputs.iter().zip(gains) {
                sum = bounded(input[frame]).mul_add(*gain, sum);
            }
            io.outputs[0][frame] = bounded(sum);
        }
    }

    fn reset(&mut self) {
        self.fade.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::super::switch::WAYS;
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::SELECT.spec();
        assert_eq!(spec.knobs[SELECT].name, "select");
        assert_eq!(spec.knobs[FADE].name, "fade");
        assert_eq!(spec.knobs[SELECT].labels, ["in1", "in2", "in3", "in4"]);
        for (index, port) in spec.inputs.iter().enumerate() {
            assert_eq!(port.name, format!("in{}", index + 1));
        }
        assert_eq!(spec.inputs.len(), WAYS);
        assert_eq!(spec.outputs[0].name, "out");
    }

    /// Hold input `n` at `n + 1` tenths, so the output says which input
    /// it carries.
    fn hold_tenths(bench: &mut Bench) {
        for (index, name) in ["in1", "in2", "in3", "in4"].iter().enumerate() {
            bench.hold(name, (index + 1) as f32 / 10.0);
        }
    }

    /// The output once settled on what `select` picks.
    fn settled(bench: &mut Bench) -> f32 {
        hold_tenths(bench);
        bench.render(0, 9_600)[9_599]
    }

    #[test]
    fn select_picks_an_input_and_its_jack_spans_all_four() {
        let mut bench = Bench::new(Kind::SELECT);
        assert!((settled(&mut bench) - 0.1).abs() < 1e-6);
        for (select, expected) in [(1.0, 0.2), (2.0, 0.3), (3.0, 0.4)] {
            bench.knob("select", select);
            assert!((settled(&mut bench) - expected).abs() < 1e-6, "{select}");
        }
        // Each quarter of 1.0 in the jack moves select one input; past the
        // end it stays on the last.
        bench.knob("select", 0.0);
        for (cv, expected) in [(0.0, 0.1), (0.25, 0.2), (0.5, 0.3), (0.75, 0.4), (5.0, 0.4)] {
            bench.hold("select", cv);
            assert!((settled(&mut bench) - expected).abs() < 1e-6, "{cv}");
        }
        // Unplugged inputs are silent.
        let mut bench = Bench::new(Kind::SELECT);
        bench.knob("select", 3.0);
        assert!(
            bench
                .render(0, 9_600)
                .iter()
                .all(|s| s.abs() < f32::EPSILON)
        );
    }

    #[test]
    fn a_clock_beat_sweeps_every_input_across_a_division() {
        // Quarter notes at 120 BPM: a beat ramp 0..1 over 24 000 frames.
        let mut clock = Bench::new(Kind::CLOCK);
        clock.knob("division", 2.0);
        let beat = clock.render(1, 24_000);
        let mut bench = Bench::new(Kind::SELECT);
        bench.knob("fade", 0.001);
        hold_tenths(&mut bench);
        let out = bench.render_fed(0, 24_000, Some("select"), |frame| beat[frame]);
        for (quarter, expected) in [0.1, 0.2, 0.3, 0.4].iter().enumerate() {
            let at = quarter * 6_000 + 5_000;
            assert!((out[at] - expected).abs() < 1e-6, "{at}: {}", out[at]);
        }
    }

    #[test]
    fn a_divider_count_steps_through_every_input_and_its_reset_goes_back() {
        // A gate every 128 frames into a divider of 4: its count steps
        // 0, 1/4, 2/4, 3/4.
        let gates = |frame: usize| if frame % 128 < 30 { 1.0 } else { 0.0 };
        let mut divide = Bench::new(Kind::DIVIDE);
        divide.knob("division", 4.0);
        let [_, mut count, ..] = divide.render_all(6 * 128, "in", gates);
        divide.hold("reset", 1.0);
        let [_, after_reset, ..] = divide.render_all(128, "in", gates);
        count.extend(after_reset);
        let mut bench = Bench::new(Kind::SELECT);
        bench.knob("fade", 0.001);
        hold_tenths(&mut bench);
        let out = bench.render_fed(0, 7 * 128, Some("select"), |frame| count[frame]);
        let heard: Vec<f32> = (0..7).map(|gate| out[gate * 128 + 120]).collect();
        let expected = [0.1, 0.2, 0.3, 0.4, 0.1, 0.2, 0.1];
        for (got, want) in heard.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{heard:?}");
        }
    }

    #[test]
    fn a_selector_loaded_on_in4_plays_in4_from_its_first_frame() {
        let mut bench = Bench::new(Kind::SELECT);
        bench.knob("select", 3.0);
        hold_tenths(&mut bench);
        let out = bench.render(0, 32);
        assert!((out[0] - 0.4).abs() < 1e-6, "{}", out[0]);
        // And again after a reset (as after a fault).
        bench.module.reset();
        bench.knob("select", 1.0);
        let out = bench.render(0, 32);
        assert!((out[0] - 0.2).abs() < 1e-6, "{}", out[0]);
    }

    #[test]
    fn moving_between_inputs_crossfades_without_a_click() {
        let mut bench = Bench::new(Kind::SELECT);
        bench.knob("fade", 0.005).hold("in1", -0.8).hold("in2", 0.8);
        bench.render(0, 960);
        // Turned at once: the output ramps across the fade, never jumps.
        bench.knob("select", 1.0);
        let out = bench.render(0, 960);
        let fade_frames = (0.005 * RATE) as usize;
        let largest = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold((out[0] + 0.8).abs(), f32::max);
        assert!(largest <= 1.6 / fade_frames as f32 + 1e-5, "{largest}");
        assert!(
            out[fade_frames / 2 - 1].abs() < 0.01,
            "{}",
            out[fade_frames / 2 - 1]
        );
        assert!(out[fade_frames..].iter().all(|s| (s - 0.8).abs() < 1e-6));
    }

    /// Frames a move from in1 (at 0) to in2 (at 1) takes, at `rate` with
    /// `fade` seconds on the knob and `jack` in its jack.
    fn fade_frames(rate: f32, fade: f32, jack: f32) -> usize {
        let mut bench = Bench::at(Kind::SELECT, rate);
        bench
            .knob("fade", fade)
            .hold("fade", jack)
            .hold("in1", 0.0)
            .hold("in2", 1.0);
        bench.render(0, 1_024);
        bench.knob("select", 1.0);
        let out = bench.render(0, 1_024);
        out.iter().position(|s| *s >= 1.0 - 1e-6).unwrap() + 1
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
    fn nonsense_in_one_input_stays_out_of_the_others() {
        let mut bench = Bench::new(Kind::SELECT);
        bench
            .hold("in1", f32::NAN)
            .hold("in2", 0.5)
            .knob("select", 1.0);
        let out = bench.render(0, 9_600);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!((out[9_599] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::SELECT, CV_LIMIT);
        survives_nonsense(Kind::SELECT);
    }
}
