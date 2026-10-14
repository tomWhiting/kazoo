//! The harmony slot: its intervals, its cost, and its changes mid-tone.

use std::f32::consts::TAU;

use super::{AMP, BLOCK, RATE, chain, only, param, render, sine};
use crate::engine::Stats;
use crate::engine::fx::{Context, FxChain, FxSettings, RAMP_SECONDS, Slot, harmony};

fn harmony_slot() -> Slot {
    Slot::ALL
        .into_iter()
        .find(|slot| slot.name() == "harmony")
        .unwrap()
}

fn amplitude(samples: &[f32], hz: f32) -> f32 {
    let (sin, cos) = samples
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(sin, cos), (i, x)| {
            let phase = TAU * hz * i as f32 / RATE as f32;
            (x.mul_add(phase.sin(), sin), x.mul_add(phase.cos(), cos))
        });
    2.0 * sin.hypot(cos) / samples.len() as f32
}

#[test]
fn harmony_intervals_and_octaves_spectrum() {
    let slot = harmony_slot();
    for (preset, shift, hz) in [
        (0.0, -12.0, 110.0),
        (1.0, 12.0, 440.0),
        (4.0, 7.0, 329.627_56),
    ] {
        let mut settings = only(slot, 0);
        param(&mut settings, slot, 0, "preset", preset);
        param(&mut settings, slot, 0, "dry", -60.0);
        param(&mut settings, slot, 0, "voice 1 shift", shift);
        param(&mut settings, slot, 0, "voice 1 level", 0.0);
        param(&mut settings, slot, 0, "voice 2 level", -60.0);
        let output = render(
            &mut chain(&settings),
            &sine(RATE as usize * 2),
            120.0,
            &Stats::default(),
        );
        let window = &output[0][RATE as usize..];
        assert!(
            amplitude(window, hz) > AMP * 0.5,
            "{hz}: shifted fundamental is too quiet"
        );
        let peak = (0..=200)
            .map(|i| hz * (i as f32).mul_add(0.0002, 0.98))
            .max_by(|a, b| amplitude(window, *a).total_cmp(&amplitude(window, *b)))
            .unwrap();
        assert!((peak / hz - 1.0).abs() < 0.01, "{peak} != {hz}");
    }
    let output = render(
        &mut chain(&only(slot, 0)),
        &sine(RATE as usize * 2),
        120.0,
        &Stats::default(),
    );
    for (hz, db) in [(110.0, 0.0), (220.0, 0.0), (440.0, -3.0)] {
        let actual = 20.0 * (amplitude(&output[0][RATE as usize..], hz) / AMP).log10();
        assert!(
            (actual - db).abs() <= 3.0,
            "{hz}: {actual} dB expected {db}"
        );
    }
}

#[test]
fn harmony_all_voices_allocate_nothing_and_recover_from_nonfinite() {
    let slot = harmony_slot();
    let mut settings = only(slot, 0);
    param(&mut settings, slot, 0, "voice 3 level", -6.0);
    let mut chain = chain(&settings);
    let stats = Stats::default();
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let input = sine(BLOCK);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..1000 {
            chain.process(
                Context { bpm: 120.0 },
                &input,
                [&mut left, &mut right],
                &stats,
            );
        }
    });
    assert_eq!(before, assert_no_alloc::violation_count());
    let mut fresh = super::chain(&settings);
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        fresh.process(
            Context { bpm: 120.0 },
            &[value; BLOCK],
            [&mut left, &mut right],
            &stats,
        );
        assert!(left.iter().chain(&right).all(|x| *x == 0.0));
    }
    let output = render(&mut fresh, &sine(RATE as usize), 120.0, &stats);
    assert!(output.iter().flatten().all(|x| x.is_finite()));
    assert!(output[0].iter().any(|x| x.abs() > 0.1));
    fresh.process(
        Context { bpm: 120.0 },
        &[f32::NAN; BLOCK],
        [&mut left, &mut right],
        &stats,
    );
    assert!(left.iter().chain(&right).all(|x| *x == 0.0));
}

#[test]
fn harmony_cost_per_block() {
    use std::time::Instant;
    let slot = harmony_slot();
    let input: [f32; BLOCK] =
        std::array::from_fn(|i| (TAU * 220.0 * i as f32 / RATE as f32).sin() * AMP);
    let stats = Stats::default();
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let context = Context { bpm: 120.0 };
    let blocks = if cfg!(debug_assertions) { 32 } else { 1000 };
    for voices in 0..=3 {
        let mut settings = FxSettings::default();
        for saved in &mut settings.slots {
            saved.on = true;
        }
        settings.slots[slot as usize].on = voices > 0;
        for index in 0..3 {
            param(
                &mut settings,
                slot,
                0,
                ["voice 1 level", "voice 2 level", "voice 3 level"][index],
                if index < voices { 0.0 } else { -60.0 },
            );
        }
        let mut chain = chain(&settings);
        let description = slot.kind(0).unwrap();
        let mut harmony = (description.build)();
        harmony.prepare(RATE as f32);
        for (index, value) in settings.slots[slot as usize].values[0]
            .iter()
            .take(description.params.len())
            .enumerate()
        {
            harmony.set_param(index, *value);
        }
        harmony.reset();
        for _ in 0..100 {
            chain.process(context, &input, [&mut left, &mut right], &stats);
            harmony.process(&context, [&input, &input], [&mut left, &mut right]);
        }
        let start = Instant::now();
        for _ in 0..blocks {
            chain.process(context, &input, [&mut left, &mut right], &stats);
        }
        let chain_us = start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(blocks);
        let start = Instant::now();
        for _ in 0..blocks {
            harmony.process(&context, [&input, &input], [&mut left, &mut right]);
        }
        let harmony_us = start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(blocks);
        println!(
            "voices={voices} harmony_us={harmony_us:.3} chain_us={chain_us:.3} block_us=2666.667 harmony_percent={:.2}",
            harmony_us / 2666.667 * 100.0
        );
        assert!(left.iter().chain(&right).all(|x| x.is_finite()));
    }
}

