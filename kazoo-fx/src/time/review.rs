//! Regression tests for the independent review of the time family: each
//! one failed on the code as it stood before its fix.

use std::f32::consts::TAU;

use super::KINDS;
use super::testkit::{context, noise, peak, rms, sine, tone_level};
use crate::{Effect, EffectKind};

/// Build `kind` and prepare it at `rate`.
fn at(kind: &EffectKind, rate: f32) -> Box<dyn Effect> {
    let mut effect = (kind.build)();
    effect.prepare(rate);
    effect
}

/// Set the parameter called `name`; it must exist.
fn set(effect: &mut dyn Effect, kind: &EffectKind, name: &str, value: f32) {
    let index = kind
        .params
        .iter()
        .position(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("{} has no {name}", kind.id));
    effect.set_param(index, value);
}

/// Run a mono signal through both sides; returns (left, right).
fn run(effect: &mut dyn Effect, bpm: f64, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; input.len()];
    let mut right = vec![0.0; input.len()];
    for ((chunk, out_left), out_right) in input
        .chunks(256)
        .zip(left.chunks_mut(256))
        .zip(right.chunks_mut(256))
    {
        effect.process(&context(bpm), [chunk, chunk], [out_left, out_right]);
    }
    (left, right)
}

/// A sine at `hz`, its phase stepped in double precision with the double
/// precision 2π: the single-precision one is 3e-8 high, which puts a
/// 7 kHz tone 0.2 mHz sharp, enough over half a second to leave a -75 dBc
/// residue beside a fit at the true pitch.
fn tone_at(rate: f32, hz: f32, level: f32, seconds: f32) -> Vec<f32> {
    let step = std::f64::consts::TAU * f64::from(hz) / f64::from(rate);
    (0..(seconds * rate) as usize)
        .map(|n| level * (step * n as f64).sin() as f32)
        .collect()
}

fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

fn granular() -> &'static EffectKind {
    &super::granular::KIND
}

#[test]
fn granular_feedback_at_its_cap_dies_away() {
    let kind = granular();
    for spread in [0.0, 0.2] {
        let mut cloud = at(kind, 48_000.0);
        set(cloud.as_mut(), kind, "position", 0.05);
        set(cloud.as_mut(), kind, "feedback", 0.9);
        set(cloud.as_mut(), kind, "density", 100.0);
        set(cloud.as_mut(), kind, "size", 1.0);
        set(cloud.as_mut(), kind, "spread", spread);
        cloud.reset();
        let mut signal = noise(48_000, 0.5, 71);
        signal.resize(48_000 * 21, 0.0);
        let (left, right) = run(cloud.as_mut(), 120.0, &signal);
        let tail = 48_000 * 20;
        assert!(
            peak(&left[tail..]) < 1e-4,
            "{spread} {}",
            peak(&left[tail..])
        );
        assert!(peak(&right[tail..]) < 1e-4, "{spread}");
    }
}

#[test]
fn a_coherent_cloud_is_never_far_louder_than_its_source() {
    let kind = granular();
    let mut cloud = at(kind, 48_000.0);
    for (name, value) in [
        ("position", 0.05),
        ("spread", 0.0),
        ("detune", 0.0),
        ("feedback", 0.0),
        ("width", 0.0),
        ("density", 100.0),
        ("size", 1.0),
        ("mix", 1.0),
    ] {
        set(cloud.as_mut(), kind, name, value);
    }
    cloud.reset();
    let steady = vec![0.5; 48_000 * 3];
    let (left, _) = run(cloud.as_mut(), 120.0, &steady);
    // With every grain reading the same audio, the pool of 64 may add up to
    // at most the fourth root of 64 over the source: +9 dB, 0.5 to 1.414.
    let loudest = peak(&left[48_000..]);
    assert!(
        loudest <= 0.5_f32.mul_add(64f32.sqrt().sqrt(), 1e-3),
        "{loudest}"
    );
}

