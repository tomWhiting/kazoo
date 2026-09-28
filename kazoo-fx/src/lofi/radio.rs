//! Radio, telephone, walkie-talkie and megaphone.
//!
//! # The model
//!
//! Each is the real signal chain, simulated, rather than a filter preset.
//! All four are mono devices: the two inputs are summed and the device's
//! sound comes out of both sides.
//!
//! - **AM radio.** Simulated at complex baseband. The transmitter band-
//!   limits and limits the programme and amplitude-modulates a carrier
//!   (`drive` is the modulation depth; past 100% the envelope folds, the
//!   harsh overmodulation of a pirate station). The path fades slowly as
//!   the ionosphere moves (`drift`), atmospheric static crackles in
//!   (`noise`), and a neighbouring station 5.2 kHz away chatters. The
//!   receiver's IF filter passes ±4.5 kHz around where it is tuned: off
//!   station (`tune`), one sideband is cut, the envelope detector
//!   distorts, and the neighbour's carrier beats against ours in a whistle.
//!   The AGC levels the carrier, so fades pull the static up with them,
//!   and a small speaker gives up below about 150 Hz.
//! - **Telephone.** A carbon microphone (compressive, lopsided), the
//!   300–3400 Hz line, an 8 kHz, 8-bit µ-law codec (sampled and held, so
//!   its steps and images are real), reconstruction, and line hum, hiss and
//!   clicks. `drift` is the line's level wandering; `tune` slides the band.
//! - **Walkie-talkie.** Narrow-band FM at complex baseband: a clipped,
//!   pre-emphasised mic, 2.5 kHz deviation, a noisy channel, an IF filter
//!   that `tune` and `drift` pull off centre (distortion, then noise), a
//!   discriminator, de-emphasis and the voice band. The transmitter keys up
//!   on voice; when it drops, the receiver hears raw noise until its
//!   squelch closes: the "kssh" at the end of every over (its loudness
//!   follows `noise`).
//! - **Megaphone.** A voice microphone, a small amplifier clipped hard
//!   (`drive`, oversampled twice), then a horn's narrow band and resonances
//!   (`tune` shifts them, `drift` sways them as it is waved about).
//!
//! Changing `model` crossfades between them.

use std::f32::consts::TAU;

use super::parts::{self, Biquad, Butterworth, CONTROL, Decibels, Drift, Hiss, Oversampler2};
use crate::dsp::{DelayLine, Noise, Smoothed};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const MODEL: usize = 0;
const TUNE: usize = 1;
const DRIFT: usize = 2;
const DRIVE: usize = 3;
const NOISE: usize = 4;
const MIX: usize = 5;
const OUTPUT: usize = 6;
const COUNT: usize = 7;

/// The devices in `model` order; the fourth and last is the megaphone.
const MODELS: usize = 4;
const AM: usize = 0;
const PHONE: usize = 1;
const WALKIE: usize = 2;

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
        name: "model",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["am radio", "telephone", "walkie-talkie", "megaphone"],
        },
    },
    ParamSpec {
        name: "tune",
        min: -100.0,
        max: 100.0,
        default: 0.0,
        unit: "%",
        curve: Curve::Linear,
    },
    percent("drift", 20.0),
    percent("drive", 30.0),
    percent("noise", 20.0),
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

/// The radio.
pub static KIND: EffectKind = EffectKind {
    id: "radio",
    name: "Radio",
    description: "AM radio, telephone, walkie-talkie or megaphone, each its real signal \
                  chain: modulation, band limits, codecs, distortion, interference, tuning \
                  drift and squelch.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Radio::new())
}

/// What every device reads each sample.
#[derive(Debug, Clone, Copy)]
struct Controls {
    /// Off station, -1 up to 1.
    tune: f32,
    /// How far things wander, 0 up to 1, and where they have wandered to
    /// now (a slow value and a slower one, each -1 up to 1).
    drift: f32,
    wander: f32,
    fade: f32,
    /// 0 up to 1.
    drive: f32,
    noise: f32,
}

