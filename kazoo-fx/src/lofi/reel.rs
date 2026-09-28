//! A studio reel-to-reel machine.
//!
//! # The model
//!
//! Subtle and warm at its defaults, the way a well-aligned two-track at
//! 15 ips is: a little low-end weight, a little rounding of transients, a
//! trace of hiss.
//!
//! 1. **Transport.** Speed error from the reels (under a turn a second at
//!    15 ips), the pinch roller, the capstan, the guides and the motor,
//!    each a [`Wobble`] partial whose rate scales with tape speed. A good
//!    machine measures under 0.05%; `flutter` runs from perfect up to a
//!    tired 0.4%.
//! 2. **Record equalisation.** Treble is boosted before the tape by the
//!    standard time constant for the speed (IEC 70 µs at 7.5 ips, 35 µs at
//!    15, AES 17.5 µs at 30) and cut after it, so treble saturates first
//!    and slower speeds saturate sooner.
//! 3. **Tape.** Jiles–Atherton magnetisation ([`Magnetic`]) at four times
//!    the rate. `drive` is how hard the tape is hit; small signals keep
//!    their level whatever the drive, loud ones compress into the Langevin
//!    curve. `bias` sets how much of the raw hysteresis loop survives: at
//!    its middle the machine is biased for lowest distortion; below,
//!    under-bias leaves the loop in (grit, lag, a little extra treble);
//!    above, over-bias erases treble as it records, more at slow speeds.
//! 4. **Print-through.** Stored on the reel, each wrap magnetises its
//!    neighbours faintly. The wrap next to a passage is one turn of the
//!    reel away, about 1.6 s at 15 ips on a half-full 10½-inch reel, so
//!    the ghost is a quiet, mid-band copy that far before (tape stored
//!    heads-out) or after (tails-out). Pre-echo delays the programme by one
//!    turn so its ghost can arrive first; switching modes crossfades.
//! 5. **Playback head.** Gap and spacing loss from the geometry
//!    ([`head_corner`]), the head bump at about 3.5 Hz per inch per second
//!    with the ripple just above it and the roll-off below, then the
//!    playback de-emphasis.
//!
//! Changing speed glides every speed-dependent part as a real transport
//! spooling up and down does.

use super::parts::{
    self, Biquad, Butterworth, CONTROL, Decibels, Head, Hiss, Magnetic, Oversampler4, Partial,
    Wobble,
};
use crate::dsp::{DelayLine, Smoothed, db_to_gain};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const SPEED: usize = 0;
const DRIVE: usize = 1;
const BIAS: usize = 2;
const BUMP: usize = 3;
const FLUTTER: usize = 4;
const HISS: usize = 5;
const PRINT: usize = 6;
const GHOST: usize = 7;
const MIX: usize = 8;
const OUTPUT: usize = 9;
const COUNT: usize = 10;

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
        name: "speed",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["7.5 ips", "15 ips", "30 ips"],
        },
    },
    ParamSpec {
        name: "drive",
        min: -12.0,
        max: 18.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    percent("bias", 50.0),
    percent("bump", 50.0),
    percent("flutter", 15.0),
    percent("hiss", 10.0),
    ParamSpec {
        name: "print",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["off", "pre-echo", "post-echo"],
        },
    },
    percent("ghost", 30.0),
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

/// The reel-to-reel machine.
pub static KIND: EffectKind = EffectKind {
    id: "reel",
    name: "Reel-to-reel",
    description: "A studio reel-to-reel machine: 7.5, 15 or 30 ips, oversampled \
                  Jiles-Atherton tape saturation with bias, head bump, gap loss, flutter, \
                  hiss and print-through echoes. Warm and subtle by default.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Reel::new())
}

/// Tape speed for each step, inches per second.
const IPS: [f32; 3] = [7.5, 15.0, 30.0];
/// Metres per second in an inch per second.
const INCH: f32 = 0.025_4;
/// Reproduce head gap and tape spacing, metres.
const GAP: f32 = 3.0e-6;
const SPACING: f32 = 0.4e-6;
/// Circumference of a half-full 10½-inch reel, metres.
const WRAP: f32 = 0.6;
/// Peak speed error at full `flutter`.
const MAX_FLUTTER: f32 = 0.004;
/// Hiss at full `hiss` at 15 ips, before de-emphasis, as linear RMS.
const HISS_TOP: f32 = 0.012;
/// How long a print-mode or speed change crossfades, seconds.
const FADE: f32 = 0.05;

