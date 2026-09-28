//! The oscillator: sine → triangle → saw → square, morphing continuously.
//!
//! Every edge and corner is band-limited. A jump in the wave (the saw's
//! fall, the square's rise and fall) is replaced by an integrated cubic
//! B-spline step (BLEP), and a sudden change of slope (the triangle's
//! corners) by the same step integrated once more (BLAMP). The cubic
//! B-spline spans four samples and its spectrum has a fourth-order null at
//! every multiple of the rate being generated, which is exactly where the
//! harmonics that would fold back into the audible band sit. The
//! corrections reach two samples either side of each event, so the
//! oscillator runs two samples behind itself.
//!
//! Below 176.4 kHz the wave is generated at a multiple of the stream's rate
//! and brought back down through halfband filters (see
//! [`super::oversample`]); at 176.4 kHz and up it runs at the stream's rate.
//! The phase is kept in double precision, so a sub-audio drone holds its
//! pitch to within a hundredth of a cent at any rate.

use super::oversample::{Decimator, MAX_FACTOR, factor_for};
use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;
use crate::catalogue::C4_HZ;

const OCTAVE: usize = 0;
const TUNE: usize = 1;
const SHAPE: usize = 2;
const WIDTH: usize = 3;
const LEVEL: usize = 4;
const FM_DEPTH: usize = 5;

const IN_PITCH: usize = 0;
const IN_FM: usize = 1;

/// The rate the wave is generated at, at least.
const INTERNAL_RATE: f32 = 176_400.0;

/// Highest phase step at the internal rate. It keeps the four-sample
/// corrections of successive edges apart; at the lowest internal rate it is
/// 35 kHz, far above hearing.
const MAX_STEP: f64 = 0.2;

/// Output bound. A band-limited pulse overshoots ±1 (Gibbs), and the
/// halfband filters' phase near the top of the band moves its harmonics
/// about, so a narrow pulse can peak near ±2; clipping it there would put
/// back the aliasing everything else here takes out.
const LIMIT: f32 = 4.0;

/// Samples of the wave waiting for corrections: the two before the newest,
/// the newest, and the one to come.
const PENDING: usize = 4;

#[derive(Debug)]
pub struct Vco {
    rate: f32,
    factor: usize,
    decimator: Decimator,
    phase: f64,
    /// The phase step at the last stream frame, to glide from.
    step: f64,
    pending: [f64; PENDING],
    /// Index of the newest sample in `pending`.
    newest: usize,
}

impl Vco {
    pub fn new(sample_rate: f32) -> Self {
        let factor = factor_for(sample_rate, INTERNAL_RATE);
        Self {
            rate: sample_rate,
            factor,
            decimator: Decimator::new(factor),
            phase: 0.0,
            step: 0.0,
            pending: [0.0; PENDING],
            newest: 0,
        }
    }

    /// Re-plan for a stream at `sample_rate`, without allocating.
    fn follow_rate(&mut self, sample_rate: f32) {
        if sample_rate.to_bits() != self.rate.to_bits() {
            *self = Self::new(sample_rate);
        }
    }

    /// Add `amount × kernel(τ)` to the four samples around an event `ago`
    /// samples (0 to 1) before the newest.
    fn correct(&mut self, ago: f64, amount: f64, kernel: fn(f64) -> f64) {
        if amount == 0.0 {
            return;
        }
        // The samples two before the newest to one after it sit at τ = −2 +
        // ago ... 1 + ago from the event.
        for offset in 0..PENDING {
            // Offset is below 4: exact.
            let tau = ago + offset as f64 - 2.0;
            let slot = (self.newest + PENDING - 2 + offset) % PENDING;
            self.pending[slot] = amount.mul_add(kernel(tau), self.pending[slot]);
        }
    }

