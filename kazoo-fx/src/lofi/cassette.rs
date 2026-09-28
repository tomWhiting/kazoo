//! A cassette deck.
//!
//! # The model
//!
//! The signal goes through a deck in the order a real one takes it:
//!
//! 1. **Transport.** The tape runs at 1⅞ inches per second past the heads.
//!    Its speed error comes from the parts that turn it: the hubs (about
//!    half a turn a second, the slow wow), the pinch roller and capstan
//!    (a 2 mm capstan turns about 7.6 times a second), and the motor belt.
//!    Each is a [`Wobble`] partial. Wow and flutter are separate knobs, as
//!    on a flutter meter's weighting.
//! 2. **Noise reduction, encode.** A Dolby-B-style compander: a first-order
//!    high shelf above about 1.5 kHz boosts quiet treble by up to 10 dB and
//!    backs off as the treble gets louder. (Dolby B's shelf also slides in
//!    frequency; this is the fixed-band approximation.)
//! 3. **Record equalisation.** Treble is boosted before the tape by the
//!    formulation's playback time constant (120 µs for ferric type I,
//!    70 µs for chrome type II and metal type IV) and cut again on
//!    playback. The boost is why cassettes run out of treble headroom
//!    first: loud cymbals saturate before loud bass does.
//! 4. **Tape.** Jiles–Atherton magnetisation ([`Magnetic`]), oversampled
//!    twice. Each formulation has its own maximum output level and a
//!    typical bias error: ferric saturates earliest and is the grittiest,
//!    metal is the cleanest and has the most headroom.
//! 5. **Tape noise and dropouts.** Hiss is on the tape, so it goes through
//!    everything after it, playback equalisation and noise reduction
//!    included (hence the de-emphasis and Dolby's hiss reduction, and its
//!    breathing). Dropouts lift the tape off the head: the level dips and
//!    the treble goes first.
//! 6. **Playback head.** Gap loss, spacing loss (Wallace's 54.6 dB per
//!    wavelength) and azimuth loss from a tilted head, worked out from the
//!    geometry ([`head_corner`]); the tilt also offsets the two tracks in
//!    time, and the tape weaving in the guides moves that offset, so the
//!    stereo image smears. The head bump (a low resonance where the
//!    wavelength matches the head's contact length) sits at about 70 Hz.
//! 7. **Noise reduction, decode.** The inverse of the encoder, built as the
//!    real decoder is: the same shelf in a feedback loop, so it undoes the
//!    encoder exactly when the levels match. `pump` misaligns its level
//!    detector, which is what a mis-calibrated deck does: dull treble that
//!    breathes with the music.
//!
//! `chew` is tape that has been through a mangled transport: crinkles lift
//! it off the head (treble loss, level flutter, dropouts) and it snags,
//! so bursts of heavy, irregular warble come and go.

use super::parts::{
    self, Biquad, Butterworth, CONTROL, Decibels, Drift, Dropouts, Head, Hiss, Magnetic,
    Oversampler2, Partial, Ramp, Wobble,
};
use crate::dsp::{DelayLine, Noise, OnePole, Smoothed, db_to_gain, gain_to_db};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const TAPE: usize = 0;
const DRIVE: usize = 1;
const WOW: usize = 2;
const FLUTTER: usize = 3;
const AZIMUTH: usize = 4;
const NR: usize = 5;
const PUMP: usize = 6;
const HISS: usize = 7;
const DROPOUTS: usize = 8;
const CHEW: usize = 9;
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
        name: "tape",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["type I", "type II", "type IV"],
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
    percent("wow", 20.0),
    percent("flutter", 20.0),
    percent("azimuth", 15.0),
    ParamSpec {
        name: "nr",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["off", "dolby b", "encode only", "decode only"],
        },
    },
    percent("pump", 0.0),
    percent("hiss", 25.0),
    percent("dropouts", 10.0),
    percent("chew", 0.0),
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

/// The cassette deck.
pub static KIND: EffectKind = EffectKind {
    id: "cassette",
    name: "Cassette deck",
    description: "A cassette deck: tape formulation, Jiles-Atherton saturation, head bump, \
                  gap and azimuth loss, Dolby-style noise reduction, wow, flutter, hiss, \
                  dropouts and chewed tape.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Cassette::new())
}

