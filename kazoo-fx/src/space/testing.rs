//! Test helpers for the space family: running an effect over whole
//! signals, measuring decay times, and the checks every effect must pass.

use crate::dsp::Noise;
use crate::{Context, Effect, EffectKind};

/// The rate the tests run at.
pub const RATE: f32 = 48_000.0;

/// The context the tests pass.
pub const CONTEXT: Context = Context { bpm: 120.0 };

/// The kind with this id, from the family's own list.
pub fn kind(id: &str) -> &'static EffectKind {
    super::KINDS
        .iter()
        .find(|kind| kind.id == id)
        .unwrap_or_else(|| panic!("no space effect {id}"))
}

/// A prepared effect of this kind with the given knobs set.
pub fn build(id: &str, knobs: &[(&str, f32)]) -> Box<dyn Effect> {
    build_at(id, RATE, knobs)
}

/// [`build`] at another sample rate.
pub fn build_at(id: &str, rate: f32, knobs: &[(&str, f32)]) -> Box<dyn Effect> {
    let kind = kind(id);
    let mut effect = (kind.build)();
    // Knobs set before prepare start settled, with no glide from the
    // defaults.
    for &(name, value) in knobs {
        let index = kind
            .params
            .iter()
            .position(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("{id} has no knob {name}"));
        effect.set_param(index, value);
    }
    effect.prepare(rate);
    effect
}

/// Run both channels through in blocks of 256.
pub fn run(effect: &mut dyn Effect, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut out_l = vec![0.0; left.len()];
    let mut out_r = vec![0.0; right.len()];
    let mut start = 0;
    while start < left.len() {
        let end = (start + 256).min(left.len());
        let outputs = [&mut out_l[start..end], &mut out_r[start..end]];
        effect.process(&CONTEXT, [&left[start..end], &right[start..end]], outputs);
        start = end;
    }
    (out_l, out_r)
}

/// Run a mono signal (fed to both sides).
pub fn run_mono(effect: &mut dyn Effect, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    run(effect, input, input)
}

/// `seconds` of silence.
pub fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * RATE) as usize]
}

/// A unit impulse followed by `seconds` of silence.
pub fn impulse(seconds: f32) -> Vec<f32> {
    let mut signal = silence(seconds);
    signal[0] = 1.0;
    signal
}

/// White noise at `level` peak.
pub fn noise(seconds: f32, level: f32, seed: u32) -> Vec<f32> {
    let mut source = Noise::new(seed);
    (0..(seconds * RATE) as usize)
        .map(|_| source.sample() * level)
        .collect()
}

/// A sine at `hz`.
pub fn sine(seconds: f32, hz: f32, level: f32) -> Vec<f32> {
    (0..(seconds * RATE) as usize)
        .map(|n| level * (std::f32::consts::TAU * hz * n as f32 / RATE).sin())
        .collect()
}

/// Root mean square.
pub fn rms(signal: &[f32]) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    let sum: f64 = signal.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / signal.len() as f64).sqrt() as f32
}

/// Largest absolute sample.
pub fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}

/// A one-pole lowpass over a whole signal (for measuring a band).
pub fn lowpassed(signal: &[f32], hz: f32) -> Vec<f32> {
    let coeff = 1.0 - (-std::f32::consts::TAU * hz / RATE).exp();
    let mut state = 0.0f32;
    signal
        .iter()
        .map(|&x| {
            state = (x - state).mul_add(coeff, state);
            state
        })
        .collect()
}

/// A one-pole highpass over a whole signal.
pub fn highpassed(signal: &[f32], hz: f32) -> Vec<f32> {
    let low = lowpassed(signal, hz);
    signal.iter().zip(low).map(|(&x, l)| x - l).collect()
}