fn freeze_switch_steps(from: f32, to: f32) -> f32 {
    let kind = granular();
    let mut cloud = at(kind, 48_000.0);
    for (name, value) in [
        ("position", 0.0),
        ("spread", 0.0),
        ("detune", 0.0),
        ("pitch", 0.0),
        ("feedback", 0.0),
        ("width", 0.0),
        ("density", 60.0),
        ("mix", 1.0),
        ("freeze", from),
    ] {
        set(cloud.as_mut(), kind, name, value);
    }
    cloud.reset();
    let steady = vec![0.5; 48_000];
    let (before, _) = run(cloud.as_mut(), 120.0, &steady);
    set(cloud.as_mut(), kind, "freeze", to);
    let (after, _) = run(cloud.as_mut(), 120.0, &steady[..4_800]);
    let mut joined = before[before.len() - 100..].to_vec();
    joined.extend_from_slice(&after);
    largest_step(&joined)
}

#[test]
fn switching_freeze_does_not_click() {
    let on = freeze_switch_steps(0.0, 1.0);
    assert!(on < 0.05, "freeze on: {on}");
    let off = freeze_switch_steps(1.0, 0.0);
    assert!(off < 0.05, "freeze off: {off}");
}

#[test]
fn every_effect_survives_any_sample_rate() {
    for kind in KINDS {
        for rate in [
            0.3,
            1.0,
            2.0,
            3.0,
            100.0,
            8_000.0,
            22_050.0,
            384_000.0,
            1e9,
            f32::NAN,
            -48_000.0,
        ] {
            let mut effect = at(kind, rate);
            let input = noise(4_096, 0.5, 73);
            let (left, right) = run(effect.as_mut(), 120.0, &input);
            assert!(
                left.iter().chain(&right).all(|v| v.is_finite()),
                "{} {rate}",
                kind.id
            );
        }
    }
}

#[test]
fn through_zero_flanging_holds_the_dry_back_exactly() {
    let kind = &super::flanger::KIND;
    for rate in [44_100.0, 48_000.0] {
        let mut flanger = at(kind, rate);
        set(flanger.as_mut(), kind, "mode", 1.0);
        set(flanger.as_mut(), kind, "mix", 0.0);
        set(flanger.as_mut(), kind, "manual", 0.002_01);
        flanger.reset();
        let latency = flanger.latency();
        let mut click = vec![0.0; 4_096];
        click[0] = 1.0;
        let (left, _) = run(flanger.as_mut(), 120.0, &click);
        assert!(
            (left[latency] - 1.0).abs() < 1e-6,
            "{rate} {}",
            left[latency]
        );
        // The knob moves the sweep, not the latency.
        set(flanger.as_mut(), kind, "manual", 0.007);
        assert_eq!(flanger.latency(), latency, "{rate}");
    }
}

fn tape_echo_rms(drive: f32) -> f32 {
    let kind = &super::tape::KIND;
    let mut tape = at(kind, 48_000.0);
    for (name, value) in [
        ("wow", 0.0),
        ("flutter", 0.0),
        ("hiss", 0.0),
        ("feedback", 0.0),
        ("mix", 1.0),
        ("drive", drive),
    ] {
        set(tape.as_mut(), kind, name, value);
    }
    tape.reset();
    let input = sine(48_000 * 2, 440.0, 0.5);
    let (left, _) = run(tape.as_mut(), 120.0, &input);
    rms(&left[48_000..])
}

#[test]
fn tape_drive_adds_grit_not_a_level_drop() {
    let clean = tape_echo_rms(0.0);
    let driven = tape_echo_rms(1.0);
    let change = 20.0 * (driven / clean).log10();
    assert!(change.abs() < 3.0, "{clean} {driven} {change} dB");
}

fn hiss_level(rate: f32) -> f32 {
    let kind = &super::tape::KIND;
    let render = |hiss: f32| {
        let mut tape = at(kind, rate);
        for (name, value) in [
            ("wow", 0.0),
            ("flutter", 0.0),
            ("feedback", 0.0),
            ("mix", 1.0),
            ("hiss", hiss),
        ] {
            set(tape.as_mut(), kind, name, value);
        }
        tape.reset();
        let input = tone_at(rate, 1_000.0, 0.5, 1.0);
        run(tape.as_mut(), 120.0, &input).0
    };
    let with = render(1.0);
    let without = render(0.0);
    let from = (0.5 * rate) as usize;
    let difference: Vec<f32> = with[from..]
        .iter()
        .zip(&without[from..])
        .map(|(a, b)| a - b)
        .collect();
    rms(&difference)
}

#[test]
fn tape_hiss_is_as_loud_at_every_rate() {
    let base = hiss_level(48_000.0);
    assert!(base > 1e-5, "{base}");
    for rate in [96_000.0, 192_000.0] {
        let now = hiss_level(rate);
        let change = 20.0 * (now / base).log10();
        assert!(change.abs() < 0.5, "{rate} {change} dB");
    }
}