/// A click is broadband: a step in level or waveform puts energy where a
/// tone of a few hundred hertz has none. The third difference falls 18 dB
/// per octave below its peak, so it hears a step at full size and the
/// tones under test 60 dB down. A smooth fade shows only at its corners,
/// where the gain's slope turns: at most the signal's peak over the ramp's
/// length.
fn clicks(samples: &[f32]) -> f32 {
    samples
        .windows(4)
        .map(|w| 3.0f32.mul_add(w[1] - w[2], w[3] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Level in decibels over every window of 800 frames, hopped by 1 ms. 800
/// frames is one whole period of 60 Hz, the lowest partial a 240 Hz tone
/// takes two octaves down, and so of every octave of it: a window that
/// holds a whole number of periods reads a steady tone as steady, where a
/// 2 ms window reads its own waveform's swing as a level change.
fn levels(samples: &[f32]) -> Vec<f32> {
    samples
        .windows(800)
        .step_by(48)
        .map(|w| {
            10.0 * (w.iter().map(|x| x * x).sum::<f32>() / 800.0)
                .max(1e-20)
                .log10()
        })
        .collect()
}

fn tone_240(count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| (TAU * 240.0 * i as f32 / RATE as f32).sin() * AMP)
        .collect()
}

fn harmony_at(preset: usize) -> FxSettings {
    let slot = Slot::Harmony;
    let mut settings = only(slot, 0);
    let saved = &mut settings.slots[slot as usize];
    harmony::adjust(&mut saved.values[0], &mut saved.custom, 0, preset as f32);
    settings
}

/// Renders `settings` settled, makes `change`, and renders until settled
/// again; answers (the worst click over what the steady sound and a fade
/// of the whole sound over the ramp allow, the worst level over the louder
/// steady level in dB).
fn transition(settings: &FxSettings, change: impl FnOnce(&mut FxChain, &Stats)) -> (f32, f32) {
    let stats = Stats::default();
    let input = tone_240(RATE as usize);
    let mut chain = chain(settings);
    let before = render(&mut chain, &input[..14_400], 120.0, &stats);
    change(&mut chain, &stats);
    let during = render(&mut chain, &input[14_400..33_600], 120.0, &stats);
    let after = render(&mut chain, &input[33_600..48_000], 120.0, &stats);
    let steady = [&before[0][4_800..], &after[0][..]];
    let peak = [&before[0][..], &during[0], &after[0]]
        .iter()
        .flat_map(|x| x.iter())
        .fold(0.0_f32, |peak, x| peak.max(x.abs()));
    let fade = peak / (RATE as f32 * RAMP_SECONDS);
    let click = clicks(&during[0]) / (steady.iter().map(|x| clicks(x)).fold(0.0, f32::max) + fade);
    let loudest = steady
        .iter()
        .flat_map(|x| levels(x))
        .fold(f32::MIN, f32::max);
    let mut joined = before[0][4_800..].to_vec();
    joined.extend_from_slice(&during[0]);
    joined.extend_from_slice(&after[0]);
    let swell = levels(&joined).into_iter().fold(f32::MIN, f32::max) - loudest;
    (click, swell)
}

#[test]
fn harmony_changes_mid_tone_never_click_or_swell() {
    let slot = Slot::Harmony;
    // Each pair moves an audible voice's shift: through unison with the dry
    // (octave up to down, octaves to two down) and across it (fifth to
    // octave down, minor third to octaves, octave down to octave up).
    for (from, to) in [(1, 0), (2, 3), (4, 0), (7, 2), (0, 1)] {
        let (click, swell) = transition(&harmony_at(from), |chain, stats| {
            chain.param(slot, 0, 0, to as f32, stats);
        });
        let name = |index: usize| harmony::PRESETS[index];
        assert!(
            click <= 1.0,
            "{} to {}: click {click}",
            name(from),
            name(to)
        );
        assert!(
            swell <= 1.0,
            "{} to {}: swell {swell} dB",
            name(from),
            name(to)
        );
    }
    for (index, value) in [(3, -60.0), (3, 6.0), (2, -11.0), (4, 7.0), (1, -60.0)] {
        let (click, swell) = transition(&harmony_at(2), |chain, stats| {
            chain.param(slot, 0, index, value, stats);
        });
        assert!(click <= 1.0, "knob {index} to {value}: click {click}");
        assert!(swell <= 1.0, "knob {index} to {value}: swell {swell} dB");
    }
    let stats = Stats::default();
    let mut chain = chain(&harmony_at(1));
    let input = tone_240(BLOCK);
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for block in 0..320 {
            if block % 40 == 0 {
                chain.param(slot, 0, 0, (block / 40) as f32, &stats);
            }
            chain.process(
                Context { bpm: 120.0 },
                &input,
                [&mut left, &mut right],
                &stats,
            );
        }
    });
    assert_eq!(before, assert_no_alloc::violation_count());
    for on in [false, true] {
        let mut settings = harmony_at(2);
        settings.slots[slot as usize].on = !on;
        let (click, swell) = transition(&settings, |chain, _| chain.on(slot, on));
        assert!(click <= 1.0, "on {on}: click {click}");
        assert!(swell <= 1.0, "on {on}: swell {swell} dB");
    }
}