static PARTS: [Partial; 5] = [
    Partial {
        hz: 0.8,
        weight: 0.25,
        wander: 0.2,
        steady: 0.5,
    },
    Partial {
        hz: 2.1,
        weight: 0.15,
        wander: 0.1,
        steady: 0.6,
    },
    Partial {
        hz: 4.8,
        weight: 0.3,
        wander: 0.03,
        steady: 0.8,
    },
    Partial {
        hz: 11.0,
        weight: 0.15,
        wander: 0.2,
        steady: 0.3,
    },
    Partial {
        hz: 25.0,
        weight: 0.15,
        wander: 0.05,
        steady: 0.8,
    },
];

/// Record equalisation for a speed: the time constant's frequency, and the
/// boost.
fn emphasis(ips: f32) -> (f32, f32) {
    // 70 µs, 35 µs and 17.5 µs all halve with each doubling of speed.
    let turnover = 2_274.0 * ips / 7.5;
    let boost = 2.0f32.mul_add(-(ips / 7.5).log2(), 10.0);
    (turnover, boost)
}

/// A delay tap that crossfades, rather than slides, to a new length.
#[derive(Debug, Clone, Copy)]
struct Tap {
    from: f32,
    to: f32,
    fade: f32,
}

impl Tap {
    const fn at(delay: f32) -> Self {
        Self {
            from: delay,
            to: delay,
            fade: 1.0,
        }
    }

    /// Head for `delay`, once any crossfade under way has finished.
    fn aim(&mut self, delay: f32) {
        if self.fade >= 1.0 && (delay - self.to).abs() > 0.5 {
            self.from = self.to;
            self.to = delay;
            self.fade = 0.0;
        }
    }

    fn step(&mut self, by: f32) {
        self.fade = (self.fade + by).min(1.0);
    }

    /// Read `line` here, `extra` samples further back.
    fn read(&self, line: &DelayLine, extra: f32) -> f32 {
        let to = line.read(self.to + extra);
        if self.fade >= 1.0 {
            to
        } else {
            // Equal power: the two taps hold unrelated audio.
            let angle = self.fade * std::f32::consts::FRAC_PI_2;
            let from = line.read(self.from + extra);
            from.mul_add(angle.cos(), to * angle.sin())
        }
    }
}

/// What one sample shares between the two channels.
#[derive(Debug, Clone, Copy)]
struct Frame {
    drive: f32,
    loop_share: f32,
    ghost: f32,
    hiss: f32,
}

/// One channel of the machine.
#[derive(Debug, Clone)]
struct Channel {
    line: DelayLine,
    emphasis: Biquad,
    oversampler: Oversampler4,
    tape: Magnetic,
    erasure: Biquad,
    print: Biquad,
    print_top: Biquad,
    hiss: Hiss,
    head: Butterworth,
    bump: Biquad,
    ripple: Biquad,
    low_cut: Biquad,
    deemphasis: Biquad,
}

impl Channel {
    fn new(seed: u32) -> Self {
        Self {
            line: DelayLine::default(),
            emphasis: Biquad::new(),
            oversampler: Oversampler4::default(),
            tape: Magnetic::default(),
            erasure: Biquad::new(),
            print: Biquad::new(),
            print_top: Biquad::new(),
            hiss: Hiss::new(seed),
            head: Butterworth::default(),
            bump: Biquad::new(),
            ripple: Biquad::new(),
            low_cut: Biquad::new(),
            deemphasis: Biquad::new(),
        }
    }

    fn reset(&mut self) {
        self.line.clear();
        self.emphasis.reset();
        self.oversampler.reset();
        self.tape.reset();
        self.erasure.reset();
        self.print.reset();
        self.print_top.reset();
        self.hiss.reset();
        self.head.reset();
        self.bump.reset();
        self.ripple.reset();
        self.low_cut.reset();
        self.deemphasis.reset();
    }