#[test]
fn feedback_loops_leave_no_subnormal_hum() {
    for (kind, name, value) in [
        (&super::phaser::KIND, "colour", 0.9),
        (&super::bbd::KIND, "feedback", 0.95),
        (&super::flanger::KIND, "feedback", -0.95),
    ] {
        let mut effect = at(kind, 48_000.0);
        set(effect.as_mut(), kind, name, value);
        if kind.id == "phaser" {
            set(effect.as_mut(), kind, "stages", 3.0);
        }
        effect.reset();
        let mut signal = noise(24_000, 0.5, 79);
        signal.resize(48_000 * 40, 0.0);
        let (left, right) = run(effect.as_mut(), 120.0, &signal);
        let tail = 48_000 * 39;
        for sample in left[tail..].iter().chain(&right[tail..]) {
            assert!(
                *sample == 0.0 || sample.is_normal(),
                "{} leaves {sample:e}",
                kind.id
            );
        }
    }
}

/// Energy in `signal` away from the harmonics of `hz`, relative to the
/// tone, in decibels: what aliasing leaves behind. Each harmonic is fitted
/// and subtracted sample by sample in double precision, so the measure
/// reaches well below -140 dBc. `signal` must hold whole cycles of `hz`.
fn inharmonic_db(signal: &[f32], hz: f32, rate: f32) -> f64 {
    let len = signal.len() as f64;
    let mut residual: Vec<f64> = signal.iter().map(|v| f64::from(*v)).collect();
    let mut fundamental = 0.0;
    // Every harmonic strictly below Nyquist.
    let highest = ((0.5 * f64::from(rate) / f64::from(hz)).ceil() as usize)
        .saturating_sub(1)
        .max(1);
    for count in 1..=highest {
        let step = std::f64::consts::TAU * f64::from(hz) * count as f64 / f64::from(rate);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, sample) in residual.iter().enumerate() {
            let (sin, cos) = (step * n as f64).sin_cos();
            re = sample.mul_add(cos, re);
            im = sample.mul_add(sin, im);
        }
        let (a, b) = (2.0 * re / len, 2.0 * im / len);
        if count == 1 {
            fundamental = a.hypot(b);
        }
        for (n, sample) in residual.iter_mut().enumerate() {
            let (sin, cos) = (step * n as f64).sin_cos();
            *sample -= a.mul_add(cos, b * sin);
        }
    }
    let rest = residual.iter().map(|v| v * v).sum::<f64>() / len;
    10.0 * (rest.max(1e-30) / (0.5 * fundamental * fundamental)).log10()
}

#[test]
fn a_hot_chorus_does_not_alias() {
    let kind = &super::chorus::KIND;
    let mut chorus = at(kind, 48_000.0);
    set(chorus.as_mut(), kind, "mix", 1.0);
    set(chorus.as_mut(), kind, "depth", 0.0);
    chorus.reset();
    let input = tone_at(48_000.0, 7_000.0, 1.0, 1.0);
    let (left, _) = run(chorus.as_mut(), 120.0, &input);
    let floor = inharmonic_db(&left[24_000..], 7_000.0, 48_000.0);
    assert!(floor < -80.0, "{floor} dBc");
}

#[test]
fn the_anti_aliased_saturator_beats_plain_tanh() {
    let rate = 48_000.0;
    let input = tone_at(rate, 7_000.0, 4.0, 0.5);
    let mut saturator = super::parts::Saturator::default();
    let smooth: Vec<f32> = input.iter().map(|x| saturator.process(*x, 1.0)).collect();
    let plain: Vec<f32> = input.iter().map(|x| x.tanh()).collect();
    let smooth_floor = inharmonic_db(&smooth[4_800..], 7_000.0, rate);
    let plain_floor = inharmonic_db(&plain[4_800..], 7_000.0, rate);
    assert!(
        smooth_floor < plain_floor - 6.0,
        "{smooth_floor} {plain_floor}"
    );
}

