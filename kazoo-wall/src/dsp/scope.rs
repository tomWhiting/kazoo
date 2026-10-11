//! Oscilloscope: draws inputs `a` and `b` across `time`, at `zoom`, and
//! passes both straight through to its outputs so it can sit inline.
//!
//! In `trigger` mode each sweep waits for input a to rise through the
//! `trigger` level (a share of the screen's half-height, so it zooms with
//! the trace), so a repeating wave stands still; when nothing rises for a
//! screen and a tenth of a second it sweeps anyway, as a scope's auto
//! trigger does, and says the sweep was not triggered. In `free` mode the
//! trace rolls from right to left, newest at the right. In `xy` mode it is
//! input a across against input b up (a Lissajous figure).
//!
//! Each point of the trace holds the lowest and highest of its frames, so
//! a long span shows the whole swing of a fast wave rather than aliasing
//! it. A new `time` is taken at the next sweep's start (rolling, the next
//! point's), so a cable in its jack never stops a sweep under way.
//!
//! It hands the engine a capture when a sweep finishes, at most one every
//! [`PUBLISH_GAP_SECONDS`] (a sweep finished sooner is kept and handed on
//! when that has passed, so short sweeps go sixty times a second); and while a long sweep is under way (or a long
//! wait for a trigger), one every two of those anyway, with the sweep as
//! far as it has got over what is left of the last, so the screen is drawn
//! as it goes and the readings never go stale. Both inputs are metered
//! (see [`super::measure::Meter`]); after its inputs are re-plugged it
//! hands nothing on until both have a reading of what is plugged in now.

use super::measure::{Capture, Meter, PUBLISH_GAP_SECONDS, TRACE_POINTS, Trace};
use super::{Io, Module, Tick, bounded};
use crate::{MAX_INPUTS, SUB_BLOCK};

const TIME: usize = 0;
const ZOOM: usize = 1;
const TRIGGER: usize = 2;
const MODE: usize = 3;

const IN_A: usize = 0;
const IN_B: usize = 1;

/// The trigger's hysteresis, as a share of the screen's half-height: a
/// rise counts once input a has been this far below the level.
const TRIGGER_HYSTERESIS: f32 = 0.02;

/// How long past a screen's span a sweep waits for a trigger before it
/// sweeps anyway.
const AUTO_SECONDS: f32 = 0.1;

/// How far the span may drift (a cable wobbling the `time` knob) before
/// the screen is laid out afresh, as a share of it: less than this, the
/// sweep under way carries on.
const SPAN_DRIFT: u32 = 50;

/// The scope's modes, as its `mode` knob names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Each sweep starts on input a rising through the trigger level.
    Trigger,
    /// The trace rolls, newest at the right.
    Free,
    /// Input a against input b.
    Xy,
}

impl Mode {
    /// The mode the knob's `value` stands for.
    #[must_use]
    pub fn of(value: f32) -> Self {
        match value.round() {
            v if v >= 2.0 => Self::Xy,
            v if v >= 1.0 => Self::Free,
            _ => Self::Trigger,
        }
    }
}

/// Where a triggered sweep is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for input a to rise through the level.
    Armed,
    /// Drawing.
    Sweeping,
}

#[derive(Debug)]
pub struct Scope {
    rate: f32,
    meters: [Meter; 2],
    phase: Phase,
    /// The trace being drawn, over what is left of the one before.
    sweep: Trace,
    /// The point being filled, and its frames so far.
    point: usize,
    filled: u32,
    /// Points of the sweep holding something: in `trigger` and `xy` mode
    /// all of them once a sweep has finished on this layout, those drawn
    /// so far before; rolling, those rolled in.
    drawn: usize,
    /// Points across the screen, and frames to each.
    capacity: usize,
    frames_per_point: u32,
    /// Points still to be drawn before input b's trace is all of what is
    /// plugged into b now: a screen's worth from when b was re-plugged.
    b_owed: usize,
    /// The span the screen is laid out for, in frames, and the span its
    /// `time` knob asks for now (taken at the next sweep's start).
    span_frames: u32,
    wanted: u32,
    /// The mode it is laid out for.
    mode: Option<Mode>,
    /// Whether the sweep under way (or the last, while armed) started on
    /// a trigger.
    triggered: bool,
    /// Whether input a has been below the trigger level since the last
    /// sweep, and how long it has waited for a rise.
    below: bool,
    waited: u32,
    /// Frames since the last capture was handed on, and the least between
    /// two.
    since: u32,
    gap: u32,
    knobs: [f32; 4],
    capture: Capture,
    out: Out,
}