/// 1⅞ inches per second, in metres per second.
const SPEED: f32 = 0.047_625;
/// Playback gap of a good cassette head.
const GAP: f32 = 1.2e-6;
/// Width of one stereo track, and centre-to-centre distance of the pair.
const TRACK: f32 = 0.6e-3;
const TRACK_PITCH: f32 = 0.9e-3;
/// Azimuth error at full `azimuth`: about 20 minutes of arc.
const MAX_TILT: f32 = 0.006;
/// Peak speed errors at full `wow`, `flutter` and during a full `chew` snag.
const MAX_WOW: f32 = 0.015;
const MAX_FLUTTER: f32 = 0.006;
const MAX_CHEW: f32 = 0.025;
/// Where the compander splits treble from the rest.
const NR_SPLIT: f32 = 1_500.0;
/// The most the compander boosts quiet treble.
const NR_MAX_DB: f32 = 10.0;
/// Hiss at full `hiss` before playback equalisation, as linear RMS.
const HISS_TOP: f32 = 0.02;

static WOW_PARTS: [Partial; 3] = [
    Partial {
        hz: 0.55,
        weight: 0.45,
        wander: 0.15,
        steady: 0.6,
    },
    Partial {
        hz: 0.9,
        weight: 0.3,
        wander: 0.2,
        steady: 0.4,
    },
    Partial {
        hz: 1.7,
        weight: 0.25,
        wander: 0.25,
        steady: 0.3,
    },
];

static FLUTTER_PARTS: [Partial; 4] = [
    Partial {
        hz: 3.6,
        weight: 0.2,
        wander: 0.1,
        steady: 0.6,
    },
    Partial {
        hz: 7.6,
        weight: 0.4,
        wander: 0.05,
        steady: 0.7,
    },
    Partial {
        hz: 12.5,
        weight: 0.25,
        wander: 0.15,
        steady: 0.3,
    },
    Partial {
        hz: 19.0,
        weight: 0.15,
        wander: 0.2,
        steady: 0.2,
    },
];

static CHEW_PARTS: [Partial; 4] = [
    Partial {
        hz: 2.7,
        weight: 0.3,
        wander: 0.3,
        steady: 0.0,
    },
    Partial {
        hz: 4.9,
        weight: 0.3,
        wander: 0.3,
        steady: 0.0,
    },
    Partial {
        hz: 8.3,
        weight: 0.25,
        wander: 0.3,
        steady: 0.0,
    },
    Partial {
        hz: 13.0,
        weight: 0.15,
        wander: 0.3,
        steady: 0.0,
    },
];

/// What a tape formulation changes.
#[derive(Debug, Clone, Copy)]
struct Formulation {
    /// Record treble boost (and playback cut), dB.
    emphasis: f32,
    /// Where it starts: the playback time constant as a frequency, Hz.
    turnover: f32,
    /// Maximum output level relative to ferric.
    level: f32,
    /// Share of the raw hysteresis loop a typical deck leaves in.
    underbias: f32,
    /// Hiss relative to ferric, dB.
    hiss: f32,
    /// Head-to-tape spacing, metres (smoother oxides ride closer).
    spacing: f32,
}

const FORMULATIONS: [Formulation; 3] = [
    // Type I, ferric: 120 µs.
    Formulation {
        emphasis: 12.0,
        turnover: 1_326.0,
        level: 1.0,
        underbias: 0.18,
        hiss: 0.0,
        spacing: 0.34e-6,
    },
    // Type II, chrome: 70 µs.
    Formulation {
        emphasis: 10.0,
        turnover: 2_274.0,
        level: 1.25,
        underbias: 0.12,
        hiss: -3.0,
        spacing: 0.3e-6,
    },
    // Type IV, metal: 70 µs.
    Formulation {
        emphasis: 8.0,
        turnover: 2_274.0,
        level: 1.6,
        underbias: 0.08,
        hiss: -4.0,
        spacing: 0.28e-6,
    },
];

/// Boost the compander gives treble at envelope `env`, as linear gain.
fn nr_boost(env: f32, offset_db: f32) -> f32 {
    let level = gain_to_db(env) + offset_db;
    db_to_gain(NR_MAX_DB / (1.0 + ((level + 35.0) / 6.0).exp()))
}

