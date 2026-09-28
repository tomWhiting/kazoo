//! Fold: West Coast wavefolding, as the Buchla 259's timbre circuit or as
//! a Serge-style cascade of transistor folders.
//!
//! A folder does not clip a loud wave: it turns it back on itself, so each
//! peak that crosses a threshold comes back down, and pushing harder adds
//! folds, and bright, vocal harmonics, rather than just squaring up. Both
//! models follow published analyses of the real circuits.
//!
//! - **Buchla**: the 259's timbre section, after Esqueda, Pöntynen,
//!   Välimäki and Parker. Five op-amp folding cells sit in parallel with a
//!   direct path. Each cell holds its output at zero until the input passes
//!   `R1 / R2 × 6 V` (its op-amp saturates on its 6 V supply), then follows
//!   the input with slope `R3 R2 / (R1 R3 + R2 R3 + R1 R2)`. The cells come
//!   in at 0.6, 1.8, 2.994, 4.08 and 5.46 V, and the two mixing amplifiers
//!   weigh them -12, +17.65, -27.78, +36.36 and -21.43 against the direct
//!   path's +5: the published resistor values, computed here from the
//!   table rather than copied. The sum is a piecewise-linear curve whose
//!   folds are uneven; past 12 V (the input buffer's rails) it is held
//!   flat. The mixing amplifier's 100 pF across 1.2 MΩ makes a fixed
//!   lowpass at 1.33 kHz, part of the 259's voice.
//! - **Serge**: Esqueda et al. show the Serge and Lockhart folders fold
//!   the same way, and give the Lockhart's transistor pair a closed form
//!   with the Lambert W function:
//!   `Vout = λ Vt W(Δ e^(λ β Vin)) - α Vin`, with `λ = sgn(Vin)`,
//!   `α = 2 RL / R`, `β = (R + 2 RL) / (Vt R)` and `Δ = RL Is / Vt`
//!   (R = 15 kΩ, RL = 7.5 kΩ, Is = 10 aA, Vt = 26 mV). One such stage
//!   follows the input up to 0.36 V and then turns back. Four in a row,
//!   each behind a gain of 1/4 and followed by a gain of 4 so that every
//!   stage folds at the same point, give the multi-fold Serge sound; an
//!   output buffer saturates softly (`tanh`). The inverted output of each
//!   stage is turned back up the right way.
//!
//! `folds` sets the input drive, from 1 V to 10 V for a full-scale input.
//! A folder's loudness swings about with its drive (it is bounded, so more
//! drive means more folds, not more level), so a fixed makeup brings it
//! back: a table over `folds`, `symmetry` and the model, worked out when
//! the effect is built from each curve's loudness for a -12 dBFS sine
//! across four octaves of notes, within ±24 dB. It depends only on the
//! knobs, never on the signal, so a hard pick after quiet playing keeps
//! its attack. Adding folds changes the timbre, not the level. `symmetry` adds
//! up to ±5 V of offset before the folder, which folds the two halves of
//! the wave differently and brings in even harmonics; the offset is taken
//! out again afterwards (a 10 Hz highpass).
//!
//! Folding makes harmonics without limit, so both folders are anti-aliased
//! twice over. They run at an internal rate of at least 705.6 kHz, four
//! times the family's usual target (16x at 44.1 and 48 kHz, 8x at 88.2
//! and 96 kHz, 4x at 176.4 and 192 kHz; lower only below 44.1 kHz, where
//! the 16x cap bites): the 259 at full folds turns a -6 dBFS tone near
//! 5 kHz over about ten times a cycle, through cells weighted up to 36,
//! and from 352.8 kHz still folds a spur back at -45 dBc; from 705.6 kHz
//! the worst is -64 dBc. And
//! every folding stage is evaluated by its antiderivative (first-order
//! ADAA): its output is the stage's mean over the step from the last input
//! to this one, `(F(x) - F(x')) / (x - x')`. For the Buchla cells
//! `F(x) = 5x²/2 + Σ gₙ max(|x| - Tₙ, 0)² / 2`; for a Lockhart stage
//! `F = Vt / (2β) (1 + W)² - α Vin² / 2`; the Serge's output buffer is
//! anti-aliased the same way, through `ln cosh`. Each such stage delays
//! the signal by half a sample: five on the Serge path, one on the
//! Buchla's, which therefore also waits two whole samples, so the two
//! models crossfade in phase. The dry signal is put through the same
//! delays so the mix stays in phase too.
//!
//! Sources: F. Esqueda, H. Pöntynen, V. Välimäki and J. D. Parker, "Virtual
//! analog Buchla 259 wavefolder" (DAFX 2017); F. Esqueda, H. Pöntynen,
//! J. D. Parker and S. Bilbao, "Virtual analog model of the Lockhart
//! wavefolder" (SMC 2017) and "Virtual analog models of the Lockhart and
//! Serge wavefolders" (Applied Sciences, 2017); J. D. Parker, V. Zavalishin
//! and E. Le Bivic, "Reducing the aliasing of nonlinear waveshaping using
//! continuous-time convolution" (DAFX 2016).