    /// Record `programme`, add the print-through `ghost`, play back.
    fn play(&mut self, programme: f32, ghost: f32, frame: &Frame) -> f32 {
        let boosted = self.emphasis.process(programme);
        let Self {
            oversampler, tape, ..
        } = self;
        let recorded =
            oversampler.process(boosted, |v| tape.process(v, frame.drive, frame.loop_share));
        let kept = self.erasure.process(recorded);
        let printed = self.print_top.process(self.print.process(ghost));
        let on_tape = printed.mul_add(frame.ghost, self.hiss.next().mul_add(frame.hiss, kept));
        let read = self.head.process(on_tape);
        let shaped = self
            .low_cut
            .process(self.ripple.process(self.bump.process(read)));
        self.deemphasis.process(shaped)
    }
}

/// A studio reel-to-reel machine. See the module documentation for the
/// model.
#[derive(Debug, Clone)]
pub struct Reel {
    rate: f32,
    knobs: [Smoothed; COUNT],
    ips: Smoothed,
    ghost: Smoothed,
    channels: [Channel; 2],
    flutter: Wobble,
    programme: Tap,
    echo: Tap,
    base: f32,
    output_gain: Decibels,
    drive_gain: Decibels,
    until_control: usize,
    frame: Frame,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, DRIVE | MIX | OUTPUT)
}

impl Reel {
    /// A machine at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut machine = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            ips: Smoothed::new(15.0),
            ghost: Smoothed::new(0.0),
            channels: [Channel::new(0x0EE1_0001), Channel::new(0x0EE1_0002)],
            flutter: Wobble::new(&PARTS, 0x0EE1_0003),
            programme: Tap::at(0.0),
            echo: Tap::at(0.0),
            base: 0.0,
            output_gain: Decibels::new(),
            drive_gain: Decibels::new(),
            until_control: 0,
            frame: Frame {
                drive: 1.2,
                loop_share: 0.05,
                ghost: 0.0,
                hiss: 0.0,
            },
        };
        machine.prepare(48_000.0);
        machine
    }

    fn target_ips(&self) -> f32 {
        IPS[(self.knobs[SPEED].target() as usize).min(2)]
    }

    /// One turn of the reel at the chosen speed, samples.
    fn wrap_samples(&self) -> f32 {
        WRAP / (self.target_ips() * INCH) * self.rate
    }

    /// Where the programme and the echo taps should sit, samples beyond the
    /// base delay.
    fn tap_targets(&self) -> (f32, f32) {
        let wrap = self.wrap_samples();
        match self.knobs[PRINT].target() as usize {
            1 => (wrap, 0.0),
            2 => (0.0, wrap),
            _ => (0.0, 0.0),
        }
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and move the filters.
    fn control(&mut self) {
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        self.ips.set(self.target_ips());
        let ips = self.ips.step();
        let rate = self.rate;
        let (turnover, boost) = emphasis(ips);
        let bias = self.knobs[BIAS].value() / 50.0 - 1.0;
        let under = (-bias).max(0.0);
        let over = bias.max(0.0);
        self.frame.loop_share = (0.55 * under).mul_add(under, 0.05 * (1.0 - over)) + 0.01;
        let erase_db = 1.5f32.mul_add(under, -5.0 * over * (15.0 / ips).sqrt());
        let head = Head {
            speed: ips * INCH,
            gap: GAP,
            skew: 0.0,
            spacing: SPACING,
        };
        let corner = parts::head_corner(head, rate * 0.45);
        let bump_hz = 3.5 * ips;
        let bump = self.knobs[BUMP].value() / 100.0;
        for channel in &mut self.channels {
            channel.emphasis.high_shelf(turnover * 2.0, boost, rate);
            channel.deemphasis.high_shelf(turnover * 2.0, -boost, rate);
            channel.erasure.high_shelf(10_000.0, erase_db, rate);
            channel.head.lowpass(1, corner, rate);
            channel.bump.peak(bump_hz, 1.2, 3.5 * bump, rate);
            channel.ripple.peak(bump_hz * 2.2, 1.5, -1.2 * bump, rate);
            channel.low_cut.highpass(bump_hz * 0.35, 0.7, rate);
        }
        let hiss = self.knobs[HISS].value() / 100.0;
        self.frame.hiss = HISS_TOP * hiss * hiss.sqrt() * (15.0 / ips).sqrt();
        let (programme, echo) = self.tap_targets();
        self.programme.aim(self.base + programme);
        self.echo.aim(self.base + echo);
        let printing = self.knobs[PRINT].target() >= 1.0;
        let level = db_to_gain(0.3f32.mul_add(self.knobs[GHOST].value(), -50.0));
        self.ghost.set(if printing { level } else { 0.0 });
    }

    /// One stereo sample.
    fn tick(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.until_control == 0 {
            self.control();
            self.until_control = CONTROL;
        }
        self.until_control -= 1;
        let rate = self.rate;
        self.frame.drive = 1.2 * self.drive_gain.gain(self.knobs[DRIVE].step());
        self.frame.ghost = self.ghost.step();
        let mix = self.knobs[MIX].step() / 100.0;
        let output = self.output_gain.gain(self.knobs[OUTPUT].step());
        let ips = self.ips.value();
        let wobble = self.flutter.next(
            MAX_FLUTTER * self.knobs[FLUTTER].value() / 100.0,
            ips / 15.0,
            1.0,
            rate,
        ) * rate;
        let fade = 1.0 / (FADE * rate);
        self.programme.step(fade);
        self.echo.step(fade);
        let frame = self.frame;
        let (programme, echo) = (self.programme, self.echo);
        let mut out = [0.0f32; 2];
        for ((channel, input), wet) in self.channels.iter_mut().zip([left, right]).zip(&mut out) {
            channel.line.push(input);
            let dry = programme.read(&channel.line, Oversampler4::LATENCY);
            let into = programme.read(&channel.line, wobble);
            let ghost = echo.read(&channel.line, wobble);
            let played = channel.play(into, ghost, &frame);
            *wet = parts::lerp(dry, played, mix) * output;
        }
        out.into()
    }
}