#[test]
fn slow_lfos_run_at_their_rate_at_high_sample_rates() {
    for rate in [192_000.0f32, 384_000.0] {
        for hz in [0.02f32, 0.05, 0.128] {
            let mut lfo = crate::dsp::Phasor::default();
            let samples = (3.0 / hz * rate) as usize;
            let mut wraps = 0usize;
            let mut last = lfo.next(hz, rate);
            let mut first_wrap = None;
            let mut last_wrap = 0;
            for n in 1..samples {
                let now = lfo.next(hz, rate);
                if now < last {
                    wraps += 1;
                    first_wrap.get_or_insert(n);
                    last_wrap = n;
                }
                last = now;
            }
            assert!(wraps >= 2, "{rate} {hz}");
            let cycles = (wraps - 1) as f64;
            let measured = cycles * f64::from(rate) / (last_wrap - first_wrap.unwrap_or(0)) as f64;
            let error = (measured / f64::from(hz) - 1.0).abs();
            assert!(error < 1e-3, "{rate} {hz} {measured}");
        }
    }
}

#[test]
fn the_shortest_bucket_brigade_time_holds_at_low_rates() {
    let kind = &super::bbd::KIND;
    let arrival = |rate: f32| {
        let mut bbd = at(kind, rate);
        for (name, value) in [
            ("time", 0.02),
            ("feedback", 0.0),
            ("depth", 0.0),
            ("mix", 1.0),
        ] {
            set(bbd.as_mut(), kind, name, value);
        }
        bbd.reset();
        let mut click = vec![0.0; rate as usize / 4];
        click[0] = 1.0;
        let (left, _) = run(bbd.as_mut(), 120.0, &click);
        let loudest = peak(&left);
        let first = left
            .iter()
            .position(|v| v.abs() > loudest * 0.01)
            .unwrap_or(0);
        first as f64 / f64::from(rate)
    };
    let reference = arrival(48_000.0);
    assert!((reference - 0.02).abs() < 0.001, "{reference}");
    for rate in [8_000.0f32, 11_025.0] {
        let now = arrival(rate);
        assert!((now - reference).abs() < 0.001, "{rate} {now} {reference}");
    }
}

#[test]
fn a_glide_arrives_exactly() {
    let rate = 192_000.0;
    let mut glide = crate::dsp::Smoothed::new(0.3);
    glide.set_time(0.12, rate);
    glide.set(0.6);
    for _ in 0..(20.0 * rate) as usize {
        glide.step();
    }
    assert!(
        (glide.value() - 0.6).abs() < f32::EPSILON,
        "{}",
        glide.value()
    );
}

#[test]
fn a_bucket_brigade_time_change_lands_exactly_at_high_rates() {
    // Gliding to a new time must end exactly where setting it outright
    // does: a single-precision glide stalled 0.69 ms short at 192 kHz.
    let kind = &super::bbd::KIND;
    let rate = 192_000.0f32;
    let arrival = |glide: bool| {
        let mut bbd = at(kind, rate);
        for (name, value) in [("feedback", 0.0), ("depth", 0.0), ("mix", 1.0)] {
            set(bbd.as_mut(), kind, name, value);
        }
        if !glide {
            set(bbd.as_mut(), kind, "time", 0.6);
        }
        bbd.reset();
        set(bbd.as_mut(), kind, "time", 0.6);
        let quiet = vec![0.0; (4.0 * rate) as usize];
        run(bbd.as_mut(), 120.0, &quiet);
        let mut click = vec![0.0; (0.7 * rate) as usize];
        click[0] = 1.0;
        let (left, _) = run(bbd.as_mut(), 120.0, &click);
        let energy: f64 = left.iter().map(|v| f64::from(*v).powi(2)).sum();
        left.iter()
            .position(|v| f64::from(*v).powi(2) > energy * 1e-4)
            .unwrap_or(0)
    };
    let glided = arrival(true);
    let set_outright = arrival(false);
    // The chip samples at its own clock, so an arrival can differ by up to
    // one tick (0.6 s over 2048 buckets) with where the click fell.
    let tick = (0.6 * rate / 2_048.0).ceil() as usize + 1;
    assert!(
        glided.abs_diff(set_outright) <= tick,
        "{glided} {set_outright}"
    );
}

#[test]
fn full_stereo_tremolo_pans_at_equal_power() {
    let kind = &super::trem::KIND;
    let mut trem = at(kind, 48_000.0);
    set(trem.as_mut(), kind, "depth", 1.0);
    set(trem.as_mut(), kind, "stereo", 100.0);
    trem.reset();
    let steady = vec![1.0; 48_000];
    let (left, right) = run(trem.as_mut(), 120.0, &steady);
    for (l, r) in left.iter().zip(&right).skip(2_000) {
        assert!((l * l + r * r - 1.0).abs() < 1e-3, "{l} {r}");
    }
}

