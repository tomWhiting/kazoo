//! A record on a turntable.
//!
//! # The model
//!
//! - **Platter and stylus.** The programme is read back through a variable
//!   delay whose read point moves at the platter's speed. A pressing whose
//!   hole is off centre speeds up and slows down once per revolution (0.56,
//!   0.75 or 1.3 Hz at 33, 45 and 78 rpm): `warp`. The motor, belt and
//!   bearing add their own slower and faster wow: `wow`. Both are
//!   [`Wobble`]s that turn with the platter.
//! - **Stop.** `stop` brakes the platter: speed falls at a steady rate, as
//!   friction brakes do, and comes back up faster when released. A
//!   magnetic cartridge's output is proportional to stylus velocity, so
//!   the level falls with the speed and reaches true silence at rest, with
//!   no click. A live input cannot be played late for ever, so once the
//!   platter is back up to speed (or a held slow-down has lagged four
//!   seconds behind) the stylus crossfades back to the present.
//! - **Groove.** Stereo records are cut with the bass summed to mono below
//!   about 150 Hz (the elliptic equaliser a cutting lathe needs) and some
//!   crosstalk: `width` narrows the rest. Near the label the groove moves
//!   slower past the stylus, so short wavelengths are traced badly: `inner`
//!   adds the tracing distortion (mostly second harmonic, oversampled twice)
//!   and treble loss of the inner grooves. `wear` is a worn record and
//!   stylus: more of both, dulled treble, and groove noise. The cartridge's
//!   resonance lifts the top a little on a fresh record.
//!   78s are mono shellac: narrower band, a mid-range honk, and a much
//!   noisier surface.
//! - **Surface.** Dust (`dust`) is a Poisson stream of clicks with a
//!   heavy-tailed size: many small ticks, few loud ones, each landing on one
//!   groove wall or both, ringing the stylus assembly. Scratches (`scratch`)
//!   are defects at a fixed place on the disc, so each one pops once per
//!   revolution until the stylus has ploughed past it. Rumble (`rumble`) is
//!   the motor and bearing's low noise, mostly vertical, so mostly out of
//!   phase between the channels. All of it slows and fades with the
//!   platter.

use std::f32::consts::FRAC_PI_2;

use super::parts::{
    self, Biquad, Butterworth, CONTROL, Decibels, Hiss, Oversampler2, Partial, Wobble,
};
use crate::dsp::{DelayLine, Noise, OnePole, Smoothed};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const RPM: usize = 0;
const WARP: usize = 1;
const WOW: usize = 2;
const DUST: usize = 3;
const SCRATCH: usize = 4;
const RUMBLE: usize = 5;
const WEAR: usize = 6;
const INNER: usize = 7;
const WIDTH: usize = 8;
const STOP: usize = 9;
const MIX: usize = 10;
const OUTPUT: usize = 11;
const COUNT: usize = 12;

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
        name: "rpm",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["33", "45", "78"],
        },
    },
    percent("warp", 20.0),
    percent("wow", 10.0),
    percent("dust", 25.0),
    percent("scratch", 10.0),
    percent("rumble", 15.0),
    percent("wear", 20.0),
    percent("inner", 30.0),
    percent("width", 100.0),
    percent("stop", 0.0),
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

/// The record.
pub static KIND: EffectKind = EffectKind {
    id: "vinyl",
    name: "Vinyl",
    description: "A record on a turntable: off-centre warble, wow, dust crackle, scratches \
                  that pop every revolution, rumble, inner-groove distortion, wear, width, \
                  and a stop knob that brakes the platter to silence.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Vinyl::new())
}

/// Revolutions per minute for each step.
const SPEEDS: [f32; 3] = [100.0 / 3.0, 45.0, 78.0];
/// Peak speed error at full `warp` and full `wow`.
const MAX_WARP: f32 = 0.012;
const MAX_WOW: f32 = 0.004;
/// How fast the platter brakes and spins up, full speed per second.
const BRAKE: f32 = 1.0 / 1.2;
const SPIN_UP: f32 = 1.0 / 0.6;
/// How far behind the present a held slow-down may fall, seconds.
const LAG_LIMIT: f32 = 4.0;
/// How long the stylus takes to crossfade back to the present, seconds.
const RETURN: f32 = 0.03;
/// Most scratches on the disc at once.
const SITES: usize = 3;

