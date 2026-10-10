//! The pitch shifter: intervals land where they should, formants stay put
//! when asked, and nothing it is given (a sweep, a jump, nonsense) makes it
//! click, blow up or go quiet for good.

use super::*;
use crate::time::testkit::{RATE, context, noise, peak, prepared, render, rms, sine};

/// A shifter at `knobs` (by index), fully wet unless told otherwise.
fn shifter(knobs: &[(usize, f32)]) -> Box<dyn Effect> {
    let mut pitch = prepared(&KIND);
    pitch.set_param(MIX, 1.0);
    for &(index, value) in knobs {
        pitch.set_param(index, value);
    }
    pitch.reset();
    pitch
}

/// The strength of `hz` in `signal`, through a Hann window.
fn strength(signal: &[f32], hz: f64) -> f64 {
    let size = signal.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, sample) in signal.iter().enumerate() {
        let window = (-0.5f64).mul_add((std::f64::consts::TAU * n as f64 / size).cos(), 0.5);
        let angle = std::f64::consts::TAU * hz * n as f64 / f64::from(RATE);
        let value = f64::from(*sample) * window;
        re = value.mul_add(angle.cos(), re);
        im = value.mul_add(angle.sin(), im);
    }
    4.0 * re.hypot(im) / size
}

/// The strongest frequency within 6% of `near`, to a hundredth of a hertz.
fn loudest_near(signal: &[f32], near: f64) -> f64 {
    let mut best = (near, 0.0);
    let mut step = near * 0.002;
    let (mut low, mut high) = (near * 0.94, near * 1.06);
    for _ in 0..4 {
        let steps = ((high - low) / step).round() as usize;
        for i in 0..=steps {
            let hz = step.mul_add(i as f64, low);
            let level = strength(signal, hz);
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

/// Cents between two frequencies.
fn cents(hz: f64, wanted: f64) -> f64 {
    1_200.0 * (hz / wanted).log2()
}

#[test]
fn a_sine_comes_out_at_the_interval_either_way_with_the_formants() {
    let input = sine(36_000, 220.0, 0.5);
    for formant in [0.0, 1.0] {
        for semitones in [7.0f32, -12.0, 12.0, -5.0, 24.0, -24.0] {
            let mut pitch = shifter(&[(SHIFT, semitones), (FORMANT, formant)]);
            let (left, right) = render(pitch.as_mut(), context(120.0), &input, &input, 256);
            let wanted = 220.0 * f64::from(semitones / 12.0).exp2();
            for side in [&left, &right] {
                let heard = loudest_near(&side[12_000..], wanted);
                assert!(
                    cents(heard, wanted).abs() < 2.0,
                    "{semitones} st, formant {formant}: {heard} Hz for {wanted}"
                );
                // The input's own pitch is gone from the wet sound.
                let left_over = strength(&side[12_000..], 220.0);
                assert!(
                    left_over < 0.02 * strength(&side[12_000..], heard),
                    "{semitones} {formant} {left_over}"
                );
            }
        }
    }
}

#[test]
fn the_fine_knob_moves_in_cents() {
    let input = sine(36_000, 220.0, 0.5);
    for (semitones, fine) in [(0.0, 50.0), (7.0, -30.0), (-12.0, 100.0)] {
        let mut pitch = shifter(&[(SHIFT, semitones), (FINE, fine), (FORMANT, 0.0)]);
        let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 256);
        let wanted = 220.0 * f64::from((semitones + fine / 100.0) / 12.0).exp2();
        let heard = loudest_near(&left[12_000..], wanted);
        assert!(
            cents(heard, wanted).abs() < 2.0,
            "{semitones} {fine}: {heard}"
        );
    }
}

#[test]
fn with_no_shift_it_is_a_plain_delay() {
    let mut pitch = shifter(&[(SHIFT, 0.0), (FORMANT, 0.0)]);
    let mut click = vec![0.0; 4_800];
    click[0] = 1.0;
    let (left, _) = render(pitch.as_mut(), context(120.0), &click, &click, 64);
    let start = Geometry::new(RATE).start() as usize;
    let loudest = left.iter().enumerate().fold((0, 0.0f32), |best, (n, v)| {
        if v.abs() > best.1 { (n, v.abs()) } else { best }
    });
    // The guard against folding adds a fraction of a sample.
    assert!(loudest.0.abs_diff(start - 1) <= 2, "{loudest:?} {start}");
    // A tone comes through at its level, a fixed time later.
    let input = sine(24_000, 1_000.0, 0.5);
    let mut pitch = shifter(&[(SHIFT, 0.0), (FORMANT, 0.0)]);
    let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 64);
    let level = rms(&left[4_800..]) / rms(&input[4_800..]);
    assert!((level - 1.0).abs() < 0.01, "{level}");
}

/// A pulse train at about 140 Hz through one resonance at 600 Hz: a
/// buzzing vowel with a single formant.
fn vowel(len: usize) -> Vec<f32> {
    let radius = 0.985f32;
    let angle = std::f32::consts::TAU * 600.0 / RATE;
    let (a1, a2) = (-2.0 * radius * angle.cos(), radius * radius);
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    (0..len)
        .map(|n| {
            let pulse = if n % 342 == 0 { 1.0 } else { 0.0 };
            let y = (-a1).mul_add(y1, (-a2).mul_add(y2, pulse));
            y2 = y1;
            y1 = y;
            y * 0.02
        })
        .collect()
}

/// Where the harmonics of `f0` in `signal` are loudest, on average: the
/// amplitude-weighted mean of their frequencies, up to 4 kHz.
fn harmonic_centre(signal: &[f32], f0: f64) -> f64 {
    let (mut weighted, mut total) = (0.0, 0.0);
    for k in 1..=(4_000.0 / f0) as usize {
        let hz = k as f64 * f0;
        let level = strength(signal, hz);
        weighted = hz.mul_add(level, weighted);
        total += level;
    }
    weighted / total.max(1e-12)
}

#[test]
fn kept_formants_stay_where_they_were_and_moved_ones_move() {
    let f0 = f64::from(RATE) / 342.0;
    let input = vowel(48_000);
    let before = harmonic_centre(&input[24_000..], f0);
    let centre = |formant: f32| {
        let mut pitch = shifter(&[(SHIFT, 12.0), (FORMANT, formant)]);
        let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 256);
        harmonic_centre(&left[24_000..], 2.0 * f0)
    };
    let (moved, kept) = (centre(0.0), centre(1.0));
    assert!(moved > 1.6 * before, "moved {moved} from {before}");
    assert!(
        (kept / before - 1.0).abs() < 0.25,
        "kept {kept} from {before}"
    );
}

