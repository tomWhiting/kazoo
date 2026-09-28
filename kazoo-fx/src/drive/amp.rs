//! Amp: a two-stage valve preamp, a passive tone stack, a sagging power
//! amp and a speaker cabinet.
//!
//! The model runs the valves and the tone stack at an internal rate of at
//! least 705.6 kHz (16x at 44.1 and 48 kHz, 8x at 88.2 and 96 kHz, 4x at
//! 176.4 and 192 kHz; lower only below 44.1 kHz, where the 16x cap bites)
//! and the cabinet at the host's rate. The valves
//! themselves, the kinks where a grid starts to conduct and the power
//! amp's saturation are also anti-aliased by their antiderivatives (the
//! valve's from an integral table built alongside its curve).
//!
//! 1. **Two 12AX7 gain stages**, each a common-cathode triode with a
//!    100 kΩ plate load on a 250 V supply and a bypassed 1.5 kΩ cathode
//!    resistor. The valve itself is Koren's model,
//!    `E1 = (Vpk / Kp) ln(1 + e^(Kp (1/µ + Vgk / √(Kvb + Vpk²))))`,
//!    `Ip = 2 E1^Ex / Kg1` (µ = 100, Ex = 1.4, Kg1 = 1060, Kp = 600,
//!    Kvb = 300). The stage's load line, `Vp = B+ - Rp Ip(Vgk, Vpk)`, is
//!    solved once when the effect is built for every grid voltage from
//!    -12 V to +10 V (by then the plate has bottomed out) and kept as a
//!    table read with cubic interpolation, carried on in a straight line
//!    past either end. It compresses into cutoff on one side and saturates
//!    on the other, so a valve's even harmonics come for free.
//! 2. **Grid blocking.** The second grid is fed from the first plate
//!    through a 22 nF coupling capacitor with a 1 MΩ grid leak; the first,
//!    from the guitar, through 100 nF standing in for the input network.
//!    When a grid is driven above its cathode it conducts, through about
//!    100 kΩ (the 68 kΩ grid stopper plus the source feeding it) into the
//!    grid's own 2 kΩ, and charges the capacitor, which shifts the stage's
//!    bias down for a while after a hard note: the swelling, farting
//!    blocking distortion of a cranked amp. The capacitor is stepped with
//!    the backward Euler rule, solved exactly for whichever side of
//!    conduction the grid is on.
//! 3. **The gain knob**: a log pot between the stages. Like `master`, it
//!    changes loudness as well as dirt, as on the real amp; `level` trims
//!    the result.
//! 4. **The tone stack**: the passive treble/bass/middle network shared by
//!    the Fender Bassman and the Marshall, from its real parts. The treble
//!    pot is a divider with the output on its wiper, the bass pot a
//!    variable resistor, and the middle pot a divider with the third
//!    capacitor on its wiper. Its third-order transfer function in the
//!    three pot positions is Yeh and Smith's closed form (the tests solve
//!    that network numerically and check the two agree). As on the amp,
//!    treble and middle are linear pots and bass is a log pot; `voice` picks the Fender (5F6-A: 250 pF, 20 nF,
//!    20 nF, 250 kΩ treble, 1 MΩ bass, 25 kΩ mid, 56 kΩ slope) or the
//!    Marshall parts (470 pF, 22 nF, 22 nF, 220 kΩ, 1 MΩ, 25 kΩ, 33 kΩ),
//!    crossfading between the two. The Marshall uses the same network
//!    with its own parts.
//! 5. **The power amp**: a push-pull pair, symmetric soft saturation, fed
//!    by the `master` knob. Its supply **sags**: drawing current pulls the
//!    rectifier and filter capacitor down (15 ms), and they recover when
//!    the playing eases (120 ms), so the headroom breathes with the
//!    playing. `sag` sets how soft the supply is; both channels share it.
//! 6. **The cabinet**: a closed-back 12-inch speaker, roughly: a
//!    resonant low end at 90 Hz, a boxy dip at 450 Hz, the cone's presence
//!    peak at 2.4 kHz and a steep fall from 5 kHz. `cab` switches it out
//!    for a line-out sound. It runs at the host's rate, its corners
//!    prewarped so they land where they should; the price is that below
//!    them it leads the analogue speaker a little, 3.3 µs at 44.1 kHz and
//!    0.2 µs at 192 kHz, too little to hear or to count as latency.
//!
//! Sources: N. Koren, "Improved VT models for SPICE simulations" (Glass
//! Audio, 1996); D. T. Yeh and J. O. Smith, "Discretization of the '59
//! Fender Bassman tone stack" (DAFX 2006); J. Pakarinen and D. T. Yeh,
//! "A review of digital techniques for modeling vacuum-tube guitar
//! amplifiers" (Computer Music Journal, 2009), for grid conduction and
//! power-supply sag.

use std::f64::consts::TAU;

use super::filter::{Analogue, Iir3, warp};
use super::kit::{
    DEFAULT_RATE, Knobs, Retune, audio_taper, ceiling, flush64, for_each_frame, fraction,
    gain as db_gain, log_cosh, one_pole, weight,
};
use super::oversample::TARGET_RATE;
use super::pair::{self, Pair};
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const GAIN: usize = 0;
const BASS: usize = 1;
const MIDDLE: usize = 2;
const TREBLE: usize = 3;
const MASTER: usize = 4;
const SAG: usize = 5;
const VOICE: usize = 6;
const CAB: usize = 7;
const LEVEL: usize = 8;

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