impl Default for Reel {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Reel {
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
        self.ips.set_time(0.25, control_rate);
        self.ghost.set_time(0.03, rate);
        let reach = self.flutter.reach(MAX_FLUTTER, IPS[0] / 15.0);
        self.base = (reach + 0.000_5).mul_add(rate, 4.0);
        let longest_wrap = WRAP / (IPS[0] * INCH) * rate;
        let longest =
            2.0f32.mul_add(self.base, longest_wrap) as usize + Oversampler4::LATENCY as usize + 16;
        for channel in &mut self.channels {
            channel.line.resize(longest);
            channel.hiss.band(40.0, rate * 0.45, rate);
            channel.print.bandpass(900.0, 0.7, rate);
            channel.print_top.lowpass(4_000.0, 0.7, rate);
        }
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        self.ips.snap(self.target_ips());
        for channel in &mut self.channels {
            channel.reset();
        }
        self.flutter.reset();
        let (programme, echo) = self.tap_targets();
        self.programme = Tap::at(self.base + programme);
        self.echo = Tap::at(self.base + echo);
        self.until_control = 0;
        self.control();
        self.ghost.snap(self.ghost.target());
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
        self, CONTEXT, built, frequency_deviation, peak, render, rms, silence, sine,
    };
    use super::*;
    use crate::dsp::gain_to_db;

    fn clean(extra: &[(usize, f32)]) -> Box<dyn Effect> {
        let mut params = vec![(FLUTTER, 0.0), (HISS, 0.0)];
        params.extend_from_slice(extra);
        built(&KIND, &params)
    }

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn silence_stays_silent_without_hiss() {
        let input = silence(1.0);
        let mut machine = clean(&[(PRINT, 1.0)]);
        let (left, right) = render(machine.as_mut(), &input, &input);
        assert!(peak(&left) < 1e-6 && peak(&right) < 1e-6);
    }

