//! Noise: white, pink and brown, morphing continuously.
//!
//! The noise sounds the same at every stream rate: it is made to measure
//! against 48 kHz. Above that, the white source is band-limited to 22 kHz
//! (a fourth-order Butterworth low-pass) and raised by √(rate / 48 kHz), so
//! its density in the audible band is what it is at 48 kHz, and no
//! ultrasonic hiss reaches the master or the amplifiers. The pink and brown
//! filters keep their corners in hertz, not in samples.

use super::{Io, Module, Rng, Tick, finite, seed};
use crate::SUB_BLOCK;

const COLOUR: usize = 0;
const LEVEL: usize = 1;

/// The rate the colours are defined at.
const REFERENCE_RATE: f32 = 48_000.0;

/// Where the white source is cut above the reference rate.
const BAND_LIMIT_HZ: f32 = 22_000.0;

/// Paul Kellet's pink filter: the pole and input gain of each one-pole
/// branch, at the reference rate.
const PINK_BRANCHES: [(f32, f32); 5] = [
    (0.998_86, 0.055_517_9),
    (0.993_32, 0.075_075_9),
    (0.969_00, 0.153_852),
    (0.866_50, 0.310_485_6),
    (0.550_00, 0.532_952_2),
];

/// The brown integrator's pole at the reference rate (about 150 Hz).
const BROWN_POLE: f32 = 1.0 / 1.02;

/// Output bound: band-limited noise has peaks well past its average level.
const LIMIT: f32 = 4.0;

/// One second-order section of the band limit (a TPT state-variable
/// low-pass).
#[derive(Debug, Clone, Copy)]
struct LowPass {
    g: f32,
    k: f32,
    s1: f32,
    s2: f32,
}

impl LowPass {
    fn new(sample_rate: f32, q: f32) -> Self {
        let cutoff = BAND_LIMIT_HZ.min(sample_rate * 0.45);
        Self {
            g: (std::f32::consts::PI * cutoff / sample_rate).tan(),
            k: 1.0 / q,
            s1: 0.0,
            s2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let Self { g, k, .. } = *self;
        let high = (x - (g + k).mul_add(self.s1, self.s2)) / g.mul_add(g + k, 1.0);
        let band = g.mul_add(high, self.s1);
        self.s1 = g.mul_add(high, band);
        let low = g.mul_add(band, self.s2);
        self.s2 = g.mul_add(band, low);
        self.s1 = finite(self.s1);
        self.s2 = finite(self.s2);
        low
    }
}

#[derive(Debug)]
pub struct Noise {
    rate: f32,
    rng: Rng,
    /// The white source's gain and band limit (none at or below the
    /// reference rate).
    gain: f32,
    band_limit: Option<[LowPass; 2]>,
    /// Pink branches: pole, input gain, state.
    pink: [(f32, f32, f32); 5],
    /// The two high-frequency terms of Kellet's filter.
    pink_top: [f32; 2],
    brown_pole: f32,
    brown: f32,
}

impl Noise {
    pub fn new(sample_rate: f32) -> Self {
        let rate = sample_rate;
        let ratio = REFERENCE_RATE / rate;
        let above = rate > REFERENCE_RATE * 1.25;
        let pink = PINK_BRANCHES.map(|(pole, gain)| {
            // Same corner in hertz, same gain at DC.
            let moved = pole.powf(ratio);
            (moved, gain * (1.0 - moved) / (1.0 - pole), 0.0)
        });
        Self {
            rate,
            rng: Rng::new(seed()),
            gain: if above {
                (rate / REFERENCE_RATE).sqrt()
            } else {
                1.0
            },
            // Butterworth fourth order: two sections, Q 0.541 and 1.307.
            band_limit: above.then(|| {
                [
                    LowPass::new(rate, 0.541_196_1),
                    LowPass::new(rate, 1.306_563),
                ]
            }),
            pink,
            pink_top: [0.0; 2],
            brown_pole: BROWN_POLE.powf(ratio),
            brown: 0.0,
        }
    }

    fn white(&mut self) -> f32 {
        let white = self.rng.bipolar() * self.gain;
        match &mut self.band_limit {
            Some([first, second]) => second.process(first.process(white)),
            None => white,
        }
    }

    fn pink(&mut self, white: f32) -> f32 {
        let mut sum = white.mul_add(0.536_2, self.pink_top[1]);
        for (pole, gain, state) in &mut self.pink {
            *state = pole.mul_add(*state, white * *gain);
            sum += *state;
        }
        self.pink_top[0] = (-0.761_6f32).mul_add(self.pink_top[0], -white * 0.016_898_0);
        sum += self.pink_top[0];
        self.pink_top[1] = white * 0.115_926;
        sum * 0.11
    }