static PARAMS: [ParamSpec; 9] = [
    percent("gain", 50.0),
    percent("bass", 50.0),
    percent("middle", 50.0),
    percent("treble", 50.0),
    percent("master", 50.0),
    percent("sag", 30.0),
    ParamSpec {
        name: "voice",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["fender", "marshall"],
        },
    },
    ParamSpec {
        name: "cab",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["off", "on"],
        },
    },
    ParamSpec {
        name: "level",
        min: -24.0,
        max: 12.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
];

/// The Amp's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "amp",
    name: "Valve amp",
    description: "Two 12AX7 stages with grid blocking, a Fender or Marshall tone stack from the real parts, a sagging power amp and a speaker cabinet.",
    params: &PARAMS,
    build: || Box::new(Amp::new()),
};

/// Volts at the amp's input for a full-scale sample.
const INPUT_VOLTS: f64 = 1.0;
/// Full scale at the output for the power amp's full swing, set so the
/// default settings come out about as loud as they went in.
const OUTPUT_SCALE: f64 = 0.156;

// The 12AX7, after Koren.
const MU: f64 = 100.0;
const EX: f64 = 1.4;
const KG1: f64 = 1060.0;
const KP: f64 = 600.0;
const KVB: f64 = 300.0;

// The gain stage around it.
const SUPPLY: f64 = 250.0;
const R_PLATE: f64 = 100e3;
const R_CATHODE: f64 = 1.5e3;
const GRID_LOW: f64 = -12.0;
const GRID_HIGH: f64 = 10.0;
const TABLE_SIZE: usize = 4_097;

// The grid circuit.
const C_COUPLING: f64 = 22e-9;
const C_INPUT: f64 = 100e-9;
const R_LEAK: f64 = 1e6;
/// The grid stopper plus the driving plate's resistance.
const R_SOURCE: f64 = 100e3;
/// The grid-to-cathode path when it conducts.
const R_CONDUCTING: f64 = 2e3;

/// The gain pot never quite reaches zero.
const GAIN_FLOOR: f64 = 0.005;
/// Plate volts to power-amp drive.
const POWER_SCALE: f64 = 1.0 / 20.0;
const MASTER_LOW_DB: f64 = -12.0;
const MASTER_RANGE_DB: f64 = 30.0;
const SAG_DEPTH: f64 = 1.5;
const SAG_ATTACK: f64 = 0.015;
const SAG_RELEASE: f64 = 0.12;
const TRANSFORMER_HZ: f64 = 40.0;

/// The rate the valves run at: four times the family's usual target.
/// Flat out, two valves and the power amp square a loud note up so hard
/// that at 176.4 kHz its folds still reach about -42 dBc, and at 352.8 kHz
/// -47 dBc at the pitch that folds onto half itself (44.1 kHz); from
/// 705.6 kHz that is -65 dBc, for twice the work.
const INTERNAL_RATE: f32 = 4.0 * TARGET_RATE;

/// How far the antiderivatives delay, in samples at the circuit's rate:
/// two grids, two valves and the power stage each add half a sample.
const ADAA_DELAY: f64 = 2.5;

/// The share of a pot's track left at each end: 0.01 %, about 100 Ω of
/// the bass pot's 1 MΩ.
const POT_END: f64 = 1e-4;

/// One tone stack's parts: `c1..c3` in farads, `r1` treble pot, `r2`
/// bass pot, `r3` middle pot, `r4` the slope resistor, in ohms.
#[derive(Debug, Clone, Copy)]
struct StackParts {
    c1: f64,
    c2: f64,
    c3: f64,
    r1: f64,
    r2: f64,
    r3: f64,
    r4: f64,
}

const FENDER: StackParts = StackParts {
    c1: 250e-12,
    c2: 20e-9,
    c3: 20e-9,
    r1: 250e3,
    r2: 1e6,
    r3: 25e3,
    r4: 56e3,
};

const MARSHALL: StackParts = StackParts {
    c1: 470e-12,
    c2: 22e-9,
    c3: 22e-9,
    r1: 220e3,
    r2: 1e6,
    r3: 25e3,
    r4: 33e3,
};

/// The sum of some terms: keeps the tone stack's long polynomials legible.
fn sum<const N: usize>(terms: [f64; N]) -> f64 {
    terms.iter().sum()
}