/// A complex sample.
#[derive(Debug, Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    fn turned(size: f32, angle: f32) -> Self {
        let (im, re) = angle.sin_cos();
        Self {
            re: re * size,
            im: im * size,
        }
    }

    fn size(self) -> f32 {
        self.re.hypot(self.im)
    }
}

/// A phase accumulator in turns, kept in double precision so a steady
/// offset stays steady for hours.
#[derive(Debug, Clone, Copy, Default)]
struct Turns(f64);

impl Turns {
    fn advance(&mut self, hz: f32, rate: f32) -> f32 {
        let step = f64::from(hz) / f64::from(rate);
        if step.is_finite() {
            self.0 = (self.0 + step).rem_euclid(1.0);
        }
        (self.0 * std::f64::consts::TAU) as f32
    }
}

/// A one-pole smoother on a plain value, with a settable state.
#[derive(Debug, Clone, Copy, Default)]
struct Follow {
    value: f32,
}

impl Follow {
    fn next(&mut self, input: f32, coeff: f32) -> f32 {
        self.value = (input - self.value).mul_add(coeff, self.value);
        crate::dsp::flush(&mut self.value);
        self.value
    }
}

/// One-pole coefficient for a time constant of `seconds`.
fn coeff(seconds: f32, rate: f32) -> f32 {
    1.0 - (-1.0 / (seconds * rate).max(1.0)).exp()
}

/// The AM broadcast chain.
#[derive(Debug, Clone)]
struct Am {
    rate: f32,
    tx_low: Biquad,
    tx_band: Butterworth,
    carrier: Turns,
    neighbour: Turns,
    chatter: Hiss,
    chatter_gate: Drift,
    if_re: Butterworth,
    if_im: Butterworth,
    agc: Follow,
    audio_band: Butterworth,
    audio_low: Biquad,
    noise: Noise,
}

impl Am {
    fn new() -> Self {
        Self {
            rate: 48_000.0,
            tx_low: Biquad::new(),
            tx_band: Butterworth::default(),
            carrier: Turns::default(),
            neighbour: Turns::default(),
            chatter: Hiss::new(0x0A01_0001),
            chatter_gate: Drift::new(),
            if_re: Butterworth::default(),
            if_im: Butterworth::default(),
            agc: Follow::default(),
            audio_band: Butterworth::default(),
            audio_low: Biquad::new(),
            noise: Noise::new(0x0A01_0002),
        }
    }

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        self.tx_low.highpass(80.0, 0.7, rate);
        self.tx_band.lowpass(2, 4_500.0, rate);
        self.chatter.band(300.0, 2_000.0, rate);
        self.if_re.lowpass(2, 4_500.0, rate);
        self.if_im.lowpass(2, 4_500.0, rate);
        self.audio_band.lowpass(2, 4_500.0, rate);
        // A transistor radio's small speaker gives up below about 150 Hz.
        self.audio_low.highpass(150.0, 0.7, rate);
    }

    /// The carrier's level at `fade`.
    fn carrier_level(controls: &Controls) -> f32 {
        (0.3 * controls.drift).mul_add(-controls.fade.mul_add(1.0, 1.0), 1.0)
    }

    fn reset(&mut self, controls: &Controls) {
        self.tx_low.reset();
        self.tx_band.reset();
        self.carrier = Turns::default();
        self.neighbour = Turns::default();
        self.chatter.reset();
        self.chatter_gate = Drift::new();
        self.if_re.reset();
        self.if_im.reset();
        self.audio_band.reset();
        self.audio_low.reset();
        self.noise = Noise::new(0x0A01_0002);
        // Start tuned in: the carrier already through the IF filter and
        // the AGC already locked to it.
        let level = Self::carrier_level(controls);
        self.if_re.settle(level);
        self.agc = Follow { value: level };
    }

    fn tick(&mut self, x: f32, controls: &Controls) -> f32 {
        let rate = self.rate;
        let depth = 0.8f32.mul_add(controls.drive, 0.6);
        let limited = (1.5 * self.tx_band.process(self.tx_low.process(x))).tanh() / 1.5;
        let level = Self::carrier_level(controls);
        let envelope = depth.mul_add(limited, 1.0) * level;
        let offset = controls
            .tune
            .mul_add(-3_000.0, -400.0 * controls.drift * controls.wander);
        let ours = Complex::turned(envelope, self.carrier.advance(offset, rate));
        let near = 0.25f32.mul_add(controls.tune.abs(), 0.05 * controls.noise);
        let gate = self
            .chatter_gate
            .next(&mut self.noise, 0.6, rate)
            .mul_add(0.5, 0.5);
        let talk = (0.5 * gate).mul_add(self.chatter.next() * 0.3, 1.0) * near;
        let theirs = Complex::turned(talk, self.neighbour.advance(offset + 5_200.0, rate));
        let hiss = 0.03 * controls.noise;
        let crack = if parts::chance(&mut self.noise, 30.0 * controls.noise / rate) {
            parts::unit(&mut self.noise).powi(6) * 2.0 * controls.noise
        } else {
            0.0
        };
        let re = self
            .noise
            .sample()
            .mul_add(hiss, ours.re + theirs.re + crack);
        let im = self.noise.sample().mul_add(hiss, ours.im + theirs.im);
        let received = Complex {
            re: self.if_re.process(re),
            im: self.if_im.process(im),
        };
        let detected = received.size();
        let carrier = self.agc.next(detected, coeff(0.08, rate)).max(0.1);
        let audio = detected / carrier - 1.0;
        let heard = self.audio_low.process(self.audio_band.process(audio)) / depth;
        heard.tanh()
    }
}

