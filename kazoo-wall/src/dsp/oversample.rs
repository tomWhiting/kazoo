//! Oversampling for the modules whose sound would otherwise alias: the
//! oscillator's edges and the filter's drive.
//!
//! Each 2× step is a polyphase IIR halfband: two parallel chains of
//! first-order allpass sections, one per polyphase branch, whose sum is an
//! elliptic lowpass at a quarter of the higher rate. It costs a handful of
//! multiplies per sample and delays the passband by only a few samples,
//! which matters here: a filter mixed in parallel with its own dry signal
//! would comb if the drive path lagged by the long delay of a linear-phase
//! FIR. The phase is not linear near the top of the passband, which nobody
//! hears.
//!
//! The coefficients come from the closed-form elliptic design in Valenzuela
//! and Constantinides, "Digital signal processing schemes for efficient
//! interpolation and decimation" (IEE Proceedings, 1983), as laid out by
//! Laurent de Soras in HIIR. They are computed when a module is built.
//!
//! How far to oversample follows the stream's rate: enough stages to reach
//! a target internal rate, so a 48 kHz wall runs its oscillators at 192 kHz
//! inside while a 192 kHz wall runs them as they are.

use std::f64::consts::PI;

/// Most 2× stages: 8× at most.
pub const MAX_STAGES: usize = 3;

/// Most oversampled frames per stream frame.
pub const MAX_FACTOR: usize = 1 << MAX_STAGES;

/// Most allpass coefficients a stage holds.
const MAX_COEFFICIENTS: usize = 12;

/// Stopband rejection every stage is designed for.
const ATTENUATION_DB: f64 = 110.0;

/// The top of the band every stage keeps flat, as a fraction of the
/// stream's rate: 20 kHz at 48 kHz.
const PASSBAND: f64 = 0.416_7;

/// How many times `sample_rate` must be multiplied (1, 2, 4 or 8) to reach
/// `target` Hz.
#[must_use]
pub fn factor_for(sample_rate: f32, target: f32) -> usize {
    let mut factor = 1;
    while factor < MAX_FACTOR && sample_rate * (factor as f32) < target {
        factor *= 2;
    }
    factor
}

/// One halfband's allpass coefficients.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Design {
    coefficients: [f64; MAX_COEFFICIENTS],
    len: usize,
}

impl Design {
    /// A halfband rejecting [`ATTENUATION_DB`] with a transition band
    /// `transition` wide, as a fraction of the higher rate (0 to 1/2),
    /// centred on a quarter of it.
    fn new(transition: f64) -> Self {
        let transition = transition.clamp(0.01, 0.45);
        let (k, q) = transition_parameters(transition);
        let order = order_for(ATTENUATION_DB, q);
        let len = ((order - 1) / 2).min(MAX_COEFFICIENTS);
        let mut coefficients = [0.0; MAX_COEFFICIENTS];
        for (index, coefficient) in coefficients.iter_mut().take(len).enumerate() {
            *coefficient = coefficient_at(index, k, q, len * 2 + 1);
        }
        Self { coefficients, len }
    }
}

fn transition_parameters(transition: f64) -> (f64, f64) {
    let k = (transition.mul_add(-2.0, 1.0) * PI / 4.0).tan().powi(2);
    let root = (1.0 - k * k).powf(0.25);
    let e = 0.5 * (1.0 - root) / (1.0 + root);
    let e4 = e.powi(4);
    let q = e * e4.mul_add(e4.mul_add(150.0f64.mul_add(e4, 15.0), 2.0), 1.0);
    (k, q)
}

fn order_for(attenuation_db: f64, q: f64) -> usize {
    let power = 10f64.powf(-attenuation_db / 10.0);
    let a = power / (1.0 - power);
    let order = (a * a / 16.0).log(q).ceil().max(3.0) as usize;
    if order % 2 == 0 { order + 1 } else { order }
}

fn coefficient_at(index: usize, k: f64, q: f64, order: usize) -> f64 {
    let c = (index + 1) as f64;
    let order = order as f64;
    let mut numerator = 0.0;
    let mut sign = 1.0;
    for i in 0..64_u32 {
        let i = f64::from(i);
        let term = q.powf(i * (i + 1.0)) * (i.mul_add(2.0, 1.0) * c * PI / order).sin() * sign;
        numerator += term;
        sign = -sign;
        if term.abs() < 1e-100 {
            break;
        }
    }
    let mut denominator = 0.0;
    let mut sign = -1.0;
    for i in 1..64_u32 {
        let i = f64::from(i);
        let term = q.powf(i * i) * (i * 2.0 * c * PI / order).cos() * sign;
        denominator += term;
        sign = -sign;
        if term.abs() < 1e-100 {
            break;
        }
    }
    let ww = numerator * q.powf(0.25) / (denominator + 0.5);
    let wwsq = ww * ww;
    let x = ((1.0 - wwsq * k) * (1.0 - wwsq / k)).sqrt() / (1.0 + wwsq);
    (1.0 - x) / (1.0 + x)
}

/// One 2× halfband stage: the design and both branches' allpass memories.
#[derive(Debug, Clone, Copy)]
struct Stage {
    design: Design,
    /// Per coefficient: the section's last input and last output.
    memory: [[f64; 2]; MAX_COEFFICIENTS],
}