/// The reverberation time of an impulse response: Schroeder's backward
/// integration, a straight line fitted to the decay between -5 and -25 dB,
/// extended to 60 dB (a T20 measurement).
pub fn rt60(response: &[f32]) -> f32 {
    let mut energy: Vec<f64> = response.iter().map(|&x| f64::from(x).powi(2)).collect();
    for n in (0..energy.len().saturating_sub(1)).rev() {
        energy[n] += energy[n + 1];
    }
    let total = energy[0].max(1e-300);
    let curve: Vec<f64> = energy
        .iter()
        .map(|&e| 10.0 * (e / total).max(1e-30).log10())
        .collect();
    let start = curve
        .iter()
        .position(|&db| db <= -5.0)
        .expect("never reached -5 dB");
    let end = curve
        .iter()
        .position(|&db| db <= -25.0)
        .expect("never reached -25 dB");
    // Least squares over the span.
    let points = (end - start + 1) as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (n, &db) in curve.iter().enumerate().take(end + 1).skip(start) {
        let t = n as f64 / f64::from(RATE);
        sx += t;
        sy += db;
        sxx = t.mul_add(t, sxx);
        sxy = t.mul_add(db, sxy);
    }
    let slope = points.mul_add(sxy, -(sx * sy)) / points.mul_add(sxx, -(sx * sx));
    (-60.0 / slope) as f32
}

/// Every knob of `kind` set to `value(spec)`.
fn set_all(effect: &mut dyn Effect, kind: &EffectKind, pick: fn(&crate::ParamSpec) -> f32) {
    for (index, spec) in kind.params.iter().enumerate() {
        effect.set_param(index, pick(spec));
    }
}

/// The checks every effect in the family must pass.
pub fn contract(id: &str) {
    poison_is_refused(id);
    extremes_stay_bounded(id);
    bad_knobs_are_ignored(id);
    silence_stays_silent(id);
    block_shapes_are_honoured(id);
    unprepared_is_silent(id);
    kind_is_well_formed(id);
    rates_agree(id);
    loudness_matches_the_dry(id);
    dry_is_untouched(id);
}

fn poison_is_refused(id: &str) {
    let mut effect = build(id, &[]);
    let mut input = noise(0.5, 0.5, 7);
    input[100] = f32::NAN;
    input[200] = f32::INFINITY;
    input[300] = f32::NEG_INFINITY;
    input[400] = 1e30;
    let (l, r) = run_mono(effect.as_mut(), &input);
    assert!(
        l.iter().chain(&r).all(|x| x.is_finite()),
        "{id}: poison got through"
    );
    // And the state is clean afterwards: ordinary audio still comes out.
    let (l, r) = run_mono(effect.as_mut(), &noise(0.5, 0.5, 8));
    assert!(
        l.iter().chain(&r).all(|x| x.is_finite()),
        "{id}: poisoned state"
    );
    assert!(rms(&l) > 1e-4, "{id}: went dead after poison");
}

/// Loud noise, then silence: the output is finite throughout, and the
/// silence is never louder than the noise was (nothing runs away, not even
/// a frozen tail).
fn never_runs_away(id: &str, effect: &mut dyn Effect, noise_seconds: f32, silence_seconds: f32) {
    let mut input = noise(noise_seconds, 1.0, 3);
    input.extend(silence(silence_seconds));
    let (l, r) = run_mono(effect, &input);
    assert!(
        l.iter().chain(&r).all(|x| x.is_finite()),
        "{id}: not finite"
    );
    let second = RATE as usize;
    let noisy = (noise_seconds * RATE) as usize;
    let loud = rms(&l[noisy - second / 2..noisy]).max(rms(&r[noisy - second / 2..noisy]));
    let end = l.len() - second / 4;
    let tail = rms(&l[end..]).max(rms(&r[end..]));
    assert!(
        tail <= 1.05 * loud.max(1e-9),
        "{id}: {tail} after the input stopped, {loud} while it played"
    );
}