/// The telephone line.
#[derive(Debug, Clone)]
struct Phone {
    rate: f32,
    low_cut: Butterworth,
    high_cut: Butterworth,
    clock: f64,
    held: f32,
    last: f32,
    rebuild: Butterworth,
    hum: Turns,
    hiss: Hiss,
    noise: Noise,
    level: Drift,
}

impl Phone {
    fn new() -> Self {
        Self {
            rate: 48_000.0,
            low_cut: Butterworth::default(),
            high_cut: Butterworth::default(),
            clock: 0.0,
            held: 0.0,
            last: 0.0,
            rebuild: Butterworth::default(),
            hum: Turns::default(),
            hiss: Hiss::new(0x0A02_0001),
            noise: Noise::new(0x0A02_0002),
            level: Drift::new(),
        }
    }

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        self.hiss.band(300.0, 3_400.0, rate);
    }

    fn control(&mut self, controls: &Controls) {
        let rate = self.rate;
        let shift = controls.tune.exp2();
        self.low_cut.highpass(2, 300.0 * shift, rate);
        self.high_cut
            .lowpass(4, (3_400.0 * shift).min(rate * 0.45), rate);
        self.rebuild.lowpass(4, 3_600.0f32.min(rate * 0.45), rate);
    }

    fn reset(&mut self) {
        self.low_cut.reset();
        self.high_cut.reset();
        self.clock = 0.0;
        self.held = 0.0;
        self.last = 0.0;
        self.rebuild.reset();
        self.hum = Turns::default();
        self.hiss.reset();
        self.noise = Noise::new(0x0A02_0002);
        self.level = Drift::new();
    }

    /// The µ-law codec: compress, keep 8 bits, expand.
    fn codec(x: f32) -> f32 {
        const MU: f32 = 255.0;
        let x = x.clamp(-1.0, 1.0);
        let squeezed = (MU * x.abs()).ln_1p() / MU.ln_1p();
        let kept = (squeezed * 127.0).round() / 127.0;
        (((MU.ln_1p() * kept).exp_m1()) / MU).copysign(x)
    }

    fn tick(&mut self, x: f32, controls: &Controls) -> f32 {
        let rate = self.rate;
        let heat = 3.0f32.mul_add(controls.drive, 1.0);
        let carbon = (heat * 0.15f32.mul_add(x * x, x)).tanh() / heat.sqrt();
        let line = self.high_cut.process(self.low_cut.process(carbon));
        let wander = self.level.next(&mut self.noise, 1.5, rate);
        let sent = line * (0.4 * controls.drift).mul_add(wander, 1.0) * 1.5;
        // The codec samples at 8 kHz: take a sample, interpolated to the
        // exact clock edge, whenever the clock ticks over.
        let step = 8_000.0 / f64::from(rate);
        self.clock += step;
        if self.clock >= 1.0 {
            self.clock -= 1.0;
            let back = (self.clock / step) as f32;
            self.held = Self::codec(parts::lerp(sent, self.last, back));
        }
        self.last = sent;
        let hum_phase = self.hum.advance(50.0, rate);
        let hum = 0.5f32.mul_add((3.0 * hum_phase).sin(), hum_phase.sin()) * 0.01 * controls.noise;
        let click = if parts::chance(&mut self.noise, 2.0 * controls.noise / rate) {
            self.noise.sample() * 0.3 * controls.noise
        } else {
            0.0
        };
        let hiss = self.hiss.next() * 0.006 * controls.noise;
        (self.rebuild.process(self.held) + hum + hiss + click).tanh()
    }
}