    fn brown(&mut self, white: f32) -> f32 {
        self.brown = (self.brown - white).mul_add(self.brown_pole, white);
        self.brown * 3.5
    }
}

impl Module for Noise {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        if tick.sample_rate.to_bits() != self.rate.to_bits() {
            *self = Self::new(tick.sample_rate);
        }
        for frame in 0..SUB_BLOCK {
            let colour = io.knob_at(COLOUR, frame);
            let white = self.white();
            let pink = self.pink(white);
            let brown = self.brown(white);
            let sample = if colour <= 1.0 {
                (pink - white).mul_add(colour, white)
            } else {
                (brown - pink).mul_add(colour - 1.0, pink)
            };
            let level = io.knob_at(LEVEL, frame);
            io.outputs[0][frame] = finite(sample * level).clamp(-LIMIT, LIMIT);
        }
        for (_, _, state) in &mut self.pink {
            *state = finite(*state);
        }
        self.pink_top = self.pink_top.map(finite);
        self.brown = finite(self.brown);
    }

    fn reset(&mut self) {
        for (_, _, state) in &mut self.pink {
            *state = 0.0;
        }
        self.pink_top = [0.0; 2];
        self.brown = 0.0;
        if let Some(sections) = &mut self.band_limit {
            for section in sections {
                section.s1 = 0.0;
                section.s2 = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_indices_match_the_catalogue() {
        let spec = Kind::NOISE.spec();
        assert_eq!(spec.knobs[COLOUR].name, "colour");
        assert_eq!(spec.knobs[LEVEL].name, "level");
    }

    /// Average absolute difference between neighbouring samples, relative
    /// to the average level: high for white, low for brown.
    fn roughness(samples: &[f32]) -> f32 {
        let level: f32 = samples.iter().map(|s| s.abs()).sum::<f32>() / samples.len() as f32;
        let steps: f32 =
            samples.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / samples.len() as f32;
        steps / level
    }

    #[test]
    fn colours_get_darker() {
        let mut rough = Vec::new();
        for colour in [0.0, 1.0, 2.0] {
            let mut bench = Bench::new(Kind::NOISE);
            bench.knob("colour", colour).knob("level", 1.0);
            let samples = bench.render(0, 48_000);
            let rms = (samples.iter().map(|s| s * s).sum::<f32>() / 48_000.0).sqrt();
            assert!(rms > 0.02, "colour {colour} is too quiet: {rms}");
            rough.push(roughness(&samples));
        }
        assert!(rough[0] > rough[1] && rough[1] > rough[2], "{rough:?}");
    }

    #[test]
    fn level_zero_is_silent() {
        let mut bench = Bench::new(Kind::NOISE);
        bench.knob("level", 0.0);
        bench.render(0, 4_800);
        assert!(bench.render(0, 480).iter().all(|s| s.abs() < 1e-3));
    }

    /// Power of `samples` through a band-pass (Q 2) at `hz`: the same
    /// width in hertz at every rate, so it compares densities across rates.
    fn band(samples: &[f32], hz: f64, rate: f64) -> f64 {
        let g = (std::f64::consts::PI * hz / rate).tan();
        let k = 0.5;
        let (mut s1, mut s2, mut power) = (0.0_f64, 0.0_f64, 0.0);
        for (i, x) in samples.iter().enumerate() {
            let high = (f64::from(*x) - (g + k).mul_add(s1, s2)) / g.mul_add(g + k, 1.0);
            let band = g.mul_add(high, s1);
            s1 = g.mul_add(high, band);
            let low = g.mul_add(band, s2);
            s2 = g.mul_add(band, low);
            if i > samples.len() / 8 {
                power = band.mul_add(band, power);
            }
        }
        power / samples.len() as f64
    }

    #[test]
    fn every_colour_sounds_the_same_at_every_rate() {
        // Before: white was 6 dB quieter in the audible band at 192 kHz, and
        // brown lost 20 dB of its bass.
        for colour in [0.0, 1.0, 2.0] {
            let render = |rate: f32| {
                let mut bench = Bench::at(Kind::NOISE, rate);
                bench.knob("colour", colour).knob("level", 1.0);
                bench.render(0, (rate * 8.0) as usize)
            };
            let reference = render(48_000.0);
            for rate in [96_000.0, 192_000.0] {
                let samples = render(rate);
                // A band-pass of the same Q is squeezed near the top of a
                // slower rate, so compare where both are true to width.
                for hz in [100.0, 1_000.0, 4_000.0] {
                    let difference = 10.0
                        * (band(&samples, hz, f64::from(rate)) / band(&reference, hz, 48_000.0))
                            .log10();
                    assert!(
                        difference.abs() < 0.6,
                        "colour {colour} {rate} {hz} Hz: {difference:.2} dB"
                    );
                }
            }
        }
    }

    #[test]
    fn no_ultrasonic_hiss_above_48_khz() {
        let rate = 192_000.0;
        let mut bench = Bench::at(Kind::NOISE, rate);
        bench.knob("colour", 0.0).knob("level", 1.0);
        let samples = bench.render(0, 192_000);
        let audible = band(&samples, 10_000.0, f64::from(rate));
        let ultrasonic = band(&samples, 60_000.0, f64::from(rate));
        // Per hertz: the band-pass is six times wider at 60 kHz. White noise
        // used to be as dense up there as at 10 kHz; the band limit is 35 dB
        // down at 60 kHz, which this gentle band-pass reads as about 24.
        let below = 10.0 * (ultrasonic / 6.0 / audible).log10();
        assert!(below < -20.0, "{below:.1} dB");
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::NOISE, LIMIT);
        survives_nonsense(Kind::NOISE);
    }
}