use std::f64::consts::TAU;

use super::filter::{Analogue, Iir3};
use super::kit::{
    DEFAULT_RATE, Knobs, ceiling, for_each_frame, fraction, gain as db_gain, log_cosh, weight,
};
use super::oversample::TARGET_RATE;
use super::pair::{self, Pair};
use super::solve::rising_root_within;
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const FOLDS: usize = 0;
const SYMMETRY: usize = 1;
const MODEL: usize = 2;
const MIX: usize = 3;
const LEVEL: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "folds",
        min: 0.0,
        max: 100.0,
        default: 40.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "symmetry",
        min: -100.0,
        max: 100.0,
        default: 0.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["buchla", "serge"],
        },
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 100.0,
        default: 100.0,
        unit: "%",
        curve: Curve::Linear,
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

/// The Fold's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "fold",
    name: "Wavefolder",
    description: "West Coast wavefolding: the Buchla 259's timbre circuit or a Serge-style cascade of transistor folders, with symmetry, anti-aliased twice over.",
    params: &PARAMS,
    build: || Box::new(Fold::new()),
};

/// Volts into the folder for a full-scale input, at the bottom and top of
/// the `folds` knob.
const DRIVE_LOW: f64 = 1.0;
const DRIVE_HIGH: f64 = 10.0;
/// The offset at full `symmetry`, in volts.
const MAX_OFFSET: f64 = 5.0;
const BLOCK_HZ: f64 = 10.0;
/// Steps closer than this (in volts) are evaluated directly, not by
/// antiderivative.
const TINY_STEP: f64 = 1e-7;

// The Buchla 259's timbre circuit, from Esqueda et al.'s table 1.
/// The folding cells' op-amp supply.
const CELL_SUPPLY: f64 = 6.0;
/// Each cell's R1, R2, R3 and the resistor it is mixed through, and
/// whether it reaches the output through the second (inverting) mixer.
const CELL_PARTS: [(f64, f64, f64, f64, bool); 5] = [
    (10e3, 100e3, 100e3, 100e3, false),
    (49.9e3, 100e3, 43.2e3, 43.2e3, false),
    (91e3, 100e3, 56e3, 56e3, false),
    (30e3, 100e3, 68e3, 68e3, true),
    (68e3, 100e3, 33e3, 33e3, true),
];
/// The mixing amplifiers' feedback resistors, and the resistor between
/// them.
const RF1: f64 = 24.9e3;
const RF2: f64 = 1.2e6;
const R7: f64 = 24.9e3;
/// The direct path's mixing resistor.
const R_DIRECT: f64 = 240e3;
/// The mixer's integrating capacitor.
const C_MIX: f64 = 100e-12;
/// The most the makeup will lift or cut: 24 dB.
const MAX_MAKEUP: f64 = 16.0;
/// The makeup table's grid: `folds` from 0 to 100 and `symmetry` from
/// -100 to 100, evenly spaced.
const MAKEUP_FOLDS: usize = 21;
const MAKEUP_SYMMETRY: usize = 11;
/// The level the makeup is set for: a sine at -12 dBFS, sampled this many
/// times a cycle.
const MAKEUP_LEVEL: f64 = 0.25;
const MAKEUP_POINTS: usize = 128;
/// Where the input buffer's rails hold the folder's input.
const INPUT_RAIL: f64 = 12.0;
/// Output volts per full scale for the Buchla: its small-signal gain is 5.
const BUCHLA_SCALE: f64 = 0.1617;