#[test]
fn a_digital_time_change_keeps_its_level() {
    let kind = &super::digital::KIND;
    let mut delay = at(kind, 48_000.0);
    for (name, value) in [
        ("feedback", 0.0),
        ("cross", 0.0),
        ("mix", 1.0),
        ("lowcut", 20.0),
        ("highcut", 20_000.0),
        ("ltime", 0.1),
    ] {
        set(delay.as_mut(), kind, name, value);
    }
    delay.reset();
    let source = noise(48_000 * 2, 0.5, 83);
    let (before, _) = run(delay.as_mut(), 120.0, &source[..48_000]);
    set(delay.as_mut(), kind, "ltime", 0.3);
    let (after, _) = run(delay.as_mut(), 120.0, &source[48_000..]);
    let steady = rms(&before[24_000..]);
    // The 40 ms crossfade, in 10 ms windows.
    for window in after[..1_920].chunks(480) {
        let change = 20.0 * (rms(window) / steady).log10();
        assert!(change.abs() < 1.5, "{change} dB");
    }
}

#[test]
fn tape_wow_and_flutter_really_wobble_the_pitch() {
    let kind = &super::tape::KIND;
    let spread = |wow: f32, flutter: f32| {
        let mut tape = at(kind, 48_000.0);
        for (name, value) in [
            ("hiss", 0.0),
            ("feedback", 0.0),
            ("mix", 1.0),
            ("drive", 0.0),
            ("wow", wow),
            ("flutter", flutter),
        ] {
            set(tape.as_mut(), kind, name, value);
        }
        tape.reset();
        let input = sine(48_000 * 4, 1_000.0, 0.25);
        let (left, _) = run(tape.as_mut(), 120.0, &input);
        // How much of the tone has left 1 kHz for its sidebands.
        let steady = &left[48_000..];
        1.0 - tone_level(steady, 1_000.0) / (rms(steady) * std::f32::consts::SQRT_2)
    };
    let still = spread(0.0, 0.0);
    assert!(still < 1e-3, "{still}");
    assert!(spread(1.0, 0.0) > 0.01, "wow");
    assert!(spread(0.0, 1.0) > 0.01, "flutter");
}

#[test]
fn tape_hiss_fades_with_the_echoes() {
    let kind = &super::tape::KIND;
    let mut tape = at(kind, 48_000.0);
    set(tape.as_mut(), kind, "hiss", 1.0);
    set(tape.as_mut(), kind, "feedback", 0.0);
    tape.reset();
    let mut signal = noise(24_000, 0.5, 89);
    signal.resize(48_000 * 4, 0.0);
    let (left, _) = run(tape.as_mut(), 120.0, &signal);
    assert!(rms(&left[24_000..40_000]) > 1e-3);
    assert!(
        peak(&left[48_000 * 3..]) < 1e-6,
        "{}",
        peak(&left[48_000 * 3..])
    );
}

#[test]
fn the_compander_breathes() {
    // The chip's noise sits between compressor and expander, so it rises
    // under a loud note and falls away in the gaps.
    let kind = &super::bbd::KIND;
    let hum = |level: f32| {
        let mut bbd = at(kind, 48_000.0);
        for (name, value) in [
            ("feedback", 0.0),
            ("depth", 0.0),
            ("mix", 1.0),
            ("time", 0.05),
        ] {
            set(bbd.as_mut(), kind, name, value);
        }
        bbd.reset();
        let input = sine(48_000, 100.0, level);
        let (left, _) = run(bbd.as_mut(), 120.0, &input);
        let steady = &left[24_000..];
        // What is left once the 100 Hz tone and its harmonics are out:
        // mostly the chip's noise, lifted by the expander.
        let residual: Vec<f32> = steady
            .windows(3)
            .map(|w| (-2.0_f32).mul_add(w[1], w[0] + w[2]))
            .collect();
        rms(&residual)
    };
    let quiet = hum(0.01);
    let loud = hum(0.5);
    assert!(loud > quiet * 4.0, "{quiet} {loud}");
}

