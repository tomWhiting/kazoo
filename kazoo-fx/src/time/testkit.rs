//! Helpers the time family's tests share.

use crate::{Context, Effect, EffectKind};

/// The rate every test runs at.
pub const RATE: f32 = 48_000.0;

/// A context at `bpm`.
pub const fn context(bpm: f64) -> Context {
    Context { bpm }
}

/// An effect of `kind`, built and prepared at [`RATE`].
pub fn prepared(kind: &EffectKind) -> Box<dyn Effect> {
    let mut effect = (kind.build)();
    effect.prepare(RATE);
    effect
}

/// `len` samples of silence with a single 1.0 at `at`.
pub fn impulse(len: usize, at: usize) -> Vec<f32> {
    let mut signal = vec![0.0; len];
    if let Some(sample) = signal.get_mut(at) {
        *sample = 1.0;
    }
    signal
}

/// The index of the loudest sample.
pub fn peak_index(signal: &[f32]) -> usize {
    signal
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |best, (index, sample)| {
            if sample.abs() > best.1 {
                (index, sample.abs())
            } else {
                best
            }
        })
        .0
}

/// The loudest sample's size.
pub fn peak(signal: &[f32]) -> f32 {
    signal
        .iter()
        .fold(0.0, |best: f32, sample| best.max(sample.abs()))
}

/// Root-mean-square level.
pub fn rms(signal: &[f32]) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    (signal.iter().map(|sample| sample * sample).sum::<f32>() / signal.len() as f32).sqrt()
}

/// Run `left` and `right` through `effect` in blocks of `block`.
pub fn render(
    effect: &mut dyn Effect,
    context: Context,
    left: &[f32],
    right: &[f32],
    block: usize,
) -> (Vec<f32>, Vec<f32>) {
    let len = left.len().min(right.len());
    let mut out_left = vec![0.0; len];
    let mut out_right = vec![0.0; len];
    let mut start = 0;
    while start < len {
        let end = (start + block.max(1)).min(len);
        effect.process(
            &context,
            [&left[start..end], &right[start..end]],
            [&mut out_left[start..end], &mut out_right[start..end]],
        );
        start = end;
    }
    (out_left, out_right)
}

/// `len` samples of white noise at `level`, seeded.
pub fn noise(len: usize, level: f32, seed: u32) -> Vec<f32> {
    let mut source = crate::dsp::Noise::new(seed);
    (0..len).map(|_| source.sample() * level).collect()
}

/// The size of a sine at `hz` in `signal`, by correlation.
pub fn tone_level(signal: &[f32], hz: f32) -> f32 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, sample) in signal.iter().enumerate() {
        let angle = std::f64::consts::TAU * f64::from(hz) * n as f64 / f64::from(RATE);
        re = f64::from(*sample).mul_add(angle.cos(), re);
        im = f64::from(*sample).mul_add(angle.sin(), im);
    }
    (2.0 * re.hypot(im) / signal.len().max(1) as f64) as f32
}

/// A sine at `hz` and `level`, `len` samples long.
pub fn sine(len: usize, hz: f32, level: f32) -> Vec<f32> {
    (0..len)
        .map(|n| level * (std::f32::consts::TAU * hz * n as f32 / RATE).sin())
        .collect()
}