/// The tone stack's transfer function for treble `t`, middle `m` and
/// bass `l` pot positions (each 0 to 1, `l` already through its log
/// taper): Yeh and Smith's equation (1), term for term.
fn tone_stack(parts: &StackParts, t: f64, m: f64, l: f64) -> Analogue {
    let StackParts {
        c1,
        c2,
        c3,
        r1,
        r2,
        r3,
        r4,
    } = *parts;
    let (c12, c13, c23, all) = (c1 * c2, c1 * c3, c2 * c3, c1 * c2 * c3);
    let mm = m * m;
    let b1 = sum([
        t * c1 * r1,
        m * c3 * r3,
        l * sum([c1 * r2, c2 * r2]),
        sum([c1 * r3, c2 * r3]),
    ]);
    let b2 = sum([
        t * sum([c12 * r1 * r4, c13 * r1 * r4]),
        -mm * sum([c13 * r3 * r3, c23 * r3 * r3]),
        m * sum([c13 * r1 * r3, c13 * r3 * r3, c23 * r3 * r3]),
        l * sum([c12 * r1 * r2, c12 * r2 * r4, c13 * r2 * r4]),
        l * m * sum([c13 * r2 * r3, c23 * r2 * r3]),
        sum([c12 * r1 * r3, c12 * r3 * r4, c13 * r3 * r4]),
    ]);
    let b3 = sum([
        l * m * sum([all * r1 * r2 * r3, all * r2 * r3 * r4]),
        -mm * sum([all * r1 * r3 * r3, all * r3 * r3 * r4]),
        m * sum([all * r1 * r3 * r3, all * r3 * r3 * r4]),
        t * all * r1 * r3 * r4,
        -t * m * all * r1 * r3 * r4,
        t * l * all * r1 * r2 * r4,
    ]);
    let a1 = sum([
        sum([c1 * r1, c1 * r3, c2 * r3, c2 * r4, c3 * r4]),
        m * c3 * r3,
        l * sum([c1 * r2, c2 * r2]),
    ]);
    let a2 = sum([
        m * sum([c13 * r1 * r3, -c23 * r3 * r4, c13 * r3 * r3, c23 * r3 * r3]),
        l * m * sum([c13 * r2 * r3, c23 * r2 * r3]),
        -mm * sum([c13 * r3 * r3, c23 * r3 * r3]),
        l * sum([c12 * r2 * r4, c12 * r1 * r2, c13 * r2 * r4, c23 * r2 * r4]),
        sum([c12 * r1 * r4, c13 * r1 * r4, c12 * r3 * r4]),
        sum([c12 * r1 * r3, c13 * r3 * r4, c23 * r3 * r4]),
    ]);
    let a3 = sum([
        l * m * sum([all * r1 * r2 * r3, all * r2 * r3 * r4]),
        -mm * sum([all * r1 * r3 * r3, all * r3 * r3 * r4]),
        m * sum([all * r3 * r3 * r4, all * r1 * r3 * r3, -all * r1 * r3 * r4]),
        l * all * r1 * r2 * r4,
        all * r1 * r3 * r4,
    ]);
    Analogue {
        b: [0.0, b1, b2, b3],
        a: [1.0, a1, a2, a3],
    }
}

/// Koren's plate current for the 12AX7, in amps.
fn plate_current(vgk: f64, vpk: f64) -> f64 {
    if vpk <= 0.0 {
        return 0.0;
    }
    let inner = KP * (1.0 / MU + vgk / vpk.mul_add(vpk, KVB).sqrt());
    let soft = if inner > 30.0 {
        inner
    } else {
        inner.exp().ln_1p()
    };
    let e1 = vpk / KP * soft;
    if e1 > 0.0 {
        2.0 * e1.powf(EX) / KG1
    } else {
        0.0
    }
}

