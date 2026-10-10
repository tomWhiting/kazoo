//! Every effect's wet signal, beside its mix.
//!
//! For an effect with a mix knob, the wet output is exactly what the mixed
//! output is with the knob all the way to wet, whatever the knob is at; for
//! one without, the wet output is the output. Asking for the wet signal
//! leaves the mixed output exactly as it was, and the wet signal is finite
//! and bounded whatever goes in and wherever the knobs are.

use kazoo_fx::{Context, Effect, EffectKind, catalogue};

const RATE: f32 = 48_000.0;
/// An odd block size, so blocks never line up with anything inside.
const BLOCK: usize = 333;
/// Every family holds its wet signal under +6 dBFS.
const CEILING: f32 = 2.0;
/// The loudest input any family takes (+36 dBFS): a dry signal passed
/// through untouched is held there at most.
const DRY_HEADROOM: f32 = 64.0;

const CONTEXT: Context = Context { bpm: 120.0 };

/// A stereo test signal: a chord and some noise, different on each side,
/// with a gap of silence so tails show.
fn programme(len: usize) -> [Vec<f32>; 2] {
    let mut noise = kazoo_fx::dsp::Noise::new(23);
    let mut left = Vec::with_capacity(len);
    let mut right = Vec::with_capacity(len);
    for n in 0..len {
        let t = n as f32 / RATE;
        let gate = if (n / 4_800) % 3 == 2 { 0.0 } else { 1.0 };
        let chord = 0.5f32.mul_add(
            (std::f32::consts::TAU * 330.0 * t).sin(),
            (std::f32::consts::TAU * 220.0 * t).sin(),
        );
        left.push(gate * 0.25f32.mul_add(chord, 0.1 * noise.sample()));
        right.push(gate * 0.25f32.mul_add(-chord, 0.1 * noise.sample()));
    }
    [left, right]
}

/// `kind` with `knobs` (by index) set, then prepared, so every knob starts
/// where it was set rather than gliding there.
fn built(kind: &EffectKind, knobs: &[(usize, f32)]) -> Box<dyn Effect> {
    let mut effect = (kind.build)();
    for &(index, value) in knobs {
        effect.set_param(index, value);
    }
    effect.prepare(RATE);
    effect
}

/// The mixed output, through [`Effect::process`].
fn mixed(effect: &mut dyn Effect, input: &[Vec<f32>; 2]) -> [Vec<f32>; 2] {
    let len = input[0].len();
    let mut out = [vec![0.0; len], vec![0.0; len]];
    let mut start = 0;
    while start < len {
        let end = (start + BLOCK).min(len);
        let [left, right] = &mut out;
        effect.process(
            &CONTEXT,
            [&input[0][start..end], &input[1][start..end]],
            [&mut left[start..end], &mut right[start..end]],
        );
        start = end;
    }
    out
}

/// The mixed output and the wet output, through [`Effect::process_wet`].
fn both(effect: &mut dyn Effect, input: &[Vec<f32>; 2]) -> ([Vec<f32>; 2], [Vec<f32>; 2]) {
    let len = input[0].len();
    let mut out = [vec![0.0; len], vec![0.0; len]];
    let mut wet = [vec![0.0; len], vec![0.0; len]];
    let mut start = 0;
    while start < len {
        let end = (start + BLOCK).min(len);
        let [left, right] = &mut out;
        let [wet_left, wet_right] = &mut wet;
        effect.process_wet(
            &CONTEXT,
            [&input[0][start..end], &input[1][start..end]],
            [&mut left[start..end], &mut right[start..end]],
            [&mut wet_left[start..end], &mut wet_right[start..end]],
        );
        start = end;
    }
    (out, wet)
}

/// The mix knob's index, if the effect has one.
fn mix_knob(kind: &EffectKind) -> Option<usize> {
    kind.params.iter().position(|spec| spec.name == "mix")
}

/// The largest difference between two stereo signals.
fn largest_difference(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| a.iter().zip(b))
        .fold(0.0, |most, (x, y)| most.max((x - y).abs()))
}