#[test]
fn colour_sharpens_the_phaser_and_the_sweep_moves_the_notch() {
    let kind = &super::phaser::KIND;
    let peak_of = |colour: f32| {
        let mut phaser = at(kind, 48_000.0);
        for (name, value) in [("depth", 0.0), ("centre", 1_000.0), ("colour", colour)] {
            set(phaser.as_mut(), kind, name, value);
        }
        phaser.reset();
        let input = sine(24_000, 1_000.0, 0.2);
        let (left, _) = run(phaser.as_mut(), 120.0, &input);
        tone_level(&left[12_000..], 1_000.0) / 0.2
    };
    assert!(
        peak_of(0.8) > peak_of(0.0) * 1.5,
        "{} {}",
        peak_of(0.8),
        peak_of(0.0)
    );
    let mut phaser = at(kind, 48_000.0);
    set(phaser.as_mut(), kind, "colour", 0.0);
    set(phaser.as_mut(), kind, "centre", 1_000.0);
    set(phaser.as_mut(), kind, "rate", 1.0);
    phaser.reset();
    let input = sine(48_000 * 2, 414.0, 0.2);
    let (left, _) = run(phaser.as_mut(), 120.0, &input);
    let levels: Vec<f32> = left.chunks(2_400).map(rms).collect();
    let lowest = levels.iter().fold(f32::MAX, |a, b| a.min(*b));
    let highest = levels.iter().fold(0.0f32, |a, b| a.max(*b));
    assert!(highest > lowest * 5.0, "{lowest} {highest}");
}

#[test]
fn a_through_zero_sweep_crosses_the_dry() {
    // Sweeping the wet through the dry takes the comb's first notch past
    // the top of the band and back, so a high tone swings from cancelled
    // to whole.
    let kind = &super::flanger::KIND;
    let mut flanger = at(kind, 48_000.0);
    for (name, value) in [
        ("mode", 1.0),
        ("feedback", 0.0),
        ("depth", 1.0),
        ("manual", 0.002),
        ("rate", 0.5),
        ("mix", 0.5),
    ] {
        set(flanger.as_mut(), kind, name, value);
    }
    flanger.reset();
    let input = sine(48_000 * 3, 6_000.0, 0.5);
    let (left, _) = run(flanger.as_mut(), 120.0, &input);
    // Short windows: the notch passes a 6 kHz tone in well under 1 ms.
    let levels: Vec<f32> = left[4_800..].chunks(48).map(rms).collect();
    let lowest = levels.iter().fold(f32::MAX, |a, b| a.min(*b));
    let highest = levels.iter().fold(0.0f32, |a, b| a.max(*b));
    assert!(lowest < 0.02 && highest > 0.33, "{lowest} {highest}");
}

#[test]
fn shifter_feedback_climbs_like_a_barber_pole() {
    let kind = &super::shift::KIND;
    let mut shifter = at(kind, 48_000.0);
    for (name, value) in [
        ("shift", 100.0),
        ("feedback", 0.8),
        ("delay", 0.05),
        ("mix", 1.0),
    ] {
        set(shifter.as_mut(), kind, name, value);
    }
    shifter.reset();
    let input = sine(48_000 * 2, 500.0, 0.2);
    let (left, _) = run(shifter.as_mut(), 120.0, &input);
    let steady = &left[24_000..];
    for step in 1..=3 {
        let hz = 100.0_f32.mul_add(step as f32, 500.0);
        assert!(tone_level(steady, hz) > 0.01, "{hz}");
    }
}

