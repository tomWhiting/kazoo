//! Every effect sounds the same at every sample rate, from 44.1 kHz to
//! 384 kHz: the same level, the same timing and no more noise, for tones
//! from 100 Hz to 15 kHz. Latency is checked here too.

use super::KINDS;
use super::testkit::context;
use crate::{Effect, EffectKind};

const RATES: [f32; 6] = [48_000.0, 44_100.0, 88_200.0, 96_000.0, 192_000.0, 384_000.0];
const TONES_HZ: [f64; 4] = [100.0, 1_000.0, 5_000.0, 15_000.0];
const TONE_LEVEL: f64 = 0.25;
/// Where the shifter is set, so its output tone sits clear of its input.
const SHIFT_HZ: f32 = 200.0;
/// The quietest residue worth comparing, as a power in dB against full
/// scale (a full-scale sine is -3 dB): below what the best converters
/// resolve (about -125 dB). Taken absolutely, not against the tone, because an effect
/// that darkens (a tape's gap loss takes 24 dB off 15 kHz) leaves its tone
/// quiet and a residue far below hearing would read as high.
const NOISE_FLOOR_DB: f64 = -130.0;
/// A tone this far down at 48 kHz, in dB, is in the effect's stopband.
const STOPBAND_DB: f64 = -60.0;
/// How far under the tone going in whatever leaks of a tone the effect
/// rejects must stay, in dB.
const STOPBAND_LEAK_DB: f64 = 80.0;
/// The most harmonics taken out of a tone before measuring what is left.
const HARMONICS: usize = 8;

/// Each effect held still and linear: no modulation, no feedback, fully
/// wet, and no deliberate noise or drive.
fn still(kind: &EffectKind, rate: f32) -> Box<dyn Effect> {
    let mut effect = (kind.build)();
    effect.prepare(rate);
    for (index, spec) in kind.params.iter().enumerate() {
        let value = match spec.name {
            "feedback" | "colour" | "depth" | "wow" | "flutter" | "hiss" | "drive" | "detune"
            | "reverse" | "duck" => 0.0,
            "spread" if kind.id == "granular" => 0.0,
            "mix" => 1.0,
            "shift" => SHIFT_HZ,
            _ => continue,
        };
        effect.set_param(index, value);
    }
    effect.reset();
    effect
}

fn run(effect: &mut dyn Effect, input: &[f32]) -> Vec<f32> {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((chunk, out_left), out_right) in input
        .chunks(512)
        .zip(left.chunks_mut(512))
        .zip(right.chunks_mut(512))
    {
        effect.process(&context(120.0), [chunk, chunk], [out_left, out_right]);
    }
    left
}

/// How loud the tone at `hz` is in `signal`, and how far below it
/// everything else below 20 kHz sits once the tone and its first
/// harmonics have been fitted and subtracted sample by sample, in
/// decibels. `signal` must hold a whole number of cycles of `hz`, so the
/// fits are independent.
fn fit(signal: &[f32], hz: f64, rate: f32) -> (f64, f64) {
    let len = signal.len() as f64;
    let mut residual: Vec<f64> = signal.iter().map(|v| f64::from(*v)).collect();
    let mut tone = 0.0;
    let highest = ((0.45 * f64::from(rate) / hz).floor() as usize).clamp(1, HARMONICS);
    for count in 1..=highest {
        let step = std::f64::consts::TAU * hz * count as f64 / f64::from(rate);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, sample) in residual.iter().enumerate() {
            let (sin, cos) = (step * n as f64).sin_cos();
            re = sample.mul_add(cos, re);
            im = sample.mul_add(sin, im);
        }
        let (a, b) = (2.0 * re / len, 2.0 * im / len);
        if count == 1 {
            tone = a.hypot(b);
        }
        for (n, sample) in residual.iter_mut().enumerate() {
            let (sin, cos) = (step * n as f64).sin_cos();
            *sample -= a.mul_add(cos, b * sin);
        }
    }
    audible(&mut residual, rate);
    // The filter's own start-up is left out of the measurement.
    let settled = &residual[residual.len() / 4..];
    let rest = settled.iter().map(|v| v * v).sum::<f64>() / settled.len() as f64;
    let power = 0.5 * tone * tone;
    (tone, 10.0 * (rest.max(1e-30) / power.max(1e-30)).log10())
}