/// One side of the compander: a one-pole split, a treble envelope, and the
/// shelf gain it sets, worked out every 16 samples on the same schedule on
/// both sides so a matched pair still cancels exactly.
#[derive(Debug, Clone, Copy)]
struct Compander {
    split: f32,
    env: f32,
    gain: Ramp,
}

impl Default for Compander {
    fn default() -> Self {
        Self {
            split: 0.0,
            env: 0.0,
            gain: Ramp::new(db_to_gain(NR_MAX_DB), 16),
        }
    }
}

impl Compander {
    fn follow(&mut self, treble: f32, attack: f32, release: f32) {
        let size = treble.abs();
        let coeff = if size > self.env { attack } else { release };
        self.env = (size - self.env).mul_add(coeff, self.env);
        crate::dsp::flush(&mut self.env);
    }

    /// Boost quiet treble: `x + (g − 1)·treble(x)`.
    fn encode(&mut self, x: f32, split: f32, attack: f32, release: f32) -> f32 {
        let env = self.env;
        let gain = self.gain.next(|| nr_boost(env, 0.0));
        let treble = (1.0 - split) * (x - self.split);
        self.split = (x - self.split).mul_add(split, self.split);
        crate::dsp::flush(&mut self.split);
        self.follow(treble, attack, release);
        (gain - 1.0).mul_add(treble, x)
    }

    /// The exact inverse: `y = e − (g − 1)·treble(y)`, solved for `y`.
    fn decode(&mut self, e: f32, split: f32, offset_db: f32, attack: f32, release: f32) -> f32 {
        let env = self.env;
        let gain = self.gain.next(|| nr_boost(env, offset_db));
        let beta = (gain - 1.0) * (1.0 - split);
        let y = beta.mul_add(self.split, e) / (1.0 + beta);
        let treble = (1.0 - split) * (y - self.split);
        self.split = (y - self.split).mul_add(split, self.split);
        crate::dsp::flush(&mut self.split);
        self.follow(treble, attack, release);
        y
    }
}

/// Everything one sample shares between the two tracks.
#[derive(Debug, Clone, Copy)]
struct Frame {
    drive: f32,
    underbias: f32,
    encode: f32,
    decode: f32,
    pump_db: f32,
    release: f32,
    hiss: f32,
    loss: f32,
    crinkle: f32,
}

/// One track of the tape.
#[derive(Debug, Clone)]
struct Track {
    line: DelayLine,
    encoder: Compander,
    decoder: Compander,
    emphasis: Biquad,
    deemphasis: Biquad,
    oversampler: Oversampler2,
    tape: Magnetic,
    hiss: Hiss,
    dropout: OnePole,
    head: Butterworth,
    bump: Biquad,
    low_cut: Biquad,
}

impl Track {
    fn new(seed: u32) -> Self {
        Self {
            line: DelayLine::default(),
            encoder: Compander::default(),
            decoder: Compander::default(),
            emphasis: Biquad::new(),
            deemphasis: Biquad::new(),
            oversampler: Oversampler2::new(),
            tape: Magnetic::default(),
            hiss: Hiss::new(seed),
            dropout: OnePole::default(),
            head: Butterworth::default(),
            bump: Biquad::new(),
            low_cut: Biquad::new(),
        }
    }

    fn reset(&mut self) {
        self.line.clear();
        self.encoder = Compander::default();
        self.decoder = Compander::default();
        self.emphasis.reset();
        self.deemphasis.reset();
        self.oversampler.reset();
        self.tape.reset();
        self.hiss.reset();
        self.dropout.reset();
        self.head.reset();
        self.bump.reset();
        self.low_cut.reset();
    }