#[test]
fn keeping_the_formants_keeps_the_level() {
    for input in [sine(36_000, 220.0, 0.5), vowel(36_000)] {
        for semitones in [7.0, 24.0, -12.0] {
            let level = |formant: f32| {
                let mut pitch = shifter(&[(SHIFT, semitones), (FORMANT, formant)]);
                let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 256);
                rms(&left[12_000..])
            };
            let ratio = level(1.0) / level(0.0);
            assert!((0.7..1.4).contains(&ratio), "{semitones}: {ratio}");
            assert!(level(0.0) > 0.7 * rms(&input[12_000..]), "{}", level(0.0));
        }
    }
}

#[test]
fn splices_do_not_chop_a_steady_note() {
    let input = sine(48_000, 220.0, 0.5);
    for semitones in [7.0, -12.0, 12.0, 1.0] {
        let mut pitch = shifter(&[(SHIFT, semitones), (FORMANT, 0.0)]);
        let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 256);
        let windows: Vec<f32> = left[12_000..].chunks(480).map(rms).collect();
        let (low, high) = windows.iter().fold((f32::MAX, 0.0f32), |(low, high), w| {
            (low.min(*w), high.max(*w))
        });
        assert!(high / low < 1.12, "{semitones}: {low} to {high}");
    }
}