#[test]
fn granular_pitch_reverse_and_determinism() {
    let kind = granular();
    let render = |pitch: f32, reverse: f32| {
        let mut cloud = at(kind, 48_000.0);
        for (name, value) in [
            ("spread", 0.0),
            ("detune", 0.0),
            ("feedback", 0.0),
            ("width", 0.0),
            ("mix", 1.0),
            ("position", 0.5),
            ("pitch", pitch),
            ("reverse", reverse),
        ] {
            set(cloud.as_mut(), kind, name, value);
        }
        cloud.reset();
        let input = sine(48_000 * 3, 440.0, 0.4);
        run(cloud.as_mut(), 120.0, &input).0
    };
    let up = render(12.0, 0.0);
    let steady = &up[48_000..];
    assert!(tone_level(steady, 880.0) > tone_level(steady, 440.0) * 4.0);
    assert_eq!(render(12.0, 0.0), up, "the same input gives the same cloud");
    // A reversed sine is still a sine at the same pitch.
    let back = render(0.0, 1.0);
    assert!(tone_level(&back[48_000..], 440.0) > 0.1);
    // A chirp played backwards falls instead of rising: compare where the
    // energy of a rising sweep ends up.
    let mut cloud = at(kind, 48_000.0);
    for (name, value) in [
        ("spread", 0.0),
        ("detune", 0.0),
        ("feedback", 0.0),
        ("width", 0.0),
        ("mix", 1.0),
        ("reverse", 1.0),
        ("size", 0.5),
        ("density", 2.0),
        ("position", 1.0),
    ] {
        set(cloud.as_mut(), kind, name, value);
    }
    cloud.reset();
    let chirp: Vec<f32> = (0..48_000 * 3)
        .map(|n| {
            let t = n as f32 / 48_000.0;
            0.4 * (TAU * 200.0 * t * t).sin()
        })
        .collect();
    let (left, _) = run(cloud.as_mut(), 120.0, &chirp);
    let crossings = |part: &[f32]| {
        part.windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count()
    };
    // Within a reversed grain the pitch falls: the first half of a grain
    // crosses zero more often than its second half.
    let grain = &left[48_000 + 2_400..48_000 + 2_400 + 12_000];
    assert!(crossings(&grain[..6_000]) > crossings(&grain[6_000..]));
}

#[test]
fn every_parameter_jump_is_click_free() {
    for kind in KINDS {
        for (index, spec) in kind.params.iter().enumerate() {
            if matches!(spec.name, "freeze") {
                continue;
            }
            let jump = |from: f32, to: f32| {
                let mut effect = at(kind, 48_000.0);
                effect.set_param(index, from);
                effect.reset();
                let input = sine(48_000, 220.0, 0.2);
                let (a, _) = run(effect.as_mut(), 120.0, &input[..24_000]);
                effect.set_param(index, to);
                let (b, _) = run(effect.as_mut(), 120.0, &input[24_000..]);
                let edge = a[23_900..]
                    .iter()
                    .chain(&b[..2_400])
                    .copied()
                    .collect::<Vec<f32>>();
                let calm = [&a[12_000..23_900], &b[12_000..]]
                    .iter()
                    .map(|part| curvature(part))
                    .fold(0.0, f32::max);
                (curvature(&edge), calm)
            };
            for (from, to) in [(spec.min, spec.max), (spec.max, spec.min)] {
                let (edge, calm) = jump(from, to);
                assert!(
                    edge <= calm.mul_add(4.0, 0.02),
                    "{} {} {from}->{to}: {edge} vs {calm}",
                    kind.id,
                    spec.name
                );
            }
        }
    }
}