/// `signal` through an eighth-order Butterworth lowpass at 20 kHz (four
/// bilinear sections), so what is measured as noise is what can be heard:
/// an oversampled effect's decimation keeps the audible band clean by
/// design and lets harmonics past 20 kHz fold back above it, where at
/// 88.2 kHz and up they would otherwise count.
fn audible(signal: &mut [f64], rate: f32) {
    let rate = f64::from(rate);
    let w = std::f64::consts::TAU * 20_000.0f64.min(0.45 * rate) / rate;
    let (sin, cos) = w.sin_cos();
    for q in [0.509_8, 0.601_3, 0.900_0, 2.562_9] {
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b0 = 0.5 * (1.0 - cos) / a0;
        let (b1, b2) = (2.0 * b0, b0);
        let (a1, a2) = (-2.0 * cos / a0, (1.0 - alpha) / a0);
        let (mut z1, mut z2) = (0.0, 0.0);
        for sample in signal.iter_mut() {
            let x = *sample;
            let y = b0.mul_add(x, z1);
            z1 = b1.mul_add(x, (-a1).mul_add(y, z2));
            z2 = b2.mul_add(x, -a2 * y);
            *sample = y;
        }
    }
}

struct Measure {
    gain_db: [f64; TONES_HZ.len()],
    noise_db: [f64; TONES_HZ.len()],
    peak_seconds: f64,
}

fn measure(kind: &EffectKind, rate: f32) -> Measure {
    let len = rate as usize;
    let mut gain_db = [0.0; TONES_HZ.len()];
    let mut noise_db = [0.0; TONES_HZ.len()];
    for (k, hz) in TONES_HZ.iter().enumerate() {
        let step = std::f64::consts::TAU * hz / f64::from(rate);
        let tone: Vec<f32> = (0..len)
            .map(|n| (TONE_LEVEL * (step * n as f64).sin()) as f32)
            .collect();
        let out = run(still(kind, rate).as_mut(), &tone);
        let heard = if kind.id == "shift" {
            hz + f64::from(SHIFT_HZ)
        } else {
            *hz
        };
        let (amplitude, noise) = fit(&out[len / 2..], heard, rate);
        gain_db[k] = 20.0 * (amplitude / TONE_LEVEL).max(1e-12).log10();
        // From against the tone to against full scale.
        noise_db[k] = 10.0f64.mul_add((0.5 * amplitude * amplitude).max(1e-30).log10(), noise);
    }
    let mut click = vec![0.0; len];
    click[0] = 1.0;
    let out = run(still(kind, rate).as_mut(), &click);
    let loudest = out
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |best, (n, v)| {
            if v.abs() > best.1 { (n, v.abs()) } else { best }
        })
        .0;
    Measure {
        gain_db,
        noise_db,
        peak_seconds: loudest as f64 / f64::from(rate),
    }
}

/// The test tone's power in dB against full scale (a full-scale sine is
/// -3 dB).
fn input_db() -> f64 {
    10.0 * (0.5 * TONE_LEVEL * TONE_LEVEL).log10()
}

/// How far a tone's level may stray from 48 kHz's, given its level there
/// in dB: half a decibel in the passband; on a steep filter's skirt, where
/// a corner moving by a hundredth of an octave moves a tone 34 dB down on
/// a 96 dB-per-octave slope by 1 dB, 3 % of the attenuation; and nothing
/// to hold at all 60 dB or more down, in the stopband, where no one hears
/// the difference.
fn level_allowance(base_db: f64) -> f64 {
    if base_db < STOPBAND_DB {
        f64::INFINITY
    } else {
        0.5f64.max(0.03 * base_db.abs())
    }
}