static OFF_CENTRE: [Partial; 1] = [Partial {
    hz: 100.0 / 3.0 / 60.0,
    weight: 1.0,
    wander: 0.0,
    steady: 1.0,
}];

static DRIVE: [Partial; 3] = [
    Partial {
        hz: 0.3,
        weight: 0.4,
        wander: 0.3,
        steady: 0.3,
    },
    Partial {
        hz: 3.3,
        weight: 0.35,
        wander: 0.1,
        steady: 0.6,
    },
    Partial {
        hz: 8.0,
        weight: 0.25,
        wander: 0.1,
        steady: 0.5,
    },
];

/// One sample of what the surface adds to each channel: dust ticks and
/// scratch pops (kicks for their resonators) and rumble.
#[derive(Debug, Clone, Copy)]
struct Surface {
    ticks: [f32; 2],
    pops: [f32; 2],
    rumble: [f32; 2],
}

/// A scratch at one place on the disc.
#[derive(Debug, Clone, Copy, Default)]
struct Site {
    angle: f32,
    size: f32,
    turns: u32,
}

/// One channel's groove, cartridge and surface.
#[derive(Debug, Clone)]
struct Groove {
    line: DelayLine,
    trace_split: Biquad,
    oversampler: Oversampler2,
    trace_dc: OnePole,
    lowpass: Butterworth,
    resonance: Biquad,
    honk: Biquad,
    low_cut: Biquad,
    tick: Biquad,
    pop: Biquad,
    surface: Hiss,
    rumble: Butterworth,
    rumble_cut: Biquad,
}

impl Groove {
    fn new(seed: u32) -> Self {
        Self {
            line: DelayLine::default(),
            trace_split: Biquad::new(),
            oversampler: Oversampler2::new(),
            trace_dc: OnePole::default(),
            lowpass: Butterworth::default(),
            resonance: Biquad::new(),
            honk: Biquad::new(),
            low_cut: Biquad::new(),
            tick: Biquad::new(),
            pop: Biquad::new(),
            surface: Hiss::new(seed),
            rumble: Butterworth::default(),
            rumble_cut: Biquad::new(),
        }
    }

    fn reset(&mut self) {
        self.line.clear();
        self.trace_split.reset();
        self.oversampler.reset();
        self.trace_dc.reset();
        self.lowpass.reset();
        self.resonance.reset();
        self.honk.reset();
        self.low_cut.reset();
        self.tick.reset();
        self.pop.reset();
        self.surface.reset();
        self.rumble.reset();
        self.rumble_cut.reset();
    }

    /// Trace the groove: tracing distortion then the pickup's response.
    fn trace(&mut self, x: f32, distortion: f32) -> f32 {
        let treble = self.trace_split.process(x);
        let Self {
            oversampler,
            trace_dc,
            ..
        } = self;
        let traced = oversampler.process(treble, |v| {
            let t = (3.0 * v).tanh();
            let even = t * t;
            // Track and remove the squared term's average so it adds
            // harmonics, not offset.
            let centre = trace_dc.lowpass(even);
            (distortion * 0.3).mul_add(even - centre, v)
        });
        let whole = x - treble + traced;
        let dull = self.lowpass.process(whole);
        self.low_cut
            .process(self.honk.process(self.resonance.process(dull)))
    }
}

/// A record on a turntable. See the module documentation for the model.
#[derive(Debug, Clone)]
pub struct Vinyl {
    rate: f32,
    knobs: [Smoothed; COUNT],
    rpm: Smoothed,
    grooves: [Groove; 2],
    side_cut: Biquad,
    warp: Wobble,
    wow: Wobble,
    platter: f32,
    lags: [f64; 2],
    active: usize,
    fade: f32,
    angle: f32,
    sites: [Site; SITES],
    noise: Noise,
    base: f32,
    output_gain: Decibels,
    until_control: usize,
    shellac: f32,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, MIX | OUTPUT | WIDTH)
}