fn extremes_stay_bounded(id: &str) {
    let kind = kind(id);
    for pick in [
        (|s: &crate::ParamSpec| s.min) as fn(&crate::ParamSpec) -> f32,
        |s| s.max,
        |s| s.default,
    ] {
        let mut effect = build(id, &[]);
        set_all(effect.as_mut(), kind, pick);
        never_runs_away(id, effect.as_mut(), 3.0, 1.0);
    }
    // Every knob alone at each end.
    for index in 0..kind.params.len() {
        for value in [kind.params[index].min, kind.params[index].max] {
            let mut effect = build(id, &[]);
            effect.set_param(index, value);
            never_runs_away(id, effect.as_mut(), 1.0, 0.5);
        }
    }
}

fn bad_knobs_are_ignored(id: &str) {
    let kind = kind(id);
    let mut plain = build(id, &[]);
    let mut prodded = build(id, &[]);
    for index in 0..kind.params.len() {
        prodded.set_param(index, f32::NAN);
    }
    prodded.set_param(kind.params.len(), 0.5);
    prodded.set_param(usize::MAX, 0.5);
    let input = noise(0.5, 0.5, 11);
    let (a, _) = run_mono(plain.as_mut(), &input);
    let (b, _) = run_mono(prodded.as_mut(), &input);
    assert_eq!(a, b, "{id}: a bad knob changed the sound");
    // Infinities clamp to the ends rather than poisoning.
    for index in 0..kind.params.len() {
        prodded.set_param(index, f32::INFINITY);
        prodded.set_param(index, f32::NEG_INFINITY);
    }
    let (l, r) = run_mono(prodded.as_mut(), &input);
    assert!(l.iter().chain(&r).all(|x| x.is_finite()));
}

fn silence_stays_silent(id: &str) {
    let mut effect = build(id, &[]);
    let (l, r) = run_mono(effect.as_mut(), &silence(1.0));
    assert!(
        l.iter().chain(&r).all(|&x| x == 0.0),
        "{id}: noise from silence"
    );
    // After a burst, the tail dies away without lingering in denormals.
    let mut burst = noise(0.2, 0.8, 13);
    burst.extend(silence(20.0));
    let (l, r) = run_mono(effect.as_mut(), &burst);
    let tail_start = l.len() - (RATE as usize);
    for &x in l[tail_start..].iter().chain(&r[tail_start..]) {
        assert!(x == 0.0 || x.is_normal(), "{id}: denormal in the tail");
        assert!(x.abs() < 1e-6, "{id}: tail never died: {x}");
    }
    // Reset silences at once.
    run_mono(effect.as_mut(), &noise(0.3, 0.8, 17));
    effect.reset();
    let (l, r) = run_mono(effect.as_mut(), &silence(0.5));
    assert!(peak(&l).max(peak(&r)) < 1e-6, "{id}: reset left a tail");
}

fn block_shapes_are_honoured(id: &str) {
    let mut effect = build(id, &[]);
    let input = [0.5f32; 64];
    let mut left = [1.0f32; 64];
    let mut right = [1.0f32; 64];
    effect.process(
        &CONTEXT,
        [&input[..0], &input[..0]],
        [&mut left[..0], &mut right[..0]],
    );
    effect.process(
        &CONTEXT,
        [&input[..], &input[..10]],
        [&mut left[..], &mut right[..]],
    );
    assert!(
        left[10..].iter().chain(&right[10..]).all(|&x| x == 0.0),
        "{id}: tail not silenced"
    );
    assert!(left.iter().chain(&right).all(|x| x.is_finite()));
}

fn unprepared_is_silent(id: &str) {
    let kind = kind(id);
    let mut effect = (kind.build)();
    let input = [0.5f32; 64];
    let mut left = [1.0f32; 64];
    let mut right = [1.0f32; 64];
    effect.set_param(0, 0.5);
    effect.reset();
    effect.process(&CONTEXT, [&input, &input], [&mut left, &mut right]);
    assert!(
        left.iter().chain(&right).all(|&x| x == 0.0),
        "{id}: unprepared made sound"
    );
    for rate in [f32::NAN, 4_000.0, 768_000.0] {
        effect.prepare(rate);
        effect.process(&CONTEXT, [&input, &input], [&mut left, &mut right]);
        assert!(
            left.iter().chain(&right).all(|&x| x == 0.0),
            "{id}: ran at {rate} Hz"
        );
    }
}

