//! The pitch shifter against what tends to trip a time-domain shifter up:
//! attacks (heard once each, never twice or not at all), chords (which no
//! one splice can line up) and host rates far from 48 kHz (the same
//! intervals, formants and level at 44.1, 96 and 192 kHz).

use super::*;
use crate::time::testkit::context;

/// A shifter at `rate` with `knobs` (by index), fully wet.
fn shifter_at(rate: f32, knobs: &[(usize, f32)]) -> Box<dyn Effect> {
    let mut pitch = (KIND.build)();
    pitch.set_param(MIX, 1.0);
    for &(index, value) in knobs {
        pitch.set_param(index, value);
    }
    pitch.prepare(rate);
    pitch
}

/// `input` through `effect` in blocks of 256, the left output.
fn through(effect: &mut dyn Effect, input: &[f32]) -> Vec<f32> {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((chunk, l), r) in input
        .chunks(256)
        .zip(left.chunks_mut(256))
        .zip(right.chunks_mut(256))
    {
        effect.process(&context(120.0), [chunk, chunk], [l, r]);
    }
    left
}

/// The strength of `hz` in `signal` at `rate`, through a Hann window.
fn strength(signal: &[f32], hz: f64, rate: f32) -> f64 {
    let size = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, sample) in signal.iter().enumerate() {
        let window = (-0.5f64).mul_add((std::f64::consts::TAU * n as f64 / size).cos(), 0.5);
        let angle = std::f64::consts::TAU * hz * n as f64 / f64::from(rate);
        let value = f64::from(*sample) * window;
        re = value.mul_add(angle.cos(), re);
        im = value.mul_add(angle.sin(), im);
    }
    4.0 * re.hypot(im) / size
}

/// The strongest frequency within 3% of `near`, to a hundredth of a hertz.
fn loudest_near(signal: &[f32], near: f64, rate: f32) -> f64 {
    let mut best = (near, 0.0);
    let mut step = near * 0.002;
    let (mut low, mut high) = (near * 0.97, near * 1.03);
    for _ in 0..4 {
        let steps = ((high - low) / step).round() as usize;
        for i in 0..=steps {
            let hz = step.mul_add(i as f64, low);
            let level = strength(signal, hz, rate);
            if level > best.1 {
                best = (hz, level);
            }
        }
        low = best.0 - step;
        high = best.0 + step;
        step /= 10.0;
    }
    best.0
}

fn rms(signal: &[f32]) -> f32 {
    (signal.iter().map(|x| x * x).sum::<f32>() / signal.len().max(1) as f32).sqrt()
}

/// A sine, its phase worked out in double precision (in single precision a
/// long tone's phase grows coarse enough to spread a noise floor about
/// -60 dB down).
fn tone(len: usize, hz: f32, level: f32, rate: f32) -> Vec<f32> {
    let step = std::f64::consts::TAU * f64::from(hz) / f64::from(rate);
    (0..len)
        .map(|n| level * (step * n as f64).sin() as f32)
        .collect()
}

/// A pulse train at about 140 Hz through one resonance at 600 Hz, at
/// `rate`, and its pulse period in samples.
fn vowel(len: usize, rate: f32) -> (Vec<f32>, usize) {
    let period = (rate / 140.0).round() as usize;
    let radius = (-std::f32::consts::PI * 115.0 / rate).exp();
    let angle = std::f32::consts::TAU * 600.0 / rate;
    let (a1, a2) = (-2.0 * radius * angle.cos(), radius * radius);
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    let signal = (0..len)
        .map(|n| {
            let pulse = if n % period == 0 { 1.0 } else { 0.0 };
            let y = (-a1).mul_add(y1, (-a2).mul_add(y2, pulse));
            y2 = y1;
            y1 = y;
            y * 0.02 * 48_000.0 / rate
        })
        .collect();
    (signal, period)
}