    /// Record `x` and play it back.
    fn play(&mut self, x: f32, frame: &Frame, nr: &NrTimes) -> f32 {
        let encoded = self.encoder.encode(x, nr.split, nr.attack, nr.release);
        let into = parts::lerp(x, encoded, frame.encode);
        let boosted = self.emphasis.process(into);
        let Self {
            oversampler, tape, ..
        } = self;
        let recorded =
            oversampler.process(boosted, |v| tape.process(v, frame.drive, frame.underbias));
        let on_tape = self
            .hiss
            .next()
            .mul_add(frame.hiss, recorded * frame.crinkle);
        let lifted = parts::lerp(on_tape, self.dropout.lowpass(on_tape), frame.loss);
        let read = self
            .head
            .process(lifted * (-0.7f32).mul_add(frame.loss, 1.0));
        let shaped = self.low_cut.process(self.bump.process(read));
        let flat = self.deemphasis.process(shaped);
        let decoded = self
            .decoder
            .decode(flat, nr.split, frame.pump_db, nr.attack, frame.release);
        parts::lerp(flat, decoded, frame.decode)
    }
}

/// The compander's fixed coefficients at the current rate.
#[derive(Debug, Clone, Copy)]
struct NrTimes {
    split: f32,
    attack: f32,
    release: f32,
}

impl NrTimes {
    fn at(rate: f32) -> Self {
        let coeff = |seconds: f32| 1.0 - (-1.0 / (seconds * rate)).exp();
        Self {
            split: coeff(1.0 / (std::f32::consts::TAU * NR_SPLIT)),
            attack: coeff(0.001),
            release: coeff(0.06),
        }
    }
}

/// A cassette deck. See the module documentation for the model.
#[derive(Debug, Clone)]
pub struct Cassette {
    rate: f32,
    knobs: [Smoothed; COUNT],
    emphasis: Smoothed,
    turnover: Smoothed,
    level: Smoothed,
    underbias: Smoothed,
    hiss_db: Smoothed,
    spacing: Smoothed,
    encode: Smoothed,
    decode: Smoothed,
    tracks: [Track; 2],
    wow: Wobble,
    flutter: Wobble,
    chew: Wobble,
    weave: Drift,
    weave_noise: Noise,
    dropouts: Dropouts,
    snags: Dropouts,
    snag: OnePole,
    crinkle: OnePole,
    crinkle_noise: Noise,
    nr: NrTimes,
    base: f32,
    until_control: usize,
    frame: Frame,
    tilt: f32,
    drive_gain: Decibels,
    output_gain: Decibels,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, DRIVE | MIX | OUTPUT)
}

