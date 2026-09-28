//! VHS audio.
//!
//! # The model
//!
//! A VHS deck records sound two ways, and `mode` picks which one you hear.
//!
//! **Linear** is the mono audio track along the tape's edge, read by a
//! fixed head as on a cassette but at 1.31 inches per second in SP (half
//! that in LP, a third in EP). It is the sound of a rented tape:
//!
//! - The deck's automatic level control rides the recording level (a slow
//!   2:1 leveller around -12 dBFS), so quiet passages come up and loud ones
//!   pump.
//! - The tape saturates ([`Magnetic`], oversampled twice).
//! - Gap and spacing loss at that speed ([`head_corner`]) take the treble
//!   down to about 10 kHz in SP and much lower in EP (decks boost treble a
//!   little more at slow speeds to compensate); the low end rolls off
//!   under about 100 Hz.
//! - The transport wobbles: the supply hub, the capstan, the tracking
//!   servo hunting, and the video head drum, whose 29.97 Hz rotation
//!   ripples the tape tension ([`Wobble`]).
//! - Hiss is high: a linear track manages about 42 dB of signal to noise.
//!
//! **Hi-fi** is the stereo FM sound buried under the picture by the
//! rotating heads. The tape moves past those heads at nearly six metres a
//! second, so the transport's wow hardly touches it and the bandwidth is
//! full; what gives it away is the head switching. Every field (59.94 times
//! a second) one head hands over to the other, and a tape that does not
//! track cleanly buzzes there, and drops into noise bursts that the deck's
//! noise-reduction expander makes louder when the music is louder.
//!
//! `tracking` runs from a cleanly tracked tape to one that needs the
//! tracking knob: dropouts, head-switching buzz, and bursts of fast warble
//! as the servo loses and finds the control track (the audio cousin of the
//! colour flicker on the picture). `age` is a worn tape: more hiss, less
//! treble, more dropouts, more wobble.

use super::parts::{
    self, Biquad, Butterworth, CONTROL, Decibels, Dropouts, Head, Hiss, Magnetic, Oversampler2,
    Partial, Ramp, Wobble,
};
use crate::dsp::{DelayLine, Noise, OnePole, Phasor, Smoothed, db_to_gain, gain_to_db};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const MODE: usize = 0;
const SPEED: usize = 1;
const WOBBLE: usize = 2;
const TRACKING: usize = 3;
const AGE: usize = 4;
const HISS: usize = 5;
const MIX: usize = 6;
const OUTPUT: usize = 7;
const COUNT: usize = 8;

const fn percent(name: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.0,
        max: 100.0,
        default,
        unit: "%",
        curve: Curve::Linear,
    }
}

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "mode",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["linear", "hi-fi"],
        },
    },
    ParamSpec {
        name: "speed",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["SP", "LP", "EP"],
        },
    },
    percent("wobble", 30.0),
    percent("tracking", 10.0),
    percent("age", 25.0),
    percent("hiss", 30.0),
    percent("mix", 100.0),
    ParamSpec {
        name: "output",
        min: -18.0,
        max: 6.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
];

/// VHS audio.
pub static KIND: EffectKind = EffectKind {
    id: "vhs",
    name: "VHS",
    description: "VHS audio: the mono linear track's narrow band, hiss, level control and \
                  wobble, or the hi-fi track's head-switching buzz, with a tracking knob \
                  from clean to mistracked and an age knob for worn tape.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Vhs::new())
}

/// Linear-track tape speed for SP, LP and EP, metres per second.
const SPEEDS: [f32; 3] = [0.033_35, 0.016_67, 0.011_12];
/// How much extra treble the deck's playback equalisation buys back at
/// each speed, as a factor on the head's corner.
const EQ_HELP: [f32; 3] = [1.0, 1.2, 1.4];
/// Linear head gap and tape spacing, metres.
const GAP: f32 = 1.0e-6;
const SPACING: f32 = 0.25e-6;
/// Peak speed error at full `wobble` on new tape in SP.
const MAX_WOBBLE: f32 = 0.012;
/// Peak speed error of a full tracking warble burst.
const MAX_WARBLE: f32 = 0.015;
/// Field rate: how often the video heads hand over, Hz.
const FIELD: f32 = 59.94;
/// Linear-track hiss at full `hiss` in SP, as linear RMS.
const HISS_TOP: f32 = 0.02;