/// One Buchla cell: where it comes in, and the slope it adds there once
/// mixed. Resistor values are as in the paper's table.
const fn cell((r1, r2, r3, mixing, second): (f64, f64, f64, f64, bool)) -> (f64, f64) {
    let threshold = r1 / r2 * CELL_SUPPLY;
    let slope = r3 * r2 / (r1 * r3 + r2 * r3 + r1 * r2);
    let weight = if second {
        RF2 * RF1 / (R7 * mixing)
    } else {
        -RF2 / mixing
    };
    (threshold, slope * weight)
}

/// The direct path's slope through both mixers.
const DIRECT: f64 = RF2 * RF1 / (R7 * R_DIRECT);

/// The Buchla's cells, plus the input rail as a last "cell" that cancels
/// all slope beyond it.
fn buchla_cells() -> [(f64, f64); 6] {
    let mut cells = [(0.0, 0.0); 6];
    let mut total = DIRECT;
    for (slot, parts) in cells.iter_mut().zip(CELL_PARTS) {
        *slot = cell(parts);
        total += slot.1;
    }
    cells[5] = (INPUT_RAIL, -total);
    cells
}

/// The Buchla's mixed output for input `volts`.
fn buchla(cells: &[(f64, f64); 6], volts: f64) -> f64 {
    let size = volts.abs();
    let folded = cells.iter().fold(DIRECT * size, |sum, (threshold, gain)| {
        gain.mul_add((size - threshold).max(0.0), sum)
    });
    folded * volts.signum()
}

/// The antiderivative of [`buchla`].
fn buchla_integral(cells: &[(f64, f64); 6], volts: f64) -> f64 {
    let size = volts.abs();
    cells
        .iter()
        .fold(0.5 * DIRECT * size * size, |sum, (threshold, gain)| {
            let over = (size - threshold).max(0.0);
            (0.5 * gain).mul_add(over * over, sum)
        })
}

// The Lockhart stage, from Esqueda et al.'s table 1.
const R_EMITTER: f64 = 15e3;
const R_LOAD: f64 = 7.5e3;
const SATURATION_CURRENT: f64 = 1e-17;
const LOCKHART_VT: f64 = 0.026;
const ALPHA: f64 = 2.0 * R_LOAD / R_EMITTER;
const BETA: f64 = (R_EMITTER + 2.0 * R_LOAD) / (LOCKHART_VT * R_EMITTER);
/// `ln Δ`, `Δ = RL Is / Vt`, computed at build.
fn log_delta() -> f64 {
    (R_LOAD * SATURATION_CURRENT / LOCKHART_VT).ln()
}
/// The gain before and after each stage in the cascade.
const PRE: f64 = 0.25;
const POST: f64 = 4.0;
const STAGES: usize = 4;
/// Antiderivative stages on the Serge path: the four folders and the
/// output buffer.
const SERGE_STAGES: usize = STAGES + 1;
/// Both paths' delay at the folder's own rate: the Serge's five half
/// samples, and the Buchla's one half sample plus two whole ones.
const FOLD_DELAY: f64 = 2.5;
/// Output volts per full scale for the Serge cascade, after its buffer.
const SERGE_SCALE: f64 = 1.0;
/// The Lambert W is solved to this.
const W_TOLERANCE: f64 = 1e-12;

/// `W(e^u)`, the Lambert W of an exponential, solved in the log domain
/// (`w + ln w = u`) so a large argument never overflows. `guess` is a
/// starting point, usually the last answer.
fn lambert_w_exp(u: f64, guess: f64) -> f64 {
    if u < -40.0 {
        // W(x) is x to within x² this far down.
        return u.exp();
    }
    let (low, high) = if u > 30.0 {
        (1.0, u)
    } else {
        let x = u.exp();
        (x / (1.0 + x), x.ln_1p())
    };
    rising_root_within(low, high, guess, W_TOLERANCE, |w| {
        (w + w.ln() - u, 1.0 + 1.0 / w)
    })
}

/// One Lockhart stage, turned the right way up: its output and its
/// antiderivative for input `volts` (after the pre-gain), and the W it
/// solved, which seeds the next solve.
fn lockhart(volts: f64, guess: f64, log_delta: f64) -> (f64, f64, f64) {
    let side = if volts < 0.0 { -1.0 } else { 1.0 };
    let w = lambert_w_exp((side * BETA).mul_add(volts, log_delta), guess);
    let out = (side * LOCKHART_VT).mul_add(-w, ALPHA * volts);
    let one_w = 1.0 + w;
    let integral =
        (0.5 * ALPHA * volts).mul_add(volts, -LOCKHART_VT / (2.0 * BETA) * one_w * one_w);
    (out, integral, w)
}

