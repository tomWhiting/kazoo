use super::testkit::{
    chord, crossings, glide_fourth, magnitude, pluck, vibrato_30c_6hz, window_pitch,
};
use crate::{Context, Effect};

const RATE: f32 = 48_000.0;
const CONTEXT: Context = Context { bpm: 120.0 };

fn effect(settings: &[(&str, f32)]) -> Box<dyn Effect> {
    let kind = crate::find("sidestep").expect("sidestep must exist in the catalogue");
    let mut effect = (kind.build)();
    for &(name, value) in settings {
        let index = kind
            .params
            .iter()
            .position(|p| p.name == name)
            .expect("knob exists");
        effect.set_param(index, value);
    }
    effect.prepare(RATE);
    effect
}

fn isolated(wave: &str) -> Box<dyn Effect> {
    let mut settings = vec![
        ("sub1", 0.0),
        ("cutoff", 12_000.0),
        ("reso", 0.0),
        ("track", 0.0),
        ("fenv", 0.0),
        ("env", 0.0),
    ];
    settings.push((wave, 100.0));
    effect(&settings)
}

fn render(effect: &mut dyn Effect, input: &[f32]) -> Vec<f32> {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((input, left), right) in input
        .chunks(512)
        .zip(left.chunks_mut(512))
        .zip(right.chunks_mut(512))
    {
        effect.process(&CONTEXT, [input, input], [left, right]);
        assert_eq!(left, right);
    }
    left
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn follows(signal: &[f32]) {
    let output = render(isolated("sub1").as_mut(), signal);
    let source = window_pitch(signal, RATE, 10.0);
    let heard = window_pitch(&output, RATE, 10.0);
    let mut previous = None;
    let mut count = 0;
    for &(time, pitch) in &heard {
        if time < 0.05 {
            continue;
        }
        let expected = source.iter().find(|p| (p.0 - time).abs() < 1e-6).unwrap().1 / 2.0;
        let error = cents(pitch, expected);
        assert!(
            error.abs() < 5.0,
            "at {time}: {pitch} expected {expected}, {error} cents"
        );
        if let Some((last, wanted)) = previous {
            assert!(
                cents(pitch, last).abs() <= cents(expected, wanted).abs() + 2.0,
                "step at {time}: {} vs {}",
                cents(pitch, last),
                cents(expected, wanted)
            );
        }
        previous = Some((pitch, expected));
        count += 1;
    }
    assert!(count > 180, "only {count} windows");
}

#[test]
fn the_sub_follows_a_glide_without_steps() {
    follows(&glide_fourth(RATE));
}
#[test]
fn the_sub_follows_a_vibrato_without_steps() {
    follows(&vibrato_30c_6hz(RATE));
}

#[test]
fn the_square_is_the_inputs_own_pitch() {
    let input = pluck(RATE, 110.0, 2.0);
    let output = render(isolated("square").as_mut(), &input);
    let measured = window_pitch(&output, RATE, 10.0);
    let mut count = 0;
    for (time, pitch) in measured {
        if time > 0.05 && time < 1.8 {
            assert!(cents(pitch, 110.0).abs() < 3.0, "{time}: {pitch}");
            count += 1;
        }
    }
    assert!(count > 150);
}

#[test]
fn a_low_b_clocks_with_high_at_500() {
    let mut effect = effect(&[
        ("high", 500.0),
        ("square", 100.0),
        ("sub1", 0.0),
        ("cutoff", 12_000.0),
        ("reso", 0.0),
        ("env", 0.0),
        ("track", 0.0),
        ("fenv", 0.0),
    ]);
    let output = render(effect.as_mut(), &pluck(RATE, 30.87, 3.0));
    let edges = crossings(&output);
    let cycles: Vec<_> = edges
        .windows(2)
        .filter(|p| p[0] > 0.1 * f64::from(RATE))
        .collect();
    let good = cycles
        .iter()
        .filter(|p| ((f64::from(RATE) / (p[1] - p[0])) / 30.87 - 1.0).abs() < 0.005)
        .count();
    assert!(
        good as f64 / cycles.len() as f64 >= 0.95,
        "{good}/{}",
        cycles.len()
    );
}

#[test]
fn a_chord_is_never_silenced_and_folds_every_note() {
    let frequencies = [130.81, 164.81, 196.0];
    let input = chord(RATE, &frequencies);
    let output = render(isolated("fold").as_mut(), &input);
    let segment = &output[4800..4800 + 65_536];
    let strongest = super::testkit::spectrum(segment)
        .into_iter()
        .fold(0.0, f64::max)
        .sqrt()
        / segment.len() as f64;
    for frequency in frequencies {
        let mut peak = 0.0_f64;
        let products = [1.0, 3.0, 5.0].map(|harmonic| f64::from(frequency) * harmonic);
        let intermodulated = frequencies
            .into_iter()
            .filter(|other| other.to_bits() != frequency.to_bits())
            .flat_map(|other| {
                [
                    f64::from(frequency) + 2.0 * f64::from(other),
                    (f64::from(frequency) - 2.0 * f64::from(other)).abs(),
                ]
            });
        let other_notes: Vec<_> = frequencies
            .into_iter()
            .filter(|other| other.to_bits() != frequency.to_bits())
            .map(f64::from)
            .collect();
        let mixed_products = [
            (f64::from(frequency) + other_notes[0] + other_notes[1]).abs(),
            (f64::from(frequency) + other_notes[0] - other_notes[1]).abs(),
            (f64::from(frequency) - other_notes[0] + other_notes[1]).abs(),
            (f64::from(frequency) - other_notes[0] - other_notes[1]).abs(),
        ];
        for product in products
            .into_iter()
            .chain(intermodulated)
            .chain(mixed_products)
        {
            let measured = magnitude(segment, RATE, product);
            let db = 20.0 * (measured / strongest).log10();
            println!("fold {frequency} Hz, product {product} Hz: {measured}, {db} dB relative");
            peak = peak.max(measured);
        }
        assert!(
            20.0 * (peak / strongest).log10() > -20.0,
            "missing {frequency} Hz and its folded products"
        );
    }
    let sub = render(isolated("sub1").as_mut(), &input);
    assert!(sub.iter().all(|s| s.is_finite()));
    assert!(sub.iter().map(|s| s.abs()).sum::<f32>() / sub.len() as f32 > 0.001);
}

#[test]
fn each_onset_restarts_the_envelope() {
    let mut effect = inspected(&[
        ("attack", 2.0),
        ("decay", 30.0),
        ("sustain", 5.0),
        ("cutoff", 12_000.0),
        ("reso", 0.0),
        ("track", 0.0),
        ("fenv", 0.0),
    ]);
    let single = pluck(RATE, 220.0, 0.5);
    let mut input = vec![0.0; single.len()];
    for (offset, strength) in [(0, 0.0625_f32), (7200, 0.25), (14_400, 1.0)] {
        for (to, from) in input[offset..].iter_mut().zip(&single) {
            *to = strength.mul_add(*from, *to);
        }
    }
    let trace: Vec<_> = input
        .iter()
        .map(|sample| effect.test_sample(*sample).1)
        .collect();
    let attacks: Vec<_> = trace
        .windows(2)
        .enumerate()
        .filter(|(_, p)| p[1][3] < p[0][3])
        .map(|(n, _)| n + 1)
        .collect();
    println!("onsets at samples {attacks:?}");
    assert_eq!(attacks.len(), 3);
    for (attack, plucked) in attacks.iter().zip([0, 7200, 14_400]) {
        assert!(
            *attack >= plucked && *attack <= plucked + 144,
            "onset {attack} after {plucked}"
        );
        assert!(trace[*attack + 96][0] > trace[*attack][0] + 0.5);
    }
}

#[test]
fn the_gate_closes_below_sense() {
    let mut effect = inspected(&[("release", 20.0), ("sense", -45.0)]);
    let mut input = glide_fourth(RATE);
    for sample in &mut input[24_000..] {
        *sample *= 0.004;
    }
    let trace: Vec<_> = input
        .iter()
        .map(|sample| effect.test_sample(*sample))
        .collect();
    let gate = trace
        .iter()
        .enumerate()
        .skip(24_000)
        .find(|(_, sample)| 20.0 * sample.1[1].max(1e-8).log10() < -45.0)
        .unwrap()
        .0;
    let deadline = gate + 1200;
    let loudest = trace[deadline..]
        .iter()
        .fold(0.0f32, |a, sample| a.max(sample.0.abs()));
    println!("gate closes at {gate}, release deadline {deadline}, peak after {loudest}");
    assert!(loudest < 1e-4, "release peak {loudest}");
}

fn inspected(settings: &[(&str, f32)]) -> super::sidestep::Sidestep {
    let kind = crate::find("sidestep").expect("sidestep must exist in the catalogue");
    let mut effect = super::sidestep::Sidestep::new();
    for &(name, value) in settings {
        let index = kind
            .params
            .iter()
            .position(|p| p.name == name)
            .expect("knob exists");
        effect.set_param(index, value);
    }
    effect.prepare(RATE);
    effect
}

#[test]
fn the_edges_are_band_limited() {
    let settings = [
        ("square", 100.0),
        ("sub1", 0.0),
        ("cutoff", 12_000.0),
        ("reso", 0.0),
        ("track", 0.0),
        ("fenv", 0.0),
        ("env", 0.0),
    ];
    let energy = |signal: &[f32], tone: f64| {
        let bins = super::testkit::spectrum(&signal[32_768..]);
        let mut alias = 0.0;
        let mut fundamental = 0.0;
        for (bin, power) in bins.iter().enumerate().skip(1) {
            let frequency = bin as f64 * f64::from(RATE) / 32_768.0;
            let harmonic = (frequency / tone).round();
            let distance = (frequency - harmonic * tone).abs() * 32_768.0 / f64::from(RATE);
            if distance > 3.0 {
                alias += power;
            }
            if (frequency - tone).abs() * 32_768.0 / f64::from(RATE) <= 3.0 {
                fundamental += power;
            }
        }
        (alias, fundamental)
    };
    // At exactly 1 kHz every folded harmonic is another 1 kHz multiple.
    // A neighbouring coherent FFT bin separates aliases from harmonics.
    for tone in [1000.0, 683.0 * f64::from(RATE) / 32_768.0] {
        let input: Vec<f32> = (0..65_536)
            .map(|n| {
                (std::f64::consts::TAU * tone * n as f64 / f64::from(RATE)).sin() as f32 * 0.25
            })
            .collect();
        let mut limited = inspected(&settings);
        let mut naive = inspected(&settings);
        naive.test_band_limit(false);
        let corrected = render(&mut limited, &input);
        let uncorrected = render(&mut naive, &input);
        let (alias, fundamental) = energy(&corrected, tone);
        let (naive_alias, _) = energy(&uncorrected, tone);
        let reduction = 10.0 * (naive_alias / alias).log10();
        let under = 10.0 * (alias / fundamental).log10();
        println!(
            "{tone} Hz: polyBLEP alias reduction {reduction} dB; alias/fundamental {under} dB; naive {naive_alias}, corrected {alias}"
        );
        if tone > 1000.0 {
            assert!(reduction >= 12.0, "reduction {reduction} dB");
        }
        assert!(under <= -30.0, "alias/fundamental {under} dB");
    }
}

#[test]
fn latency_is_one_sample() {
    let mut effect = isolated("dry");
    assert_eq!(effect.latency(), 1);
    let input: Vec<f32> = (0..512).map(|n| if n == 0 { 0.25 } else { 0.0 }).collect();
    let output = render(effect.as_mut(), &input);
    let coefficient = -(-std::f32::consts::TAU * 30.0 / RATE).exp_m1();
    let mut state = 0.0;
    let mut previous = 0.0;
    for (x, y) in input.iter().zip(&output) {
        assert!((y - previous).abs() < 1e-6, "{y} != {previous}");
        state = (x - state).mul_add(coefficient, state);
        previous = x - state;
    }
}

#[test]
fn survives_nonsense() {
    let mut effect = effect(&[]);
    for rate in [f32::NAN, f32::INFINITY, -1.0, 0.0, 8000.0, 192_000.0] {
        effect.prepare(rate);
        for index in 0..24 {
            effect.set_param(index, f32::NAN);
            effect.set_param(index, f32::INFINITY);
        }
        let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        let output = render(effect.as_mut(), &bad);
        assert!(output.iter().all(|x| *x == 0.0));
        let signal = glide_fourth(RATE);
        let normal = render(effect.as_mut(), &signal[..4096]);
        let kind = crate::find("sidestep").unwrap();
        let mut fresh = (kind.build)();
        fresh.prepare(rate);
        let reference = render(fresh.as_mut(), &signal[..4096]);
        assert_eq!(normal, reference, "nonsense poisoned state at {rate}");
        effect.reset();
        assert_eq!(render(effect.as_mut(), &signal[..4096]), reference);
    }
}

#[test]
fn stays_bounded() {
    let kind = crate::find("sidestep").expect("sidestep must exist in the catalogue");
    for end in [false, true] {
        let settings: Vec<_> = kind
            .params
            .iter()
            .map(|p| (p.name, if end { p.max } else { p.min }))
            .collect();
        let mut effect = effect(&settings);
        let input: Vec<_> = (0..48_000)
            .map(|n| if n % 2 == 0 { 1e30 } else { -1e30 })
            .collect();
        let output = render(effect.as_mut(), &input);
        assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 4.0));
    }
}

#[test]
fn processing_never_allocates() {
    let mut effect = effect(&[
        ("square", 50.0),
        ("sub1", 50.0),
        ("sub2", 50.0),
        ("ring", 50.0),
        ("half", 50.0),
        ("fold", 50.0),
    ]);
    let input = [0.1; 512];
    let mut left = [0.0; 512];
    let mut right = [0.0; 512];
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..938 {
            effect.process(&CONTEXT, [&input, &input], [&mut left, &mut right]);
        }
        for index in 0..24 {
            effect.set_param(index, 50.0);
        }
        effect.reset();
    });
}
