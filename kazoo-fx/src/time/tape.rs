//! Tape echo, in the spirit of the Roland RE-201 Space Echo and the
//! Maestro Echoplex.
//!
//! # The model
//!
//! A loop of tape runs past one record head and three playback heads spaced
//! evenly along the path, as on the RE-201, so the heads sound at a third,
//! two thirds and all of the `time` knob. `heads` picks which of them play.
//! What they play is summed and fed back to the record head (`feedback`,
//! the RE-201's "intensity").
//!
//! - **Transport.** `time` sets the motor speed. The motor has inertia: a
//!   new time is reached along a smooth curve whose slope is capped, so a
//!   turn of the knob bends the pitch of the echoes the way a real tape
//!   speeds up and slows down, without zipper noise or clicks.
//! - **Wow, flutter and drift.** The tape speed wobbles with a slow capstan
//!   wobble (wow, 0.55 Hz), a faster pinch-roller flutter (7.3 Hz) with an
//!   idler partial (13.7 Hz), and a random drift that wanders to a new speed
//!   every couple of seconds. What a head hears is the integral of the speed
//!   error over the stretch of tape between the record head and that head:
//!   slow wobbles shift a long echo more than a short one, while fast flutter
//!   averages out over a long stretch. Each head gets its own exact integral,
//!   so the three heads wobble differently, as they do on the machine.
//! - **Record path.** Pre-emphasis lifts the treble into an anti-aliased
//!   `tanh` tape saturation (`drive`), and de-emphasis takes it back out, so
//!   the top end saturates first, as on tape. The saturation runs at an
//!   internal rate of at least 176.4 kHz, so the top octave it is lifted
//!   into neither folds back nor dulls at 44.1 kHz; the heads read the
//!   oversampling's few samples sooner, so the echoes keep their time. A slow makeup gain gives back
//!   the level the saturation squeezes out (never more than the drive put
//!   in), so turning `drive` up adds grit and compression without turning
//!   the echoes down. Then the playback losses: a
//!   gap-loss lowpass and a head bump (a low resonance), both of whose
//!   frequencies follow the tape speed (a slow tape is darker and its bump
//!   lower), and `wear` darkens further. Because these sit in the record path
//!   every repeat passes through them again, so the losses and the grit
//!   compound, repeat by repeat, as a tape echo's do.
//! - **Safety.** The feedback is capped at 0.95 and the head bump's lift is
//!   taken back out of the loop, so the loop gain stays below one at every
//!   frequency, and the saturation tames anything that tries to run away.
//! - **Hiss.** A little bright noise rides under the echoes and fades with
//!   them, so an idle echo is silent.
//!
//! Sources: the RE-201 and EP-3 service manuals (head layout, transport),
//! and the tape-recording literature on gap loss, head bump and
//! pre-/de-emphasis.

use std::f32::consts::TAU;