/// Each model's static curve, as heard after the output stage, for input
/// `volts`: the Buchla's mixer and the Serge's cascade and buffer. The
/// mixer's gentle lowpass and the DC block are left out; the makeup only
/// needs the loudness.
fn static_curve(serge: bool, cells: &[(f64, f64); 6], log_delta: f64, volts: f64) -> f64 {
    if serge {
        let mut signal = volts;
        let mut guess = 0.0;
        for _ in 0..STAGES {
            let (out, _, w) = lockhart(PRE * signal, guess, log_delta);
            guess = w;
            signal = POST * out;
        }
        signal.tanh() * SERGE_SCALE
    } else {
        buchla(cells, volts) * BUCHLA_SCALE
    }
}

/// The makeup, in dB, for each model over the knob grid: the gain that
/// brings a sine at [`MAKEUP_LEVEL`] back to its own loudness once folded,
/// with the folded signal's DC taken away as the output's block does. It
/// depends only on the knobs, so a hard pick after quiet playing keeps its
/// attack.
/// The makeup table, worked out once for every folder built.
static MAKEUP: std::sync::OnceLock<[[[f64; MAKEUP_SYMMETRY]; MAKEUP_FOLDS]; 2]> =
    std::sync::OnceLock::new();

fn makeup_table() -> [[[f64; MAKEUP_SYMMETRY]; MAKEUP_FOLDS]; 2] {
    let cells = buchla_cells();
    let log_delta = log_delta();
    let mut table = [[[0.0; MAKEUP_SYMMETRY]; MAKEUP_FOLDS]; 2];
    let reference = MAKEUP_LEVEL / std::f64::consts::SQRT_2;
    for (model, grid) in table.iter_mut().enumerate() {
        for (row, cells_row) in grid.iter_mut().enumerate() {
            let folds = row as f64 / (MAKEUP_FOLDS - 1) as f64;
            let drive = (DRIVE_HIGH / DRIVE_LOW).powf(folds) * DRIVE_LOW;
            for (column, slot) in cells_row.iter_mut().enumerate() {
                let symmetry = (column as f64 / (MAKEUP_SYMMETRY - 1) as f64).mul_add(2.0, -1.0);
                let offset = MAX_OFFSET * symmetry;
                let mut wave = [0.0; MAKEUP_POINTS];
                for (point, sample) in wave.iter_mut().enumerate() {
                    let phase = TAU * point as f64 / MAKEUP_POINTS as f64;
                    let volts = drive.mul_add(MAKEUP_LEVEL * phase.sin(), offset);
                    *sample = static_curve(model == 1, &cells, log_delta, volts);
                }
                let power = heard_power(&wave, model == 0);
                let gain = if power > 0.0 {
                    (reference / power.sqrt()).clamp(1.0 / MAX_MAKEUP, MAX_MAKEUP)
                } else {
                    MAX_MAKEUP
                };
                *slot = 20.0 * gain.log10();
            }
        }
    }
    table
}

/// The notes the makeup is set for, in Hz: four octaves, from a guitar's
/// low strings to a voice's upper register. The Buchla's mixer
/// lowpass (at 1.3 kHz) rounds off a high note's folded harmonics more
/// than a low one's, so its loudness follows the pitch, as on the module;
/// the makeup answers the loudness across these, not at one of them.
const MAKEUP_NOTES: [f64; 4] = [110.0, 220.0, 440.0, 880.0];