    /// Generate one sample at the internal rate and return the one leaving
    /// the correction window.
    fn next(&mut self, step: f64, weights: Weights, width: f64) -> f64 {
        let start = self.phase;
        let end = start + step;
        self.newest = (self.newest + 1) % PENDING;
        // The slot just vacated held the sample three back, already sent.
        self.pending[(self.newest + 1) % PENDING] = 0.0;
        self.phase = if end >= 1.0 { end - 1.0 } else { end };
        let ago = |at: f64| {
            if step > 0.0 {
                ((end - at) / step).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let crosses = |at: f64| start < at && at <= end;
        if crosses(1.0) {
            // The saw falls from +1 to −1, the square rises from −1 to +1,
            // the triangle turns up from its trough.
            self.correct(ago(1.0), 2.0 * (weights.square - weights.saw), blep);
            self.correct(ago(1.0), 8.0 * step * weights.triangle, blamp);
        }
        for fall in [width, width + 1.0] {
            if crosses(fall) {
                self.correct(ago(fall), -2.0 * weights.square, blep);
            }
        }
        for peak in [0.5, 1.5] {
            if crosses(peak) {
                self.correct(ago(peak), -8.0 * step * weights.triangle, blamp);
            }
        }
        let naive = weights.wave(self.phase, width);
        self.pending[self.newest] += naive;
        self.pending[(self.newest + PENDING - 2) % PENDING]
    }
}

/// How much of each wave the morph holds.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Weights {
    sine: f64,
    triangle: f64,
    saw: f64,
    square: f64,
}

impl Weights {
    fn new(shape: f32) -> Self {
        let shape = f64::from(shape.clamp(0.0, 3.0));
        let mut weights = Self {
            sine: 0.0,
            triangle: 0.0,
            saw: 0.0,
            square: 0.0,
        };
        if shape < 1.0 {
            weights.sine = 1.0 - shape;
            weights.triangle = shape;
        } else if shape < 2.0 {
            weights.triangle = 2.0 - shape;
            weights.saw = shape - 1.0;
        } else {
            weights.saw = 3.0 - shape;
            weights.square = shape - 2.0;
        }
        weights
    }

    /// The unfiltered morph at `phase` (0 to 1).
    fn wave(self, phase: f64, width: f64) -> f64 {
        let mut value = 0.0;
        if self.sine > 0.0 {
            // Single precision is plenty for the sine itself (its error sits
            // near −140 dB) and much quicker.
            let sine = f64::from((phase as f32 * std::f32::consts::TAU).sin());
            value = self.sine.mul_add(sine, value);
        }
        if self.triangle > 0.0 {
            let triangle = 4.0f64.mul_add(-(phase - 0.5).abs(), 1.0);
            value = self.triangle.mul_add(triangle, value);
        }
        if self.saw > 0.0 {
            value = self.saw.mul_add(phase.mul_add(2.0, -1.0), value);
        }
        if self.square > 0.0 {
            // A narrow pulse carries DC, as on the hardware; the master
            // chain's high-pass takes it out.
            let square = if phase < width { 1.0 } else { -1.0 };
            value = self.square.mul_add(square, value);
        }
        value
    }
}

/// Evaluate the polynomial with `coefficients` (constant first) at `x`.
fn poly(coefficients: &[f64], x: f64) -> f64 {
    coefficients
        .iter()
        .rev()
        .fold(0.0, |acc, c| acc.mul_add(x, *c))
}

/// The integrated cubic B-spline step minus the ideal step, at `tau`
/// samples from the jump (−2 to 2; zero outside).
fn blep(tau: f64) -> f64 {
    const PIECES: [[f64; 5]; 4] = [
        [2.0 / 3.0, 4.0 / 3.0, 1.0, 1.0 / 3.0, 1.0 / 24.0],
        [1.0 / 2.0, 2.0 / 3.0, 0.0, -1.0 / 3.0, -1.0 / 8.0],
        [-1.0 / 2.0, 2.0 / 3.0, 0.0, -1.0 / 3.0, 1.0 / 8.0],
        [-2.0 / 3.0, 4.0 / 3.0, -1.0, 1.0 / 3.0, -1.0 / 24.0],
    ];
    piece(&PIECES, tau)
}

/// The cubic B-spline step integrated once more, minus the ideal corner
/// (a ramp), at `tau` samples from the corner (−2 to 2; zero outside).
fn blamp(tau: f64) -> f64 {
    const PIECES: [[f64; 6]; 4] = [
        [
            4.0 / 15.0,
            2.0 / 3.0,
            2.0 / 3.0,
            1.0 / 3.0,
            1.0 / 12.0,
            1.0 / 120.0,
        ],
        [
            7.0 / 30.0,
            1.0 / 2.0,
            1.0 / 3.0,
            0.0,
            -1.0 / 12.0,
            -1.0 / 40.0,
        ],
        [
            7.0 / 30.0,
            -1.0 / 2.0,
            1.0 / 3.0,
            0.0,
            -1.0 / 12.0,
            1.0 / 40.0,
        ],
        [
            4.0 / 15.0,
            -2.0 / 3.0,
            2.0 / 3.0,
            -1.0 / 3.0,
            1.0 / 12.0,
            -1.0 / 120.0,
        ],
    ];
    piece(&PIECES, tau)
}

/// The piece of a four-piece kernel on [−2, 2) that holds `tau`.
fn piece<const N: usize>(pieces: &[[f64; N]; 4], tau: f64) -> f64 {
    if !(-2.0..2.0).contains(&tau) {
        return 0.0;
    }
    // −2 ≤ tau < 2: the index is 0 to 3.
    let index = ((tau + 2.0).floor() as usize).min(3);
    poly(&pieces[index], tau)
}

impl Module for Vco {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        self.follow_rate(tick.sample_rate);
        let octave = io.knob(OCTAVE).round();
        // Rates are a few hundred kHz at most: exact.
        let internal = f64::from(tick.sample_rate) * self.factor as f64;
        let factor = self.factor;
        let mut oversampled = [0.0; MAX_FACTOR];
        for frame in 0..SUB_BLOCK {
            let tune = io.knob_at(TUNE, frame);
            let depth = io.knob_at(FM_DEPTH, frame);
            let pitch = finite(io.inputs[IN_PITCH][frame]).clamp(-10.0, 10.0);
            let fm = finite(io.inputs[IN_FM][frame]).clamp(-4.0, 4.0);
            let volts = f64::from(octave + tune / 12.0 + pitch);
            let hz = f64::from(C4_HZ) * volts.exp2() * f64::from(fm.mul_add(depth, 1.0).max(0.0));
            let step = (hz / internal).clamp(0.0, MAX_STEP);
            let weights = Weights::new(io.knob_at(SHAPE, frame));
            let width = f64::from(io.knob_at(WIDTH, frame));
            let from = self.step;
            for (index, sample) in oversampled.iter_mut().take(factor).enumerate() {
                // Glide the step across the oversampled frames. Both are at
                // most 8: exact.
                let t = (index + 1) as f64 / factor as f64;
                *sample = self.next((step - from).mul_add(t, from), weights, width);
            }
            self.step = step;
            let sample = self.decimator.process(&oversampled[..factor]) as f32;
            let level = io.knob_at(LEVEL, frame);
            io.outputs[0][frame] = finite(sample * level).clamp(-LIMIT, LIMIT);
        }
        self.decimator.flush();
        if !self.phase.is_finite() {
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.step = 0.0;
        self.pending = [0.0; PENDING];
        self.newest = 0;
        self.decimator.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, db, frequency, stays_in_range, survives_nonsense, tone};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_indices_match_the_catalogue() {
        let spec = Kind::VCO.spec();
        for (index, name) in [
            (OCTAVE, "octave"),
            (TUNE, "tune"),
            (SHAPE, "shape"),
            (WIDTH, "width"),
            (LEVEL, "level"),
            (FM_DEPTH, "fm_depth"),
        ] {
            assert_eq!(spec.knobs[index].name, name);
        }
        for (index, name) in [(IN_PITCH, "pitch"), (IN_FM, "fm")] {
            assert_eq!(spec.inputs[index].name, name);
        }
    }