/// Every rate against 48 kHz, for one effect.
fn the_same_at_every_rate(kind: &EffectKind) {
    let mut failures = Vec::new();
    let base = measure(kind, RATES[0]);
    for rate in RATES {
        let now = measure(kind, rate);
        for (k, hz) in TONES_HZ.iter().enumerate() {
            println!(
                "{:9} {:6} {:6} Hz gain {:7.2} dB  noise {:7.1} dBFS",
                kind.id, rate, hz, now.gain_db[k], now.noise_db[k]
            );
            let allowed = level_allowance(base.gain_db[k]);
            if allowed.is_finite() && (now.gain_db[k] - base.gain_db[k]).abs() > allowed {
                failures.push(format!("{} {rate} {hz} level", kind.id));
            }
            let noisy = if allowed.is_finite() {
                now.noise_db[k] > base.noise_db[k].max(NOISE_FLOOR_DB) + 3.0
            } else {
                // A tone in the stopband: it must stay there, and what leaks
                // of it must stay well under what went in, rather than match
                // another rate's leak.
                now.gain_db[k] > STOPBAND_DB + 3.0
                    || now.noise_db[k] > input_db() - STOPBAND_LEAK_DB
            };
            if noisy {
                failures.push(format!("{} {rate} {hz} noise", kind.id));
            }
        }
        if (now.peak_seconds - base.peak_seconds).abs() > 0.000_25 {
            failures.push(format!("{} {rate} timing", kind.id));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// One cross-rate test per effect, so they run in parallel.
macro_rules! across_rates {
    ($($effect:ident),* $(,)?) => {
        $(
            #[test]
            fn $effect() {
                the_same_at_every_rate(&crate::time::$effect::KIND);
            }
        )*

        #[test]
        fn every_effect_is_checked_across_rates() {
            let checked = [$(stringify!($effect)),*];
            let listed: Vec<&str> = KINDS.iter().map(|kind| kind.id).collect();
            assert_eq!(listed, checked);
        }
    };
}

across_rates!(
    tape, bbd, digital, chorus, flanger, phaser, trem, shift, granular
);

/// Set the parameter called `name`, if `kind` has one.
fn set(effect: &mut dyn Effect, kind: &EffectKind, name: &str, value: f32) {
    if let Some(index) = kind.params.iter().position(|spec| spec.name == name) {
        effect.set_param(index, value);
    }
}

/// Where a click comes out loudest, and how loud.
fn click_peak(effect: &mut dyn Effect, rate: f32) -> (usize, f32) {
    let mut click = vec![0.0; rate as usize / 4];
    click[0] = 1.0;
    let out = run(effect, &click);
    out.iter().enumerate().fold((0, 0.0f32), |best, (n, v)| {
        if v.abs() > best.1 { (n, v.abs()) } else { best }
    })
}

#[test]
fn latency_is_the_heard_processing_delay_at_every_rate() {
    for kind in KINDS {
        for rate in RATES {
            let mut effect = still(kind, rate);
            match kind.id {
                "shift" => {
                    let latency = effect.latency();
                    assert!((4..=12).contains(&latency), "{rate} {latency}");
                    let (at, _) = click_peak(effect.as_mut(), rate);
                    assert!(at.abs_diff(latency) <= 2, "{rate} {at} {latency}");
                    set(effect.as_mut(), kind, "mix", 0.0);
                    effect.reset();
                    let (at, size) = click_peak(effect.as_mut(), rate);
                    assert_eq!(at, latency, "{rate}");
                    assert!((size - 1.0).abs() < 1e-6, "{rate} {size}");
                }
                "flanger" => {
                    // Both paths run seven samples behind for the
                    // band-limited read; the dry click lands there.
                    assert_eq!(effect.latency(), 7, "{rate} classic");
                    set(effect.as_mut(), kind, "mix", 0.0);
                    effect.reset();
                    let (at, size) = click_peak(effect.as_mut(), rate);
                    assert_eq!(at, 7, "{rate} classic");
                    assert!((size - 1.0).abs() < 1e-3, "{rate} {size}");
                    set(effect.as_mut(), kind, "mode", 1.0);
                    set(effect.as_mut(), kind, "mix", 0.0);
                    effect.reset();
                    let latency = effect.latency();
                    assert_eq!(latency, (0.01 * rate).round() as usize + 7, "{rate}");
                    let (at, size) = click_peak(effect.as_mut(), rate);
                    assert_eq!(at, latency, "{rate}");
                    assert!((size - 1.0).abs() < 1e-3, "{rate} {size}");
                }
                _ => {
                    assert_eq!(effect.latency(), 0, "{} {rate}", kind.id);
                    // Fully dry (or, without a mix, held still), the click
                    // comes straight through.
                    set(effect.as_mut(), kind, "mix", 0.0);
                    effect.reset();
                    let (at, size) = click_peak(effect.as_mut(), rate);
                    assert_eq!(at, 0, "{} {rate}", kind.id);
                    assert!((size - 1.0).abs() < 1e-3, "{} {rate} {size}", kind.id);
                }
            }
        }
    }
}