/// The power of one cycle of `wave` without its DC, harmonic by harmonic:
/// through the Buchla's mixer lowpass when `mixed` (the part of the fold
/// the output actually carries), averaged over [`MAKEUP_NOTES`].
fn heard_power(wave: &[f64; MAKEUP_POINTS], mixed: bool) -> f64 {
    let corner = 1.0 / (RF2 * C_MIX) / TAU;
    let count = MAKEUP_POINTS as f64;
    let harmonics: Vec<f64> = (1..MAKEUP_POINTS / 2)
        .map(|harmonic| {
            let (mut re, mut im) = (0.0, 0.0);
            for (point, sample) in wave.iter().enumerate() {
                let phase = TAU * (harmonic * point) as f64 / count;
                re = sample.mul_add(phase.cos(), re);
                im = sample.mul_add(phase.sin(), im);
            }
            // A real signal's harmonic carries twice its bin's power.
            2.0 * re.mul_add(re, im * im) / (count * count)
        })
        .collect();
    if !mixed {
        return harmonics.iter().sum();
    }
    let through = |note: f64| {
        harmonics
            .iter()
            .enumerate()
            .map(|(below, power)| {
                let ratio = note * (below + 1) as f64 / corner;
                power / ratio.mul_add(ratio, 1.0)
            })
            .sum::<f64>()
    };
    MAKEUP_NOTES.iter().map(|note| through(*note)).sum::<f64>() / MAKEUP_NOTES.len() as f64
}

/// What the knobs mean to the folder this sample.
#[derive(Debug, Clone, Copy)]
struct Settings {
    drive: f64,
    /// The fixed gain that brings the folded signal back to the input's
    /// loudness at these knob settings.
    makeup: f64,
    offset: f64,
    serge: f64,
    mix: f64,
    anti_alias: bool,
}

/// A Lockhart stage's memory for its antiderivative.
#[derive(Debug, Clone, Copy, Default)]
struct Stage {
    last: f64,
    last_integral: f64,
    last_w: f64,
}

#[derive(Debug, Clone, Copy)]
struct Folder {
    cells: [(f64, f64); 6],
    log_delta: f64,
    /// The last input to the Buchla, after drive and offset, and its
    /// antiderivative there.
    last: f64,
    last_integral: f64,
    stages: [Stage; STAGES],
    /// The last input to the Serge's output buffer, for its antiderivative.
    last_buffer: f64,
    /// The dry signal and its running half-sample averages, one per
    /// antiderivative stage of the Serge path.
    dry: [f64; SERGE_STAGES + 1],
    /// The Buchla's output, and the dry signal averaged once, over the last
    /// two samples: the Buchla path waits two samples so both paths are
    /// delayed alike.
    buchla_behind: [f64; 2],
    dry_behind: [f64; 2],
    /// Whether the Serge cascade has been skipped (its share was nothing)
    /// and must pick up the current input before it is heard again.
    serge_stale: bool,
    mixer: Iir3,
    block: Iir3,
}

impl Default for Folder {
    fn default() -> Self {
        Self {
            cells: buchla_cells(),
            log_delta: log_delta(),
            last: 0.0,
            last_integral: 0.0,
            stages: [Stage::default(); STAGES],
            last_buffer: 0.0,
            dry: [0.0; SERGE_STAGES + 1],
            buchla_behind: [0.0; 2],
            dry_behind: [0.0; 2],
            serge_stale: true,
            mixer: Iir3::new(),
            block: Iir3::new(),
        }
    }
}

impl pair::Circuit for Folder {
    fn reset(&mut self) {
        self.last = 0.0;
        self.last_integral = 0.0;
        for stage in &mut self.stages {
            *stage = Stage::default();
            let (_, integral, w) = lockhart(0.0, 0.0, self.log_delta);
            stage.last_integral = integral;
            stage.last_w = w;
        }
        self.last_buffer = 0.0;
        self.dry = [0.0; SERGE_STAGES + 1];
        self.buchla_behind = [0.0; 2];
        self.dry_behind = [0.0; 2];
        self.serge_stale = true;
        self.mixer.reset();
        self.block.reset();
    }
}

impl Folder {
    /// The dry signal through each number of half-sample averages, and
    /// the averages moved on by one sample.
    fn dry(&mut self, dry: f64, anti_alias: bool) -> [f64; SERGE_STAGES + 1] {
        if !anti_alias {
            return [dry; SERGE_STAGES + 1];
        }
        let mut delayed = [dry; SERGE_STAGES + 1];
        for k in 1..=SERGE_STAGES {
            delayed[k] = 0.5 * (delayed[k - 1] + self.dry[k - 1]);
        }
        self.dry = delayed;
        delayed
    }