static TRANSPORT: [Partial; 3] = [
    Partial {
        hz: 0.35,
        weight: 0.35,
        wander: 0.2,
        steady: 0.5,
    },
    Partial {
        hz: 1.8,
        weight: 0.35,
        wander: 0.05,
        steady: 0.7,
    },
    Partial {
        hz: 6.5,
        weight: 0.3,
        wander: 0.3,
        steady: 0.2,
    },
];

static DRUM: [Partial; 1] = [Partial {
    hz: 29.97,
    weight: 1.0,
    wander: 0.01,
    steady: 0.9,
}];

static WARBLE: [Partial; 3] = [
    Partial {
        hz: 12.0,
        weight: 0.4,
        wander: 0.3,
        steady: 0.0,
    },
    Partial {
        hz: 17.0,
        weight: 0.35,
        wander: 0.3,
        steady: 0.0,
    },
    Partial {
        hz: 24.0,
        weight: 0.25,
        wander: 0.3,
        steady: 0.0,
    },
];

/// The linear track: mono.
#[derive(Debug, Clone)]
struct Linear {
    level: f32,
    alc: Ramp,
    oversampler: Oversampler2,
    tape: Magnetic,
    hiss: Hiss,
    dropout: OnePole,
    head: Butterworth,
    low_cut: Butterworth,
}

impl Linear {
    fn new() -> Self {
        Self {
            level: 0.0,
            alc: Ramp::new(db_to_gain(6.0), CONTROL),
            oversampler: Oversampler2::new(),
            tape: Magnetic::default(),
            hiss: Hiss::new(0x0005_0001),
            dropout: OnePole::default(),
            head: Butterworth::default(),
            low_cut: Butterworth::default(),
        }
    }

    fn reset(&mut self) {
        self.level = 0.0;
        self.alc.snap(db_to_gain(6.0));
        self.oversampler.reset();
        self.tape.reset();
        self.hiss.reset();
        self.dropout.reset();
        self.head.reset();
        self.low_cut.reset();
    }

    /// Record `x` through the level control onto the linear track and
    /// play it back.
    fn play(&mut self, x: f32, frame: &Frame) -> f32 {
        // The leveller follows the programme's RMS: fast up, slow down.
        let power = x * x;
        let coeff = if power > self.level {
            frame.alc_attack
        } else {
            frame.alc_release
        };
        self.level = (power - self.level).mul_add(coeff, self.level);
        crate::dsp::flush(&mut self.level);
        let level = self.level;
        let gain = self.alc.next(|| {
            let level_db = gain_to_db(level.sqrt());
            db_to_gain((-0.5 * (level_db + 12.0)).clamp(-12.0, 6.0))
        });
        let Self {
            oversampler, tape, ..
        } = self;
        let recorded = oversampler.process(x * gain, |v| tape.process(v, 2.0, 0.15));
        let on_tape = self.hiss.next().mul_add(frame.hiss, recorded) + frame.buzz;
        let lifted = parts::lerp(on_tape, self.dropout.lowpass(on_tape), frame.loss);
        let read = self
            .head
            .process(lifted * (-0.8f32).mul_add(frame.loss, 1.0));
        self.low_cut.process(read)
    }
}

/// One channel of the hi-fi track.
#[derive(Debug, Clone)]
struct HiFi {
    level: f32,
    hiss: Hiss,
    burst: Noise,
    band: Butterworth,
    low_cut: Biquad,
}

impl HiFi {
    fn new(seed: u32) -> Self {
        Self {
            level: 0.0,
            hiss: Hiss::new(seed),
            burst: Noise::new(seed ^ 0x00FF_00FF),
            band: Butterworth::default(),
            low_cut: Biquad::new(),
        }
    }

    fn reset(&mut self, seed: u32) {
        self.level = 0.0;
        self.hiss.reset();
        self.burst = Noise::new(seed ^ 0x00FF_00FF);
        self.band.reset();
        self.low_cut.reset();
    }

    /// Play `x` back off the hi-fi heads.
    fn play(&mut self, x: f32, frame: &Frame) -> f32 {
        let size = x.abs();
        let coeff = if size > self.level { 0.01 } else { 0.000_5 };
        self.level = (size - self.level).mul_add(coeff, self.level);
        crate::dsp::flush(&mut self.level);
        // Noise the FM path lets in comes back through the 1:2 expander:
        // its level follows the programme.
        let expand = self.level.mul_add(0.8, 0.05);
        let noise = self.hiss.next() * frame.hiss * 0.08 * expand.sqrt();
        let burst = self.burst.sample() * frame.loss * 0.1 * expand;
        let held = parts::finite(x).clamp(-2.0, 2.0) * (1.0 - frame.loss);
        let sum = (frame.buzz * expand).mul_add(4.0, held + noise + burst);
        self.band.process(self.low_cut.process(sum))
    }
}