impl Cassette {
    /// A deck at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut deck = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            emphasis: Smoothed::new(0.0),
            turnover: Smoothed::new(1_000.0),
            level: Smoothed::new(1.0),
            underbias: Smoothed::new(0.0),
            hiss_db: Smoothed::new(0.0),
            spacing: Smoothed::new(0.3e-6),
            encode: Smoothed::new(0.0),
            decode: Smoothed::new(0.0),
            tracks: [Track::new(0x5EED_0001), Track::new(0x5EED_0002)],
            wow: Wobble::new(&WOW_PARTS, 0xCA55_0001),
            flutter: Wobble::new(&FLUTTER_PARTS, 0xCA55_0002),
            chew: Wobble::new(&CHEW_PARTS, 0xCA55_0003),
            weave: Drift::new(),
            weave_noise: Noise::new(0xCA55_0004),
            dropouts: Dropouts::new(0xCA55_0005),
            snags: Dropouts::new(0xCA55_0006),
            snag: OnePole::default(),
            crinkle: OnePole::default(),
            crinkle_noise: Noise::new(0xCA55_0007),
            nr: NrTimes::at(48_000.0),
            base: 0.0,
            until_control: 0,
            frame: Frame {
                drive: 1.5,
                underbias: 0.0,
                encode: 0.0,
                decode: 0.0,
                pump_db: 0.0,
                release: 0.0,
                hiss: 0.0,
                loss: 0.0,
                crinkle: 1.0,
            },
            tilt: 0.0,
            drive_gain: Decibels::new(),
            output_gain: Decibels::new(),
        };
        deck.prepare(48_000.0);
        deck
    }

    fn formulation(&self) -> Formulation {
        FORMULATIONS[(self.knobs[TAPE].target() as usize).min(2)]
    }

    /// Point the derived glides at what the stepped knobs ask for.
    fn aim(&mut self) {
        let tape = self.formulation();
        self.emphasis.set(tape.emphasis);
        self.turnover.set(tape.turnover);
        self.level.set(tape.level);
        self.underbias.set(tape.underbias);
        self.hiss_db.set(tape.hiss);
        self.spacing.set(tape.spacing);
        let nr = self.knobs[NR].target() as usize;
        self.encode.set(if matches!(nr, 1 | 2) { 1.0 } else { 0.0 });
        self.decode.set(if matches!(nr, 1 | 3) { 1.0 } else { 0.0 });
    }

    const fn derived(&mut self) -> [&mut Smoothed; 6] {
        [
            &mut self.emphasis,
            &mut self.turnover,
            &mut self.level,
            &mut self.underbias,
            &mut self.hiss_db,
            &mut self.spacing,
        ]
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and move the filters.
    fn control(&mut self) {
        self.aim();
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        for glide in self.derived() {
            glide.step();
        }
        let rate = self.rate;
        let emphasis = self.emphasis.value();
        let turnover = self.turnover.value() * 2.0;
        let chew = self.knobs[CHEW].value() / 100.0;
        // The tilt is a few thousandths of a radian: tan θ = θ.
        let skew = TRACK * self.tilt;
        let head = Head {
            speed: SPEED,
            gap: GAP,
            skew,
            spacing: chew.mul_add(0.8e-6, self.spacing.value()),
        };
        let corner = parts::head_corner(head, rate * 0.45);
        for track in &mut self.tracks {
            track.emphasis.high_shelf(turnover, emphasis, rate);
            track.deemphasis.high_shelf(turnover, -emphasis, rate);
            track.head.lowpass(1, corner, rate);
        }
        let hiss = self.knobs[HISS].value() / 100.0;
        self.frame.hiss = HISS_TOP * hiss * hiss.sqrt() * db_to_gain(self.hiss_db.value());
        let pump = self.knobs[PUMP].value() / 100.0;
        self.frame.pump_db = -8.0 * pump;
        self.frame.release = 1.0 - (-1.0 / (0.06 * 2.0f32.mul_add(pump, 1.0) * rate)).exp();
        self.frame.underbias = self.underbias.value();
    }

    /// One stereo sample.
    fn tick(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.until_control == 0 {
            self.control();
            self.until_control = CONTROL;
        }
        self.until_control -= 1;
        let rate = self.rate;
        let drive = self.drive_gain.gain(self.knobs[DRIVE].step());
        let mix = self.knobs[MIX].step() / 100.0;
        let output = self.output_gain.gain(self.knobs[OUTPUT].step());
        self.frame.encode = self.encode.step();
        self.frame.decode = self.decode.step();
        self.frame.drive = 1.5 * drive / self.level.value();

        let chew = self.knobs[CHEW].value() / 100.0;
        let snag_gate = self.snags.next(0.6 * chew, 1.0, 0.35, rate);
        let snag = self.snag.lowpass(snag_gate);
        let wobble = self
            .wow
            .next(MAX_WOW * self.knobs[WOW].value() / 100.0, 1.0, 1.0, rate)
            + self.flutter.next(
                MAX_FLUTTER * self.knobs[FLUTTER].value() / 100.0,
                1.0,
                1.0,
                rate,
            )
            + self.chew.next(MAX_CHEW * chew * snag, 1.0, 1.0, rate);
        let weave = self.weave.next(&mut self.weave_noise, 3.0, rate);
        self.tilt = MAX_TILT * self.knobs[AZIMUTH].value() / 100.0 * 0.15f32.mul_add(weave, 1.0);
        let offset = TRACK_PITCH * self.tilt / SPEED * 0.5;

        let dropouts = self.knobs[DROPOUTS].value() / 100.0;
        self.frame.loss = self
            .dropouts
            .next(3.0f32.mul_add(dropouts, 4.0 * chew), 0.9, 0.06, rate)
            .max(0.0);
        let rough = self.crinkle.lowpass(self.crinkle_noise.sample() * 3.0);
        self.frame.crinkle = (chew * 0.35).mul_add(-rough.abs(), 1.0).max(0.3);

        let base = self.base;
        let dry_at = (base + Oversampler2::LATENCY / rate) * rate;
        let reads = [
            (base + wobble - offset) * rate,
            (base + wobble + offset) * rate,
        ];
        let frame = self.frame;
        let nr = self.nr;
        let mut out = [0.0f32; 2];
        for ((track, (input, read)), wet) in self
            .tracks
            .iter_mut()
            .zip([left, right].into_iter().zip(reads))
            .zip(&mut out)
        {
            track.line.push(input);
            let dry = track.line.read(dry_at);
            let played = track.play(track.line.read(read), &frame, &nr);
            *wet = parts::lerp(dry, played, mix) * output;
        }
        out.into()
    }
}