    fn buchla(&mut self, volts: f64, anti_alias: bool) -> f64 {
        let integral = buchla_integral(&self.cells, volts);
        let step = volts - self.last;
        let out = if !anti_alias {
            buchla(&self.cells, volts)
        } else if step.abs() > TINY_STEP {
            (integral - self.last_integral) / step
        } else {
            buchla(&self.cells, 0.5 * (volts + self.last))
        };
        self.last = volts;
        self.last_integral = integral;
        self.mixer.process(out) * BUCHLA_SCALE
    }

    fn serge(&mut self, volts: f64, anti_alias: bool) -> f64 {
        let log_delta = self.log_delta;
        let mut signal = volts;
        for stage in &mut self.stages {
            let input = PRE * signal;
            let (out, integral, w) = lockhart(input, stage.last_w, log_delta);
            let step = input - stage.last;
            let folded = if !anti_alias {
                out
            } else if step.abs() > TINY_STEP {
                (integral - stage.last_integral) / step
            } else {
                lockhart(0.5 * (input + stage.last), w, log_delta).0
            };
            *stage = Stage {
                last: input,
                last_integral: integral,
                last_w: w,
            };
            signal = POST * folded;
        }
        let step = signal - self.last_buffer;
        let buffered = if !anti_alias {
            signal.tanh()
        } else if step.abs() > TINY_STEP {
            (log_cosh(signal) - log_cosh(self.last_buffer)) / step
        } else {
            (0.5 * (signal + self.last_buffer)).tanh()
        };
        self.last_buffer = signal;
        buffered * SERGE_SCALE
    }

    /// `value` two samples late, through `line`.
    const fn behind(line: &mut [f64; 2], value: f64) -> f64 {
        let out = line[1];
        line[1] = line[0];
        line[0] = value;
        out
    }

    fn tick(&mut self, sample: f32, settings: &Settings) -> f32 {
        let dry = f64::from(sample);
        let volts = settings.drive.mul_add(dry, settings.offset);
        let anti_alias = settings.anti_alias;
        let delayed = self.dry(dry, anti_alias);
        let pointed = self.buchla(volts, anti_alias);
        let (pointed, pointed_dry) = if anti_alias {
            (
                Self::behind(&mut self.buchla_behind, pointed),
                Self::behind(&mut self.dry_behind, delayed[1]),
            )
        } else {
            (pointed, dry)
        };
        // The Serge cascade is the costly one: it is only run while it is
        // heard, and picks up where the input is when it comes back.
        let round = if settings.serge > 0.0 {
            if self.serge_stale {
                self.serge(volts, false);
                self.serge_stale = false;
            }
            self.serge(volts, anti_alias)
        } else {
            self.serge_stale = true;
            0.0
        };
        let folded = self
            .block
            .process(settings.serge.mul_add(round - pointed, pointed));
        let wet = folded * settings.makeup;
        let aligned = settings
            .serge
            .mul_add(delayed[SERGE_STAGES] - pointed_dry, pointed_dry);
        settings.mix.mul_add(wet - aligned, aligned) as f32
    }
}

/// A wavefolder. See the module documentation for the circuits.
#[derive(Debug)]
pub struct Fold {
    knobs: Knobs<5>,
    /// The makeup gain, in dB, for each model over the `folds` and
    /// `symmetry` grid. Worked out once, when the folder is built.
    makeup: [[[f64; MAKEUP_SYMMETRY]; MAKEUP_FOLDS]; 2],
    pair: Pair<Folder>,
}

impl Default for Fold {
    fn default() -> Self {
        Self::new()
    }
}

impl Fold {
    /// A wavefolder at its default settings, ready for 48 kHz until
    /// prepared.
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// The same folder run at the base rate without its antiderivatives,
    /// for the aliasing tests.
    #[cfg(test)]
    #[must_use]
    pub fn naive() -> Self {
        Self::build(false)
    }

    fn build(anti_alias: bool) -> Self {
        let mut fold = Self {
            knobs: Knobs::new(&PARAMS),
            makeup: *MAKEUP.get_or_init(makeup_table),
            pair: Pair::new(anti_alias),
        };
        fold.prepare(DEFAULT_RATE);
        fold
    }