impl Stage {
    const fn new(design: Design) -> Self {
        Self {
            design,
            memory: [[0.0; 2]; MAX_COEFFICIENTS],
        }
    }

    /// Run `even` through the even-numbered sections and `odd` through the
    /// odd-numbered ones.
    fn branches(&mut self, mut even: f64, mut odd: f64) -> (f64, f64) {
        let len = self.design.len;
        for index in 0..len {
            let coefficient = self.design.coefficients[index];
            let memory = &mut self.memory[index];
            let input = if index % 2 == 0 { even } else { odd };
            let output = (input - memory[1]).mul_add(coefficient, memory[0]);
            memory[0] = input;
            memory[1] = output;
            if index % 2 == 0 {
                even = output;
            } else {
                odd = output;
            }
        }
        (even, odd)
    }

    /// Two samples at the higher rate (`first` then `second`) down to one.
    fn down(&mut self, first: f64, second: f64) -> f64 {
        let (even, odd) = self.branches(second, first);
        0.5 * (even + odd)
    }

    /// One sample up to two at the higher rate, in time order.
    fn up(&mut self, sample: f64) -> (f64, f64) {
        self.branches(sample, sample)
    }

    const fn reset(&mut self) {
        self.memory = [[0.0; 2]; MAX_COEFFICIENTS];
    }

    fn flush(&mut self) {
        for memory in &mut self.memory {
            for value in memory.iter_mut() {
                if !value.is_finite() || value.abs() < 1e-30 {
                    *value = 0.0;
                }
            }
        }
    }
}

/// The stages for `factor` at `sample_rate`: stage 0 sits next to the
/// stream's rate and needs the sharpest cut.
fn stages(factor: usize) -> ([Stage; MAX_STAGES], usize) {
    let mut count = 0;
    while (1 << count) < factor && count < MAX_STAGES {
        count += 1;
    }
    let stages = std::array::from_fn(|index| {
        // The stage between rate × 2^index and rate × 2^(index + 1) must
        // stop everything that would fold into the kept band.
        let low = f64::from(1_u32 << index);
        Stage::new(Design::new(0.5 - PASSBAND / low))
    });
    (stages, count)
}

/// Brings an oversampled signal back down to the stream's rate.
#[derive(Debug, Clone)]
pub struct Decimator {
    stages: [Stage; MAX_STAGES],
    count: usize,
    scratch: [f64; MAX_FACTOR],
}

impl Decimator {
    /// A decimator by `factor` (1, 2, 4 or 8; others round up).
    #[must_use]
    pub fn new(factor: usize) -> Self {
        let (stages, count) = stages(factor);
        Self {
            stages,
            count,
            scratch: [0.0; MAX_FACTOR],
        }
    }

    /// Oversampled frames per stream frame.
    #[must_use]
    pub const fn factor(&self) -> usize {
        1 << self.count
    }

    /// One stream frame from `input`, which holds [`Self::factor`]
    /// oversampled frames in time order (extra frames are ignored, missing
    /// ones read as silence).
    pub fn process(&mut self, input: &[f64]) -> f64 {
        let mut len = self.factor();
        for (slot, sample) in self
            .scratch
            .iter_mut()
            .zip(input.iter().chain(std::iter::repeat(&0.0)))
            .take(len)
        {
            *slot = *sample;
        }
        for stage in (0..self.count).rev() {
            len /= 2;
            for index in 0..len {
                let first = self.scratch[index * 2];
                let second = self.scratch[index * 2 + 1];
                self.scratch[index] = self.stages[stage].down(first, second);
            }
        }
        self.scratch[0]
    }

    /// Forget the past.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    /// Zero memories that have decayed to nothing or gone non-finite.
    pub fn flush(&mut self) {
        for stage in &mut self.stages {
            stage.flush();
        }
    }
}

/// Raises a stream-rate signal to the oversampled rate.
#[derive(Debug, Clone)]
pub struct Interpolator {
    stages: [Stage; MAX_STAGES],
    count: usize,
}

impl Interpolator {
    /// An interpolator by `factor` (1, 2, 4 or 8; others round up).
    #[must_use]
    pub fn new(factor: usize) -> Self {
        let (stages, count) = stages(factor);
        Self { stages, count }
    }

    /// Oversampled frames per stream frame.
    #[must_use]
    pub const fn factor(&self) -> usize {
        1 << self.count
    }

    /// Write [`Self::factor`] oversampled frames for `sample` into
    /// `output`, in time order.
    pub fn process(&mut self, sample: f64, output: &mut [f64; MAX_FACTOR]) {
        output[0] = sample;
        let mut len = 1;
        for stage in 0..self.count {
            let mut previous = [0.0; MAX_FACTOR];
            previous[..len].copy_from_slice(&output[..len]);
            for (index, &sample) in previous.iter().take(len).enumerate() {
                let (first, second) = self.stages[stage].up(sample);
                output[index * 2] = first;
                output[index * 2 + 1] = second;
            }
            len *= 2;
        }
    }

    /// Forget the past.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    /// Zero memories that have decayed to nothing or gone non-finite.
    pub fn flush(&mut self) {
        for stage in &mut self.stages {
            stage.flush();
        }
    }
}