impl Default for Cassette {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Cassette {
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
        for glide in self.derived() {
            glide.set_time(0.05, control_rate);
        }
        self.encode.set_time(0.03, rate);
        self.decode.set_time(0.03, rate);
        let reach = self.wow.reach(MAX_WOW, 1.0)
            + self.flutter.reach(MAX_FLUTTER, 1.0)
            + self.chew.reach(MAX_CHEW, 1.0)
            + TRACK_PITCH * MAX_TILT * 1.2 / SPEED;
        self.base = reach + 0.000_5 + 4.0 / rate;
        let longest = (2.0 * self.base * rate) as usize + Oversampler2::LATENCY as usize + 16;
        for track in &mut self.tracks {
            track.line.resize(longest);
            track.hiss.band(150.0, rate * 0.45, rate);
            track.dropout.set_cutoff(1_000.0, rate);
            track.bump.peak(70.0, 1.0, 2.5, rate);
            track.low_cut.highpass(22.0, 0.6, rate);
        }
        self.snag.set_cutoff(2.0, rate);
        self.crinkle.set_cutoff(25.0, rate);
        self.nr = NrTimes::at(rate);
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        self.aim();
        for glide in self.derived() {
            glide.snap(glide.target());
        }
        self.encode.snap(self.encode.target());
        self.decode.snap(self.decode.target());
        for track in &mut self.tracks {
            track.reset();
        }
        self.wow.reset();
        self.flutter.reset();
        self.chew.reset();
        self.weave = Drift::new();
        self.weave_noise = Noise::new(0xCA55_0004);
        self.dropouts.reset();
        self.snags.reset();
        self.snag.reset();
        self.crinkle.reset();
        self.crinkle_noise = Noise::new(0xCA55_0007);
        self.tilt = MAX_TILT * self.knobs[AZIMUTH].value() / 100.0;
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

    /// The compander's boost at treble envelope `env`, dB.
    fn boost_db(env: f32) -> f32 {
        gain_to_db(nr_boost(env, 0.0))
    }

    fn quiet(extra: &[(usize, f32)]) -> Box<dyn Effect> {
        let mut params = vec![
            (WOW, 0.0),
            (FLUTTER, 0.0),
            (AZIMUTH, 0.0),
            (HISS, 0.0),
            (DROPOUTS, 0.0),
        ];
        params.extend_from_slice(extra);
        built(&KIND, &params)
    }

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn silence_stays_silent_without_hiss() {
        let mut deck = quiet(&[(NR, 1.0)]);
        let input = silence(1.0);
        let (left, right) = render(deck.as_mut(), &input, &input);
        assert!(peak(&left) < 1e-6 && peak(&right) < 1e-6);
    }

    #[test]
    fn hiss_is_audible_but_bounded() {
        let input = silence(2.0);
        for tape in 0..3 {
            let mut deck = built(
                &KIND,
                &[(TAPE, tape as f32), (HISS, 100.0), (DROPOUTS, 0.0)],
            );
            let (left, right) = render(deck.as_mut(), &input, &input);
            let level = rms(&left[4_800..]);
            assert!(level > 1e-4 && level < 0.03, "type {tape}: {level}");
            assert!(peak(&left) < 0.2 && peak(&right) < 0.2);
        }
        // Dolby B takes the hiss down.
        let mut plain = built(&KIND, &[(HISS, 100.0)]);
        let mut dolby = built(&KIND, &[(HISS, 100.0), (NR, 1.0)]);
        let a = rms(&render(plain.as_mut(), &input, &input).0[9_600..]);
        let b = rms(&render(dolby.as_mut(), &input, &input).0[9_600..]);
        assert!(b < a * 0.8, "{a} {b}");
    }