/// Where the capture stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Out {
    /// Handed on (or nothing yet).
    Taken,
    /// A finished sweep, kept until the gap has passed.
    Held,
    /// Ready for the engine to take.
    Ready,
}

impl Scope {
    pub fn new(sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate >= 1_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            rate,
            meters: [Meter::new(rate), Meter::new(rate)],
            phase: Phase::Armed,
            sweep: Trace::NONE,
            point: 0,
            filled: 0,
            drawn: 0,
            capacity: TRACE_POINTS,
            b_owed: 0,
            frames_per_point: 1,
            span_frames: 0,
            wanted: 0,
            mode: None,
            triggered: false,
            below: false,
            waited: 0,
            since: u32::MAX,
            // A few hundred kHz at most: exact.
            gap: (rate * PUBLISH_GAP_SECONDS).round() as u32,
            knobs: [0.0; 4],
            capture: Capture::NONE,
            out: Out::Taken,
        }
    }

    /// Lay the screen out afresh for `mode`: the sweep starts over, armed.
    fn restart(&mut self, mode: Mode) {
        self.mode = Some(mode);
        self.phase = Phase::Armed;
        self.point = 0;
        self.filled = 0;
        self.below = false;
        self.waited = 0;
        self.triggered = false;
        self.lay_out(self.wanted, mode == Mode::Xy);
    }

    /// Lay the points out for a span of `span_frames`: nothing drawn yet.
    fn lay_out(&mut self, span_frames: u32, xy: bool) {
        self.span_frames = span_frames;
        // At most TRACE_POINTS (96): the casts are exact.
        self.capacity = (span_frames as usize).clamp(2, TRACE_POINTS);
        self.frames_per_point = span_frames.div_ceil(self.capacity as u32).max(1);
        self.sweep = Trace {
            capacity: self.capacity as u16,
            span: (self.capacity as u32 * self.frames_per_point) as f32 / self.rate,
            xy,
            ..Trace::NONE
        };
        self.point = 0;
        self.filled = 0;
        self.drawn = 0;
        self.b_owed = 0;
    }

    /// At a sweep's start (or, rolling, a point's), take the span the
    /// `time` knob asks for, if it has moved by more than a fiftieth: a
    /// cable wobbling it never stops a sweep under way.
    fn settle(&mut self) {
        let drifted = self
            .wanted
            .abs_diff(self.span_frames)
            .saturating_mul(SPAN_DRIFT)
            > self.span_frames;
        if drifted {
            self.lay_out(self.wanted, self.mode == Some(Mode::Xy));
        }
    }

    /// How long an armed sweep waits for a rise before it sweeps anyway.
    fn patience(&self) -> u32 {
        // Seconds at a few hundred kHz: well inside u32.
        self.span_frames
            .saturating_add((self.rate * AUTO_SECONDS) as u32)
    }

    /// Fold `a` and `b` into `point` of the sweep.
    fn fold(&mut self, point: usize, (a, b): (f32, f32)) {
        let pair: [f32; 2] = (a, b).into();
        for (channel, sample) in pair.into_iter().enumerate() {
            if self.filled == 0 {
                self.sweep.low[channel][point] = sample;
                self.sweep.high[channel][point] = sample;
            } else {
                self.sweep.low[channel][point] = self.sweep.low[channel][point].min(sample);
                self.sweep.high[channel][point] = self.sweep.high[channel][point].max(sample);
            }
        }
    }

    /// Hand on the screen as it is, if at least `gap` frames have passed
    /// since the last, and both inputs have a reading of what is plugged
    /// in now: a finished sweep, or one under way over what is left of the
    /// last; rolling, the points rolled in, oldest first.
    fn publish(&mut self, gap: u32) {
        if self.since < gap || !self.meters.iter().all(Meter::fresh) {
            return;
        }
        self.take_screen();
        self.hand_on();
    }

    /// A sweep (or figure) has finished: keep it to hand on as soon as
    /// [`Self::gap`] has passed since the last, so a short sweep is handed
    /// on sixty times a second, never fewer for falling between two.
    fn finished(&mut self) {
        self.take_screen();
        self.out = Out::Held;
        self.offer();
    }

    /// Hand on the finished sweep kept, once the gap has passed and both
    /// inputs have a reading of what is plugged in now.
    fn offer(&mut self) {
        if self.out == Out::Held && self.since >= self.gap && self.meters.iter().all(Meter::fresh) {
            self.hand_on();
        }
    }

    /// Copy the screen as it is into the capture: a finished sweep, or one
    /// under way over what is left of the last; rolling, the points rolled
    /// in, oldest first.
    fn take_screen(&mut self) {
        let (first, points) = if self.mode == Some(Mode::Free) {
            let first = (self.point + self.capacity - self.drawn) % self.capacity;
            (first, self.drawn)
        } else {
            (0, self.drawn.max(self.point))
        };
        // A sweep under way (or the first, not yet across) shows only so
        // far; rolling, there is no sweep.
        let under_way =
            self.mode != Some(Mode::Free) && (self.point > 0 || self.drawn < self.capacity);
        let trace = &mut self.capture.trace;
        trace.swept = if under_way {
            self.point as u16
        } else {
            points as u16
        };
        trace.capacity = self.sweep.capacity;
        trace.span = self.sweep.span;
        trace.xy = self.sweep.xy;
        trace.b_current = self.b_owed == 0;
        trace.triggered = self.triggered;
        // At most TRACE_POINTS: exact.
        trace.points = points as u16;
        for index in 0..points {
            let from = (first + index) % self.capacity;
            for channel in 0..2 {
                trace.low[channel][index] = self.sweep.low[channel][from];
                trace.high[channel][index] = self.sweep.high[channel][from];
            }
        }
    }

    /// Hand the capture on, with fresh readings.
    const fn hand_on(&mut self) {
        self.capture.inputs = [*self.meters[0].latest(), *self.meters[1].latest()];
        self.capture.knobs = self.knobs;
        self.out = Out::Ready;
        self.since = 0;
    }

    /// One frame of a triggered sweep.
    fn triggered(&mut self, (a, b): (f32, f32), level: f32, hysteresis: f32) {
        if self.phase == Phase::Armed {
            if a < level - hysteresis {
                self.below = true;
            }
            let fired = self.below && a >= level;
            self.waited = self.waited.saturating_add(1);
            if !fired && self.waited <= self.patience() {
                return;
            }
            self.settle();
            self.phase = Phase::Sweeping;
            self.triggered = fired;
        }
        self.fold(self.point, (a, b));
        self.filled += 1;
        if self.filled < self.frames_per_point {
            return;
        }
        self.filled = 0;
        self.point += 1;
        self.b_owed = self.b_owed.saturating_sub(1);
        if self.point < self.capacity {
            return;
        }
        self.point = 0;
        self.drawn = self.capacity;
        self.finished();
        self.phase = Phase::Armed;
        self.below = false;
        self.waited = 0;
    }

    /// One frame of a rolling trace.
    fn rolling(&mut self, (a, b): (f32, f32)) {
        if self.filled == 0 {
            self.settle();
        }
        self.fold(self.point, (a, b));
        self.filled += 1;
        if self.filled < self.frames_per_point {
            return;
        }
        self.filled = 0;
        self.point = (self.point + 1) % self.capacity;
        self.b_owed = self.b_owed.saturating_sub(1);
        self.drawn = (self.drawn + 1).min(self.capacity);
        self.publish(self.gap);
    }

    /// One frame of a figure of input a against input b: a pair at the
    /// start of each point's frames.
    fn figure(&mut self, (a, b): (f32, f32)) {
        if self.filled == 0 {
            if self.point == 0 {
                self.settle();
            }
            self.fold(self.point, (a, b));
        }
        self.filled += 1;
        if self.filled < self.frames_per_point {
            return;
        }
        self.filled = 0;
        self.point += 1;
        self.b_owed = self.b_owed.saturating_sub(1);
        if self.point < self.capacity {
            return;
        }
        self.point = 0;
        self.drawn = self.capacity;
        self.finished();
    }
}