/// The largest step from one sample to the next.
fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .fold(0.0, |most, pair| most.max((pair[1] - pair[0]).abs()))
}

#[test]
fn knob_moves_and_splices_never_click() {
    // A 220 Hz sine at 0.5 shifted up to an octave changes by at most
    // 2π · 440 · 0.5 / 48 000 ≈ 0.029 a sample; a click is far more.
    let input = sine(96_000, 220.0, 0.5);
    let moves: [(usize, usize, f32); 8] = [
        (9_600, SHIFT, 12.0),
        (19_200, FORMANT, 0.0),
        (28_800, SHIFT, -7.0),
        (38_400, FINE, 100.0),
        (48_000, FORMANT, 1.0),
        (57_600, MIX, 0.3),
        (67_200, SHIFT, 5.0),
        (76_800, MIX, 1.0),
    ];
    let mut pitch = shifter(&[(SHIFT, 0.0)]);
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    let mut start = 0;
    for (at, index, value) in moves.iter().copied().chain([(input.len(), MIX, 1.0)]) {
        pitch.process(
            &context(120.0),
            [&input[start..at], &input[start..at]],
            [&mut left[start..at], &mut right[start..at]],
        );
        pitch.set_param(index, value);
        start = at;
    }
    let step = largest_step(&left[2_400..]);
    assert!(step < 0.045, "{step}");
}

#[test]
fn a_sweeping_shift_stays_bounded_and_settles_on_the_interval() {
    let tone = sine(96_000, 220.0, 0.4);
    let hiss = noise(96_000, 0.1, 5);
    let input: Vec<f32> = tone.iter().zip(&hiss).map(|(a, b)| a + b).collect();
    for formant in [0.0, 1.0] {
        let mut pitch = shifter(&[(FORMANT, formant)]);
        let mut left = vec![0.0; input.len()];
        let mut right = vec![0.0; input.len()];
        for (n, ((chunk, out_left), out_right)) in input
            .chunks(32)
            .zip(left.chunks_mut(32))
            .zip(right.chunks_mut(32))
            .enumerate()
        {
            let t = n as f32 * 32.0 / RATE;
            if t < 1.0 {
                // A CV sweep: two octaves each way at 3 Hz, the fine knob
                // wobbling under it.
                pitch.set_param(SHIFT, 24.0 * (std::f32::consts::TAU * 3.0 * t).sin());
                pitch.set_param(FINE, 100.0 * (std::f32::consts::TAU * 11.0 * t).sin());
            } else {
                pitch.set_param(SHIFT, 7.0);
                pitch.set_param(FINE, 0.0);
            }
            pitch.process(&context(120.0), [chunk, chunk], [out_left, out_right]);
        }
        assert!(left.iter().chain(&right).all(|x| x.is_finite()));
        assert!(peak(&left) <= 2.0 && peak(&right) <= 2.0, "{}", peak(&left));
        assert!(rms(&left[..48_000]) > 0.05, "{}", rms(&left[..48_000]));
        let wanted = 220.0 * (7.0f64 / 12.0).exp2();
        let heard = loudest_near(&left[60_000..], wanted);
        assert!(cents(heard, wanted).abs() < 2.0, "{formant}: {heard}");
    }
}

#[test]
fn nonsense_in_is_silence_out_and_leaves_no_trace() {
    for formant in [0.0, 1.0] {
        let mut pitch = shifter(&[(SHIFT, 7.0), (FORMANT, formant)]);
        let nonsense: Vec<f32> = (0..9_600)
            .map(|n| match n % 3 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                _ => f32::NEG_INFINITY,
            })
            .collect();
        let (left, right) = render(pitch.as_mut(), context(120.0), &nonsense, &nonsense, 64);
        assert!(peak(&left) == 0.0 && peak(&right) == 0.0, "{formant}");
        let input = sine(36_000, 220.0, 0.5);
        let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 64);
        let wanted = 220.0 * (7.0f64 / 12.0).exp2();
        let heard = loudest_near(&left[12_000..], wanted);
        assert!(cents(heard, wanted).abs() < 2.0, "{formant}: {heard}");
    }
}

