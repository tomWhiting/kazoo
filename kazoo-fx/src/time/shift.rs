//! Frequency shifter, in the spirit of the Bode frequency shifter (Moog
//! 1630).
//!
//! # The model
//!
//! A pitch shifter multiplies every frequency; a frequency shifter adds the
//! same number of hertz to each, so harmonics stop being harmonic and a
//! sound turns bell-like, clangorous or, a few hertz from home, slowly
//! phasing. Harald Bode's circuit does it the analogue way, and so does this:
//!
//! - **Hilbert transformer.** Two chains of allpass filters whose outputs
//!   stay 90° apart from 20 Hz to within 20 Hz of Nyquist, each section an
//!   allpass in `z⁻²` (the structure of Olli Niemitalo's pair). The
//!   coefficients are designed for the running rate, as many as it takes to
//!   hold the mirror image 120 dB down, and run in double precision. Their
//!   outputs are the signal's in-phase and quadrature parts.
//! - **Quadrature oscillator** at the shift frequency. Multiplying each part
//!   by its matching oscillator phase and adding gives the upper sideband
//!   alone (`up`); subtracting gives the lower (`down`). `both` sends up to
//!   the left and down to the right, like the Bode's two outputs.
//! - **Feedback** through a short delay sends the shifted sound back to be
//!   shifted again, so each pass climbs (or falls) further: the endless
//!   barber-pole spirals. Capped at 0.9 and soft-clipped.
//!
//! A fourth-order lowpass ahead of the transformer keeps a side's upward
//! shift from folding back off the top of the band (a side that only
//! shifts down keeps its whole band), and a highpass at 15 Hz keeps out the lowest
//! octave, where no Hilbert pair holds 90°.
//!
//! The allpass chains smear an impulse over a few samples; the peak of its
//! envelope (about 7 samples, at any rate) is reported as the latency, and
//! the dry signal is held back by the same amount so a part mix lines up.
//!
//! Sources: H. Bode, "History of electronic sound modification" (JAES 1984);
//! O. Niemitalo, "Hilbert transform" (the allpass-pair structure);
//! R. A. Valenzuela and A. G. Constantinides, "Digital signal processing
//! schemes for efficient interpolation and decimation" (IEE Proc. 1983), in
//! L. de Soras's closed-form design from his HIIR library.