/// The largest second difference: a click shows as a spike in it.
fn curvature(signal: &[f32]) -> f32 {
    signal
        .windows(3)
        .map(|w| (-2.0_f32).mul_add(w[1], w[0] + w[2]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_whole_note_at_twenty_bpm_fits_at_high_rates() {
    for kind in [&super::tape::KIND, &super::digital::KIND] {
        let rate = 384_000.0f32;
        let mut effect = at(kind, rate);
        let (sync, time) = if kind.id == "tape" {
            ("sync", None)
        } else {
            ("lsync", Some("rsync"))
        };
        set(effect.as_mut(), kind, sync, 14.0);
        if let Some(other) = time {
            set(effect.as_mut(), kind, other, 14.0);
        }
        for (name, value) in [("feedback", 0.0), ("mix", 1.0)] {
            set(effect.as_mut(), kind, name, value);
        }
        if kind.id == "tape" {
            for name in ["wow", "flutter", "hiss"] {
                set(effect.as_mut(), kind, name, 0.0);
            }
        }
        effect.reset();
        let mut click = vec![0.0; (12.1 * rate) as usize];
        click[0] = 1.0;
        let (left, _) = run(effect.as_mut(), 20.0, &click);
        let at_peak = super::testkit::peak_index(&left);
        let seconds = at_peak as f64 / f64::from(rate);
        assert!((seconds - 12.0).abs() < 0.001, "{} {seconds}", kind.id);
    }
}

#[test]
fn every_head_combination_plays_its_heads() {
    let kind = &super::tape::KIND;
    let places = [0.2f32, 0.4, 0.6];
    let sets: [[bool; 3]; 7] = [
        [true, false, false],
        [false, true, false],
        [false, false, true],
        [true, true, false],
        [false, true, true],
        [true, false, true],
        [true, true, true],
    ];
    for (step, heads) in sets.iter().enumerate() {
        let mut tape = at(kind, 48_000.0);
        for (name, value) in [
            ("wow", 0.0),
            ("flutter", 0.0),
            ("hiss", 0.0),
            ("feedback", 0.0),
            ("mix", 1.0),
            ("time", 0.6),
            ("heads", step as f32),
        ] {
            set(tape.as_mut(), kind, name, value);
        }
        tape.reset();
        let mut click = vec![0.0; 48_000];
        click[0] = 1.0;
        let (left, _) = run(tape.as_mut(), 120.0, &click);
        let loudest = peak(&left);
        for (place, on) in places.iter().zip(heads) {
            let at_head = (place * 48_000.0) as usize;
            let here = peak(&left[at_head - 48..at_head + 48]);
            if *on {
                assert!(here > loudest * 0.5, "step {step} head at {place} missing");
            } else {
                assert!(here < loudest * 0.01, "step {step} head at {place} sounds");
            }
        }
    }
}

#[test]
fn the_juno_chorus_sweeps_its_measured_range_at_its_rate() {
    // Mode I: a click every 10 ms through the wet side alone shows where
    // the delay sits; it should run 1.66 to 5.35 ms and back in 1/0.513 s.
    let kind = &super::chorus::KIND;
    let mut chorus = at(kind, 48_000.0);
    set(chorus.as_mut(), kind, "mix", 1.0);
    chorus.reset();
    let gap = 480;
    let mut clicks = vec![0.0; 48_000 * 5];
    for n in (0..clicks.len()).step_by(gap) {
        clicks[n] = 1.0;
    }
    let (left, _) = run(chorus.as_mut(), 120.0, &clicks);
    let delays: Vec<f32> = left
        .chunks(gap)
        .map(|window| super::testkit::peak_index(window) as f32 / 48.0)
        .collect();
    let shortest = delays.iter().fold(f32::MAX, |a, b| a.min(*b));
    let longest = delays.iter().fold(0.0f32, |a, b| a.max(*b));
    // The reconstruction lowpass adds a few hundredths of a millisecond.
    assert!((shortest - 1.66).abs() < 0.1, "{shortest}");
    assert!((longest - 5.35).abs() < 0.1, "{longest}");
    let tops: Vec<usize> = (1..delays.len() - 1)
        .filter(|&n| delays[n] >= longest - 0.03 && delays[n] > delays[n - 1] - 0.001)
        .collect();
    let first = tops.first().copied().unwrap_or(0);
    let next = tops.iter().copied().find(|&n| n > first + 50).unwrap_or(0);
    let period = (next - first) as f32 * gap as f32 / 48_000.0;
    assert!((period - 1.0 / 0.513).abs() < 0.05, "{period}");
}

#[test]
fn harmonic_tremolo_trades_lows_for_highs() {
    let kind = &super::trem::KIND;
    let mut trem = at(kind, 48_000.0);
    set(trem.as_mut(), kind, "shape", 3.0);
    set(trem.as_mut(), kind, "depth", 1.0);
    set(trem.as_mut(), kind, "rate", 4.0);
    trem.reset();
    // A low and a high tone together; the two should swell in turn.
    let low = sine(48_000, 150.0, 0.3);
    let high = sine(48_000, 4_000.0, 0.3);
    let input: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
    let (left, _) = run(trem.as_mut(), 120.0, &input);
    let window = 1_200;
    let lows: Vec<f32> = left.chunks(window).map(|w| tone_level(w, 150.0)).collect();
    let highs: Vec<f32> = left
        .chunks(window)
        .map(|w| tone_level(w, 4_000.0))
        .collect();
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (low_mean, high_mean) = (mean(&lows), mean(&highs));
    let covariance: f32 = lows
        .iter()
        .zip(&highs)
        .map(|(l, h)| (l - low_mean) * (h - high_mean))
        .sum();
    let spread = |v: &[f32], m: f32| v.iter().map(|x| (x - m) * (x - m)).sum::<f32>().sqrt();
    let correlation = covariance / (spread(&lows, low_mean) * spread(&highs, high_mean));
    assert!(correlation < -0.8, "{correlation}");
    assert!(spread(&lows, low_mean) > 0.1, "the lows barely move");
}