#[test]
fn a_hard_panned_sound_stays_on_its_side() {
    let input = sine(24_000, 220.0, 0.5);
    let quiet = vec![0.0; input.len()];
    for formant in [0.0, 1.0] {
        let mut pitch = shifter(&[(SHIFT, -5.0), (FORMANT, formant)]);
        let (left, right) = render(pitch.as_mut(), context(120.0), &input, &quiet, 128);
        assert!(rms(&left[9_600..]) > 0.2, "{formant}");
        assert!(peak(&right) < 1e-6, "{formant} {}", peak(&right));
    }
}

#[test]
fn the_wet_side_is_the_shifted_sound_at_every_mix() {
    let input = sine(24_000, 220.0, 0.5);
    let mut wet_only = shifter(&[(SHIFT, 7.0)]);
    let (heard, _) = render(wet_only.as_mut(), context(120.0), &input, &input, 128);
    for mix in [0.0, 0.5, 1.0] {
        let mut pitch = shifter(&[(SHIFT, 7.0), (MIX, mix)]);
        let mut out = [vec![0.0; input.len()], vec![0.0; input.len()]];
        let mut wet = [vec![0.0; input.len()], vec![0.0; input.len()]];
        let [out_left, out_right] = &mut out;
        let [wet_left, wet_right] = &mut wet;
        pitch.process_wet(
            &context(120.0),
            [&input, &input],
            [out_left, out_right],
            [wet_left, wet_right],
        );
        let most = heard
            .iter()
            .zip(&wet[0])
            .fold(0.0f32, |most, (a, b)| most.max((a - b).abs()));
        assert!(most < 1e-5, "{mix}: {most}");
    }
}

#[test]
fn a_low_note_comes_out_clean() {
    // Nothing the shifter does (the guard, the splices, the formant
    // keeping and its loudness match) may add harmonics of its own.
    let input = sine(48_000, 82.0, 0.5);
    for formant in [0.0, 1.0] {
        for semitones in [7.0f32, -5.0, 12.0] {
            let mut pitch = shifter(&[(SHIFT, semitones), (FORMANT, formant)]);
            let (left, _) = render(pitch.as_mut(), context(120.0), &input, &input, 256);
            let hz = 82.0 * f64::from(semitones / 12.0).exp2();
            let tone = strength(&left[12_000..], hz);
            for harmonic in [2.0, 3.0] {
                let level = 20.0 * (strength(&left[12_000..], harmonic * hz) / tone).log10();
                assert!(
                    level < -60.0,
                    "{formant} {semitones} {harmonic}: {level} dB"
                );
            }
        }
    }
}

#[test]
fn a_big_move_bends_without_a_bump() {
    // A sine at 0.5 changes by at most 0.029 a sample up to 440 Hz. With
    // the formants kept, a move that sweeps through no shift lets the
    // loudness match catch up for a moment: a few decibels, never a click.
    let input = sine(96_000, 220.0, 0.5);
    for formant in [0.0, 1.0] {
        for (from, to) in [(-6.0, 5.0), (0.0, 12.0), (12.0, -7.0), (-7.0, 12.0)] {
            let mut pitch = shifter(&[(SHIFT, from), (FORMANT, formant)]);
            let mut left = vec![0.0; input.len()];
            let mut right = vec![0.0; input.len()];
            let (before, after) = input.split_at(48_000);
            let (left_before, left_after) = left.split_at_mut(48_000);
            let (right_before, right_after) = right.split_at_mut(48_000);
            pitch.process(
                &context(120.0),
                [before, before],
                [left_before, right_before],
            );
            pitch.set_param(SHIFT, to);
            pitch.process(&context(120.0), [after, after], [left_after, right_after]);
            let step = largest_step(&left[24_000..]);
            let most = if formant > 0.5 { 0.06 } else { 0.03 };
            assert!(step < most, "{formant} {from} to {to}: {step}");
        }
    }
}