    #[test]
    fn wow_and_flutter_track_their_knobs() {
        let tone = sine(1_000.0, 0.25, 3.0);
        let deviation = |wow: f32, flutter: f32| {
            let mut deck = quiet(&[(WOW, wow), (FLUTTER, flutter)]);
            let (left, _) = render(deck.as_mut(), &tone, &tone);
            frequency_deviation(&left, 1_000.0, 0.5)
        };
        let still = deviation(0.0, 0.0);
        assert!(still < 2e-4, "{still}");
        let half = deviation(50.0, 0.0);
        let full = deviation(100.0, 0.0);
        assert!(full > 0.004 && full < MAX_WOW * 1.1, "{full}");
        assert!(half > full * 0.3 && half < full * 0.7, "{half} {full}");
        let flutter = deviation(0.0, 100.0);
        assert!(flutter > 0.001 && flutter < MAX_FLUTTER * 1.2, "{flutter}");
    }

    #[test]
    fn matched_noise_reduction_stays_close_to_flat() {
        let tone = sine(5_000.0, 0.02, 1.0);
        let mut off = quiet(&[]);
        let mut on = quiet(&[(NR, 1.0)]);
        let a = rms(&render(off.as_mut(), &tone, &tone).0[9_600..]);
        let b = rms(&render(on.as_mut(), &tone, &tone).0[9_600..]);
        // Only the head's own treble loss (which the decoder expands) keeps
        // it from exact.
        assert!((gain_to_db(b) - gain_to_db(a)).abs() < 1.5, "{a} {b}");
        // Decoding a tape that was never encoded dulls quiet treble.
        let mut dull = quiet(&[(NR, 3.0)]);
        let c = rms(&render(dull.as_mut(), &tone, &tone).0[9_600..]);
        assert!(gain_to_db(c) < gain_to_db(a) - 4.0, "{a} {c}");
        assert!(boost_db(1.0) < 0.1 && boost_db(1e-4) > 9.5);
    }

    #[test]
    fn metal_has_more_headroom_than_ferric() {
        let tone = sine(8_000.0, 0.9, 0.5);
        let level = |tape: f32| {
            let mut deck = quiet(&[(TAPE, tape), (DRIVE, 6.0)]);
            let (left, _) = render(deck.as_mut(), &tone, &tone);
            rms(&left[4_800..])
        };
        // Treble saturates: ferric squashes a loud 8 kHz tone the most.
        let (ferric, metal) = (level(0.0), level(2.0));
        assert!(ferric < metal * 0.9, "{ferric} {metal}");
    }

    #[test]
    fn azimuth_offsets_the_tracks() {
        let tone = sine(3_000.0, 0.3, 0.5);
        let mut deck = quiet(&[(AZIMUTH, 100.0)]);
        let (left, right) = render(deck.as_mut(), &tone, &tone);
        let diff: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        assert!(rms(&diff[4_800..]) > 0.05, "{}", rms(&diff[4_800..]));
        let mut straight = quiet(&[]);
        let (left, right) = render(straight.as_mut(), &tone, &tone);
        let diff: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        assert!(rms(&diff) < 1e-6);
    }

    #[test]
    fn knob_moves_do_not_click() {
        let tone = sine(200.0, 0.3, 1.0);
        let mut deck = quiet(&[]);
        let mut out_l = vec![0.0; tone.len()];
        let mut out_r = vec![0.0; tone.len()];
        for (n, ((in_l, o_l), o_r)) in tone
            .chunks(480)
            .zip(out_l.chunks_mut(480))
            .zip(out_r.chunks_mut(480))
            .enumerate()
        {
            if n == 20 {
                deck.set_param(TAPE, 2.0);
                deck.set_param(NR, 1.0);
                deck.set_param(DRIVE, 18.0);
                deck.set_param(MIX, 30.0);
            }
            deck.process(&CONTEXT, [in_l, in_l], [o_l, o_r]);
        }
        let jump = out_l
            .windows(2)
            .skip(4_800)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        // A 200 Hz tone at 0.3 moves at most about 0.008 per sample.
        assert!(jump < 0.03, "{jump}");
    }
}