impl Vinyl {
    /// A record at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut record = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            rpm: Smoothed::new(SPEEDS[0]),
            grooves: [Groove::new(0x0071_0001), Groove::new(0x0071_0002)],
            side_cut: Biquad::new(),
            warp: Wobble::new(&OFF_CENTRE, 0x0071_0003),
            wow: Wobble::new(&DRIVE, 0x0071_0004),
            platter: 1.0,
            lags: [0.0; 2],
            active: 0,
            fade: 1.0,
            angle: 0.0,
            sites: [Site::default(); SITES],
            noise: Noise::new(0x0071_0005),
            base: 0.0,
            output_gain: Decibels::new(),
            until_control: 0,
            shellac: 0.0,
        };
        record.prepare(48_000.0);
        record
    }

    fn target_rpm(&self) -> f32 {
        SPEEDS[(self.knobs[RPM].target() as usize).min(2)]
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and move the filters.
    fn control(&mut self) {
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        self.rpm.set(self.target_rpm());
        let rpm = self.rpm.step();
        // 0 for 33 and 45, 1 for 78, gliding between.
        self.shellac = ((rpm - 45.0) / 33.0).clamp(0.0, 1.0);
        let rate = self.rate;
        let wear = self.knobs[WEAR].value() / 100.0;
        let inner = self.knobs[INNER].value() / 100.0;
        let vinyl_top = parts::lerp(20_000.0, 12_000.0, inner) * (-0.5f32).mul_add(wear, 1.0);
        let top = parts::lerp(vinyl_top, 6_000.0, self.shellac).min(rate * 0.45);
        let shine = 2.0 * (1.0 - wear) * (1.0 - self.shellac);
        for groove in &mut self.grooves {
            groove.lowpass.lowpass(1, top, rate);
            groove
                .resonance
                .peak(14_000.0f32.min(rate * 0.4), 2.0, shine, rate);
            groove.honk.peak(2_500.0, 0.9, 3.0 * self.shellac, rate);
            groove
                .low_cut
                .highpass(parts::lerp(18.0, 80.0, self.shellac), 0.7, rate);
        }
    }

    /// Move the platter one sample toward its target speed.
    fn spin(&mut self) {
        // The platter's own inertia is the glide.
        let target = 1.0 - self.knobs[STOP].target() / 100.0;
        let rate = self.rate;
        if self.platter > target {
            self.platter = (self.platter - BRAKE / rate).max(target);
        } else {
            self.platter = (self.platter + SPIN_UP / rate).min(target);
        }
    }

    /// Advance the read heads and bring the stylus back to the present
    /// when it can.
    fn follow(&mut self) {
        let behind = f64::from(1.0 - self.platter);
        for lag in &mut self.lags {
            *lag += behind;
        }
        let limit = f64::from(LAG_LIMIT * self.rate);
        if self.platter < 1e-4 {
            // At rest, silent: rejoin the present at once.
            self.lags = [0.0; 2];
            self.fade = 1.0;
        } else if self.fade >= 1.0
            && ((self.platter > 0.999 && self.lags[self.active] > 1.0)
                || self.lags[self.active] > limit)
        {
            self.active = 1 - self.active;
            self.lags[self.active] = 0.0;
            self.fade = 0.0;
        }
        self.fade = (self.fade + 1.0 / (RETURN * self.rate)).min(1.0);
    }

    /// Read the programme at `offset` samples of wobble for one groove.
    fn read(&self, line: &DelayLine, offset: f32) -> f32 {
        let at = |lag: f64| line.read(self.base + offset + lag as f32);
        let now = at(self.lags[self.active]);
        if self.fade >= 1.0 {
            now
        } else {
            let angle = self.fade * FRAC_PI_2;
            at(self.lags[1 - self.active]).mul_add(angle.cos(), now * angle.sin())
        }
    }

    /// One sample of dust, scratches and rumble for the two channels.
    fn surface(&mut self, turned: bool) -> Surface {
        let platter = self.platter;
        let dust = self.knobs[DUST].value() / 100.0;
        let wear = self.knobs[WEAR].value() / 100.0;
        let scratch = self.knobs[SCRATCH].value() / 100.0;
        let rate = self.rate;
        let mut ticks = [0.0f32; 2];
        let per_second =
            (300.0 * dust).mul_add(dust, 20.0 * wear) * 3.0f32.mul_add(self.shellac, 1.0);
        if parts::chance(&mut self.noise, per_second * platter / rate) {
            // The resonator rings about a fifth of its kick back out.
            let size = parts::unit(&mut self.noise).powi(4) * platter;
            let signed = if parts::chance(&mut self.noise, 0.5) {
                size
            } else {
                -size
            };
            let wall = parts::unit(&mut self.noise);
            ticks = if wall < 0.3 {
                [signed, 0.0]
            } else if wall < 0.6 {
                [0.0, signed]
            } else if wall < 0.8 {
                [signed, signed]
            } else {
                [signed, -signed]
            };
        }
        let mut pops = [0.0f32; 2];
        if turned {
            for site in &mut self.sites {
                if site.turns == 0 && parts::chance(&mut self.noise, 0.5 * scratch) {
                    site.angle = parts::unit(&mut self.noise);
                    site.size = parts::unit(&mut self.noise).mul_add(0.7, 0.3);
                    site.turns = 4 + (parts::unit(&mut self.noise) * 16.0) as u32;
                }
            }
        }
        let step = self.rpm.value() / 60.0 * platter / rate;
        for site in &mut self.sites {
            if site.turns > 0 {
                let passed = if self.angle >= site.angle {
                    self.angle - step < site.angle
                } else {
                    turned && self.angle + 1.0 - step < site.angle
                };
                if passed {
                    // The resonator rings about a thirteenth of its kick.
                    let size = site.size * 4.0 * scratch * platter;
                    pops = [size, size * 0.8];
                    site.turns -= 1;
                }
            }
        }
        let rumble = self.knobs[RUMBLE].value() / 100.0 * 0.03 * platter;
        let vertical = self.noise.sample() * rumble;
        let lateral = self.noise.sample() * rumble * 0.5;
        Surface {
            ticks,
            pops,
            rumble: [vertical + lateral, lateral - vertical],
        }
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
        let width = self.knobs[WIDTH].step() / 100.0 * (1.0 - self.shellac);
        self.spin();
        self.follow();
        let platter = self.platter;
        let rpm = self.rpm.value();
        let turn = rpm / SPEEDS[0];
        let before = self.angle;
        self.angle = (self.angle + rpm / 60.0 * platter / rate).rem_euclid(1.0);
        let turned = self.angle < before;

        let warp = self.warp.next(
            MAX_WARP * self.knobs[WARP].value() / 100.0,
            turn,
            platter,
            rate,
        );
        let wow = self.wow.next(
            MAX_WOW * self.knobs[WOW].value() / 100.0,
            1.0,
            platter,
            rate,
        );
        let offset = (warp + wow) * rate;
        let surface = self.surface(turned);

        let wear = self.knobs[WEAR].value() / 100.0;
        let inner = self.knobs[INNER].value() / 100.0;
        let distortion = inner.mul_add(0.6, 0.4 * wear);
        let surface_level = wear * 0.004 * platter * 4.0f32.mul_add(self.shellac, 1.0);

        for (groove, input) in self.grooves.iter_mut().zip([left, right]) {
            groove.line.push(input);
        }
        let played = [0, 1].map(|c| self.read(&self.grooves[c].line, offset));
        // Cut to disc: mono bass, and the width knob on what is left.
        let mid = 0.5 * (played[0] + played[1]);
        let side = self.side_cut.process(0.5 * (played[0] - played[1])) * width;
        let cut = [mid + side, mid - side];
        let dry_at = self.base + Oversampler2::LATENCY;
        let mut out = [0.0f32; 2];
        for (c, ((groove, wet), x)) in self.grooves.iter_mut().zip(&mut out).zip(cut).enumerate() {
            let traced = groove.trace(x, distortion) * platter;
            let tick = groove.tick.process(surface.ticks[c]);
            let pop = groove.pop.process(surface.pops[c]);
            let noise = groove.surface.next() * surface_level;
            let low = groove
                .rumble_cut
                .process(groove.rumble.process(surface.rumble[c]));
            let record = low.mul_add(4.0, traced + tick + pop + noise);
            *wet = parts::lerp(groove.line.read(dry_at), record, mix) * output;
        }
        out.into()
    }
}