    /// The makeup for `model` (0 Buchla, 1 Serge) at these knob values, in
    /// dB, read bilinearly from the table.
    fn makeup_db(&self, model: usize, folds: f32, symmetry: f32) -> f64 {
        let across = |value: f64, cells: usize| {
            let at = value.clamp(0.0, 1.0) * (cells - 1) as f64;
            let low = (at.floor() as usize).min(cells - 2);
            (low, at - low as f64)
        };
        let (row, down) = across(f64::from(folds) / 100.0, MAKEUP_FOLDS);
        let (column, along) = across((f64::from(symmetry) + 100.0) / 200.0, MAKEUP_SYMMETRY);
        let table = &self.makeup[model];
        let near = along.mul_add(
            table[row][column + 1] - table[row][column],
            table[row][column],
        );
        let far = along.mul_add(
            table[row + 1][column + 1] - table[row + 1][column],
            table[row + 1][column],
        );
        down.mul_add(far - near, near)
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        let drive = (DRIVE_HIGH / DRIVE_LOW).powf(fraction(knob[FOLDS])) * DRIVE_LOW;
        let serge = f64::from(weight(knob[MODEL], 1.0));
        let makeup_db = serge.mul_add(
            self.makeup_db(1, knob[FOLDS], knob[SYMMETRY])
                - self.makeup_db(0, knob[FOLDS], knob[SYMMETRY]),
            self.makeup_db(0, knob[FOLDS], knob[SYMMETRY]),
        );
        let settings = Settings {
            drive,
            makeup: 10f64.powf(makeup_db / 20.0),
            offset: MAX_OFFSET * f64::from(knob[SYMMETRY]) / 100.0,
            serge,
            mix: fraction(knob[MIX]),
            anti_alias: self.pair.anti_alias(),
        };
        let out_gain = db_gain(knob[LEVEL]);
        self.pair
            .process([left, right], |_, folder, x| folder.tick(x, &settings))
            .map(|out| ceiling((f64::from(out) * out_gain) as f32))
    }
}