/// What one sample shares between the paths.
#[derive(Debug, Clone, Copy)]
struct Frame {
    hiss: f32,
    loss: f32,
    buzz: f32,
    alc_attack: f32,
    alc_release: f32,
}

/// VHS audio. See the module documentation for the model.
#[derive(Debug, Clone)]
pub struct Vhs {
    rate: f32,
    knobs: [Smoothed; COUNT],
    speed: Smoothed,
    hifi: Smoothed,
    lines: [DelayLine; 2],
    linear: Linear,
    stereo: [HiFi; 2],
    transport: Wobble,
    drum: Wobble,
    warble: Wobble,
    dropouts: Dropouts,
    bursts: Dropouts,
    burst: OnePole,
    switch: Phasor,
    tick: f32,
    tick_decay: f32,
    tick_noise: Noise,
    base: f32,
    output_gain: Decibels,
    until_control: usize,
    frame: Frame,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, MIX | OUTPUT)
}

impl Vhs {
    /// A deck at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut deck = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            speed: Smoothed::new(0.0),
            hifi: Smoothed::new(0.0),
            lines: [DelayLine::default(), DelayLine::default()],
            linear: Linear::new(),
            stereo: [HiFi::new(0x0005_0002), HiFi::new(0x0005_0003)],
            transport: Wobble::new(&TRANSPORT, 0x0005_0004),
            drum: Wobble::new(&DRUM, 0x0005_0005),
            warble: Wobble::new(&WARBLE, 0x0005_0006),
            dropouts: Dropouts::new(0x0005_0007),
            bursts: Dropouts::new(0x0005_0008),
            burst: OnePole::default(),
            switch: Phasor::default(),
            tick: 0.0,
            tick_decay: 0.0,
            tick_noise: Noise::new(0x0005_0009),
            base: 0.0,
            output_gain: Decibels::new(),
            until_control: 0,
            frame: Frame {
                hiss: 0.0,
                loss: 0.0,
                buzz: 0.0,
                alc_attack: 0.0,
                alc_release: 0.0,
            },
        };
        deck.prepare(48_000.0);
        deck
    }

    fn step_index(&self) -> usize {
        (self.knobs[SPEED].target() as usize).min(2)
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and move the filters.
    fn control(&mut self) {
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        let step = self.step_index();
        self.speed.set(SPEEDS[step]);
        let speed = self.speed.step();
        let help = EQ_HELP[step];
        let rate = self.rate;
        let age = self.knobs[AGE].value() / 100.0;
        let tracking = self.knobs[TRACKING].value() / 100.0;
        let head = Head {
            speed,
            gap: GAP,
            skew: 0.0,
            spacing: age.mul_add(0.4e-6, SPACING) + tracking * 0.2e-6,
        };
        let corner = (parts::head_corner(head, rate) * help).min(rate * 0.45);
        self.linear.head.lowpass(2, corner, rate);
        let low = 90.0 * (SPEEDS[0] / speed).powf(0.25);
        self.linear.low_cut.highpass(1, low, rate);
        let hiss = self.knobs[HISS].value() / 100.0;
        let slow = (SPEEDS[0] / speed).sqrt();
        self.frame.hiss = HISS_TOP * hiss * hiss.sqrt() * slow * age.mul_add(1.0, 1.0);
    }

    /// One stereo sample.
    fn tick(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.until_control == 0 {
            self.control();
            self.until_control = CONTROL;
        }
        self.until_control -= 1;
        let rate = self.rate;
        let mix = self.knobs[MIX].step() / 100.0;
        let output = self.output_gain.gain(self.knobs[OUTPUT].step());
        self.hifi.set(self.knobs[MODE].target());
        let hifi = self.hifi.step();
        let age = self.knobs[AGE].value() / 100.0;
        let tracking = self.knobs[TRACKING].value() / 100.0;
        let slowness = self.speed.value();

        let depth = MAX_WOBBLE * self.knobs[WOBBLE].value() / 100.0
            * age.mul_add(0.5, 1.0)
            * (SPEEDS[0] / slowness).sqrt();
        let transport = self.transport.next(depth, slowness / SPEEDS[0], 1.0, rate);
        let drum = self.drum.next(depth * 0.1, 1.0, 1.0, rate);
        let gate = self.bursts.next(1.5 * tracking, 1.0, 0.25, rate);
        let warble = self.warble.next(
            MAX_WARBLE * tracking * self.burst.lowpass(gate),
            1.0,
            1.0,
            rate,
        );

        self.frame.loss = self.dropouts.next(
            (8.0 * tracking).mul_add(tracking, 1.5 * age),
            0.95,
            0.04,
            rate,
        );
        if self.switch.next(FIELD, rate) + FIELD / rate >= 1.0 {
            self.tick = 1.0;
        }
        self.tick *= self.tick_decay;
        crate::dsp::flush(&mut self.tick);
        let buzz_level = 0.01f32.mul_add(tracking, 0.004 * age);
        self.frame.buzz = self.tick * self.tick_noise.sample() * buzz_level;

        let base = self.base;
        let frame = self.frame;
        for (line, input) in self.lines.iter_mut().zip([left, right]) {
            line.push(input);
        }
        let linear_at = (base + transport + drum + warble) * rate;
        // The hi-fi path has no oversampler; it waits for the linear one.
        let hifi_at = 0.5f32
            .mul_add(warble, base + 0.15f32.mul_add(transport, drum))
            .mul_add(rate, Oversampler2::LATENCY);
        let dry_at = base.mul_add(rate, Oversampler2::LATENCY);
        let mono = 0.5 * (self.lines[0].read(linear_at) + self.lines[1].read(linear_at));
        let narrow = self.linear.play(mono, &frame);
        let mut out = [0.0f32; 2];
        for ((wet, line), channel) in out.iter_mut().zip(&self.lines).zip(&mut self.stereo) {
            let wide = channel.play(line.read(hifi_at), &frame);
            let played = parts::lerp(narrow, wide, hifi);
            *wet = parts::lerp(line.read(dry_at), played, mix) * output;
        }
        out.into()
    }
}