impl Default for Vinyl {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Vinyl {
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
        self.rpm.set_time(0.3, control_rate);
        let reach = self.warp.reach(MAX_WARP, 1.0) + self.wow.reach(MAX_WOW, 1.0);
        self.base = (reach + 0.000_5).mul_add(rate, 4.0);
        let longest = 2.0f32.mul_add(self.base, LAG_LIMIT * rate) as usize + 64;
        self.side_cut.highpass(150.0, 0.7, rate);
        for groove in &mut self.grooves {
            groove.line.resize(longest);
            groove.trace_split.highpass(1_500.0, 0.7, rate);
            // The oversampled tracer runs at twice the rate.
            groove.trace_dc.set_cutoff(5.0, rate * 2.0);
            groove.tick.bandpass(3_000.0, 0.8, rate);
            groove.pop.bandpass(900.0, 0.7, rate);
            groove
                .surface
                .band(500.0, 12_000.0f32.min(rate * 0.45), rate);
            groove.rumble.lowpass(1, 30.0, rate);
            groove.rumble_cut.highpass(8.0, 0.7, rate);
        }
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        self.rpm.snap(self.target_rpm());
        self.side_cut.reset();
        for groove in &mut self.grooves {
            groove.reset();
        }
        self.warp.reset();
        self.wow.reset();
        self.platter = 1.0 - self.knobs[STOP].value() / 100.0;
        self.lags = [0.0; 2];
        self.active = 0;
        self.fade = 1.0;
        self.angle = 0.0;
        self.sites = [Site::default(); SITES];
        self.noise = Noise::new(0x0071_0005);
        self.until_control = 0;
        self.shellac = 0.0;
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