/// Where the harmonics of `f0` are loudest on average, up to 4 kHz.
fn harmonic_centre(signal: &[f32], f0: f64, rate: f32) -> f64 {
    let (mut weighted, mut total) = (0.0, 0.0);
    for k in 1..=(4_000.0 / f0) as usize {
        let hz = k as f64 * f0;
        let level = strength(signal, hz, rate);
        weighted = hz.mul_add(level, weighted);
        total += level;
    }
    weighted / total.max(1e-12)
}

/// Clicks: short bursts of decaying noise every `spacing` frames.
fn clicks(len: usize, spacing: usize, rate: f32) -> Vec<f32> {
    let mut noise = crate::dsp::Noise::new(41);
    let decay = (-1.0 / (0.001 * rate)).exp();
    let mut envelope = 0.0f32;
    (0..len)
        .map(|n| {
            if n >= spacing / 2 && (n - spacing / 2) % spacing == 0 && n + spacing < len {
                envelope = 0.8;
            }
            envelope *= decay;
            envelope * noise.sample()
        })
        .collect()
}

/// How many separate bursts `signal` holds: rises of its 1 ms level past
/// 0.02, each after at least 4 ms below 0.005 (a burst's own lead-in, part
/// of a window, does not count against it), so a hit played twice even
/// 5 ms apart counts twice. Two octaves up a 1 ms burst
/// lasts a quarter of that and the guard takes most of a noise burst's
/// top, so the threshold sits well under the input's bursts.
fn bursts(signal: &[f32], rate: f32) -> usize {
    let window = (0.001 * rate) as usize;
    let quiet_for = (0.004 * rate) as usize / window;
    let (mut count, mut quiet, mut armed) = (0, 0, true);
    for chunk in signal.chunks(window) {
        let level = rms(chunk);
        quiet = if level < 0.005 { quiet + 1 } else { 0 };
        armed |= quiet >= quiet_for;
        if level > 0.02 && armed {
            count += 1;
            armed = false;
        }
    }
    count
}

#[test]
fn every_attack_comes_out_once() {
    // Bursts 150 ms apart: shifted up, a careless splice replays the last
    // 25 ms (a double hit); shifted down, it skips 25 ms (a lost hit).
    let rate = 48_000.0;
    let input = clicks(96_000, 7_200, rate);
    let heard = bursts(&input, rate);
    assert_eq!(heard, 12);
    for semitones in [12.0, 7.0, 24.0, -5.0, -12.0, -24.0] {
        for formant in [0.0, 1.0] {
            let mut pitch = shifter_at(rate, &[(SHIFT, semitones), (FORMANT, formant)]);
            let out = through(pitch.as_mut(), &input);
            assert_eq!(bursts(&out, rate), heard, "{semitones} {formant}");
        }
    }
}

/// What of `signal`'s spectrum up to 4 kHz lies more than 6 Hz from every
/// one of `notes`, in dB against the whole: a ratio of sums over the same
/// Hann-windowed bins, so the window's own scale cancels.
fn seams(signal: &[f32], notes: &[f64], rate: f32) -> f64 {
    let step = f64::from(rate) / signal.len() as f64;
    let (mut away, mut all) = (0.0f64, 0.0f64);
    for k in 1..=(4_000.0 / step) as usize {
        let hz = k as f64 * step;
        let power = strength(signal, hz, rate).powi(2);
        all += power;
        if notes.iter().all(|note| (hz - note).abs() > 6.0) {
            away += power;
        }
    }
    10.0 * (away.max(1e-30) / all.max(1e-30)).log10()
}