use super::parts::{
    Biquad, Follower, LONGEST_SYNCED_SECONDS, SYNC_LABELS, Sinc, SoftClip, accept, clean,
    coefficient, defaults, equal_power, frames, guard, silence_from, synced_seconds,
};
use crate::drive::oversample::{Oversampler, TARGET_RATE, factor_for};
use crate::dsp::{DelayLine, Noise, OnePole, Phasor, Smoothed, db_to_gain, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const TIME: usize = 0;
const SYNC: usize = 1;
const HEADS: usize = 2;
const FEEDBACK: usize = 3;
const WOW: usize = 4;
const FLUTTER: usize = 5;
const DRIVE: usize = 6;
const WEAR: usize = 7;
const HISS: usize = 8;
const MIX: usize = 9;

/// Which heads play at each step of the `heads` knob.
const HEAD_LABELS: &[&str] = &["1", "2", "3", "1+2", "2+3", "1+3", "1+2+3"];
const HEAD_SETS: [[bool; 3]; 7] = [
    [true, false, false],
    [false, true, false],
    [false, false, true],
    [true, true, false],
    [false, true, true],
    [true, false, true],
    [true, true, true],
];
/// Where each head sits along the path, as a fraction of `time`.
const HEAD_PLACES: [f32; 3] = [1.0 / 3.0, 2.0 / 3.0, 1.0];

const MIN_TIME: f32 = 0.04;
const MAX_FREE_TIME: f32 = 2.0;
/// The most the delay may change per second while the motor slews: a pitch
/// bend of at most 0.6 either way.
const MOTOR_SLEW: f32 = 0.6;
const MOTOR_SECONDS: f32 = 0.15;

/// Peak speed error at full `wow`, as a fraction of the speed.
const WOW_DEPTH: f32 = 0.006;
const WOW_HZ: f32 = 0.55;
/// Peak speed error at full `flutter`.
const FLUTTER_DEPTH: f32 = 0.004;
const FLUTTER_HZ: f32 = 7.3;
const IDLER_HZ: f32 = 13.7;
const IDLER_SHARE: f32 = 0.4;
/// Peak speed drift at full `wow`.
const DRIFT_DEPTH: f32 = 0.002;
const DRIFT_SECONDS: f32 = 2.2;

const BUMP_DB: f32 = 3.0;
const DRIVE_DB: f32 = 24.0;
const HISS_LEVEL: f32 = 0.03;
/// Filters follow the transport this often, in samples.
const REFRESH: u32 = 32;

const PARAMS: [ParamSpec; 10] = [
    ParamSpec {
        name: "time",
        min: MIN_TIME,
        max: MAX_FREE_TIME,
        default: 0.35,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "sync",
        min: 0.0,
        max: 14.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: SYNC_LABELS,
        },
    },
    ParamSpec {
        name: "heads",
        min: 0.0,
        max: 6.0,
        default: 2.0,
        unit: "",
        curve: Curve::Stepped {
            labels: HEAD_LABELS,
        },
    },
    ParamSpec {
        name: "feedback",
        min: 0.0,
        max: 0.95,
        default: 0.4,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "wow",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "flutter",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "drive",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "wear",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "hiss",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The tape echo.
pub static KIND: EffectKind = EffectKind {
    id: "tape",
    name: "Tape echo",
    description: "A three-head tape echo with wow, flutter, saturation and losses that \
                  compound on every repeat; turning the time bends the pitch like real tape.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Tape::new())
}

/// One sinusoidal speed error and what it does to each head's delay.
#[derive(Debug, Clone, Copy, Default)]
struct Wobble {
    phase: Phasor,
    hz: f32,
}

impl Wobble {
    fn new(hz: f32) -> Self {
        Self {
            phase: Phasor::default(),
            hz,
        }
    }

    /// The delay error, in seconds, of a head `delays` seconds downstream
    /// for a speed error of peak `depth`: the integral of
    /// `depth · sin(2π f τ)` over the last `delay` seconds, negated (a fast
    /// tape arrives early).
    fn errors(self, depth: f32, delays: [f32; 3]) -> [f32; 3] {
        let angle = TAU * self.phase.phase();
        let now = angle.cos();
        let scale = depth / (TAU * self.hz);
        delays.map(|delay| scale * (now - TAU.mul_add(-self.hz * delay, angle).cos()))
    }

    fn advance(&mut self, sample_rate: f32) {
        self.phase.next(self.hz, sample_rate);
    }
}

/// One track of the tape: its loop and its record chain.
#[derive(Debug, Clone, Default)]
struct Track {
    line: DelayLine,
    emphasis: Biquad,
    deemphasis: Biquad,
    /// The saturation runs oversampled: the pre-emphasis lifts the top
    /// octave into it, and at 44.1 kHz its harmonics would fold straight
    /// back and its antiderivative dull the top.
    oversampler: Oversampler,
    clip: SoftClip,
    gap: Biquad,
    bump: Biquad,
    low: Biquad,
    hiss_tone: OnePole,
    level: Follower,
    /// The record level going into the saturation, held back by the
    /// oversampling's latency so it lines up with what comes out.
    lifted: DelayLine,
    /// The record level before and after saturation, for the makeup gain.
    before: Follower,
    after: Follower,
}

impl Track {
    fn clear(&mut self) {
        self.line.clear();
        self.emphasis.reset();
        self.deemphasis.reset();
        self.oversampler.reset();
        self.lifted.clear();
        self.clip.reset();
        self.gap.reset();
        self.bump.reset();
        self.low.reset();
        self.hiss_tone.reset();
        self.level.reset();
        self.before.reset();
        self.after.reset();
    }
}

/// The tape echo.
#[derive(Debug)]
pub struct Tape {
    rate: f32,
    prepared: bool,
    settled: bool,
    values: [f32; 10],
    motor: f64,
    motor_coeff: f64,
    motor_limit: f64,
    feedback: Smoothed,
    wow: Smoothed,
    flutter: Smoothed,
    drive: Smoothed,
    wear: Smoothed,
    hiss: Smoothed,
    mix: Smoothed,
    heads: [Smoothed; 3],
    capstan: Wobble,
    pinch: Wobble,
    idler: Wobble,
    drift: f32,
    drift_target: f32,
    drift_coeff: f32,
    drift_countdown: u32,
    noise: Noise,
    tracks: [Track; 2],
    sinc: Sinc,
    /// How many samples the oversampled saturation holds the record path
    /// back: the heads read that much sooner, so the echoes keep their time.
    record_latency: f64,
    refresh: u32,
    bump_trim: f32,
    /// White noise's level, raised with the rate so the hiss has the same
    /// power per hertz, and so the same loudness, at every rate.
    hiss_scale: f32,
}

impl Default for Tape {
    fn default() -> Self {
        Self::new()
    }
}

impl Tape {
    /// A tape echo at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        let heads = head_gains(values[HEADS]);
        Self {
            rate: 48_000.0,
            prepared: false,
            settled: false,
            values,
            motor: f64::from(values[TIME]),
            motor_coeff: 1.0,
            motor_limit: 1.0,
            feedback: Smoothed::new(values[FEEDBACK]),
            wow: Smoothed::new(values[WOW]),
            flutter: Smoothed::new(values[FLUTTER]),
            drive: Smoothed::new(values[DRIVE]),
            wear: Smoothed::new(values[WEAR]),
            hiss: Smoothed::new(values[HISS]),
            mix: Smoothed::new(values[MIX]),
            heads: heads.map(Smoothed::new),
            capstan: Wobble::new(WOW_HZ),
            pinch: Wobble::new(FLUTTER_HZ),
            idler: Wobble::new(IDLER_HZ),
            drift: 0.0,
            drift_target: 0.0,
            drift_coeff: 1.0,
            drift_countdown: 0,
            noise: Noise::new(0x7A9E_0201),
            tracks: [Track::default(), Track::default()],
            sinc: Sinc::default(),
            record_latency: 0.0,
            refresh: 0,
            bump_trim: 1.0 / db_to_gain(BUMP_DB),
            hiss_scale: 1.0,
        }
    }

    const fn smoothers(&mut self) -> [&mut Smoothed; 10] {
        let [one, two, three] = &mut self.heads;
        [
            &mut self.feedback,
            &mut self.wow,
            &mut self.flutter,
            &mut self.drive,
            &mut self.wear,
            &mut self.hiss,
            &mut self.mix,
            one,
            two,
            three,
        ]
    }

    /// The time the motor is heading for, in seconds.
    fn target_time(&self, bpm: f64) -> f32 {
        synced_seconds(self.values[SYNC], bpm)
            .unwrap_or(self.values[TIME])
            .clamp(MIN_TIME, LONGEST_SYNCED_SECONDS)
    }

    fn step_motor(&mut self, target: f64) {
        let step =
            ((target - self.motor) * self.motor_coeff).clamp(-self.motor_limit, self.motor_limit);
        self.motor += step;
    }

    fn step_drift(&mut self) {
        if self.drift_countdown == 0 {
            self.drift_target = self.noise.sample();
            self.drift_countdown = (DRIFT_SECONDS * self.rate) as u32;
        }
        self.drift_countdown -= 1;
        self.drift = (self.drift_target - self.drift).mul_add(self.drift_coeff, self.drift);
    }

    /// Each head's delay now, in samples, with the speed errors applied.
    /// Long delays need the precision of a double here.
    fn head_delays(&self, wow: f32, flutter: f32) -> [f64; 3] {
        let exact = HEAD_PLACES.map(|place| f64::from(place) * self.motor);
        let places = exact.map(|place| place as f32);
        let capstan = self.capstan.errors(wow * WOW_DEPTH, places);
        let pinch = self.pinch.errors(flutter * FLUTTER_DEPTH, places);
        let idler = self
            .idler
            .errors(flutter * FLUTTER_DEPTH * IDLER_SHARE, places);
        let drift = f64::from(self.drift * wow * DRIFT_DEPTH);
        let mut delays = [0.0; 3];
        for (k, delay) in delays.iter_mut().enumerate() {
            let wobble = f64::from(capstan[k] + pinch[k] + idler[k]);
            let seconds = exact[k].mul_add(1.0 - drift, wobble);
            *delay = seconds * f64::from(self.rate);
        }
        delays
    }

    fn refresh_filters(&mut self, wear: f32) {
        let speed = 1.0 / (self.motor as f32).max(MIN_TIME);
        let gap_hz = 1_800.0 * speed * 0.7f32.mul_add(-wear, 1.0);
        let bump_hz = (38.0 * speed).clamp(25.0, 400.0);
        let rate = self.rate;
        for track in &mut self.tracks {
            track.gap.set_lowpass(gap_hz.max(300.0), 0.707, rate);
            track.bump.set_peak(bump_hz, 1.0, BUMP_DB, rate);
        }
    }

    fn render(&mut self, bpm: f64, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let target = f64::from(self.target_time(bpm));
        if !self.settled {
            self.motor = target;
            self.settled = true;
        }
        for i in 0..n {
            self.step_motor(target);
            self.step_drift();
            let feedback = self.feedback.step();
            let wow = self.wow.step();
            let flutter = self.flutter.step();
            let drive = db_to_gain(self.drive.step() * DRIVE_DB);
            let wear = self.wear.step();
            let hiss = self.hiss.step() * HISS_LEVEL;
            let (dry, wet) = equal_power(self.mix.step());
            let gains = [
                self.heads[0].step(),
                self.heads[1].step(),
                self.heads[2].step(),
            ];
            if self.refresh == 0 {
                self.refresh_filters(wear);
                self.refresh = REFRESH;
            }
            self.refresh -= 1;
            let delays = self.head_delays(wow, flutter);
            let count = (gains[0] + gains[1] + gains[2]).max(1.0);
            let listen = 1.0 / count.sqrt();
            let bump_trim = self.bump_trim;
            let latency = self.record_latency as usize;
            for (c, track) in self.tracks.iter_mut().enumerate() {
                let x = clean(input[c][i]);
                let mut heard = 0.0;
                for (gain, delay) in gains.iter().zip(delays) {
                    if *gain > 1e-4 {
                        let read = self.sinc.read(&track.line, delay - self.record_latency);
                        heard = gain.mul_add(read, heard);
                    }
                }
                let into = (feedback * bump_trim / count).mul_add(heard, x);
                let lifted = track.emphasis.process(into);
                let clip = &mut track.clip;
                let saturated = track
                    .oversampler
                    .process(lifted, |v| clip.process(v * drive) / drive);
                // Give back the level the saturation squeezed out, so drive
                // turns up the grit rather than turning down the echoes.
                let squeezed = track.after.process(saturated).max(1e-9);
                track.lifted.push(lifted);
                let aligned = track.lifted.tap(latency + 1);
                let makeup = (track.before.process(aligned) / squeezed).clamp(1.0, drive);
                let played = track.deemphasis.process(saturated * makeup);
                let mut record = track
                    .low
                    .process(track.bump.process(track.gap.process(played)));
                flush(&mut record);
                track.line.push(record);
                let echo = heard * listen;
                let level = track.level.process(echo);
                let white = self.noise.sample() * self.hiss_scale;
                let hum = track.hiss_tone.lowpass(white) * hiss * level;
                output[c][i] = guard(dry.mul_add(x, wet * (echo + hum)));
            }
            self.capstan.advance(self.rate);
            self.pinch.advance(self.rate);
            self.idler.advance(self.rate);
        }
    }
}

/// Which heads play (1.0) and which do not (0.0) at `heads` step `step`.
fn head_gains(step: f32) -> [f32; 3] {
    let index = (step.round().max(0.0) as usize).min(HEAD_SETS.len() - 1);
    HEAD_SETS[index].map(|on| if on { 1.0 } else { 0.0 })
}

impl Effect for Tape {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        self.sinc = Sinc::new();
        let longest = (LONGEST_SYNCED_SECONDS + 0.1) * rate;
        for track in &mut self.tracks {
            track.line.resize(longest as usize);
            track.emphasis.set_shelf(2_000.0, 6_000.0, rate);
            track.deemphasis.set_shelf(6_000.0, 2_000.0, rate);
            // The saturation's antiderivative adds half a sample at its rate.
            track
                .oversampler
                .configure(factor_for(rate, TARGET_RATE), rate, 0.5);
            track.low.set_highpass(40.0, 0.707, rate);
            track.hiss_tone.set_cutoff(7_000.0, rate);
            track.level.set_times(0.005, 0.2, rate);
            track.before.set_times(0.05, 0.3, rate);
            track.after.set_times(0.05, 0.3, rate);
        }
        self.record_latency = self.tracks[0].oversampler.latency() as f64;
        for track in &mut self.tracks {
            track.lifted.resize(self.record_latency as usize + 2);
        }
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        for head in &mut self.heads {
            head.set_time(0.03, rate);
        }
        self.hiss_scale = (rate / 48_000.0).sqrt();
        self.motor_coeff = f64::from(coefficient(MOTOR_SECONDS, rate));
        self.motor_limit = f64::from(MOTOR_SLEW / rate);
        self.drift_coeff = coefficient(DRIFT_SECONDS * 0.6, rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for track in &mut self.tracks {
            track.clear();
        }
        for smoother in self.smoothers() {
            smoother.snap(smoother.target());
        }
        self.capstan.phase.set(0.0);
        self.pinch.phase.set(0.3);
        self.idler.phase.set(0.7);
        self.drift = 0.0;
        self.drift_target = 0.0;
        self.drift_countdown = 0;
        self.refresh = 0;
        self.settled = false;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        self.values[index] = value;
        match index {
            FEEDBACK => self.feedback.set(value),
            WOW => self.wow.set(value),
            FLUTTER => self.flutter.set(value),
            DRIVE => self.drive.set(value),
            WEAR => self.wear.set(value),
            HISS => self.hiss.set(value),
            MIX => self.mix.set(value),
            HEADS => {
                for (head, gain) in self.heads.iter_mut().zip(head_gains(value)) {
                    head.set(gain);
                }
            }
            _ => {}
        }
    }

    fn process(&mut self, context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let mut output = output;
        let n = if self.prepared {
            frames(&input, &output)
        } else {
            0
        };
        self.render(context.bpm, input, &mut output, n);
        silence_from(&mut output, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::testkit::{RATE, context, impulse, peak_index, prepared, render};

    fn quiet(effect: &mut dyn Effect) {
        for (index, value) in [
            (WOW, 0.0),
            (FLUTTER, 0.0),
            (HISS, 0.0),
            (MIX, 1.0),
            (FEEDBACK, 0.0),
        ] {
            effect.set_param(index, value);
        }
        effect.reset();
    }

    #[test]
    fn the_last_head_echoes_at_the_time() {
        let mut tape = prepared(&KIND);
        quiet(tape.as_mut());
        tape.set_param(TIME, 0.3);
        tape.reset();
        let input = impulse(RATE as usize, 100);
        let (left, right) = render(tape.as_mut(), context(120.0), &input, &input, 256);
        let expected = 100 + (0.3 * RATE) as usize;
        for channel in [&left, &right] {
            let at = peak_index(channel);
            assert!(at.abs_diff(expected) < 24, "{at} vs {expected}");
        }
    }

    #[test]
    fn every_head_plays_at_its_place() {
        let mut tape = prepared(&KIND);
        quiet(tape.as_mut());
        tape.set_param(TIME, 0.6);
        tape.set_param(HEADS, 0.0);
        tape.reset();
        let input = impulse(RATE as usize, 0);
        let (left, _) = render(tape.as_mut(), context(120.0), &input, &input, 256);
        let at = peak_index(&left);
        assert!(at.abs_diff((0.2 * RATE) as usize) < 24, "{at}");
    }

    #[test]
    fn a_synced_echo_lands_on_the_beat() {
        let mut tape = prepared(&KIND);
        quiet(tape.as_mut());
        // A dotted eighth at 100 BPM: 0.45 s.
        tape.set_param(SYNC, 7.0);
        tape.reset();
        let input = impulse(RATE as usize, 0);
        let (left, _) = render(tape.as_mut(), context(100.0), &input, &input, 512);
        let at = peak_index(&left);
        assert!(at.abs_diff((0.45 * RATE) as usize) < 24, "{at}");
    }

    #[test]
    fn echoes_lose_treble_as_they_repeat() {
        let mut tape = prepared(&KIND);
        quiet(tape.as_mut());
        tape.set_param(TIME, 0.25);
        tape.set_param(FEEDBACK, 0.9);
        tape.reset();
        let input = impulse(RATE as usize * 2, 0);
        let (left, _) = render(tape.as_mut(), context(120.0), &input, &input, 256);
        let step = (0.25 * RATE) as usize;
        let edge = |from: usize| {
            left[from..from + step]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .sum::<f32>()
                / left[from..from + step]
                    .iter()
                    .map(|v| v.abs())
                    .sum::<f32>()
                    .max(1e-9)
        };
        let first = edge(step - 100);
        let fourth = edge(4 * step - 100);
        assert!(fourth < first * 0.8, "{first} {fourth}");
    }

    #[test]
    fn changing_the_time_glides_without_a_click() {
        let mut tape = prepared(&KIND);
        quiet(tape.as_mut());
        tape.set_param(TIME, 0.1);
        tape.reset();
        let n = RATE as usize;
        let sine: Vec<f32> = (0..n)
            .map(|i| 0.5 * (TAU * 220.0 * i as f32 / RATE).sin())
            .collect();
        let ctx = context(120.0);
        let half = n / 2;
        let (a, _) = render(tape.as_mut(), ctx, &sine[..half], &sine[..half], 128);
        tape.set_param(TIME, 0.4);
        let (b, _) = render(tape.as_mut(), ctx, &sine[half..], &sine[half..], 128);
        let joined: Vec<f32> = a.into_iter().chain(b).collect();
        // A 220 Hz sine at 0.5 moves at most about 0.0144 per sample; bent up
        // by the motor it may move a little more, but a click would jump.
        let jump = joined[5_000..]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        assert!(jump < 0.04, "{jump}");
    }
}