use super::parts::{
    Biquad, Saturator, Sinc, accept, clean, defaults, flush_wide, frames, guard, silence_from,
};
use crate::dsp::{DelayLine, Smoothed, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

/// The qualities of a fourth-order Butterworth's two sections, for the
/// guard against folding off the top.
const GUARD_Q: [f64; 2] = [0.541_196_1, 1.306_563];

const SHIFT: usize = 0;
const OUTPUT: usize = 1;
const FEEDBACK: usize = 2;
const DELAY: usize = 3;
const MIX: usize = 4;

/// The most allpass coefficients the pair may use (both chains together).
const MAX_COEFFICIENTS: usize = 32;
/// How far below the shifted tone its mirror image is held, in tens of
/// decibels (120 dB).
const IMAGE_REJECTION_DB_TENTHS: i32 = 12;
/// The lowest frequency the pair holds 90° at, whatever the rate.
const LOW_EDGE_HZ: f32 = 20.0;
/// Enough terms of the elliptic series for any rate: each term falls by a
/// further power of `q`, which is below 0.9 even at 1 MHz.
const SERIES_TERMS: usize = 64;
const AGM_ROUNDS: usize = 40;
/// How far into the impulse response the latency is looked for.
const LATENCY_SEARCH: usize = 1_024;
const PI64: f64 = std::f64::consts::PI;

const MIN_DELAY: f32 = 0.002;
const MAX_DELAY: f32 = 1.0;
const REFRESH: u32 = 32;
/// Below this the transformer cannot hold 90°, so it is kept out.
const BLOCK_HZ: f32 = 15.0;

const PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "shift",
        min: -1_000.0,
        max: 1_000.0,
        default: 6.0,
        unit: "Hz",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "output",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["up", "down", "both"],
        },
    },
    ParamSpec {
        name: "feedback",
        min: 0.0,
        max: 0.9,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "delay",
        min: MIN_DELAY,
        max: MAX_DELAY,
        default: 0.1,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The frequency shifter.
pub static KIND: EffectKind = EffectKind {
    id: "shift",
    name: "Frequency shifter",
    description: "A Bode-style frequency shifter: up, down or both, with feedback for \
                  endless barber-pole spirals.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Shift::new())
}

/// One allpass section in `z⁻²`: `y = a (x + y[n-2]) - x[n-2]`, in
/// double precision (its coefficient may sit within 1e-5 of one).
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Section {
    fn process(&mut self, input: f64, a: f64) -> f64 {
        let out = a.mul_add(input + self.y2, -self.x2);
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = out;
        flush_wide(&mut self.y1);
        self.y1
    }
}

/// The two allpass chains, 90° apart. Their coefficients are designed for
/// the running sample rate in [`design`], taken in turn: the first, third,
/// fifth… to the quadrature chain, the rest to the in-phase chain.
#[derive(Debug, Clone, Copy)]
struct Hilbert {
    coefficients: [f64; MAX_COEFFICIENTS],
    count: usize,
    sections: [Section; MAX_COEFFICIENTS],
    /// The in-phase chain runs one sample late to line up with the other.
    late: f64,
}

impl Default for Hilbert {
    fn default() -> Self {
        Self {
            coefficients: [0.0; MAX_COEFFICIENTS],
            count: 0,
            sections: [Section::default(); MAX_COEFFICIENTS],
            late: 0.0,
        }
    }
}

impl Hilbert {
    /// The in-phase and quadrature parts of `input`.
    fn process(&mut self, input: f32) -> (f64, f64) {
        let input = f64::from(input);
        let mut parts = [input, input];
        let used = self.count.min(MAX_COEFFICIENTS);
        for (index, (section, a)) in self
            .sections
            .iter_mut()
            .zip(self.coefficients)
            .take(used)
            .enumerate()
        {
            let chain = &mut parts[index % 2];
            *chain = section.process(*chain, a);
        }
        let [imaginary, real] = parts;
        let delayed = self.late;
        self.late = real;
        (delayed, imaginary)
    }

    fn clear(&mut self) {
        self.sections = [Section::default(); MAX_COEFFICIENTS];
        self.late = 0.0;
    }
}

/// Where the shifter's impulse response peaks: the sample at which the
/// envelope of the in-phase and quadrature parts (which does not depend on
/// the shift) is largest, through the same band filters the signal meets.
/// This is the shifter's processing latency. Allocates nothing; runs in
/// `prepare`.
fn envelope_peak(coefficients: [f64; MAX_COEFFICIENTS], count: usize, sample_rate: f32) -> usize {
    let mut hilbert = Hilbert {
        coefficients,
        count,
        ..Hilbert::default()
    };
    let mut band = [Biquad::new(); 2];
    for (section, q) in band.iter_mut().zip(GUARD_Q) {
        section.set_guard_lowpass(0.47 * sample_rate, q, sample_rate);
    }
    let mut blocker = Biquad::new();
    blocker.set_highpass(BLOCK_HZ, 0.707, sample_rate);
    let mut loudest = (0, 0.0f64);
    for n in 0..LATENCY_SEARCH {
        let click = if n == 0 { 1.0 } else { 0.0 };
        let banded = band.iter_mut().fold(click, |x, section| section.process(x));
        let (real, imaginary) = hilbert.process(blocker.process(banded));
        let size = real.hypot(imaginary);
        if size > loudest.1 {
            loudest = (n, size);
        }
    }
    loudest.0
}

/// The arithmetic-geometric mean of `a` and `b`; it converges
/// quadratically, so a few dozen rounds are far more than double precision
/// needs.
fn agm(a: f64, b: f64) -> f64 {
    let (mut a, mut b) = (a, b);
    for _ in 0..AGM_ROUNDS {
        (a, b) = (0.5 * (a + b), (a * b).sqrt());
    }
    a
}

/// Design the allpass pair for `sample_rate`: the fewest coefficients (up
/// to [`MAX_COEFFICIENTS`]) that hold the two chains close enough to 90°,
/// from [`LOW_EDGE_HZ`] up to the same distance below Nyquist, to keep the
/// mirror image 120 dB down ([`IMAGE_REJECTION_DB_TENTHS`]). This is the polyphase half-band elliptic design
/// (Valenzuela and Constantinides, in Laurent de Soras's closed form), moved
/// up a quarter of the sample rate to make a Hilbert pair; designing it for
/// each rate keeps the image as far down at 192 kHz as at 48 kHz.
fn design(sample_rate: f32) -> ([f64; MAX_COEFFICIENTS], usize) {
    let transition = (f64::from(LOW_EDGE_HZ) / f64::from(sample_rate)).clamp(1e-6, 0.2);
    let modulus = ((1.0 - transition * 2.0) * std::f64::consts::FRAC_PI_4)
        .tan()
        .powi(2);
    // The nome of the elliptic modulus, exactly, through the
    // arithmetic-geometric mean. (The usual short series for it is only good
    // for wide transitions; at 192 kHz a 20 Hz edge is far too narrow.)
    let complement = (1.0 - modulus * modulus).max(0.0).sqrt();
    let nome = (-PI64 * agm(1.0, complement) / agm(1.0, modulus)).exp();
    let ripple = 10f64.powi(-IMAGE_REJECTION_DB_TENTHS);
    let ratio = ripple / (1.0 - ripple);
    let order = (ratio * ratio / 16.0).log(nome).ceil().max(1.0) as usize;
    let count = (order / 2).clamp(1, MAX_COEFFICIENTS);
    let order = (2 * count + 1) as f64;
    let mut coefficients = [0.0; MAX_COEFFICIENTS];
    for (index, coefficient) in coefficients.iter_mut().take(count).enumerate() {
        let place = (index + 1) as f64 * PI64 / order;
        let numerator = odd_series(nome, place) * nome.sqrt().sqrt();
        let denominator = even_series(nome, place) + 0.5;
        let weight = (numerator / denominator).powi(2);
        let spread = (1.0 - weight * modulus) * (1.0 - weight / modulus);
        let root = spread.max(0.0).sqrt() / (1.0 + weight);
        *coefficient = (1.0 - root) / (1.0 + root);
    }
    (coefficients, count)
}

/// The theta series `Σ (-1)ⁿ q^(n(n+1)) sin((2n+1) θ)` for `n` from 0.
fn odd_series(nome: f64, place: f64) -> f64 {
    let mut sum = 0.0;
    let mut sign = 1.0;
    for term in 0..SERIES_TERMS {
        let n = term as f64;
        let size = nome.powf(n * (n + 1.0)) * sign;
        sum = size.mul_add((n.mul_add(2.0, 1.0) * place).sin(), sum);
        sign = -sign;
        if size.abs() < 1e-100 {
            break;
        }
    }
    sum
}

/// The theta series `Σ (-1)ⁿ q^(n²) cos(2n θ)` for `n` from 1.
fn even_series(nome: f64, place: f64) -> f64 {
    let mut sum = 0.0;
    let mut sign = -1.0;
    for term in 1..SERIES_TERMS {
        let n = term as f64;
        let size = nome.powf(n * n) * sign;
        sum = size.mul_add((2.0 * n * place).cos(), sum);
        sign = -sign;
        if size.abs() < 1e-100 {
            break;
        }
    }
    sum
}

/// One side: its transformer, its filters and its feedback delay.
#[derive(Debug, Clone, Default)]
struct Side {
    hilbert: Hilbert,
    /// A fourth-order lowpass keeping out what this side's upward shift
    /// would push past Nyquist.
    guard_band: [Biquad; 2],
    blocker: Biquad,
    line: DelayLine,
    /// The dry signal, held back by the shifter's latency so a part mix
    /// lines up with the shifted sound.
    dry: DelayLine,
    clip: Saturator,
}

/// The frequency shifter.
#[derive(Debug)]
pub struct Shift {
    rate: f32,
    prepared: bool,
    hz: Smoothed,
    feedback: Smoothed,
    delay: Smoothed,
    mix: Smoothed,
    /// Up and down weights for the left and right sides.
    routes: [[Smoothed; 2]; 2],
    /// The oscillator's phase, 0 up to 1, in double precision so a slow
    /// shift at a high rate does not wander.
    phase: f64,
    sides: [Side; 2],
    sinc: Sinc,
    refresh: u32,
    /// Samples from input to the peak of the shifted sound's envelope.
    latency: usize,
}

impl Default for Shift {
    fn default() -> Self {
        Self::new()
    }
}

impl Shift {
    /// A frequency shifter at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            hz: Smoothed::new(values[SHIFT]),
            feedback: Smoothed::new(values[FEEDBACK]),
            delay: Smoothed::new(values[DELAY]),
            mix: Smoothed::new(values[MIX]),
            routes: routes(values[OUTPUT]).map(|side| side.map(Smoothed::new)),
            phase: 0.0,
            sides: [Side::default(), Side::default()],
            sinc: Sinc::default(),
            refresh: 0,
            latency: 0,
        }
    }

    fn smoothers(&mut self) -> impl Iterator<Item = &mut Smoothed> {
        [
            &mut self.hz,
            &mut self.feedback,
            &mut self.delay,
            &mut self.mix,
        ]
        .into_iter()
        .chain(self.routes.iter_mut().flatten())
    }

    fn render(&mut self, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let rate = self.rate;
        for i in 0..n {
            let shift = self.hz.step();
            let feedback = self.feedback.step();
            let delay = self.delay.step() * rate;
            let mix = self.mix.step();
            if self.refresh == 0 {
                // Keep out what each side's upward shift would push past the
                // top: `up` raises by the shift, `down` by minus it, and a
                // side that only shifts down keeps its whole band.
                for (side, route) in self.sides.iter_mut().zip(&self.routes) {
                    let up = if route[0].target() > 0.0 { shift } else { 0.0 };
                    let down = if route[1].target() > 0.0 { -shift } else { 0.0 };
                    let rise = up.max(down).max(0.0);
                    let top = (0.47f32.mul_add(rate, -rise)).max(500.0);
                    for (section, q) in side.guard_band.iter_mut().zip(GUARD_Q) {
                        section.set_guard_lowpass(top, q, rate);
                    }
                }
                self.refresh = REFRESH;
            }
            self.refresh -= 1;
            let (sin, cos) = (std::f64::consts::TAU * self.phase).sin_cos();
            self.phase = (self.phase + f64::from(shift) / f64::from(rate)).rem_euclid(1.0);
            for (c, side) in self.sides.iter_mut().enumerate() {
                let up_weight = self.routes[c][0].step();
                let down_weight = self.routes[c][1].step();
                let x = clean(input[c][i]);
                let echo = self.sinc.read(&side.line, f64::from(delay) + 1.0);
                let returned = side.clip.process(feedback * echo, 1.5);
                let banded = side
                    .guard_band
                    .iter_mut()
                    .fold(x + returned, |x, section| section.process(x));
                let centred = side.blocker.process(banded);
                let (real, imaginary) = side.hilbert.process(centred);
                let up = real.mul_add(cos, imaginary * sin) as f32;
                let down = real.mul_add(cos, -imaginary * sin) as f32;
                let mut shifted = up.mul_add(up_weight, down * down_weight);
                flush(&mut shifted);
                side.line.push(shifted);
                side.dry.push(x);
                let dry = side.dry.tap(self.latency + 1);
                output[c][i] = guard((shifted - dry).mul_add(mix, dry));
            }
        }
    }
}