    #[test]
    fn it_plays_c4_and_follows_volts_per_octave() {
        let mut bench = Bench::new(Kind::VCO);
        bench.knob("shape", 0.0);
        let hz = frequency(&bench.render(0, 48_000));
        assert!((hz - C4_HZ).abs() < 0.5, "{hz}");

        let mut bench = Bench::new(Kind::VCO);
        bench.knob("shape", 0.0).hold("pitch", -1.0);
        let hz = frequency(&bench.render(0, 48_000));
        assert!((hz - C4_HZ / 2.0).abs() < 0.3, "{hz}");

        let mut bench = Bench::new(Kind::VCO);
        bench
            .knob("shape", 0.0)
            .knob("octave", 1.0)
            .knob("tune", 9.0 - 12.0);
        // One octave up, then down three semitones: A4.
        let hz = frequency(&bench.render(0, 48_000));
        assert!((hz - 440.0).abs() < 0.5, "{hz}");
    }

    #[test]
    fn a_drone_stays_in_tune_at_every_rate() {
        // Four octaves under C4 at 192 kHz, where a single-precision phase
        // went sharp by 0.15 cents.
        for rate in [48_000.0, 192_000.0] {
            let mut bench = Bench::at(Kind::VCO, rate);
            bench.knob("shape", 0.0).hold("pitch", -4.0);
            let samples = bench.render(0, (rate * 4.0) as usize);
            let mut first = None;
            let mut last = 0.0;
            let mut cycles = 0_u32;
            for i in 1..samples.len() {
                let (a, b) = (f64::from(samples[i - 1]), f64::from(samples[i]));
                if a < 0.0 && b >= 0.0 {
                    let at = (i - 1) as f64 + a / (a - b);
                    if first.is_none() {
                        first = Some(at);
                    } else {
                        cycles += 1;
                    }
                    last = at;
                }
            }
            let hz = f64::from(cycles) * f64::from(rate) / (last - first.unwrap());
            let cents = 1_200.0 * (hz / (f64::from(C4_HZ) / 16.0)).log2();
            assert!(cents.abs() < 0.01, "{rate}: {cents} cents");
        }
    }

    #[test]
    fn every_shape_keeps_its_pitch() {
        for shape in [0.0, 1.0, 1.5, 2.0, 2.5, 3.0] {
            let mut bench = Bench::new(Kind::VCO);
            bench.knob("shape", shape).knob("tune", -12.0);
            let hz = frequency(&bench.render(0, 48_000));
            assert!((hz - C4_HZ / 2.0).abs() < 0.5, "shape {shape}: {hz}");
        }
    }