impl Effect for Fold {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        let rate = self.pair.prepare(base, 4.0 * TARGET_RATE, FOLD_DELAY);
        let c = 2.0 * rate;
        self.knobs.prepare(base);
        let mixer = 1.0 / (RF2 * C_MIX);
        for folder in self.pair.circuits() {
            folder.mixer.design(&Analogue::lowpass1(mixer), c);
            folder.block.design(&Analogue::highpass1(TAU * BLOCK_HZ), c);
        }
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.pair.reset();
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
    use super::super::testkit::{
        Limits, aliasing_db, built, conformance, db, power_near, render_mono, rms, sine, spectrum,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: Some(-70.0),
            hot: &[&[(FOLDS, 100.0)], &[(FOLDS, 100.0), (MODEL, 1.0)]],
            hot_spur_db: Some(-48.0),
            unity: true,
            rough_glides: &[],
            latency_knobs: &[],
        }
    );

    #[test]
    fn the_buchla_cells_are_the_published_ones() {
        let cells = buchla_cells();
        let expected = [
            (0.6, -12.0 * 0.8333),
            (2.994, -27.777 * 0.3768),
            (5.46, -21.428 * 0.2829),
            (1.8, 17.647 * 0.5743),
            (4.08, 36.363 * 0.2673),
        ];
        for ((threshold, gain), (want_threshold, want_gain)) in cells.iter().zip(expected) {
            assert!((threshold - want_threshold).abs() < 1e-3, "{threshold}");
            assert!((gain - want_gain).abs() < 0.01, "{gain} vs {want_gain}");
        }
        assert!((DIRECT - 5.0).abs() < 1e-9);
        // Held flat past the input rail.
        assert!((buchla(&cells, 13.0) - buchla(&cells, 20.0)).abs() < 1e-9);
    }

    #[test]
    fn the_antiderivatives_are_right() {
        let cells = buchla_cells();
        let log_delta = log_delta();
        let h = 1e-5;
        for i in -400..400 {
            let x = f64::from(i) * 0.037;
            let slope =
                (buchla_integral(&cells, x + h) - buchla_integral(&cells, x - h)) / (2.0 * h);
            assert!((slope - buchla(&cells, x)).abs() < 1e-5, "buchla {x}");
            let upper = lockhart(x + h, 0.0, log_delta).1;
            let lower = lockhart(x - h, 0.0, log_delta).1;
            let slope = (upper - lower) / (2.0 * h);
            let value = lockhart(x, 0.0, log_delta).0;
            assert!(
                (slope - value).abs() < 1e-5,
                "lockhart {x}: {slope} vs {value}"
            );
        }
    }

    #[test]
    fn a_lockhart_stage_folds_where_the_paper_says() {
        let log_delta = log_delta();
        let at = |v: f64| lockhart(v, 0.0, log_delta).0;
        // Unity near zero, the turn at about 0.36 V, and slope -1 beyond.
        assert!(((at(0.01) - at(-0.01)) / 0.02 - 1.0).abs() < 1e-3);
        let turn = (1..1_000)
            .map(|i| f64::from(i) * 0.001)
            .find(|v| at(v + 0.001) < at(*v))
            .unwrap_or(0.0);
        assert!((turn - 0.358).abs() < 0.005, "{turn}");
        assert!(((at(5.01) - at(4.99)) / 0.02 + 1.0).abs() < 0.02);
    }

    #[test]
    fn oversampling_and_adaa_cut_the_aliasing() {
        for model in [0.0, 1.0] {
            let setup = |mut fold: Fold| {
                fold.set_param(MODEL, model);
                fold.reset();
                fold
            };
            let naive = aliasing_db(&mut setup(Fold::naive()), 3_999.0, 0.5);
            let proper = aliasing_db(&mut setup(Fold::new()), 3_999.0, 0.5);
            assert!(
                proper < naive - 25.0,
                "model {model}: naive {naive:.1} dB, anti-aliased {proper:.1} dB"
            );
        }
    }

    /// Adding folds changes the timbre, not the loudness: across the whole
    /// `folds` knob, for both models, a 220 Hz tone at -12 dBFS comes out
    /// within 2 dB of where it goes in.
    #[test]
    fn folds_keep_the_level() {
        let input = sine(220.0, 0.25, 0.5);
        for model in [0.0, 1.0] {
            for folds in [0.0, 25.0, 50.0, 75.0, 100.0] {
                let mut fold = built(&KIND);
                fold.set_param(MODEL, model);
                fold.set_param(FOLDS, folds);
                fold.reset();
                let out = render_mono(&mut *fold, &input);
                let level = db(rms(&out[12_000..]) / rms(&input[12_000..]));
                assert!(
                    level.abs() < 2.0,
                    "model {model} folds {folds}: {level:.1} dB"
                );
            }
        }
    }

    /// The makeup is fixed by the knobs, not a compressor: a note stepping
    /// from -40 to -6 dBFS keeps its first 20 ms within 1 dB of how it
    /// settles.
    #[test]
    fn a_hard_pick_keeps_its_attack() {
        for model in [0.0, 1.0] {
            for folds in [40.0, 100.0] {
                let quiet = sine(220.0, 0.01, 1.0);
                let loud = sine(220.0, 0.5, 1.0);
                let mut fold = built(&KIND);
                fold.set_param(MODEL, model);
                fold.set_param(FOLDS, folds);
                fold.reset();
                render_mono(&mut *fold, &quiet);
                let out = render_mono(&mut *fold, &loud);
                let attack = rms(&out[..960]);
                let settled = rms(&out[out.len() - 9_600..]);
                let change = db(attack / settled);
                assert!(
                    change.abs() < 1.0,
                    "model {model} folds {folds}: attack {change:.1} dB"
                );
            }
        }
    }

    #[test]
    fn symmetry_brings_even_harmonics() {
        let input = sine(200.0, 0.5, 0.5);
        for model in [0.0, 1.0] {
            let second = |symmetry: f32| {
                let mut fold = built(&KIND);
                fold.set_param(MODEL, model);
                fold.set_param(SYMMETRY, symmetry);
                let power = spectrum(&render_mono(&mut *fold, &input));
                power_near(&power, 400.0) / power_near(&power, 200.0)
            };
            assert!(second(60.0) > second(0.0) * 1_000.0, "model {model}");
        }
    }

    #[test]
    fn a_dry_mix_is_the_input() {
        let input = sine(300.0, 0.4, 0.3);
        for model in [0.0, 1.0] {
            let mut fold = built(&KIND);
            fold.set_param(MODEL, model);
            fold.set_param(MIX, 0.0);
            fold.set_param(FOLDS, 100.0);
            let out = render_mono(&mut *fold, &input);
            let change = db(rms(&out[4_800..]) / rms(&input[4_800..]));
            assert!(change.abs() < 0.1, "model {model}: {change:.2} dB");
        }
    }
}
