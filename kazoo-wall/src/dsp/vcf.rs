//! The filter: a topology-preserving state-variable filter morphing from
//! low-pass through band-pass to high-pass, with input drive.
//!
//! - **Drive** is a tanh saturator mixed with the dry input by the drive
//!   knob. It runs oversampled (see [`super::oversample`]) so its
//!   harmonics do not fold back as inharmonic whine. With the drive at
//!   zero the input goes straight to the filter and the oversampling rests;
//!   when the drive comes up the oversampling is first run over the last
//!   few input samples so it starts settled, and the change between the two
//!   paths crossfades over one sub-block.
//! - **Resonance** is capped below self-oscillation (0.95), so the filter is
//!   stable at any cutoff.
//! - **Ceiling:** the output is untouched up to ±2 and rounded off above
//!   it, never past ±4, so only a loud resonant peak is shaped.

use std::f32::consts::PI;

use super::oversample::{Decimator, Interpolator, MAX_FACTOR, factor_for};
use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;

const CUTOFF: usize = 0;
const RESONANCE: usize = 1;
const MODE: usize = 2;
const DRIVE: usize = 3;

const IN_AUDIO: usize = 0;

/// Where the output starts to be rounded off.
const KNEE: f32 = 2.0;

/// The most the output can reach.
const CEILING: f32 = 4.0;

/// The rate the drive runs at, at least.
const DRIVE_RATE: f32 = 352_800.0;

/// Input samples kept to settle the oversampling before the drive comes in:
/// longer than its filters take to forget.
const PRIMING: usize = 64;

#[derive(Debug)]
pub struct Vcf {
    rate: f32,
    ic1: f32,
    ic2: f32,
    up: Interpolator,
    down: Decimator,
    /// Whether the last sub-block used the drive path.
    driven: bool,
    /// The last [`PRIMING`] input samples, and where the next goes.
    recent: [f32; PRIMING],
    recent_at: usize,
}

impl Vcf {
    pub fn new(sample_rate: f32) -> Self {
        let factor = factor_for(sample_rate, DRIVE_RATE);
        Self {
            rate: sample_rate,
            ic1: 0.0,
            ic2: 0.0,
            up: Interpolator::new(factor),
            down: Decimator::new(factor),
            driven: false,
            recent: [0.0; PRIMING],
            recent_at: 0,
        }
    }

    /// Settle the oversampling on the recent input, as if it had been
    /// running all along.
    fn prime(&mut self) {
        self.up.reset();
        self.down.reset();
        for offset in 0..PRIMING {
            let sample = self.recent[(self.recent_at + offset) % PRIMING];
            self.drive(sample, 0.0);
        }
    }

    /// The input through the oversampled drive at `drive` (0 to 1).
    fn drive(&mut self, dry: f32, drive: f32) -> f32 {
        let factor = self.up.factor();
        let mut oversampled = [0.0; MAX_FACTOR];
        self.up.process(f64::from(dry), &mut oversampled);
        if drive > 0.0 {
            let gain = f64::from(drive.mul_add(4.0, 1.0));
            let drive = f64::from(drive);
            for sample in oversampled.iter_mut().take(factor) {
                let x = *sample;
                *sample = ((x * gain).tanh() - x).mul_add(drive, x);
            }
        }
        self.down.process(&oversampled[..factor]) as f32
    }
}