/// The up and down weights of each side at `output` step `step`.
fn routes(step: f32) -> [[f32; 2]; 2] {
    match step.round() as i32 {
        1 => [[0.0, 1.0], [0.0, 1.0]],
        2 => [[1.0, 0.0], [0.0, 1.0]],
        _ => [[1.0, 0.0], [1.0, 0.0]],
    }
}

impl Effect for Shift {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        self.sinc = Sinc::new();
        for side in &mut self.sides {
            side.line.resize((MAX_DELAY * rate) as usize + 2);
            side.blocker.set_highpass(BLOCK_HZ, 0.707, rate);
            (side.hilbert.coefficients, side.hilbert.count) = design(rate);
        }
        let (coefficients, count) = design(rate);
        self.latency = envelope_peak(coefficients, count, rate);
        for side in &mut self.sides {
            side.dry.resize(self.latency + 2);
        }
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        self.delay.set_time(0.1, rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for side in &mut self.sides {
            side.hilbert.clear();
            for section in &mut side.guard_band {
                section.reset();
            }
            side.blocker.reset();
            side.line.clear();
            side.dry.clear();
            side.clip.reset();
        }
        for smoother in self.smoothers() {
            smoother.snap(smoother.target());
        }
        self.phase = 0.0;
        self.refresh = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        match index {
            SHIFT => self.hz.set(value),
            FEEDBACK => self.feedback.set(value),
            DELAY => self.delay.set(value),
            MIX => self.mix.set(value),
            OUTPUT => {
                for (side, weights) in self.routes.iter_mut().zip(routes(value)) {
                    for (route, weight) in side.iter_mut().zip(weights) {
                        route.set(weight);
                    }
                }
            }
            _ => {}
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let mut output = output;
        let n = if self.prepared {
            frames(&input, &output)
        } else {
            0
        };
        self.render(input, &mut output, n);
        silence_from(&mut output, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::testkit::{RATE as HOST, context, prepared, render, sine, tone_level};

    fn shifted(hz: f32, shift: f32, output: f32) -> (Vec<f32>, Vec<f32>) {
        let mut shifter = prepared(&KIND);
        shifter.set_param(SHIFT, shift);
        shifter.set_param(OUTPUT, output);
        shifter.set_param(MIX, 1.0);
        shifter.reset();
        let input = sine(HOST as usize, hz, 0.5);
        let (left, right) = render(shifter.as_mut(), context(120.0), &input, &input, 256);
        (left[4_800..].to_vec(), right[4_800..].to_vec())
    }

    #[test]
    fn a_sine_moves_up_by_the_shift() {
        for (hz, shift) in [(1_000.0, 200.0), (300.0, 57.0), (5_000.0, 900.0)] {
            let (left, _) = shifted(hz, shift, 0.0);
            let moved = tone_level(&left, hz + shift);
            let image = tone_level(&left, hz - shift);
            let stayed = tone_level(&left, hz);
            assert!((moved - 0.5).abs() < 0.03, "{hz} {shift} {moved}");
            assert!(
                image < 0.01 && stayed < 0.01,
                "{hz} {shift} {image} {stayed}"
            );
        }
    }

    #[test]
    fn a_sine_moves_down_and_both_splits_the_sides() {
        let (left, _) = shifted(1_000.0, 250.0, 1.0);
        assert!((tone_level(&left, 750.0) - 0.5).abs() < 0.03);
        assert!(tone_level(&left, 1_250.0) < 0.01);
        let (left, right) = shifted(1_000.0, 250.0, 2.0);
        assert!(tone_level(&left, 1_250.0) > 0.47);
        assert!(tone_level(&right, 750.0) > 0.47);
    }

    #[test]
    fn a_negative_shift_goes_down() {
        let (left, _) = shifted(2_000.0, -300.0, 0.0);
        assert!((tone_level(&left, 1_700.0) - 0.5).abs() < 0.03);
    }
}