/// The walkie-talkie link.
#[derive(Debug, Clone)]
struct Walkie {
    rate: f32,
    mic_low: Butterworth,
    mic_high: Butterworth,
    emphasis: Biquad,
    voice: Follow,
    hang: u32,
    key: Follow,
    phase: Turns,
    detune: Turns,
    if_re: Butterworth,
    if_im: Butterworth,
    last: Complex,
    strength: Follow,
    open: u32,
    gate: Follow,
    deemphasis: Biquad,
    out_low: Butterworth,
    out_high: Butterworth,
    noise: Noise,
}

/// Peak frequency deviation, Hz.
const DEVIATION: f32 = 2_500.0;

impl Walkie {
    fn new() -> Self {
        Self {
            rate: 48_000.0,
            mic_low: Butterworth::default(),
            mic_high: Butterworth::default(),
            emphasis: Biquad::new(),
            voice: Follow::default(),
            hang: 0,
            key: Follow::default(),
            phase: Turns::default(),
            detune: Turns::default(),
            if_re: Butterworth::default(),
            if_im: Butterworth::default(),
            last: Complex::default(),
            strength: Follow::default(),
            open: 0,
            gate: Follow::default(),
            deemphasis: Biquad::new(),
            out_low: Butterworth::default(),
            out_high: Butterworth::default(),
            noise: Noise::new(0x0A03_0001),
        }
    }

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        self.mic_low.highpass(2, 300.0, rate);
        self.mic_high.lowpass(2, 3_000.0, rate);
        self.emphasis.high_shelf(2_000.0, 6.0, rate);
        self.deemphasis.high_shelf(2_000.0, -6.0, rate);
        self.if_re.lowpass(2, 6_000.0f32.min(rate * 0.45), rate);
        self.if_im.lowpass(2, 6_000.0f32.min(rate * 0.45), rate);
        self.out_low.highpass(2, 300.0, rate);
        self.out_high.lowpass(2, 3_000.0, rate);
    }

    fn reset(&mut self) {
        let rate = self.rate;
        *self = Self::new();
        self.prepare(rate);
    }

    /// The transmitter: returns the complex baseband it sends.
    fn transmit(&mut self, x: f32, controls: &Controls) -> Complex {
        let rate = self.rate;
        let voice = self.voice.next(x.abs(), coeff(0.005, rate));
        if voice > 0.003 {
            self.hang = (0.3 * rate) as u32;
        } else {
            self.hang = self.hang.saturating_sub(1);
        }
        let keyed = if self.hang > 0 { 1.0 } else { 0.0 };
        let carrier = self.key.next(keyed, coeff(0.005, rate));
        let heat = 9.0f32.mul_add(controls.drive, 1.0);
        let shaped = self
            .emphasis
            .process(self.mic_high.process(self.mic_low.process(x)));
        let clipped = (heat * shaped).tanh() * 0.8;
        let offset = controls
            .tune
            .mul_add(2_000.0, 600.0 * controls.drift * controls.wander);
        let swing = self.phase.advance(DEVIATION * clipped, rate);
        let detune = self.detune.advance(offset, rate);
        Complex::turned(carrier, swing + detune)
    }

    fn tick(&mut self, x: f32, controls: &Controls) -> f32 {
        let rate = self.rate;
        let sent = self.transmit(x, controls);
        let hiss =
            0.15 * controls.noise * (0.5 * controls.drift).mul_add(controls.fade.max(0.0), 1.0);
        let received = Complex {
            re: self
                .if_re
                .process(self.noise.sample().mul_add(hiss, sent.re)),
            im: self
                .if_im
                .process(self.noise.sample().mul_add(hiss, sent.im)),
        };
        // The discriminator: the angle turned since the last sample.
        let turn_re = received
            .re
            .mul_add(self.last.re, received.im * self.last.im);
        let turn_im = received
            .im
            .mul_add(self.last.re, -received.re * self.last.im);
        self.last = received;
        let angle = if turn_re.hypot(turn_im) > 1e-12 {
            turn_im.atan2(turn_re)
        } else {
            0.0
        };
        let heard = (angle * rate / TAU / DEVIATION).clamp(-4.0, 4.0);
        let strength = self.strength.next(received.size(), coeff(0.005, rate));
        let present = strength > 0.5;
        if present {
            self.open = (0.12 * rate) as u32;
        } else {
            self.open = self.open.saturating_sub(1);
        }
        let open = if self.open > 0 { 1.0 } else { 0.0 };
        let gate = self.gate.next(open, coeff(0.003, rate));
        // Without a carrier the discriminator hears only the channel's
        // noise, as loud as the channel is noisy.
        let loudness = if present { 1.0 } else { controls.noise };
        let voice = self.deemphasis.process(heard * gate * loudness);
        self.out_high.process(self.out_low.process(voice)).tanh()
    }
}