    const QUIET: [(usize, f32); 5] = [
        (DUST, 0.0),
        (SCRATCH, 0.0),
        (RUMBLE, 0.0),
        (WEAR, 0.0),
        (INNER, 0.0),
    ];

    fn record(extra: &[(usize, f32)]) -> Box<dyn Effect> {
        let mut params = QUIET.to_vec();
        params.extend_from_slice(extra);
        built(&KIND, &params)
    }

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn a_clean_record_of_silence_is_silent() {
        let input = silence(1.0);
        let mut disc = record(&[(RPM, 2.0)]);
        let (left, right) = render(disc.as_mut(), &input, &input);
        assert!(peak(&left) < 1e-6 && peak(&right) < 1e-6);
    }

    #[test]
    fn crackle_and_pops_are_bounded() {
        let input = silence(4.0);
        for rpm in [0.0, 2.0] {
            let mut disc = built(
                &KIND,
                &[
                    (RPM, rpm),
                    (DUST, 100.0),
                    (SCRATCH, 100.0),
                    (RUMBLE, 100.0),
                    (WEAR, 100.0),
                ],
            );
            let (left, right) = render(disc.as_mut(), &input, &input);
            assert!(rms(&left) > 1e-3, "{rpm}: crackle should be heard");
            assert!(
                rms(&left) < 0.05 && peak(&left) < 0.6 && peak(&right) < 0.6,
                "{rpm}"
            );
        }
        let mut default = built(&KIND, &[]);
        let (left, _) = render(default.as_mut(), &input, &input);
        assert!(peak(&left) < 0.3 && rms(&left) < 0.01, "{}", rms(&left));
    }