fn bits(signal: &[Vec<f32>; 2]) -> Vec<u32> {
    signal.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn the_wet_output_is_the_mix_all_the_way_to_wet_at_any_mix() {
    let input = programme(14_400);
    let mut checked = 0;
    for kind in catalogue() {
        let Some(mix) = mix_knob(kind) else {
            continue;
        };
        let spec = kind.params[mix];
        let all_wet = mixed(built(kind, &[(mix, spec.max)]).as_mut(), &input);
        for share in [0.0, 0.5, 1.0] {
            let value = (spec.max - spec.min).mul_add(share, spec.min);
            let (out, wet) = both(built(kind, &[(mix, value)]).as_mut(), &input);
            let most = largest_difference(&wet, &all_wet);
            assert!(most < 1e-5, "{} at mix {value}: {most}", kind.id);
            // Asking for the wet signal changes nothing in the mix.
            let plain = mixed(built(kind, &[(mix, value)]).as_mut(), &input);
            assert_eq!(bits(&out), bits(&plain), "{} at mix {value}", kind.id);
            if share < 0.5 {
                // At dry the mix is the input (or near it) and the wet
                // signal is still all effect: they differ.
                assert!(largest_difference(&wet, &out) > 1e-3, "{}", kind.id);
            }
        }
        checked += 1;
    }
    assert!(checked >= 24, "{checked}");
}

#[test]
fn an_effect_with_no_mix_hands_over_its_output_as_the_wet_signal() {
    let input = programme(9_600);
    let mut checked = 0;
    for kind in catalogue().filter(|kind| mix_knob(kind).is_none()) {
        let (out, wet) = both(built(kind, &[]).as_mut(), &input);
        assert_eq!(bits(&out), bits(&wet), "{}", kind.id);
        let plain = mixed(built(kind, &[]).as_mut(), &input);
        assert_eq!(bits(&out), bits(&plain), "{}", kind.id);
        checked += 1;
    }
    assert!(checked >= 6, "{checked}");
}

#[test]
fn every_wet_output_is_finite_and_bounded_at_every_knob_end() {
    let mut input = programme(4_800);
    for (n, sample) in input[0].iter_mut().enumerate().step_by(97) {
        *sample = match n % 4 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => f32::NEG_INFINITY,
            _ => 1e30,
        };
    }
    for side in &mut input {
        for sample in side.iter_mut().skip(2_400).take(400) {
            if sample.is_finite() {
                *sample *= 40.0;
            }
        }
    }
    for kind in catalogue() {
        let mut settings: Vec<Vec<(usize, f32)>> = vec![Vec::new()];
        for (index, spec) in kind.params.iter().enumerate() {
            settings.push(vec![(index, spec.min)]);
            settings.push(vec![(index, spec.max)]);
        }
        for knobs in settings {
            let (out, wet) = both(built(kind, &knobs).as_mut(), &input);
            // The wet signal is all effect, under every family's ceiling;
            // the mix may carry the dry signal through untouched (the
            // reverbs do), up to the loudest input any family takes.
            for (signal, most) in [(&wet, CEILING), (&out, DRY_HEADROOM)] {
                let worst = signal.iter().flatten().fold(0.0f32, |most, x| {
                    if x.is_finite() {
                        most.max(x.abs())
                    } else {
                        f32::INFINITY
                    }
                });
                assert!(worst <= most, "{} {knobs:?}: {worst}", kind.id);
            }
        }
    }
}

#[test]
fn slices_of_different_lengths_process_only_the_shortest() {
    let input = programme(200);
    for kind in catalogue() {
        for (outputs, wets) in [(200, 120), (120, 200), (0, 200), (200, 0)] {
            let mut effect = built(kind, &[]);
            let mut out = [vec![9.0f32; outputs], vec![9.0f32; outputs]];
            let mut wet = [vec![9.0f32; wets], vec![9.0f32; wets]];
            let [left, right] = &mut out;
            let [wet_left, wet_right] = &mut wet;
            effect.process_wet(
                &CONTEXT,
                [&input[0], &input[1]],
                [left, right],
                [wet_left, wet_right],
            );
            let shortest = outputs.min(wets);
            for side in out.iter().chain(&wet) {
                assert!(side.iter().all(|x| x.is_finite()), "{}", kind.id);
                assert!(
                    side.iter().skip(shortest).all(|x| *x == 0.0),
                    "{} {outputs} {wets}",
                    kind.id
                );
            }
        }
    }
}

#[test]
fn an_unprepared_effect_stays_finite_and_bounded_on_both_sides() {
    let input = programme(256);
    for kind in catalogue() {
        let mut effect = (kind.build)();
        let mut out = [vec![9.0f32; 256], vec![9.0f32; 256]];
        let mut wet = [vec![9.0f32; 256], vec![9.0f32; 256]];
        let [left, right] = &mut out;
        let [wet_left, wet_right] = &mut wet;
        effect.process_wet(
            &CONTEXT,
            [&input[0], &input[1]],
            [left, right],
            [wet_left, wet_right],
        );
        for side in out.iter().chain(&wet) {
            assert!(
                side.iter().all(|x| x.is_finite() && x.abs() <= CEILING),
                "{}",
                kind.id
            );
        }
    }
}