/// The megaphone.
#[derive(Debug, Clone)]
struct Megaphone {
    rate: f32,
    mic: Butterworth,
    oversampler: Oversampler2,
    low_cut: Butterworth,
    high_cut: Butterworth,
    first: Biquad,
    second: Biquad,
    hiss: Hiss,
}

impl Megaphone {
    fn new() -> Self {
        Self {
            rate: 48_000.0,
            mic: Butterworth::default(),
            oversampler: Oversampler2::new(),
            low_cut: Butterworth::default(),
            high_cut: Butterworth::default(),
            first: Biquad::new(),
            second: Biquad::new(),
            hiss: Hiss::new(0x0A04_0001),
        }
    }

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        self.mic.highpass(1, 300.0, rate);
        self.hiss.band(500.0, 4_500.0, rate);
    }

    fn control(&mut self, controls: &Controls) {
        let rate = self.rate;
        let shift =
            (0.7 * controls.tune).exp2() * (0.05 * controls.drift).mul_add(controls.wander, 1.0);
        self.low_cut.highpass(2, 500.0 * shift, rate);
        self.high_cut
            .lowpass(2, (4_500.0 * shift).min(rate * 0.45), rate);
        self.first
            .peak((1_100.0 * shift).min(rate * 0.45), 3.0, 6.0, rate);
        self.second
            .peak((2_600.0 * shift).min(rate * 0.45), 4.0, 5.0, rate);
    }

    fn reset(&mut self) {
        self.mic.reset();
        self.oversampler.reset();
        self.low_cut.reset();
        self.high_cut.reset();
        self.first.reset();
        self.second.reset();
        self.hiss.reset();
    }

    fn tick(&mut self, x: f32, controls: &Controls) -> f32 {
        let heat = 20.0f32.mul_add(controls.drive, 2.0);
        let hiss = self.hiss.next() * 0.01 * controls.noise;
        let voice = self.mic.process(x);
        let driven = self.oversampler.process(voice + hiss, |v| {
            let cone = (-0.1 * v * v).mul_add(v, v);
            (heat * cone).tanh() / heat.sqrt()
        });
        let horn = self.second.process(self.first.process(driven));
        (self.high_cut.process(self.low_cut.process(horn)) * 0.7).tanh()
    }
}