/// With the mix at dry, the input comes out bit for bit, however hot:
/// the space effects never touch the dry signal.
fn dry_is_untouched(id: &str) {
    let kind = kind(id);
    let Some(mix) = kind.params.iter().position(|spec| spec.name == "mix") else {
        return;
    };
    let mut effect = build(id, &[]);
    effect.set_param(mix, 0.0);
    effect.prepare(RATE);
    let tone = sine(0.5, 1_000.0, 1.4);
    let (left, right) = run_mono(effect.as_mut(), &tone);
    assert!(
        left.iter()
            .chain(&right)
            .zip(tone.iter().chain(&tone))
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "{id}: the dry signal was changed"
    );
}

fn kind_is_well_formed(id: &str) {
    let kind = kind(id);
    assert!(!kind.params.is_empty());
    for spec in kind.params {
        assert!(spec.min.is_finite() && spec.max.is_finite() && spec.min < spec.max);
        assert!(
            (spec.min..=spec.max).contains(&spec.default),
            "{id}.{}",
            spec.name
        );
        assert!(
            spec.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        match spec.curve {
            crate::Curve::Log => assert!(spec.min > 0.0),
            crate::Curve::Stepped { labels } => {
                let steps = (spec.max - spec.min) as usize + 1;
                assert_eq!(labels.len(), steps, "{id}.{}", spec.name);
                assert!(
                    spec.min.fract().abs() < f32::EPSILON && spec.max.fract().abs() < f32::EPSILON
                );
            }
            crate::Curve::Linear => {}
        }
    }
}

/// A cascade of matched biquads at `hz`: Butterworth sections of the
/// given Qs, lowpass or highpass.
pub fn butterworth(hz: f64, qs: &[f64], high: bool, rate: f32) -> Vec<super::matched::Biquad> {
    qs.iter()
        .map(|&q| {
            let mut section = super::matched::Biquad::default();
            let design = if high {
                super::matched::highpass(hz, q, f64::from(rate))
            } else {
                super::matched::lowpass(hz, q, f64::from(rate))
            };
            section.set(design);
            section
        })
        .collect()
}

/// `signal` through a cascade.
pub fn filtered(signal: &[f32], sections: &mut [super::matched::Biquad]) -> Vec<f32> {
    signal
        .iter()
        .map(|&x| {
            sections
                .iter_mut()
                .fold(f64::from(x), |value, section| section.process(value)) as f32
        })
        .collect()
}

/// Noise at 0.05 RMS, lowpassed at `top` Hz (fourth order), one second.
fn test_noise(rate: f32, top: f64) -> Vec<f32> {
    let mut band = butterworth(top, &[0.541_196_1, 1.306_563], false, rate);
    let mut source = Noise::new(41);
    let raw: Vec<f32> = (0..rate as usize).map(|_| source.sample()).collect();
    let mut input = filtered(&raw, &mut band);
    let scale = 0.05 / rms(&input);
    for sample in &mut input {
        *sample *= scale;
    }
    input
}

/// The same effect at 44.1, 48, 96 and 192 kHz, with `knobs` set (and fully
/// wet): the same level on sustained noise, on a steady 1 kHz tone, in the
/// tail the tone leaves, and above 6 kHz on full-band noise, each within
/// 1.5 dB of 48 kHz. The test signals are filtered with the same analogue
/// shapes at every rate, so they carry the same audio at every rate.
pub fn rates_agree_with(id: &str, knobs: &[(&str, f32)]) {
    let mut all: Vec<(&str, f32)> = Vec::new();
    if kind(id).params.iter().any(|spec| spec.name == "mix") {
        all.push(("mix", 1.0));
    }
    all.extend_from_slice(knobs);
    let measure = |rate: f32| {
        let samples = |seconds: f32| (seconds * rate) as usize;
        let level = |left: &[f32], right: &[f32], from: f32, to: f32| {
            let span = samples(from)..samples(to);
            let out = rms(&left[span.clone()]).hypot(rms(&right[span]));
            20.0 * (out / 0.05).log10()
        };
        let mut effect = build_at(id, rate, &all);
        let input = test_noise(rate, 5_000.0);
        let (left, right) = run(effect.as_mut(), &input, &input);
        let sustained = level(&left, &right, 0.5, 1.0);
        // The tone, and the tail it leaves, are the same waveform at every
        // rate, so they compare exactly (a noise tail is a lottery of which
        // modes happened to be ringing when it stopped).
        effect.reset();
        let mut tone: Vec<f32> = (0..samples(0.6))
            .map(|n| {
                0.05 * std::f32::consts::SQRT_2
                    * (std::f32::consts::TAU * 1_000.0 * n as f32 / rate).sin()
            })
            .collect();
        tone.extend(std::iter::repeat_n(0.0, samples(0.4)));
        let (left, right) = run(effect.as_mut(), &tone, &tone);
        let steady = level(&left, &right, 0.3, 0.6);
        let tail = level(&left, &right, 0.65, 0.95);
        // The top of the band: full-band noise, heard above 6 kHz.
        effect.reset();
        let input = test_noise(rate, 18_000.0);
        let (left, right) = run(effect.as_mut(), &input, &input);
        let above = |side: &[f32]| {
            let mut high = butterworth(6_000.0, &[0.541_196_1, 1.306_563], true, rate);
            filtered(side, &mut high)
        };
        let bright = level(&above(&left), &above(&right), 0.5, 1.0);
        [sustained, steady, tail, bright]
    };
    let reference = measure(RATE);
    for rate in [44_100.0, 96_000.0, 192_000.0] {
        let got = measure(rate);
        for ((what, want), have) in ["sustained", "tone", "tone tail", "treble"]
            .iter()
            .zip(reference)
            .zip(got)
        {
            // Silence (an EQ's tail) is silence at any rate.
            if want < -120.0 && have < -120.0 {
                continue;
            }
            assert!(
                (have - want).abs() < 1.5,
                "{id} {knobs:?} at {rate} Hz: {what} level {have:.2} dB, {want:.2} dB at 48 kHz"
            );
        }
    }
}

fn rates_agree(id: &str) {
    rates_agree_with(id, &[]);
}

/// Sustained band-limited noise at `level` RMS through the effect at its
/// defaults (fully wet), and the wet level against the dry in dB: the mean
/// power of the two sides over the input's.
pub fn noise_level(id: &str, knobs: &[(&str, f32)], level: f32) -> f32 {
    let mut all: Vec<(&str, f32)> = Vec::new();
    if kind(id).params.iter().any(|spec| spec.name == "mix") {
        all.push(("mix", 1.0));
    }
    all.extend_from_slice(knobs);
    let mut effect = build(id, &all);
    let mut band = super::matched::Biquad::default();
    band.set(super::matched::lowpass(
        5_000.0,
        std::f64::consts::FRAC_1_SQRT_2,
        f64::from(RATE),
    ));
    let mut source = Noise::new(43);
    let mut input: Vec<f32> = (0..(2.0 * RATE) as usize)
        .map(|_| band.process(f64::from(source.sample())) as f32)
        .collect();
    let scale = level / rms(&input);
    for sample in &mut input {
        *sample *= scale;
    }
    let (left, right) = run_mono(effect.as_mut(), &input);
    let settled = RATE as usize;
    let power = 0.5 * rms(&left[settled..]).hypot(rms(&right[settled..])).powi(2);
    10.0 * (power / rms(&input[settled..]).powi(2)).log10()
}

/// At its defaults, fully wet, every effect gives back sustained noise
/// within 3 dB of the dry level, so swapping one for another never jumps.
fn loudness_matches_the_dry(id: &str) {
    let level = noise_level(id, &[], 0.05);
    assert!(
        level.abs() <= 3.0,
        "{id}: wet noise at {level:.2} dB against the dry"
    );
}