/// Filter coefficients for one cutoff and resonance.
#[derive(Debug, Clone, Copy)]
struct Coefficients {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Coefficients {
    fn new(cutoff: f32, resonance: f32, sample_rate: f32) -> Self {
        let top = (sample_rate * 0.45).min(18_000.0);
        let cutoff = finite(cutoff).clamp(20.0, top);
        let g = (PI * cutoff / sample_rate).tan();
        let k = 2.0f32.mul_add(-finite(resonance).clamp(0.0, 0.95), 2.0);
        let a1 = 1.0 / g.mul_add(g + k, 1.0);
        let a2 = g * a1;
        Self {
            k,
            a1,
            a2,
            a3: g * a2,
        }
    }
}

/// The output ceiling: exactly linear up to [`KNEE`], then rounding off
/// smoothly (matching slope at the knee) towards [`CEILING`].
fn ceiling(y: f32) -> f32 {
    let magnitude = y.abs();
    if magnitude <= KNEE {
        return y;
    }
    let room = CEILING - KNEE;
    let over = room.mul_add(((magnitude - KNEE) / room).tanh(), KNEE);
    over.copysign(y)
}

impl Module for Vcf {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        if tick.sample_rate.to_bits() != self.rate.to_bits() {
            *self = Self::new(tick.sample_rate);
        }
        let modulated = io.knob_moves(CUTOFF) || io.knob_moves(RESONANCE);
        let mut coefficients = Coefficients::new(
            io.knob_at(CUTOFF, 0),
            io.knob_at(RESONANCE, 0),
            tick.sample_rate,
        );
        let mut drive_at = [0.0; SUB_BLOCK];
        for (frame, drive) in drive_at.iter_mut().enumerate() {
            *drive = io.knob_at(DRIVE, frame);
        }
        let driven = drive_at.iter().any(|drive| *drive > 0.0);
        if driven && !self.driven {
            self.prime();
        }
        let fading = driven != self.driven;
        self.driven = driven;
        for (frame, &drive) in drive_at.iter().enumerate() {
            if modulated && frame > 0 {
                coefficients = Coefficients::new(
                    io.knob_at(CUTOFF, frame),
                    io.knob_at(RESONANCE, frame),
                    tick.sample_rate,
                );
            }
            let dry = finite(io.inputs[IN_AUDIO][frame]).clamp(-16.0, 16.0);
            self.recent[self.recent_at] = dry;
            self.recent_at = (self.recent_at + 1) % PRIMING;
            let wet = if driven || fading {
                self.drive(dry, drive)
            } else {
                dry
            };
            let x = if fading {
                // Frame is below SUB_BLOCK: exact.
                let t = (frame + 1) as f32 / SUB_BLOCK as f32;
                let t = if driven { t } else { 1.0 - t };
                (wet - dry).mul_add(t, dry)
            } else if driven {
                wet
            } else {
                dry
            };
            let Coefficients { k, a1, a2, a3 } = coefficients;
            let v3 = x - self.ic2;
            let v1 = a1.mul_add(self.ic1, a2 * v3);
            let v2 = a3.mul_add(v3, a2.mul_add(self.ic1, self.ic2));
            self.ic1 = finite(2.0f32.mul_add(v1, -self.ic1));
            self.ic2 = finite(2.0f32.mul_add(v2, -self.ic2));
            let low = v2;
            // Band-pass normalised to unity at its peak.
            let band = v1 * k;
            let high = k.mul_add(-v1, x) - v2;
            let mode = io.knob_at(MODE, frame);
            let y = if mode <= 1.0 {
                (band - low).mul_add(mode, low)
            } else {
                (high - band).mul_add(mode - 1.0, band)
            };
            io.outputs[0][frame] = ceiling(finite(y));
        }
        if self.ic1.abs() < 1e-20 {
            self.ic1 = 0.0;
        }
        if self.ic2.abs() < 1e-20 {
            self.ic2 = 0.0;
        }
        self.up.flush();
        self.down.flush();
    }

    fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
        self.up.reset();
        self.down.reset();
        self.driven = false;
        self.recent = [0.0; PRIMING];
        self.recent_at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, RATE, db, stays_in_range, survives_nonsense, tone};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_indices_match_the_catalogue() {
        let spec = Kind::VCF.spec();
        for (index, name) in [
            (CUTOFF, "cutoff"),
            (RESONANCE, "resonance"),
            (MODE, "mode"),
            (DRIVE, "drive"),
        ] {
            assert_eq!(spec.knobs[index].name, name);
        }
        assert_eq!(spec.inputs[IN_AUDIO].name, "in");
    }

    /// RMS of the filter's response to a sine at `hz`, after settling.
    fn response(mode: f32, cutoff: f32, hz: f32) -> f32 {
        let mut bench = Bench::new(Kind::VCF);
        bench
            .knob("mode", mode)
            .knob("cutoff", cutoff)
            .knob("resonance", 0.0);
        let step = hz / RATE;
        let samples = bench.render_fed(0, 24_000, Some("in"), |frame| {
            (std::f32::consts::TAU * step * frame as f32).sin()
        });
        let tail = &samples[12_000..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    #[test]
    fn modes_pass_what_they_should() {
        // Low-pass at 500 Hz: lows through, highs cut.
        assert!(response(0.0, 500.0, 100.0) > 0.6);
        assert!(response(0.0, 500.0, 8_000.0) < 0.02);
        // High-pass: the other way round.
        assert!(response(2.0, 500.0, 100.0) < 0.05);
        assert!(response(2.0, 500.0, 8_000.0) > 0.6);
        // Band-pass: the centre through, both sides cut.
        assert!(response(1.0, 1_000.0, 1_000.0) > 0.6);
        assert!(response(1.0, 1_000.0, 50.0) < 0.1);
        assert!(response(1.0, 1_000.0, 16_000.0) < 0.1);
    }

    #[test]
    fn the_cutoff_input_moves_five_octaves_per_volt() {
        let mut bench = Bench::new(Kind::VCF);
        bench
            .knob("cutoff", 200.0)
            .knob("resonance", 0.0)
            .hold("cutoff", 1.0);
        // 200 Hz up five octaves is 6.4 kHz: a 1 kHz sine passes.
        let samples = bench.render_fed(0, 24_000, Some("in"), |frame| {
            (std::f32::consts::TAU * 1_000.0 / RATE * frame as f32).sin()
        });
        let tail = &samples[12_000..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.6, "{rms}");
    }

    #[test]
    fn resonance_is_capped_and_stable() {
        let mut bench = Bench::new(Kind::VCF);
        bench
            .knob("resonance", 5.0)
            .hold("resonance", 10.0)
            .knob("cutoff", 18_000.0);
        let samples = bench.render_fed(0, 48_000, Some("in"), |frame| {
            if frame % 100 == 0 { 1.0 } else { 0.0 }
        });
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= CEILING));
    }

    /// The filter wide open (low-pass at 18 kHz, no resonance) fed a sine
    /// at `hz` and `amplitude`, after settling.
    fn through(rate: f32, drive: f32, hz: f64, amplitude: f64) -> Vec<f32> {
        let mut bench = Bench::at(Kind::VCF, rate);
        bench
            .knob("mode", 0.0)
            .knob("cutoff", 18_000.0)
            .knob("resonance", 0.0)
            .knob("drive", drive);
        let fs = f64::from(rate);
        let samples = bench.render_fed(0, 8_192 + (rate / 4.0) as usize, Some("in"), |frame| {
            (amplitude * (std::f64::consts::TAU * hz * frame as f64 / fs).sin()) as f32
        });
        samples[8_192..].to_vec()
    }

    #[test]
    fn clean_signals_pass_uncoloured() {
        // Before: every signal went through 4·tanh(y/4), −46 dB THD at full
        // scale with the drive at zero.
        let fs = f64::from(RATE);
        let hz = 997.0;
        let samples = through(RATE, 0.0, hz, 1.0);
        let fundamental = tone(&samples, hz, fs);
        for harmonic in 2..=6 {
            let level = db(tone(&samples, hz * f64::from(harmonic), fs) / fundamental);
            assert!(level < -120.0, "harmonic {harmonic}: {level:.1} dBc");
        }
        assert_eq!(ceiling(1.999).to_bits(), 1.999_f32.to_bits());
        assert_eq!(ceiling(-2.0).to_bits(), (-2.0_f32).to_bits());
        assert!(ceiling(3.0) > 2.9 && ceiling(1_000.0) <= CEILING);
    }

    #[test]
    fn drive_does_not_alias() {
        // Before (no oversampling): a 4987 Hz sine at full drive aliased at
        // −25 dBc at 48 kHz; 7919 Hz at −67 dBc even at 192 kHz.
        for rate in [48_000.0_f32, 96_000.0, 192_000.0] {
            let fs = f64::from(rate);
            for hz in [4_987.0, 7_919.0] {
                let samples = through(rate, 1.0, hz, 1.0);
                let fundamental = tone(&samples, hz, fs);
                // Every product of the odd harmonics folding about the rate.
                let mut worst = f64::NEG_INFINITY;
                for harmonic in (3..=41).step_by(2) {
                    let product = hz * f64::from(harmonic);
                    let folded = (product % fs).min(fs - product % fs);
                    let nearest = (folded / hz).round() * hz;
                    if folded < 20_000.0 && (folded - nearest).abs() > 20.0 && product > fs / 2.0 {
                        worst = worst.max(db(tone(&samples, folded, fs) / fundamental));
                    }
                }
                assert!(worst < -90.0, "{rate} {hz} Hz: {worst:.1} dBc");
            }
        }
    }

    #[test]
    fn turning_the_drive_on_does_not_click() {
        // A 100 Hz sine, the drive turned from 0 to a touch: no jump bigger
        // than the sine's own steepest step, give or take.
        let mut bench = Bench::new(Kind::VCF);
        bench
            .knob("cutoff", 18_000.0)
            .knob("resonance", 0.0)
            .knob("drive", 0.0);
        let fs = f64::from(RATE);
        let sine =
            |frame: usize| (0.5 * (std::f64::consts::TAU * 100.0 * frame as f64 / fs).sin()) as f32;
        let mut samples = bench.render_fed(0, 4_800, Some("in"), sine);
        bench.knob("drive", 0.01);
        samples.extend(bench.render_fed(0, 4_800, Some("in"), |frame| sine(frame + 4_800)));
        let steepest = 0.5 * std::f32::consts::TAU * 100.0 / RATE;
        let jump = samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(jump < steepest * 1.5, "{jump} vs {steepest}");
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::VCF, CEILING);
        survives_nonsense(Kind::VCF);
    }
}