impl Default for Vhs {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Vhs {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = parts::sane_rate(sample_rate);
        self.rate = rate;
        let control_rate = rate / CONTROL as f32;
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.set_time(0.05, control_rate);
            } else {
                knob.set_time(0.02, rate);
            }
        }
        self.speed.set_time(0.2, control_rate);
        self.hifi.set_time(0.04, rate);
        let slowest = SPEEDS[0] / SPEEDS[2];
        let reach = self
            .transport
            .reach(MAX_WOBBLE * 1.5 * slowest.sqrt(), 1.0 / slowest)
            + self.drum.reach(MAX_WOBBLE * 0.15 * slowest.sqrt(), 1.0)
            + self.warble.reach(MAX_WARBLE, 1.0);
        self.base = reach + 0.000_5 + 4.0 / rate;
        let longest = (2.0 * self.base * rate) as usize + Oversampler2::LATENCY as usize + 16;
        for line in &mut self.lines {
            line.resize(longest);
        }
        self.linear.hiss.band(100.0, rate * 0.45, rate);
        self.linear.dropout.set_cutoff(800.0, rate);
        for channel in &mut self.stereo {
            channel.hiss.band(20.0, 20_000.0, rate);
            channel.band.lowpass(2, 20_000.0f32.min(rate * 0.45), rate);
            channel.low_cut.highpass(20.0, 0.7, rate);
        }
        self.burst.set_cutoff(8.0, rate);
        self.tick_decay = (-1.0 / (0.000_4 * rate)).exp();
        let coeff = |seconds: f32| 1.0 - (-1.0 / (seconds * rate)).exp();
        self.frame.alc_attack = coeff(0.03);
        self.frame.alc_release = coeff(0.8);
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        self.speed.snap(SPEEDS[self.step_index()]);
        self.hifi.snap(self.knobs[MODE].target());
        for line in &mut self.lines {
            line.clear();
        }
        self.linear.reset();
        for (channel, seed) in self.stereo.iter_mut().zip([0x0005_0002, 0x0005_0003]) {
            channel.reset(seed);
        }
        self.transport.reset();
        self.drum.reset();
        self.warble.reset();
        self.dropouts.reset();
        self.bursts.reset();
        self.burst.reset();
        self.switch.set(0.0);
        self.tick = 0.0;
        self.tick_noise = Noise::new(0x0005_0009);
        self.until_control = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(spec) = PARAMS.get(index) {
            if value.is_finite() {
                self.knobs[index].set(spec.clamp(value));
            }
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        parts::run_block(input, output, |left, right| self.tick(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::parts::testkit::{
        self, built, frequency_deviation, peak, render, rms, silence, sine,
    };
    use super::*;

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn silence_stays_silent_with_the_noise_knobs_down() {
        let input = silence(1.0);
        for mode in [0.0, 1.0] {
            let mut deck = built(
                &KIND,
                &[(MODE, mode), (HISS, 0.0), (TRACKING, 0.0), (AGE, 0.0)],
            );
            let (left, right) = render(deck.as_mut(), &input, &input);
            assert!(peak(&left) < 1e-6 && peak(&right) < 1e-6, "{mode}");
        }
    }

    #[test]
    fn noise_is_bounded_when_everything_is_up() {
        let input = silence(2.0);
        for mode in [0.0, 1.0] {
            for speed in [0.0, 2.0] {
                let mut deck = built(
                    &KIND,
                    &[
                        (MODE, mode),
                        (SPEED, speed),
                        (HISS, 100.0),
                        (TRACKING, 100.0),
                        (AGE, 100.0),
                    ],
                );
                let (left, right) = render(deck.as_mut(), &input, &input);
                assert!(rms(&left) < 0.05 && peak(&right) < 0.4, "{mode} {speed}");
            }
        }
        let mut linear = built(&KIND, &[(HISS, 100.0), (TRACKING, 0.0), (AGE, 0.0)]);
        let mut hifi = built(
            &KIND,
            &[(MODE, 1.0), (HISS, 100.0), (TRACKING, 0.0), (AGE, 0.0)],
        );
        let a = rms(&render(linear.as_mut(), &input, &input).0[4_800..]);
        let b = rms(&render(hifi.as_mut(), &input, &input).0[4_800..]);
        assert!(a > 1e-3 && b < a * 0.2, "linear {a}, hi-fi {b}");
    }

    #[test]
    fn wobble_tracks_its_knob() {
        let tone = sine(1_000.0, 0.25, 3.0);
        let deviation = |wobble: f32| {
            let mut deck = built(
                &KIND,
                &[(WOBBLE, wobble), (TRACKING, 0.0), (AGE, 0.0), (HISS, 0.0)],
            );
            let (left, _) = render(deck.as_mut(), &tone, &tone);
            frequency_deviation(&left, 1_000.0, 0.5)
        };
        assert!(deviation(0.0) < 2e-4);
        let (half, full) = (deviation(50.0), deviation(100.0));
        assert!(full > 0.004 && full < MAX_WOBBLE * 1.3, "{full}");
        assert!(half > full * 0.3 && half < full * 0.7, "{half} {full}");
    }

    #[test]
    fn slower_speeds_lose_more_treble() {
        let tone = sine(6_000.0, 0.1, 1.0);
        let level = |speed: f32| {
            let mut deck = built(
                &KIND,
                &[
                    (SPEED, speed),
                    (WOBBLE, 0.0),
                    (TRACKING, 0.0),
                    (AGE, 0.0),
                    (HISS, 0.0),
                ],
            );
            let (left, _) = render(deck.as_mut(), &tone, &tone);
            rms(&left[24_000..])
        };
        let (sp, ep) = (level(0.0), level(2.0));
        assert!(gain_to_db(ep) < gain_to_db(sp) - 6.0, "{sp} {ep}");
    }

    #[test]
    fn linear_is_mono_and_hifi_is_stereo() {
        let left = sine(500.0, 0.3, 0.5);
        let right = silence(0.5);
        let clean = [(WOBBLE, 0.0), (TRACKING, 0.0), (AGE, 0.0), (HISS, 0.0)];
        let mut linear = built(&KIND, &clean);
        let (l, r) = render(linear.as_mut(), &left, &right);
        assert!((rms(&l[4_800..]) - rms(&r[4_800..])).abs() < 1e-6);
        let mut params = clean.to_vec();
        params.push((MODE, 1.0));
        let mut hifi = built(&KIND, &params);
        let (l, r) = render(hifi.as_mut(), &left, &right);
        assert!(rms(&l[4_800..]) > 0.1 && rms(&r[4_800..]) < 1e-3);
    }
}
