//! Pluck positions and continuously moving fractional-delay strings.

use crate::analysis::score::{SCORE_STEP, Take};

pub const RATE: u32 = 48_000;

pub fn named<'a>(takes: &'a [Take], name: &str) -> &'a Take {
    takes
        .iter()
        .find(|take| take.name == name)
        .expect("instrument take")
}

pub fn cents(hz: f32, truth: f32) -> f32 {
    (1200.0 * (hz / truth).log2()).abs()
}

pub fn truth_at(take: &Take, sample: usize) -> Option<f32> {
    let position = sample.saturating_sub(SCORE_STEP / 2);
    let before = position / SCORE_STEP;
    let low = take.truth.get(before).copied().flatten()?;
    let high = take.truth.get(before + 1).copied().flatten().unwrap_or(low);
    let fraction = (position % SCORE_STEP) as f32 / SCORE_STEP as f32;
    Some(
        low.log2()
            .mul_add(1.0 - fraction, high.log2() * fraction)
            .exp2(),
    )
}

pub fn note_end(take: &Take, note: usize) -> usize {
    take.onsets
        .get(note + 1)
        .copied()
        .unwrap_or(take.samples.len() - RATE as usize / 5)
}

pub fn octave_error(hz: f32, truth: f32) -> bool {
    let difference = 1200.0 * (hz / truth).log2();
    let octave = (difference / 1200.0).round();
    octave != 0.0 && 1200.0_f32.mul_add(-octave, difference).abs() <= 100.0
}

struct StringTone {
    delay: Vec<f64>,
    position: usize,
    rate: f64,
}

impl StringTone {
    fn new(rate: u32, hz: f32, lowest: f32, pluck: f64, seed: u32) -> Self {
        let period = f64::from(rate) / f64::from(hz);
        let length = (f64::from(rate) / f64::from(lowest)).ceil() as usize + 4;
        // Start at the rising zero of the shaped excitation so its first
        // negative lobe is complete, including the fractional read position.
        let phase_offset = pluck.mul_add(0.5, -(length as f64 - period + 0.5) / period);
        let mut noise = seed;
        let delay = (0..length)
            .map(|index| {
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let phase = (index as f64 / period + phase_offset).rem_euclid(1.0);
                // A triangular displacement's modal amplitudes depend on pick
                // position; a quarter-length pluck strengthens the second mode,
                // while a midpoint pluck cancels that mode.
                let displacement = if phase < pluck {
                    phase / pluck
                } else {
                    (1.0 - phase) / (1.0 - pluck)
                };
                let grain = (f64::from(noise) / f64::from(u32::MAX)).mul_add(2.0, -1.0);
                (displacement - 0.5).mul_add(0.9, grain * 0.005)
            })
            .collect();
        Self {
            delay,
            position: 0,
            rate: f64::from(rate),
        }
    }

    fn read(&self, delay: f64) -> f64 {
        let at = (self.position as f64 - delay).rem_euclid(self.delay.len() as f64);
        let before = at.floor() as usize;
        let fraction = at - before as f64;
        let after = (before + 1) % self.delay.len();
        self.delay[before].mul_add(1.0 - fraction, self.delay[after] * fraction)
    }

    fn push(&mut self, hz: f32) -> f32 {
        let delay = self.rate / f64::from(hz) - 0.5;
        let output = self.read(delay);
        self.delay[self.position] = 0.996 * 0.5 * (output + self.read(delay + 1.0));
        self.position = (self.position + 1) % self.delay.len();
        output as f32
    }
}

/// Retain the suite's notes and sample-clock onsets, varying only excitation.
pub fn at_position(take: &Take, pluck: f64) -> Take {
    let mut output = take.clone();
    output.samples.fill(0.0);
    for (note, &onset) in take.onsets.iter().enumerate() {
        let hz = truth_at(take, onset + SCORE_STEP).expect("pluck pitch");
        let mut string = StringTone::new(take.rate, hz, hz, pluck, note as u32 + 1);
        for sample in &mut output.samples[onset..note_end(take, note)] {
            *sample = string.push(hz);
        }
    }
    output
}

/// A pluck glides or vibrates from its very first sample, with exact truth.
pub fn moving(take: &Take, hz: impl Fn(f64) -> f32) -> Take {
    let onset = take.onsets[0];
    let body = 2 * take.rate as usize;
    let mut output = take.clone();
    output.samples = vec![0.0; onset + body + take.rate as usize / 5];
    output.onsets = vec![onset];
    let mut string = StringTone::new(take.rate, hz(0.0), hz(0.0) * 0.95, 0.5, 1);
    for (index, sample) in output.samples[onset..onset + body].iter_mut().enumerate() {
        *sample = string.push(hz(index as f64 / f64::from(take.rate)));
    }
    output.truth = (0..output.samples.len().div_ceil(SCORE_STEP))
        .map(|step| {
            let at = (step * SCORE_STEP + SCORE_STEP / 2).min(output.samples.len() - 1);
            (at >= onset && at < onset + body)
                .then(|| hz((at - onset) as f64 / f64::from(take.rate)))
        })
        .collect();
    output
}