#[test]
fn a_chord_keeps_its_splice_sidebands_down() {
    // Two notes a major third apart: a splice can line up one of them or
    // the other, never both, so each splice leaves a little of a seam, at
    // the splice rate (20 to 59 a second) either side of each note. A
    // single note's seams measured 50 to 57 dB under it, a chord's 31 to
    // 40 dB (the measurement's own floor, with no shift, is 63 dB).
    let rate = 48_000.0;
    let single = tone(72_000, 220.0, 0.3, rate);
    let chord: Vec<f32> = single
        .iter()
        .zip(tone(72_000, 277.18, 0.3, rate))
        .map(|(a, b)| a + b)
        .collect();
    for semitones in [7.0f32, 12.0, -5.0] {
        let ratio = f64::from(semitones / 12.0).exp2();
        for (input, notes, most) in [
            (&single, vec![220.0], -45.0),
            (&chord, vec![220.0, 277.18], -28.0),
        ] {
            let mut pitch = shifter_at(rate, &[(SHIFT, semitones)]);
            let out = through(pitch.as_mut(), input);
            let shifted: Vec<f64> = notes.iter().map(|hz| hz * ratio).collect();
            let level = seams(&out[24_000..], &shifted, rate);
            assert!(level < most, "{semitones} {notes:?}: {level} dB");
        }
    }
}

#[test]
fn every_rate_shifts_keeps_and_holds_alike() {
    for rate in [44_100.0f32, 96_000.0, 192_000.0] {
        let len = (0.6 * rate) as usize;
        let settled = len / 3;
        // The interval, either way.
        let input = tone(len, 220.0, 0.5, rate);
        for formant in [0.0, 1.0] {
            for semitones in [7.0f32, -12.0] {
                let mut pitch = shifter_at(rate, &[(SHIFT, semitones), (FORMANT, formant)]);
                let out = through(pitch.as_mut(), &input);
                let wanted = 220.0 * f64::from(semitones / 12.0).exp2();
                let heard = loudest_near(&out[settled..], wanted, rate);
                let cents = 1_200.0 * (heard / wanted).log2();
                assert!(cents.abs() < 2.0, "{rate} {semitones} {formant}: {heard}");
                let level = rms(&out[settled..]) / rms(&input[settled..]);
                assert!(
                    (0.8..1.25).contains(&level),
                    "{rate} {semitones} {formant}: {level}"
                );
            }
        }
        // The formants, kept and moved.
        let (input, period) = vowel(len, rate);
        let f0 = f64::from(rate) / period as f64;
        let before = harmonic_centre(&input[settled..], f0, rate);
        let centre = |formant: f32| {
            let mut pitch = shifter_at(rate, &[(SHIFT, 12.0), (FORMANT, formant)]);
            let out = through(pitch.as_mut(), &input);
            (
                harmonic_centre(&out[settled..], 2.0 * f0, rate),
                rms(&out[settled..]),
            )
        };
        let ((moved, moved_level), (kept, kept_level)) = (centre(0.0), centre(1.0));
        assert!(moved > 1.6 * before, "{rate}: moved {moved} from {before}");
        assert!(
            (kept / before - 1.0).abs() < 0.25,
            "{rate}: kept {kept} from {before}"
        );
        let level = kept_level / moved_level;
        assert!((0.7..1.4).contains(&level), "{rate}: {level}");
    }
}

#[test]
fn kept_at_a_high_rate_nothing_folds_back() {
    // 15 kHz an octave up is 30 kHz: above the 24 kHz the kept path is
    // shifted at, so unguarded it folds back to 18 kHz. With the formants
    // kept (and the switch settled, so the output is the kept sound alone,
    // which carries nothing above 24 kHz), nothing may come out above
    // -80 dBFS.
    for rate in [96_000.0f32, 192_000.0] {
        let len = (0.5 * rate) as usize;
        let input = tone(len, 15_000.0, 0.5, rate);
        let mut pitch = shifter_at(rate, &[(SHIFT, 12.0), (FORMANT, 1.0)]);
        let out = through(pitch.as_mut(), &input);
        let settled = &out[len / 2..];
        let level = rms(settled) * std::f32::consts::SQRT_2;
        let db = 20.0 * level.max(1e-12).log10();
        assert!(db < -80.0, "{rate}: {db} dBFS");
        let alias = 20.0 * strength(settled, 18_000.0, rate).max(1e-12).log10();
        assert!(alias < -80.0, "{rate}: alias at 18 kHz {alias} dBFS");
    }
}