    /// The loudest alias of a wave near `near` Hz, in dB under its
    /// fundamental. The pitch is chosen so every alias lands halfway
    /// between two harmonics, where it can be measured on its own.
    fn worst_alias(rate: f32, shape: f32, width: f32, near: f64) -> f64 {
        let fs = f64::from(rate);
        let f0 = fs / ((fs / near).round() + 0.5);
        let volts = (f0 / f64::from(C4_HZ)).log2() as f32;
        let mut bench = Bench::at(Kind::VCO, rate);
        bench
            .knob("shape", shape)
            .knob("width", width)
            .hold("pitch", volts);
        let settle = 4_096;
        let samples = bench.render(0, settle + (rate / 2.0) as usize);
        let samples = &samples[settle..];
        let fundamental = tone(samples, f0, fs);
        // At most 20 kHz / 20 Hz of them.
        (0..1_000_u32)
            .map(|k| (f64::from(k) + 0.5) * f0)
            .take_while(|hz| *hz < 20_000.0)
            .map(|hz| db(tone(samples, hz, fs) / fundamental))
            .fold(f64::NEG_INFINITY, f64::max)
    }

    #[test]
    fn edges_do_not_alias() {
        // Before: a saw at 3.5 kHz had an alias at −29 dBc at 48 kHz and
        // −72 dBc at 192 kHz; a triangle at 1.76 kHz −49 dBc at 48 kHz.
        for rate in [48_000.0, 96_000.0, 192_000.0] {
            for (name, shape, width, near) in [
                ("saw", 2.0, 0.5, 3_520.0),
                ("square", 3.0, 0.5, 3_520.0),
                ("pulse", 3.0, 0.1, 3_520.0),
                ("triangle", 1.0, 0.5, 3_520.0),
                ("saw", 2.0, 0.5, 7_040.0),
            ] {
                let alias = worst_alias(rate, shape, width, near);
                assert!(alias < -90.0, "{rate} {name} {near} Hz: {alias:.1} dBc");
            }
        }
    }

    #[test]
    fn the_kernels_join_up() {
        // Each kernel is continuous across its pieces and vanishes at its
        // ends, so a correction never leaves a step of its own.
        for kernel in [blep as fn(f64) -> f64, blamp] {
            for edge in [-2.0, -1.0, 0.0, 1.0, 2.0] {
                let below = kernel(edge - 1e-9);
                let above = kernel(edge + 1e-9);
                let jump = if edge == 0.0 && kernel(0.5) < 0.0 {
                    1.0
                } else {
                    0.0
                };
                assert!(
                    (below - above - jump).abs() < 1e-6,
                    "at {edge}: {below} {above}"
                );
            }
        }
        assert!(blep(-2.0).abs() < 1e-12 && blep(1.999_999_999).abs() < 1e-8);
        assert!(blamp(-2.0).abs() < 1e-12 && blamp(1.999_999_999).abs() < 1e-8);
    }

    #[test]
    fn fm_moves_the_frequency_linearly() {
        let mut bench = Bench::new(Kind::VCO);
        bench
            .knob("shape", 0.0)
            .knob("fm_depth", 0.5)
            .hold("fm", 1.0);
        let hz = frequency(&bench.render(0, 48_000));
        assert!(C4_HZ.mul_add(-1.5, hz).abs() < 0.8, "{hz}");
    }

    #[test]
    fn it_reaches_high_notes_at_48_khz() {
        // The old oscillator stopped at 9.6 kHz at 48 kHz.
        let mut bench = Bench::new(Kind::VCO);
        let volts = (12_000.0_f32 / C4_HZ).log2();
        bench.knob("shape", 0.0).hold("pitch", volts);
        let hz = frequency(&bench.render(0, 48_000));
        assert!((hz - 12_000.0).abs() < 1.0, "{hz}");
    }

    #[test]
    fn level_scales_and_width_cv_is_bounded() {
        let mut bench = Bench::new(Kind::VCO);
        bench
            .knob("shape", 3.0)
            .knob("level", 0.5)
            .hold("width", 10.0);
        let samples = bench.render(0, 4_800);
        let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak <= LIMIT && peak > 0.3, "{peak}");
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::VCO, LIMIT);
        survives_nonsense(Kind::VCO);
    }

    #[test]
    fn reset_restarts_the_phase() {
        let mut bench = Bench::new(Kind::VCO);
        bench.knob("shape", 0.0);
        let first = bench.render(0, 32);
        bench.render(0, 1_000);
        bench.module.reset();
        let again = bench.render(0, 32);
        assert_eq!(first, again);
    }
}