    #[test]
    fn hiss_is_faint_and_bounded() {
        let input = silence(1.0);
        for speed in 0..3 {
            let mut machine = built(&KIND, &[(SPEED, speed as f32), (HISS, 100.0)]);
            let (left, right) = render(machine.as_mut(), &input, &input);
            let level = rms(&left[4_800..]);
            assert!(level > 1e-4 && level < 0.02, "{speed}: {level}");
            assert!(peak(&right) < 0.15);
        }
        let mut machine = built(&KIND, &[]);
        let (left, _) = render(machine.as_mut(), &input, &input);
        let level = gain_to_db(rms(&left[4_800..]));
        assert!(level < -60.0 && level > -100.0, "default hiss {level}");
    }

    #[test]
    fn defaults_are_subtle() {
        let tone = sine(440.0, 0.25, 1.0);
        let mut machine = built(&KIND, &[]);
        let (left, _) = render(machine.as_mut(), &tone, &tone);
        let change = gain_to_db(rms(&left[9_600..])) - gain_to_db(rms(&tone[9_600..]));
        assert!(change.abs() < 1.5, "{change}");
    }

    #[test]
    fn drive_compresses_loud_signals_only() {
        let loud = sine(200.0, 0.9, 0.5);
        let soft = sine(200.0, 0.01, 0.5);
        let level = |input: &[f32], drive: f32| {
            let mut machine = clean(&[(DRIVE, drive), (BUMP, 0.0)]);
            let (left, _) = render(machine.as_mut(), input, input);
            gain_to_db(rms(&left[9_600..]))
        };
        let squeeze = level(&loud, 18.0) - level(&loud, 0.0);
        assert!(squeeze < -3.0, "{squeeze}");
        let quiet = level(&soft, 18.0) - level(&soft, 0.0);
        assert!(quiet.abs() < 0.5, "{quiet}");
    }

    #[test]
    fn flutter_tracks_its_knob() {
        let tone = sine(1_000.0, 0.25, 3.0);
        let deviation = |flutter: f32| {
            let mut machine = built(&KIND, &[(FLUTTER, flutter), (HISS, 0.0)]);
            let (left, _) = render(machine.as_mut(), &tone, &tone);
            frequency_deviation(&left, 1_000.0, 0.5)
        };
        assert!(deviation(0.0) < 2e-4);
        let (half, full) = (deviation(50.0), deviation(100.0));
        assert!(full > 0.001 && full < MAX_FLUTTER * 1.2, "{full}");
        assert!(half > full * 0.3 && half < full * 0.7, "{half} {full}");
    }

    #[test]
    fn print_through_echoes_one_turn_away() {
        let rate = testkit::RATE;
        let wrap = (WRAP / (15.0 * INCH) * rate) as usize;
        let mut burst = sine(1_000.0, 0.5, 0.05);
        burst.resize(wrap * 3, 0.0);
        for (mode, echo_at) in [(2.0, 1usize), (1.0, 0)] {
            let mut machine = clean(&[(PRINT, mode), (GHOST, 100.0)]);
            let (left, _) = render(machine.as_mut(), &burst, &burst);
            let main = if mode > 1.5 { 0 } else { wrap };
            let window = |at: usize| peak(&left[at..at + 4_800]);
            let programme = window(main);
            let ghost = window(echo_at * wrap);
            let ratio = gain_to_db(ghost / programme);
            assert!(ratio > -30.0 && ratio < -15.0, "mode {mode}: {ratio}");
        }
    }

    #[test]
    fn switching_print_mode_does_not_click() {
        let tone = sine(150.0, 0.3, 1.0);
        let mut machine = clean(&[]);
        let mut out_l = vec![0.0; tone.len()];
        let mut out_r = vec![0.0; tone.len()];
        for (n, ((input, o_l), o_r)) in tone
            .chunks(480)
            .zip(out_l.chunks_mut(480))
            .zip(out_r.chunks_mut(480))
            .enumerate()
        {
            if n == 30 {
                machine.set_param(PRINT, 1.0);
                machine.set_param(SPEED, 0.0);
            }
            machine.process(&CONTEXT, [input, input], [o_l, o_r]);
        }
        let jump = out_l
            .windows(2)
            .skip(4_800)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(jump < 0.02, "{jump}");
    }
}