/// The radio. See the module documentation for the model.
#[derive(Debug, Clone)]
pub struct Radio {
    rate: f32,
    knobs: [Smoothed; COUNT],
    weights: [Smoothed; MODELS],
    am: Am,
    phone: Phone,
    walkie: Walkie,
    megaphone: Megaphone,
    dry: [DelayLine; 2],
    wander: Drift,
    fade: Drift,
    noise: Noise,
    controls: Controls,
    output_gain: Decibels,
    until_control: usize,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, MIX | OUTPUT)
}

impl Radio {
    /// A radio at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut radio = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            weights: [Smoothed::new(0.0); MODELS],
            am: Am::new(),
            phone: Phone::new(),
            walkie: Walkie::new(),
            megaphone: Megaphone::new(),
            dry: [DelayLine::default(), DelayLine::default()],
            wander: Drift::new(),
            fade: Drift::new(),
            noise: Noise::new(0x0A00_0001),
            controls: Controls {
                tune: 0.0,
                drift: 0.0,
                wander: 0.0,
                fade: 0.0,
                drive: 0.0,
                noise: 0.0,
            },
            output_gain: Decibels::new(),
            until_control: 0,
        };
        radio.prepare(48_000.0);
        radio
    }

    fn aim(&mut self) {
        let chosen = (self.knobs[MODEL].target() as usize).min(MODELS - 1);
        for (n, weight) in self.weights.iter_mut().enumerate() {
            weight.set(if n == chosen { 1.0 } else { 0.0 });
        }
    }

    /// Read the knobs into the controls every device shares.
    fn read_controls(&mut self) {
        self.controls.tune = self.knobs[TUNE].value() / 100.0;
        self.controls.drift = self.knobs[DRIFT].value() / 100.0;
        self.controls.drive = self.knobs[DRIVE].value() / 100.0;
        self.controls.noise = self.knobs[NOISE].value() / 100.0;
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and move the filters.
    fn control(&mut self) {
        self.aim();
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        self.read_controls();
        self.phone.control(&self.controls);
        self.megaphone.control(&self.controls);
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
        self.controls.wander = self.wander.next(&mut self.noise, 2.5, rate);
        self.controls.fade = self.fade.next(&mut self.noise, 4.0, rate);
        let controls = self.controls;
        let mono = 0.5 * (left + right);
        let mut wet = 0.0;
        let mut latency = 0.0;
        for (n, weight) in self.weights.iter_mut().enumerate() {
            let share = weight.step();
            if share > 1e-6 {
                let heard = match n {
                    AM => self.am.tick(mono, &controls),
                    PHONE => self.phone.tick(mono, &controls),
                    WALKIE => self.walkie.tick(mono, &controls),
                    // The megaphone, the only one with an oversampler.
                    _ => {
                        latency = share.mul_add(Oversampler2::LATENCY, latency);
                        self.megaphone.tick(mono, &controls)
                    }
                };
                wet = heard.mul_add(share, wet);
            }
        }
        let mut out = [0.0f32; 2];
        for ((line, input), sample) in self.dry.iter_mut().zip([left, right]).zip(&mut out) {
            line.push(input);
            *sample = parts::lerp(line.read(latency.max(1.0)), wet, mix) * output;
        }
        out.into()
    }
}