/// The root of a falling function between `low` and `high`, by bisection.
fn falling_root(mut low: f64, mut high: f64, residual: impl Fn(f64) -> f64) -> f64 {
    for _ in 0..80 {
        let middle = 0.5 * (low + high);
        if residual(middle) > 0.0 {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

/// The cubic (Catmull-Rom) through a table segment, as coefficients of
/// `t`, `t²` and `t³` after the constant, for the segment from `x1` to
/// `x2` with neighbours `x0` and `x3`.
fn segment(x0: f64, x1: f64, x2: f64, x3: f64) -> [f64; 4] {
    [
        x1,
        0.5 * (x2 - x0),
        (-0.5f64).mul_add(x3, (-2.5f64).mul_add(x1, 2.0f64.mul_add(x2, x0))),
        0.5f64.mul_add(x3 - x0, 1.5 * (x1 - x2)),
    ]
}

/// A 12AX7 gain stage's plate swing for every grid voltage, and its
/// integral, for antiderivative anti-aliasing.
#[derive(Debug, Clone)]
struct Triode {
    /// The plate's swing away from rest at each table step.
    table: Vec<f64>,
    /// The integral of the swing from the table's start to each step.
    integrals: Vec<f64>,
    /// The grid volts between table steps.
    step: f64,
    /// The cathode's resting voltage: the grid conducts above it.
    cathode: f64,
    /// The table's slope at its low and high ends, in plate volts per grid
    /// volt, to carry it on past them.
    ends: [f64; 2],
}

impl Triode {
    /// Solve the stage. Allocates the tables.
    fn new() -> Self {
        let current = falling_root(0.0, SUPPLY / (R_PLATE + R_CATHODE), |ip| {
            plate_current(-ip * R_CATHODE, ip.mul_add(-(R_PLATE + R_CATHODE), SUPPLY)) - ip
        });
        let cathode = current * R_CATHODE;
        let rest = current.mul_add(-R_PLATE, SUPPLY);
        let step = (GRID_HIGH - GRID_LOW) / (TABLE_SIZE - 1) as f64;
        let table: Vec<f64> = (0..TABLE_SIZE)
            .map(|i| {
                let grid = (i as f64).mul_add(step, GRID_LOW);
                let plate = falling_root(cathode, SUPPLY, |vp| {
                    (SUPPLY - vp) / R_PLATE - plate_current(grid - cathode, vp - cathode)
                });
                plate - rest
            })
            .collect();
        let last = TABLE_SIZE - 1;
        let ends = [
            (table[1] - table[0]) / step,
            (table[last] - table[last - 1]) / step,
        ];
        let mut integrals = Vec::with_capacity(TABLE_SIZE);
        let mut total = 0.0;
        integrals.push(total);
        for i in 0..last {
            let [c0, c1, c2, c3] = Self::coefficients(&table, i);
            total += step * (c0 + c1 / 2.0 + c2 / 3.0 + c3 / 4.0);
            integrals.push(total);
        }
        Self {
            table,
            integrals,
            step,
            cathode,
            ends,
        }
    }

    fn coefficients(table: &[f64], index: usize) -> [f64; 4] {
        let last = table.len() - 1;
        let at = |i: usize| table[i.min(last)];
        segment(
            at(index.saturating_sub(1)),
            at(index),
            at(index + 1),
            at(index + 2),
        )
    }

    /// Where `grid` falls in the table: the segment and how far along it.
    fn place(&self, grid: f64) -> (usize, f64) {
        let last = self.table.len() - 1;
        let place = ((grid - GRID_LOW) / self.step).clamp(0.0, last as f64);
        let index = (place.floor() as usize).min(last - 1);
        (index, place - index as f64)
    }

    /// The plate's swing for `grid` volts.
    fn plate(&self, grid: f64) -> f64 {
        let last = self.table.len() - 1;
        if grid < GRID_LOW {
            return self.ends[0].mul_add(grid - GRID_LOW, self.table[0]);
        }
        if grid > GRID_HIGH {
            return self.ends[1].mul_add(grid - GRID_HIGH, self.table[last]);
        }
        let (index, t) = self.place(grid);
        let [c0, c1, c2, c3] = Self::coefficients(&self.table, index);
        c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, c0)
    }

    /// The integral of the plate's swing from the table's start to
    /// `grid`.
    fn integral(&self, grid: f64) -> f64 {
        let last = self.table.len() - 1;
        if grid < GRID_LOW {
            let over = grid - GRID_LOW;
            return (0.5 * self.ends[0] * over).mul_add(over, self.table[0] * over);
        }
        if grid > GRID_HIGH {
            let over = grid - GRID_HIGH;
            let ramp = (0.5 * self.ends[1] * over).mul_add(over, self.table[last] * over);
            return self.integrals[last] + ramp;
        }
        let (index, t) = self.place(grid);
        let [c0, c1, c2, c3] = Self::coefficients(&self.table, index);
        let within = (c3 / 4.0)
            .mul_add(t, c2 / 3.0)
            .mul_add(t, c1 / 2.0)
            .mul_add(t, c0)
            * t;
        self.step.mul_add(within, self.integrals[index])
    }

    /// The plate's mean swing over the step from `last` to `grid` volts:
    /// its antiderivative anti-aliased form.
    fn smoothed(&self, grid: f64, last: f64) -> f64 {
        let step = grid - last;
        if step.abs() > 1e-6 {
            (self.integral(grid) - self.integral(last)) / step
        } else {
            self.plate(0.5 * (grid + last))
        }
    }
}

/// The share of the voltage above the cathode that reaches a conducting
/// grid, past the grid stopper and the source.
const CONDUCTING_SHARE: f64 = R_CONDUCTING / (R_SOURCE + R_CONDUCTING);

/// The grid voltage for `node` volts behind the stopper: all of it until
/// the grid reaches the cathode, then only [`CONDUCTING_SHARE`] of the
/// rest.
fn clamp(node: f64, cathode: f64) -> f64 {
    if node <= cathode {
        node
    } else {
        (node - cathode).mul_add(CONDUCTING_SHARE, cathode)
    }
}

/// The antiderivative of [`clamp`].
fn clamp_integral(node: f64, cathode: f64) -> f64 {
    if node <= cathode {
        0.5 * node * node
    } else {
        let over = node - cathode;
        (0.5 * CONDUCTING_SHARE * over)
            .mul_add(over, cathode.mul_add(over, 0.5 * cathode * cathode))
    }
}

/// A grid's coupling capacitor, grid leak and grid conduction. The kink
/// where the grid starts to conduct is anti-aliased by its antiderivative.
#[derive(Debug, Clone, Copy, Default)]
struct Grid {
    /// The voltage across the coupling capacitor.
    charge: f64,
    /// The last voltage behind the capacitor, for the clamp's
    /// antiderivative.
    last: f64,
}

impl Grid {
    /// The voltage the valve's grid sees for `input` volts before the
    /// capacitor. `leak` and `conduct` are the sample period times the
    /// leak and conduction conductances over the capacitance.
    fn tick(
        &mut self,
        input: f64,
        cathode: f64,
        [leak, conduct]: [f64; 2],
        anti_alias: bool,
    ) -> f64 {
        let idle = leak.mul_add(input, self.charge) / (1.0 + leak);
        let mut node = input - idle;
        if node > cathode {
            let driven = conduct.mul_add(input - cathode, leak.mul_add(input, self.charge));
            self.charge = driven / (1.0 + leak + conduct);
            node = (input - self.charge).max(cathode);
        } else {
            self.charge = idle;
        }
        flush64(&mut self.charge);
        let last = self.last;
        self.last = node;
        let step = node - last;
        if !anti_alias {
            clamp(node, cathode)
        } else if step.abs() > 1e-7 {
            (clamp_integral(node, cathode) - clamp_integral(last, cathode)) / step
        } else {
            clamp(0.5 * (node + last), cathode)
        }
    }
}

/// What the knobs mean to the circuit this sample.
#[derive(Debug, Clone, Copy)]
struct Settings {
    gain: f64,
    voice: f64,
    drive: f64,
    headroom: f64,
    anti_alias: bool,
}

/// The per-sample constants of the grid circuits.
#[derive(Debug, Clone, Copy, Default)]
struct GridRates {
    /// The first grid's leak and conduction, each the sample period times
    /// a conductance over the capacitance.
    input: [f64; 2],
    /// The second grid's.
    coupling: [f64; 2],
}

#[derive(Debug, Clone, Copy, Default)]
struct Circuit {
    first: Grid,
    second: Grid,
    fender: Iir3,
    marshall: Iir3,
    transformer: Iir3,
    /// The power stage's last drive, for its antiderivative.
    last_drive: f64,
    /// Each valve's last grid voltage, for its antiderivative.
    last_grid: [f64; 2],
}

impl Circuit {
    fn tick(
        &mut self,
        sample: f32,
        triode: &Triode,
        rates: &GridRates,
        settings: &Settings,
    ) -> f32 {
        let cathode = triode.cathode;
        let volts = f64::from(sample) * INPUT_VOLTS;
        let anti_alias = settings.anti_alias;
        let grid = self.first.tick(volts, cathode, rates.input, anti_alias);
        let plate = self.valve(0, triode, grid, anti_alias) * settings.gain;
        let grid = self.second.tick(plate, cathode, rates.coupling, anti_alias);
        let plate = self.valve(1, triode, grid, anti_alias);
        let fender = self.fender.process(plate);
        let marshall = self.marshall.process(plate);
        let toned = settings.voice.mul_add(marshall - fender, fender);
        let drive = toned * POWER_SCALE * settings.drive / settings.headroom;
        let power = settings.headroom * self.power(drive, anti_alias);
        self.transformer.process(power) as f32
    }

    /// Valve `index`'s plate swing for `grid`, anti-aliased by its
    /// antiderivative unless told not to.
    fn valve(&mut self, index: usize, triode: &Triode, grid: f64, anti_alias: bool) -> f64 {
        let last = self.last_grid[index];
        self.last_grid[index] = grid;
        if anti_alias {
            triode.smoothed(grid, last)
        } else {
            triode.plate(grid)
        }
    }

    /// The push-pull stage's saturation, `tanh`, anti-aliased by its
    /// antiderivative `ln cosh`.
    fn power(&mut self, drive: f64, anti_alias: bool) -> f64 {
        let last = self.last_drive;
        self.last_drive = drive;
        let step = drive - last;
        if !anti_alias {
            drive.tanh()
        } else if step.abs() > 1e-7 {
            (log_cosh(drive) - log_cosh(last)) / step
        } else {
            (0.5 * (drive + last)).tanh()
        }
    }

    fn design_stacks(&mut self, treble: f64, middle: f64, bass: f64, c: f64) {
        // Like real pots, none of the three quite reaches zero ohms at an
        // end. That keeps the stack third order at every setting, so a
        // knob leaving its end never changes the filter's structure.
        let treble = treble.clamp(POT_END, 1.0 - POT_END);
        let middle = middle.clamp(POT_END, 1.0 - POT_END);
        let bass = audio_taper(bass).max(POT_END);
        self.fender
            .design(&tone_stack(&FENDER, treble, middle, bass), c);
        self.marshall
            .design(&tone_stack(&MARSHALL, treble, middle, bass), c);
    }
}

impl pair::Circuit for Circuit {
    fn reset(&mut self) {
        self.first = Grid::default();
        self.second = Grid::default();
        self.fender.reset();
        self.marshall.reset();
        self.transformer.reset();
        self.last_drive = 0.0;
        self.last_grid = [0.0; 2];
    }
}

/// The cabinet, at the base rate.
#[derive(Debug, Clone, Copy, Default)]
struct Cabinet {
    sections: [Iir3; 5],
}

impl Cabinet {
    fn design(&mut self, rate: f64) {
        let c = 2.0 * rate;
        let designs = [
            Analogue::highpass2(warp(90.0, rate), 1.1),
            Analogue::peak(warp(450.0, rate), 0.9, 10f64.powf(-4.0 / 20.0)),
            Analogue::peak(warp(2_400.0, rate), 1.2, 10f64.powf(3.0 / 20.0)),
            Analogue::lowpass2(warp(5_000.0, rate), 0.54),
            Analogue::lowpass2(warp(5_000.0, rate), 1.31),
        ];
        for (section, design) in self.sections.iter_mut().zip(&designs) {
            section.design(design, c);
        }
    }

    fn process(&mut self, sample: f64) -> f64 {
        self.sections
            .iter_mut()
            .fold(sample, |signal, section| section.process(signal))
    }

    fn reset(&mut self) {
        for section in &mut self.sections {
            section.reset();
        }
    }
}

/// A valve amp. See the module documentation for the circuit.
#[derive(Debug)]
pub struct Amp {
    knobs: Knobs<9>,
    triode: Triode,
    rates: GridRates,
    /// The treble, middle and bass the stacks were last designed for.
    retune: Retune<3>,
    /// How far the supply has sagged: the power amp's recent output.
    sag: f64,
    sag_attack: f64,
    sag_release: f64,
    pair: Pair<Circuit>,
    cabinets: [Cabinet; 2],
}

impl Default for Amp {
    fn default() -> Self {
        Self::new()
    }
}

impl Amp {
    /// An amp at its default settings, ready for 48 kHz until prepared.
    /// Solves the valve's load line, so it allocates.
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// The same circuit run at the base rate, for the aliasing tests.
    #[cfg(test)]
    #[must_use]
    pub fn naive() -> Self {
        Self::build(false)
    }

    fn build(anti_alias: bool) -> Self {
        let mut amp = Self {
            knobs: Knobs::new(&PARAMS),
            triode: Triode::new(),
            rates: GridRates::default(),
            retune: Retune::new([
                PARAMS[TREBLE].default,
                PARAMS[MIDDLE].default,
                PARAMS[BASS].default,
            ]),
            sag: 0.0,
            sag_attack: 0.0,
            sag_release: 0.0,
            pair: Pair::new(anti_alias),
            cabinets: [Cabinet::default(); 2],
        };
        amp.prepare(DEFAULT_RATE);
        amp
    }

    fn design_stacks(&mut self, [treble, middle, bass]: [f32; 3]) {
        let c = 2.0 * self.pair.rate();
        for circuit in self.pair.circuits() {
            circuit.design_stacks(fraction(treble), fraction(middle), fraction(bass), c);
        }
    }

    /// Design for where the knobs are now, with nothing left waiting:
    /// after a prepare or a reset.
    fn retune_now(&mut self) {
        let knob = self.knobs.values();
        let now = [knob[TREBLE], knob[MIDDLE], knob[BASS]];
        self.retune.settle(now);
        self.design_stacks(now);
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        let stack = [knob[TREBLE], knob[MIDDLE], knob[BASS]];
        if self.retune.due(stack) {
            self.design_stacks(stack);
        }
        let master_db = MASTER_RANGE_DB.mul_add(fraction(knob[MASTER]), MASTER_LOW_DB);
        let settings = Settings {
            gain: audio_taper(fraction(knob[GAIN])).max(GAIN_FLOOR),
            voice: f64::from(weight(knob[VOICE], 1.0)),
            drive: 10f64.powf(master_db / 20.0),
            headroom: 1.0 / (SAG_DEPTH * fraction(knob[SAG])).mul_add(self.sag, 1.0),
            anti_alias: self.pair.anti_alias(),
        };
        let cab = f64::from(weight(knob[CAB], 1.0));
        let out_gain = db_gain(knob[LEVEL]) * OUTPUT_SCALE;
        let triode = &self.triode;
        let rates = &self.rates;
        let power = self
            .pair
            .process([left, right], |_, circuit, x| {
                circuit.tick(x, triode, rates, &settings)
            })
            .map(f64::from);
        let loudest = power[0].abs().max(power[1].abs());
        let rate = if loudest > self.sag {
            self.sag_attack
        } else {
            self.sag_release
        };
        self.sag = (loudest - self.sag).mul_add(rate, self.sag);
        flush64(&mut self.sag);
        let [first, second] = &mut self.cabinets;
        [(first, power[0]), (second, power[1])].map(|(cabinet, power)| {
            let speaker = cabinet.process(power);
            let voiced = cab.mul_add(speaker - power, power);
            ceiling((voiced * out_gain) as f32)
        })
    }
}

impl Effect for Amp {
    fn prepare(&mut self, sample_rate: f32) {
        let host = sane_rate(sample_rate);
        let host_rate = f64::from(host);
        let rate = self.pair.prepare(host, INTERNAL_RATE, ADAA_DELAY);
        let period = 1.0 / rate;
        let conducting = R_SOURCE + R_CONDUCTING;
        self.rates = GridRates {
            input: [period / (R_LEAK * C_INPUT), period / (conducting * C_INPUT)],
            coupling: [
                period / (R_LEAK * C_COUPLING),
                period / (conducting * C_COUPLING),
            ],
        };
        self.sag_attack = one_pole(SAG_ATTACK, host_rate);
        self.sag_release = one_pole(SAG_RELEASE, host_rate);
        self.knobs.prepare(host);
        let c = 2.0 * rate;
        for circuit in self.pair.circuits() {
            circuit
                .transformer
                .design(&Analogue::highpass1(TAU * TRANSFORMER_HZ), c);
        }
        for cabinet in &mut self.cabinets {
            cabinet.design(host_rate);
        }
        self.retune_now();
        self.reset();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.retune_now();
        self.sag = 0.0;
        self.pair.reset();
        for cabinet in &mut self.cabinets {
            cabinet.reset();
        }
    }

    fn latency(&self) -> usize {
        self.pair.latency()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        for_each_frame(input, output, |left, right| self.frame(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::oversample::factor_for;
    use super::super::testkit::{
        Limits, aliasing_db, built, conformance, db, guitar, render_mono, rms, sine, subharmonic_db,
    };
    use super::*;

    /// The amp flat out, with the cabinet out of the way.
    const HOT: &[(usize, f32)] = &[(GAIN, 100.0), (MASTER, 100.0), (CAB, 0.0)];
    /// Flat out, the pitch that folds onto half itself shows the amp's
    /// aliasing floor there; held to the family's hot target.
    const HOT_SUBHARMONIC_DB: f64 = -48.0;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: Some(-80.0),
            hot: &[HOT],
            hot_spur_db: Some(-48.0),
            unity: true,
            rough_glides: &[],
            // The cabinet runs at the host's rate and its low end leads
            // the analogue speaker's by a few microseconds there (see
            // the module documentation): its sound, not the latency.
            latency_knobs: &[(CAB, 0.0)],
        }
    );

    #[test]
    fn oversampling_cuts_the_aliasing() {
        let setup = |mut amp: Amp| {
            amp.set_param(GAIN, 80.0);
            amp.set_param(CAB, 0.0);
            amp.reset();
            amp
        };
        let naive = aliasing_db(&mut setup(Amp::naive()), 4_999.0, 0.3);
        let proper = aliasing_db(&mut setup(Amp::new()), 4_999.0, 0.3);
        assert!(
            proper < naive - 20.0,
            "naive {naive:.1} dB, oversampled {proper:.1} dB"
        );
    }

    /// No period doubling at any host rate, pitched where it would show.
    #[test]
    fn no_subharmonics() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let internal = f64::from(rate) * factor_for(rate, INTERNAL_RATE) as f64;
            let level = subharmonic_db(&KIND, rate, internal, &[]);
            assert!(level < -90.0, "at {rate} Hz: {level:.1} dBc");
            let hot = subharmonic_db(&KIND, rate, internal, HOT);
            assert!(
                hot < HOT_SUBHARMONIC_DB,
                "flat out at {rate} Hz: {hot:.1} dBc"
            );
        }
    }

    #[test]
    fn the_valve_stage_has_gain_and_rests_sensibly() {
        let triode = Triode::new();
        assert!((0.5..2.0).contains(&triode.cathode), "{}", triode.cathode);
        let slope = (triode.plate(0.01) - triode.plate(-0.01)) / 0.02;
        assert!((-80.0..-40.0).contains(&slope), "gain {slope}");
        assert!(triode.plate(GRID_LOW) > 50.0);
        assert!(triode.plate(GRID_HIGH) < -50.0);
        // By the table's top the plate has bottomed out, so carrying it on
        // in a straight line past the end leaves no corner.
        let top = (triode.plate(GRID_HIGH) - triode.plate(GRID_HIGH - 0.1)) / 0.1;
        let past = (triode.plate(GRID_HIGH + 1.0) - triode.plate(GRID_HIGH)) / 1.0;
        assert!(
            top.abs() < 1.0 && (top - past).abs() < 0.1,
            "{top} then {past}"
        );
        assert!((triode.plate(GRID_LOW - 1.0) - triode.plate(GRID_LOW)).abs() < 1e-3);
    }

    /// The reviewer's case: a clean amp with the middle full up and the
    /// bass swept from its end, where the stack's cubic terms used to fall
    /// to nothing and the filter's structure changed under the note.
    #[test]
    fn sweeping_the_bass_off_its_end_does_not_click() {
        let input = sine(110.0, 0.3, 0.4);
        let jump = input.len() / 2;
        let bend = |samples: &[f32]| {
            samples.windows(3).fold(0.0f32, |m, w| {
                m.max((2.0f32.mul_add(-w[1], w[2]) + w[0]).abs())
            })
        };
        let mut amp = built(&KIND);
        amp.set_param(GAIN, 0.0);
        amp.set_param(CAB, 0.0);
        amp.set_param(MIDDLE, 100.0);
        amp.set_param(BASS, 0.0);
        amp.reset();
        let before = render_mono(&mut *amp, &input[..jump]);
        amp.set_param(BASS, 100.0);
        let after = render_mono(&mut *amp, &input[jump..]);
        let rest = bend(&before[jump - 2_400..]).max(bend(&after[after.len() - 2_400..]));
        let during = bend(&after[..2_400]);
        assert!(
            during <= 2.0f32.mul_add(rest, 1e-4),
            "bend {during} against rest {rest}"
        );
    }

    #[test]
    fn the_valve_integral_matches_its_curve() {
        let triode = Triode::new();
        let h = 1e-4;
        for i in -150..=150 {
            let grid = f64::from(i) * 0.1;
            let slope = (triode.integral(grid + h) - triode.integral(grid - h)) / (2.0 * h);
            let plate = triode.plate(grid);
            assert!(
                (slope - plate).abs() < 1e-3 * plate.abs().max(1.0),
                "{grid}: {slope} vs {plate}"
            );
        }
    }

    /// A complex number, just enough to solve a small network.
    #[derive(Debug, Clone, Copy)]
    struct Complex(f64, f64);

    impl Complex {
        const ZERO: Self = Self(0.0, 0.0);

        fn add(self, other: Self) -> Self {
            Self(self.0 + other.0, self.1 + other.1)
        }
        fn sub(self, other: Self) -> Self {
            Self(self.0 - other.0, self.1 - other.1)
        }
        fn mul(self, other: Self) -> Self {
            Self(
                self.0.mul_add(other.0, -self.1 * other.1),
                self.0.mul_add(other.1, self.1 * other.0),
            )
        }
        fn div(self, other: Self) -> Self {
            let size = other.0.mul_add(other.0, other.1 * other.1);
            Self(
                self.0.mul_add(other.0, self.1 * other.1) / size,
                self.1.mul_add(other.0, -self.0 * other.1) / size,
            )
        }
        fn size(self) -> f64 {
            self.0.hypot(self.1)
        }
    }

    /// The tone stack of Yeh and Smith's figure 1 solved as a network by
    /// nodal analysis: nodes are the treble pot's top, its wiper (the
    /// output), its bottom, the bass pot's far end (the middle pot's top),
    /// the slope resistor's far end and the middle pot's wiper.
    fn network(parts: &StackParts, treble: f64, middle: f64, bass: f64, hz: f64) -> f64 {
        let jw = Complex(0.0, TAU * hz);
        let resistor = |ohms: f64| Complex(1.0 / (ohms + 1e-3), 0.0);
        let capacitor = |farads: f64| jw.mul(Complex(farads, 0.0));
        let mut matrix = [[Complex::ZERO; 6]; 6];
        let mut source = [Complex::ZERO; 6];
        let mut link = |from: usize, to: Option<usize>, admittance: Complex| {
            matrix[from][from] = matrix[from][from].add(admittance);
            if let Some(to) = to {
                matrix[to][to] = matrix[to][to].add(admittance);
                matrix[from][to] = matrix[from][to].sub(admittance);
                matrix[to][from] = matrix[to][from].sub(admittance);
            }
        };
        let (top, wiper, bottom, junction, slope, middle_wiper) = (0, 1, 2, 3, 4, 5);
        link(top, None, capacitor(parts.c1));
        link(top, Some(wiper), resistor((1.0 - treble) * parts.r1));
        link(wiper, Some(bottom), resistor(treble * parts.r1));
        link(bottom, Some(junction), resistor(bass * parts.r2));
        link(
            junction,
            Some(middle_wiper),
            resistor((1.0 - middle) * parts.r3),
        );
        link(middle_wiper, None, resistor(middle * parts.r3));
        link(slope, None, resistor(parts.r4));
        link(slope, Some(bottom), capacitor(parts.c2));
        link(slope, Some(middle_wiper), capacitor(parts.c3));
        // The input drives C1 and R4 from a 1 V source.
        source[top] = capacitor(parts.c1);
        source[slope] = resistor(parts.r4);
        for col in 0..6 {
            let pivot = (col..6)
                .max_by(|a, b| matrix[*a][col].size().total_cmp(&matrix[*b][col].size()))
                .unwrap_or(col);
            matrix.swap(col, pivot);
            source.swap(col, pivot);
            for row in col + 1..6 {
                let factor = matrix[row][col].div(matrix[col][col]);
                let pivot_row = matrix[col];
                for (cell, above) in matrix[row].iter_mut().zip(pivot_row).skip(col) {
                    *cell = cell.sub(factor.mul(above));
                }
                source[row] = source[row].sub(factor.mul(source[col]));
            }
        }
        let mut volts = [Complex::ZERO; 6];
        for row in (0..6).rev() {
            let known = matrix[row]
                .iter()
                .zip(volts)
                .skip(row + 1)
                .fold(source[row], |sum, (cell, v)| sum.sub(cell.mul(v)));
            volts[row] = known.div(matrix[row][row]);
        }
        volts[wiper].size()
    }

    fn formula(parts: &StackParts, treble: f64, middle: f64, bass: f64, hz: f64) -> f64 {
        let stack = tone_stack(parts, treble, middle, bass);
        let jw = Complex(0.0, TAU * hz);
        let poly = |coefficients: &[f64; 4]| {
            let mut sum = Complex::ZERO;
            let mut power = Complex(1.0, 0.0);
            for coeff in coefficients {
                sum = sum.add(power.mul(Complex(*coeff, 0.0)));
                power = power.mul(jw);
            }
            sum
        };
        poly(&stack.b).div(poly(&stack.a)).size()
    }

    #[test]
    fn the_tone_stack_formula_matches_the_network() {
        for parts in [FENDER, MARSHALL] {
            for (t, m, l) in [
                (0.5, 0.5, 0.5),
                (0.1, 0.9, 0.3),
                (0.9, 0.2, 0.8),
                (0.3, 0.3, 0.05),
            ] {
                for hz in [40.0, 200.0, 700.0, 2_000.0, 8_000.0] {
                    let net = db(network(&parts, t, m, l, hz));
                    let closed = db(formula(&parts, t, m, l, hz));
                    assert!(
                        (net - closed).abs() < 0.05,
                        "{parts:?} t{t} m{m} l{l} at {hz} Hz: network {net:.2} dB, formula {closed:.2} dB"
                    );
                }
            }
        }
    }

    #[test]
    fn the_stack_knobs_do_what_they_say() {
        let at = |index: usize, value: f32, hz: f64| {
            let mut amp = built(&KIND);
            amp.set_param(GAIN, 0.0);
            amp.set_param(CAB, 0.0);
            amp.set_param(index, value);
            rms(&render_mono(&mut *amp, &sine(hz, 0.01, 0.4))[9_600..])
        };
        assert!(db(at(BASS, 100.0, 80.0) / at(BASS, 0.0, 80.0)) > 6.0);
        assert!(db(at(TREBLE, 100.0, 5_000.0) / at(TREBLE, 0.0, 5_000.0)) > 6.0);
        assert!(db(at(MIDDLE, 100.0, 600.0) / at(MIDDLE, 0.0, 600.0)) > 6.0);
    }

    #[test]
    fn sag_squashes_hard_playing() {
        let input = guitar(2.0, 0.8);
        let at = |sag: f32| {
            let mut amp = built(&KIND);
            amp.set_param(MASTER, 100.0);
            amp.set_param(SAG, sag);
            rms(&render_mono(&mut *amp, &input))
        };
        assert!(db(at(100.0) / at(0.0)) < -1.0);
    }
}