    #[test]
    fn scratches_pop_once_per_revolution() {
        let input = silence(8.0);
        let mut disc = record(&[(SCRATCH, 100.0)]);
        let (left, _) = render(disc.as_mut(), &input, &input);
        let rate = testkit::RATE;
        let turn = (60.0 / SPEEDS[0] * rate) as usize;
        // Find the loud moments and check the gaps between them are whole
        // numbers of turns (from one site or several).
        let loud: Vec<usize> = left
            .iter()
            .enumerate()
            .filter(|(_, x)| x.abs() > 0.02)
            .map(|(n, _)| n)
            .collect();
        assert!(!loud.is_empty());
        let first = loud[0];
        let again = loud.iter().any(|&n| n.abs_diff(first + turn) < 200);
        assert!(again, "a scratch should come round again");
    }

    #[test]
    fn warp_tracks_its_knob() {
        let tone = sine(1_000.0, 0.25, 4.0);
        let deviation = |warp: f32| {
            let mut disc = record(&[(WARP, warp), (WOW, 0.0)]);
            let (left, _) = render(disc.as_mut(), &tone, &tone);
            frequency_deviation(&left, 1_000.0, 0.5)
        };
        assert!(deviation(0.0) < 3e-4);
        let (half, full) = (deviation(50.0), deviation(100.0));
        assert!(full > MAX_WARP * 0.9 && full < MAX_WARP * 1.1, "{full}");
        assert!(half > full * 0.4 && half < full * 0.6, "{half} {full}");
    }

    #[test]
    fn stop_glides_to_silence_without_clicks() {
        let tone = sine(400.0, 0.5, 4.0);
        let mut disc = record(&[(WARP, 0.0), (WOW, 0.0)]);
        let mut out_l = vec![0.0; tone.len()];
        let mut out_r = vec![0.0; tone.len()];
        for (n, ((input, o_l), o_r)) in tone
            .chunks(480)
            .zip(out_l.chunks_mut(480))
            .zip(out_r.chunks_mut(480))
            .enumerate()
        {
            if n == 50 {
                disc.set_param(STOP, 100.0);
            }
            disc.process(&CONTEXT, [input, input], [o_l, o_r]);
        }
        let jump = out_l
            .windows(2)
            .skip(4_800)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        // 400 Hz at 0.5 moves at most about 0.026 per sample; slowing only
        // lowers that.
        assert!(jump < 0.04, "{jump}");
        let start = 24_000 + (1.3 * testkit::RATE) as usize;
        assert!(peak(&out_l[start..]) < 1e-6, "{}", peak(&out_l[start..]));
        // Halfway down the pitch has fallen.
        let slow = frequency_deviation(&out_l[24_000..48_000], 400.0, 0.1);
        assert!(slow > 0.2, "{slow}");
    }

    #[test]
    fn letting_go_brings_the_music_back_on_time() {
        let tone = sine(300.0, 0.4, 5.0);
        let mut disc = record(&[(WARP, 0.0), (WOW, 0.0), (STOP, 60.0)]);
        let (early, _) = render(disc.as_mut(), &tone[..96_000], &tone[..96_000]);
        assert!(rms(&early[48_000..]) > 0.05);
        disc.set_param(STOP, 0.0);
        let (late, _) = render(disc.as_mut(), &tone[96_000..], &tone[96_000..]);
        // Back up to speed and back in the present: in tune again.
        assert!(frequency_deviation(&late[48_000..], 300.0, 0.0) < 1e-3);
    }

    #[test]
    fn seventy_eights_are_mono() {
        let left = sine(700.0, 0.3, 1.0);
        let right = silence(1.0);
        let mut disc = record(&[(RPM, 2.0)]);
        let (l, r) = render(disc.as_mut(), &left, &right);
        let diff: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
        assert!(rms(&diff[24_000..]) < 1e-3 * rms(&l[24_000..]).max(1e-3));
    }
}
