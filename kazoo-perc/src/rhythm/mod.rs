//! The rhythm generators. Each lives in its own file and is listed in
//! [`KINDS`]; this module holds what they share: reading clock edges,
//! measuring the clock, holding knob values and writing gate blocks.

pub mod burst;
pub mod euclid;
pub mod grids;
pub mod poly;
pub mod prob;

use kazoo_fx::ParamSpec;
use kazoo_fx::dsp::Noise;

use crate::{MAX_OUTPUTS, RhythmKind};

/// Every rhythm generator.
pub static KINDS: &[RhythmKind] = &[
    euclid::KIND,
    prob::KIND,
    grids::KIND,
    poly::KIND,
    burst::KIND,
];

/// Gate level for high.
pub(crate) const HIGH: f32 = 1.0;

/// Gate level for low.
pub(crate) const LOW: f32 = 0.0;

/// A gate or clock input read sample by sample.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Edge {
    high: bool,
}

impl Edge {
    /// Read the next sample: returns whether it rose on this sample and
    /// whether it is high. Above 0.5 is high; a NaN is low.
    pub(crate) fn next(&mut self, sample: f32) -> (bool, bool) {
        let high = sample > 0.5;
        let rising = high && !self.high;
        self.high = high;
        (rising, high)
    }
}

/// Measures the time between clock edges.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Meter {
    since: u32,
    seen: bool,
    period: u32,
}

impl Meter {
    /// Count a sample; on a clock edge, the time since the last one becomes
    /// the period.
    pub(crate) const fn tick(&mut self, rising: bool) {
        if rising {
            if self.seen && self.since > 0 {
                self.period = self.since;
            }
            self.seen = true;
            self.since = 0;
        }
        self.since = self.since.saturating_add(1);
    }

    /// The last clock period in samples, once two edges have been seen.
    pub(crate) const fn period(&self) -> Option<u32> {
        if self.period > 0 {
            Some(self.period)
        } else {
            None
        }
    }

    /// Forget every edge.
    pub(crate) const fn clear(&mut self) {
        *self = Self {
            since: 0,
            seen: false,
            period: 0,
        };
    }
}

/// A generator's knob values, each held in its range. Rhythm knobs take
/// effect at the next step, so they need no glide.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Knobs<const N: usize> {
    specs: &'static [ParamSpec; N],
    values: [f32; N],
}

impl<const N: usize> Knobs<N> {
    /// Every knob at its default.
    pub(crate) fn new(specs: &'static [ParamSpec; N]) -> Self {
        Self {
            specs,
            values: std::array::from_fn(|index| specs[index].default),
        }
    }

    /// Set knob `index` to `value`, clamped; NaN and unknown indices are
    /// ignored. Returns whether anything was set.
    pub(crate) fn set(&mut self, index: usize, value: f32) -> bool {
        if value.is_nan() {
            return false;
        }
        match (self.specs.get(index), self.values.get_mut(index)) {
            (Some(spec), Some(slot)) => {
                *slot = spec.clamp(value);
                true
            }
            _ => false,
        }
    }

    /// Knob `index` (0 for one out of range, which is never asked for).
    pub(crate) fn get(&self, index: usize) -> f32 {
        self.values.get(index).copied().unwrap_or(0.0)
    }

    /// Knob `index` as a whole number.
    pub(crate) fn count(&self, index: usize) -> usize {
        self.get(index).round().max(0.0) as usize
    }
}

/// One sample's worth of generator: given the clock and reset readings,
/// the level of every output.
pub(crate) trait Stepper {
    /// Run one sample. `clock_rise` and `clock_high` read the clock,
    /// `reset_rise` the reset input (already acted on when this is called
    /// for the same sample). Returns every output's level; the ones past
    /// the generator's own outputs are low.
    fn tick(&mut self, clock_rise: bool, clock_high: bool) -> [f32; MAX_OUTPUTS];

    /// Go back to the start, every output low.
    fn restart(&mut self);
}

/// The input edges a generator reads.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Inputs {
    pub(crate) clock: Edge,
    pub(crate) reset: Edge,
}

/// Run a block through `stepper`, as [`crate::Rhythm::process`] describes.
pub(crate) fn drive<S: Stepper>(
    stepper: &mut S,
    inputs: &mut Inputs,
    clock: &[f32],
    reset: &[f32],
    outputs: &mut [&mut [f32]],
) {
    let mut len = clock.len().min(reset.len());
    for out in outputs.iter() {
        len = len.min(out.len());
    }
    for (index, (&clock_in, &reset_in)) in clock.iter().zip(reset).take(len).enumerate() {
        let (reset_rise, _) = inputs.reset.next(reset_in);
        if reset_rise {
            stepper.restart();
        }
        let (clock_rise, clock_high) = inputs.clock.next(clock_in);
        let levels = stepper.tick(clock_rise, clock_high);
        for (out, level) in outputs.iter_mut().zip(levels) {
            if let Some(slot) = out.get_mut(index) {
                *slot = level;
            }
        }
    }
    for (index, out) in outputs.iter_mut().enumerate() {
        let from = if index < MAX_OUTPUTS { len } else { 0 };
        if let Some(rest) = out.get_mut(from..) {
            rest.fill(LOW);
        }
    }
}

/// A gate level.
pub(crate) const fn gate(high: bool) -> f32 {
    if high { HIGH } else { LOW }
}

/// A random number from 0 up to 1 (never reaching 1).
pub(crate) fn uniform(noise: &mut Noise) -> f32 {
    (noise.next_u32() >> 8) as f32 / 16_777_216.0
}