impl Module for Scope {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let time = io.knob(TIME);
        let zoom = io.knob(ZOOM).max(f32::MIN_POSITIVE);
        let level = io.knob(TRIGGER);
        let mode_knob = io.knob(MODE);
        let mode = Mode::of(mode_knob);
        // At most 10 s at a few hundred kHz: well inside u32.
        self.wanted = (time * self.rate).round().max(2.0) as u32;
        if self.mode != Some(mode) {
            self.restart(mode);
        }
        self.knobs = [time, zoom, level, mode_knob];
        for (channel, meter) in self.meters.iter_mut().enumerate() {
            meter.set_connected(io.connected[channel]);
        }
        let threshold = level / zoom;
        let hysteresis = TRIGGER_HYSTERESIS / zoom;
        for frame in 0..SUB_BLOCK {
            let a = io.inputs[IN_A][frame];
            let b = io.inputs[IN_B][frame];
            self.meters[0].feed(a);
            self.meters[1].feed(b);
            let (a, b) = (bounded(a), bounded(b));
            io.outputs[IN_A][frame] = a;
            io.outputs[IN_B][frame] = b;
            self.since = self.since.saturating_add(1);
            match mode {
                Mode::Trigger => self.triggered((a, b), threshold, hysteresis),
                Mode::Free => self.rolling((a, b)),
                Mode::Xy => self.figure((a, b)),
            }
            // A finished sweep kept goes once the gap has passed; else a
            // long sweep, or a long wait for a trigger, still hands on
            // what it has, with fresh readings.
            if self.out == Out::Held {
                self.offer();
            } else {
                self.publish(self.gap.saturating_mul(2));
            }
        }
    }

    fn reset(&mut self) {
        for meter in &mut self.meters {
            meter.reset();
        }
        self.mode = None;
        self.since = u32::MAX;
        self.capture = Capture::NONE;
        self.out = Out::Taken;
    }

    fn rewired(&mut self, moved: [bool; MAX_INPUTS]) {
        // What was measured of an input re-plugged is of the old signal;
        // the sweep starts over when a (which it triggers on) moved.
        for (meter, moved) in self.meters.iter_mut().zip(moved) {
            if moved {
                meter.rewire();
            }
        }
        if moved[IN_A] {
            self.mode = None;
        } else if moved[IN_B] {
            // a's trace stands; b's is of the old signal until a screen of
            // the new one is drawn.
            self.b_owed = self.capacity;
        }
        self.out = Out::Taken;
    }

    fn capture(&mut self) -> Option<&Capture> {
        if self.out != Out::Ready {
            return None;
        }
        self.out = Out::Taken;
        Some(&self.capture)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;
    use crate::dsp::CV_LIMIT;
    use std::f64::consts::TAU;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::SCOPE.spec();
        assert_eq!(spec.knobs[TIME].name, "time");
        assert_eq!(spec.knobs[ZOOM].name, "zoom");
        assert_eq!(spec.knobs[TRIGGER].name, "trigger");
        assert_eq!(spec.knobs[MODE].name, "mode");
        assert_eq!(spec.inputs[IN_A].name, "a");
        assert_eq!(spec.inputs[IN_B].name, "b");
        assert_eq!(spec.outputs[IN_A].name, "a");
        assert_eq!(spec.outputs[IN_B].name, "b");
        assert_eq!(Mode::of(0.0), Mode::Trigger);
        assert_eq!(Mode::of(1.2), Mode::Free);
        assert_eq!(Mode::of(2.0), Mode::Xy);
        assert_eq!(Mode::of(f32::NAN), Mode::Trigger);
    }

    fn sine(hz: f64, amplitude: f64, phase: f64) -> impl Fn(usize) -> f32 {
        move |frame| (amplitude * (TAU * hz * frame as f64 / f64::from(RATE) + phase).sin()) as f32
    }

    /// Run `bench` for `frames`, feeding `a` (and `b`), and keep every
    /// capture it hands on.
    fn captures(
        bench: &mut Bench,
        frames: usize,
        a: impl Fn(usize) -> f32,
        b: Option<&dyn Fn(usize) -> f32>,
    ) -> Vec<Capture> {
        let mut out = Vec::new();
        let mut frame = 0;
        while frame < frames {
            for offset in 0..SUB_BLOCK {
                bench.jack("a")[offset] = a(frame + offset);
                if let Some(b) = b {
                    bench.jack("b")[offset] = b(frame + offset);
                }
            }
            bench.step();
            if let Some(capture) = bench.module.capture() {
                out.push(*capture);
            }
            frame += SUB_BLOCK;
        }
        out
    }

    #[test]
    fn it_passes_both_inputs_through() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.hold("a", 0.25).hold("b", -3.0);
        bench.step();
        assert!(
            bench.outputs[0]
                .iter()
                .all(|s| (s - 0.25).abs() < f32::EPSILON)
        );
        assert!(
            bench.outputs[1]
                .iter()
                .all(|s| (s + 3.0).abs() < f32::EPSILON)
        );
        bench.hold("a", f32::NAN);
        bench.step();
        assert!(bench.outputs[0].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_repeating_wave_stands_still_on_its_trigger() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.01).knob("trigger", 0.0);
        // 220 Hz: not a whole number of samples a period, nor of screens.
        let wave = sine(220.0, 0.8, 1.0);
        let shots = captures(&mut bench, 48_000, &wave, None);
        assert!(
            (50..=61).contains(&shots.len()),
            "at most 60 a second: {}",
            shots.len()
        );
        let first = &shots[2].trace;
        assert!(first.triggered);
        assert_eq!(usize::from(first.points), TRACE_POINTS);
        assert!((first.span - 0.01).abs() < 0.001, "{}", first.span);
        for shot in &shots[2..] {
            let trace = &shot.trace;
            assert!(trace.triggered);
            // Every sweep starts at the level, rising, and draws the same
            // wave: within one sample's movement at every point.
            assert!(trace.low[0][0].abs() < 0.03, "{}", trace.low[0][0]);
            assert!(trace.high[0][3] > trace.high[0][0]);
            for point in 0..TRACE_POINTS {
                let drift = (trace.low[0][point] - first.low[0][point]).abs();
                assert!(drift < 0.04, "point {point} moved {drift}");
            }
        }
        // The metered frequency rides along.
        let reading = shots.last().unwrap().inputs[0];
        assert!((reading.hz - 220.0).abs() < 0.05, "{reading:?}");
        assert!(reading.connected && !shots[5].inputs[1].connected);
    }

    #[test]
    fn the_trigger_level_follows_the_zoom_and_flat_lines_sweep_anyway() {
        // Zoomed in ten times, a level of 0.5 is 0.05 on the signal.
        let mut bench = Bench::new(Kind::SCOPE);
        bench
            .knob("time", 0.005)
            .knob("zoom", 10.0)
            .knob("trigger", 0.5);
        let shots = captures(&mut bench, 24_000, sine(100.0, 0.08, 0.0), None);
        let trace = &shots.last().unwrap().trace;
        assert!(trace.triggered);
        assert!(
            (trace.low[0][0] - 0.05).abs() < 0.005,
            "{}",
            trace.low[0][0]
        );
        // Nothing rises: it sweeps anyway, and says so.
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.02);
        let shots = captures(&mut bench, 24_000, |_| 0.3, None);
        assert!(!shots.is_empty());
        assert!(shots.iter().all(|shot| !shot.trace.triggered));
        let trace = &shots.last().unwrap().trace;
        assert!(trace.low[0].iter().all(|s| (s - 0.3).abs() < 1e-6));
    }

    #[test]
    fn long_spans_keep_each_point_s_whole_swing() {
        // Two seconds across: a 1 kHz tone fills every point from top to
        // bottom rather than aliasing to a slow wave.
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 2.0).knob("mode", 1.0);
        let shots = captures(&mut bench, 48_000, sine(1_000.0, 0.5, 0.0), None);
        let trace = &shots.last().unwrap().trace;
        assert!(trace.points > 2);
        for point in 0..usize::from(trace.points) {
            assert!(trace.low[0][point] < -0.49 && trace.high[0][point] > 0.49);
        }
        assert!((trace.span - 2.0).abs() < 0.01);
    }

    #[test]
    fn a_free_trace_rolls_with_the_newest_at_the_right() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 1.0).knob("mode", 1.0);
        // A ramp: each point is later, so higher, than the one before.
        let ramp = |frame: usize| frame as f32 / 96_000.0;
        let shots = captures(&mut bench, 72_000, ramp, None);
        let early = &shots[0].trace;
        assert!(early.points < early.capacity, "it fills from the left");
        let late = &shots.last().unwrap().trace;
        assert_eq!(late.points, late.capacity, "once full it rolls");
        for point in 1..usize::from(late.points) {
            assert!(late.low[0][point] > late.low[0][point - 1]);
        }
        assert!((late.high[0][usize::from(late.points) - 1] - 0.75).abs() < 0.02);
        assert!(shots.iter().all(|shot| !shot.trace.triggered));
    }

    #[test]
    fn xy_draws_a_against_b() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.01).knob("mode", 2.0);
        // A circle: sine against cosine.
        let cosine = sine(200.0, 0.6, std::f64::consts::FRAC_PI_2);
        let shots = captures(&mut bench, 24_000, sine(200.0, 0.6, 0.0), Some(&cosine));
        let trace = &shots.last().unwrap().trace;
        assert!(trace.xy);
        for point in 0..usize::from(trace.points) {
            let (x, y) = (trace.low[0][point], trace.low[1][point]);
            assert!((x.hypot(y) - 0.6).abs() < 0.01, "{x} {y}");
            assert!((trace.high[0][point] - x).abs() < f32::EPSILON);
        }
        // Points go round the circle: the figure is not a line.
        let xs = trace.low[0][..usize::from(trace.points)].iter();
        let spread =
            xs.clone().fold(f32::MIN, |m, s| m.max(*s)) - xs.fold(f32::MAX, |m, s| m.min(*s));
        assert!(spread > 1.0, "{spread}");
    }

    #[test]
    fn short_spans_take_a_point_a_frame() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.001);
        let shots = captures(&mut bench, 9_600, sine(1_000.0, 0.5, 0.0), None);
        let trace = &shots.last().unwrap().trace;
        assert_eq!(trace.capacity, 48);
        assert!((trace.span - 0.001).abs() < 1e-6);
    }

    #[test]
    fn a_ten_second_sweep_is_drawn_as_it_goes_with_fresh_readings() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 10.0);
        let shots = captures(&mut bench, 48_000 * 2, sine(220.0, 0.5, 0.0), None);
        // A capture at least ten times a second, though no sweep finished.
        assert!(shots.len() >= 20, "{}", shots.len());
        let grown: Vec<u16> = shots.iter().map(|shot| shot.trace.points).collect();
        assert!(
            grown.windows(2).all(|pair| pair[1] >= pair[0]) && grown[0] < grown[grown.len() - 1],
            "the sweep grows from the left: {grown:?}"
        );
        for shot in &shots {
            let reading = shot.inputs[0];
            assert!(reading.connected && reading.frames > 0);
        }
        let last = shots.last().unwrap().inputs[0];
        assert!((last.hz - 220.0).abs() < 0.05, "{last:?}");
        // Waiting long for a trigger that never comes, it still hands on
        // readings: a flat line at 10 s waits 10.1 s before sweeping.
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 10.0);
        let shots = captures(&mut bench, 48_000, |_| 0.25, None);
        assert!(shots.len() >= 10, "{}", shots.len());
        assert!((shots.last().unwrap().inputs[0].mean - 0.25).abs() < 1e-6);
    }

    #[test]
    fn short_sweeps_hand_on_sixty_captures_a_second() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.005);
        let shots = captures(&mut bench, 48_000, sine(1_000.0, 0.5, 0.0), None);
        assert!((50..=61).contains(&shots.len()), "{}", shots.len());
    }

    #[test]
    fn a_sweep_under_way_says_so() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 10.0);
        let shots = captures(&mut bench, 48_000, sine(220.0, 0.5, 0.0), None);
        for shot in &shots {
            assert_eq!(
                shot.trace.swept, shot.trace.points,
                "the first sweep is all there is"
            );
            assert!(shot.trace.points < shot.trace.capacity);
        }
        // Short sweeps finish before each capture: whole (but the very
        // first, handed on the moment the readings are fresh).
        let mut bench = Bench::new(Kind::SCOPE);
        let shots = captures(&mut bench, 24_000, sine(220.0, 0.5, 0.0), None);
        assert!(
            shots[1..]
                .iter()
                .all(|shot| shot.trace.swept == shot.trace.capacity)
        );
        // Rolling has no sweep.
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 1.0).knob("mode", 1.0);
        let shots = captures(&mut bench, 24_000, sine(220.0, 0.5, 0.0), None);
        assert!(
            shots
                .iter()
                .all(|shot| shot.trace.swept == shot.trace.points)
        );
    }

    #[test]
    fn a_cable_swinging_the_time_knob_lets_every_sweep_finish() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.01);
        // A slow LFO swinging the span across a wide range, far more than
        // a fiftieth between sub-blocks.
        let wave = sine(300.0, 0.5, 0.0);
        let mut whole = 0;
        for block in 0..3_000 {
            let swing = (block as f32 * 0.37).sin() * 0.002;
            *bench.jack("time") = [swing; SUB_BLOCK];
            for (offset, sample) in bench.jack("a").iter_mut().enumerate() {
                *sample = wave(block * SUB_BLOCK + offset);
            }
            bench.step();
            if let Some(capture) = bench.module.capture() {
                let trace = capture.trace;
                if trace.triggered && trace.points == trace.capacity {
                    whole += 1;
                }
            }
        }
        assert!(whole > 20, "{whole}");
    }

    #[test]
    fn re_plugged_it_hands_on_nothing_of_the_old_signal() {
        let mut bench = Bench::new(Kind::SCOPE);
        captures(&mut bench, 9_600, |_| 0.75, None);
        bench.module.rewired([true, false, false, false]);
        // Until both inputs have a reading of what is in now: nothing.
        let early = captures(&mut bench, 3_200, |_| -0.5, None);
        assert!(early.is_empty(), "{}", early.len());
        let later = captures(&mut bench, 9_600, |_| -0.5, None);
        assert!(!later.is_empty());
        for shot in &later {
            assert!(
                (shot.inputs[0].mean + 0.5).abs() < 1e-6,
                "{:?}",
                shot.inputs[0]
            );
            let points = usize::from(shot.trace.points);
            assert!(
                shot.trace.low[0][..points]
                    .iter()
                    .all(|s| (s + 0.5).abs() < 1e-6)
            );
        }
        // The extremes since the last clear are kept.
        assert!((later[0].inputs[0].max - 0.75).abs() < 1e-6);
    }

    #[test]
    fn re_plugging_b_alone_keeps_a_s_readings_and_sweep() {
        let mut bench = Bench::new(Kind::SCOPE);
        let wave = sine(220.0, 0.5, 0.0);
        let shots = captures(&mut bench, 24_000, &wave, Some(&|_| 0.25));
        let before = *shots.last().unwrap();
        assert!((before.inputs[0].hz - 220.0).abs() < 0.05);
        bench.module.rewired([false, true, false, false]);
        // a keeps its history: the first capture after b's first reading
        // still knows a's frequency, and a's sweep carries on whole.
        let shots = captures(
            &mut bench,
            9_600,
            |frame| wave(24_000 + frame),
            Some(&|_| -0.75),
        );
        let first = shots.first().expect("a capture once b has a reading");
        assert!(
            (first.inputs[0].hz - 220.0).abs() < 0.05,
            "{:?}",
            first.inputs[0]
        );
        assert!(
            (first.inputs[1].mean + 0.75).abs() < 1e-6,
            "{:?}",
            first.inputs[1]
        );
        assert_eq!(
            first.inputs[1].max.to_bits(),
            0.25_f32.to_bits(),
            "b's extremes are kept"
        );
        assert_eq!(
            first.trace.points, first.trace.capacity,
            "a's screen was not cleared"
        );
        // Short sweeps redraw b long before its first reading: b's trace
        // is all of the new b.
        assert!(first.trace.b_current);
        let points = usize::from(first.trace.points);
        assert!(
            first.trace.low[1][..points]
                .iter()
                .all(|s| (s + 0.75).abs() < 1e-6)
        );
        // A long sweep still holds b's old points when b's first reading
        // comes: b's trace is not b's until a screen of the new b is drawn.
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 1.0);
        captures(&mut bench, 96_000, &wave, Some(&|_| 0.25));
        bench.module.rewired([false, true, false, false]);
        let shots = captures(
            &mut bench,
            96_000,
            |frame| wave(96_000 + frame),
            Some(&|_| -0.75),
        );
        assert!(!shots[0].trace.b_current, "b's old points are not b now");
        assert!(
            shots.last().unwrap().trace.b_current,
            "a screen later they are"
        );
        assert!(first.trace.points > 0);
    }

    #[test]
    fn a_nonsense_rate_falls_back_to_a_usable_one() {
        for rate in [f32::NAN, 0.0, -48_000.0, f32::INFINITY] {
            let scope = Scope::new(rate);
            assert!((scope.rate - 48_000.0).abs() < f32::EPSILON);
            assert!(scope.gap > 0);
        }
    }

    #[test]
    fn a_wobbling_time_knob_does_not_stop_the_sweeps() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.knob("time", 0.01);
        // A little noise into the time jack: under 2% of the span.
        let mut rng = crate::dsp::Rng::new(5);
        let mut shots = 0;
        let wave = sine(300.0, 0.5, 0.0);
        for block in 0..1_500 {
            for sample in bench.jack("time").iter_mut() {
                *sample = rng.bipolar() * 1.0e-5;
            }
            for (offset, sample) in bench.jack("a").iter_mut().enumerate() {
                *sample = wave(block * SUB_BLOCK + offset);
            }
            bench.step();
            shots += usize::from(bench.module.capture().is_some());
        }
        assert!(shots > 10, "{shots}");
    }

    #[test]
    fn a_reset_forgets_the_screen() {
        let mut bench = Bench::new(Kind::SCOPE);
        bench.hold("a", 0.5);
        for _ in 0..200 {
            bench.step();
        }
        bench.module.reset();
        assert!(bench.module.capture().is_none());
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::SCOPE, CV_LIMIT);
        survives_nonsense(Kind::SCOPE);
    }
}