impl Default for Radio {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Radio {
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
        for weight in &mut self.weights {
            weight.set_time(0.04, rate);
        }
        self.am.prepare(rate);
        self.phone.prepare(rate);
        self.walkie.prepare(rate);
        self.megaphone.prepare(rate);
        for line in &mut self.dry {
            line.resize(Oversampler2::LATENCY as usize + 8);
        }
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        self.aim();
        for weight in &mut self.weights {
            weight.snap(weight.target());
        }
        self.read_controls();
        self.wander = Drift::new();
        self.fade = Drift::new();
        self.noise = Noise::new(0x0A00_0001);
        self.controls.wander = 0.0;
        self.controls.fade = 0.0;
        self.am.reset(&self.controls);
        self.phone.reset();
        self.phone.control(&self.controls);
        self.walkie.reset();
        self.megaphone.reset();
        self.megaphone.control(&self.controls);
        for line in &mut self.dry {
            line.clear();
        }
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
    use super::super::parts::testkit::{self, built, peak, render, rms, silence, sine};
    use super::*;
    use crate::dsp::gain_to_db;

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn every_device_is_silent_with_nothing_on_the_air() {
        let input = silence(1.0);
        for model in 0..MODELS {
            let mut radio = built(
                &KIND,
                &[
                    (MODEL, model as f32),
                    (NOISE, 0.0),
                    (TUNE, 0.0),
                    (DRIFT, 0.0),
                ],
            );
            let (left, right) = render(radio.as_mut(), &input, &input);
            assert!(
                peak(&left) < 1e-5 && peak(&right) < 1e-5,
                "{model}: {}",
                peak(&left)
            );
        }
    }

    #[test]
    fn noise_is_bounded() {
        let input = silence(2.0);
        for model in 0..MODELS {
            let mut radio = built(
                &KIND,
                &[
                    (MODEL, model as f32),
                    (NOISE, 100.0),
                    (TUNE, 100.0),
                    (DRIFT, 100.0),
                ],
            );
            let (left, _) = render(radio.as_mut(), &input, &input);
            assert!(
                rms(&left) < 0.2 && peak(&left) < 1.0,
                "{model}: {}",
                rms(&left)
            );
        }
    }

    #[test]
    fn every_device_passes_a_voice_band_tone_at_a_sensible_level() {
        let tone = sine(1_000.0, 0.3, 1.0);
        for model in 0..MODELS {
            let mut radio = built(&KIND, &[(MODEL, model as f32)]);
            let (left, right) = render(radio.as_mut(), &tone, &tone);
            let change = gain_to_db(rms(&left[24_000..])) - gain_to_db(rms(&tone[24_000..]));
            assert!(change.abs() < 9.0, "{model}: {change} dB");
            assert!(rms(&left[24_000..]) > 0.0 && (rms(&left) - rms(&right)).abs() < 1e-6);
        }
    }

    #[test]
    fn devices_cut_the_band() {
        let low = sine(60.0, 0.3, 1.0);
        let high = sine(9_000.0, 0.3, 1.0);
        for model in 0..MODELS {
            let level = |input: &[f32]| {
                let mut radio = built(&KIND, &[(MODEL, model as f32), (NOISE, 0.0)]);
                let (left, _) = render(radio.as_mut(), input, input);
                rms(&left[24_000..])
            };
            let (under, over) = (level(&low), level(&high));
            assert!(under < 0.03 && over < 0.03, "{model}: {under} {over}");
        }
    }

    #[test]
    fn the_walkie_squelch_tail_follows_noise() {
        let mut burst = sine(800.0, 0.3, 0.5);
        burst.resize(48_000, 0.0);
        let tail = |noise: f32| {
            let mut radio = built(&KIND, &[(MODEL, 2.0), (NOISE, noise), (DRIFT, 0.0)]);
            let (left, _) = render(radio.as_mut(), &burst, &burst);
            // The voice stops at 0.5 s; the transmitter hangs 0.3 s.
            rms(&left[39_000..45_000])
        };
        assert!(tail(0.0) < 1e-4);
        assert!(tail(100.0) > 0.01, "{}", tail(100.0));
    }

    #[test]
    fn am_off_station_whistles() {
        let input = silence(1.0);
        let mut radio = built(&KIND, &[(TUNE, 60.0), (NOISE, 0.0), (DRIFT, 0.0)]);
        let (left, _) = render(radio.as_mut(), &input, &input);
        assert!(rms(&left[24_000..]) > 1e-3);
    }
}
