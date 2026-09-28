//! The real-time contract, checked on every effect in the family.

use super::KINDS;
use super::testkit::{RATE, context, impulse, noise, peak, prepared, render, rms};
use crate::{Curve, Effect, EffectKind};

/// Every parameter whose name marks it as a feedback amount.
fn feedback_params(kind: &EffectKind) -> impl Iterator<Item = usize> + '_ {
    kind.params
        .iter()
        .enumerate()
        .filter(|(_, spec)| matches!(spec.name, "feedback" | "colour"))
        .map(|(index, _)| index)
}

fn all_finite(signal: &[f32]) -> bool {
    signal.iter().all(|sample| sample.is_finite())
}

#[test]
fn ids_are_unique_and_lower_case() {
    for (n, kind) in KINDS.iter().enumerate() {
        assert!(
            kind.id.chars().all(|c| c.is_ascii_lowercase()),
            "{}",
            kind.id
        );
        assert!(KINDS[n + 1..].iter().all(|other| other.id != kind.id));
        for spec in kind.params {
            assert!(
                spec.min <= spec.default && spec.default <= spec.max,
                "{}",
                spec.name
            );
            assert!(
                !matches!(spec.name, "left" | "right"),
                "{} shadows an input",
                spec.name
            );
            if let Curve::Stepped { labels } = spec.curve {
                assert_eq!(
                    labels.len(),
                    (spec.max - spec.min) as usize + 1,
                    "{}",
                    spec.name
                );
            }
        }
    }
}

fn poison_in_gives_finite_out_and_leaves_no_trace(kind: &EffectKind) {
    let ctx = context(120.0);
    let mut effect = prepared(kind);
    let mut poison = noise(4_800, 0.5, 3);
    poison[10] = f32::NAN;
    poison[200] = f32::INFINITY;
    poison[300] = f32::NEG_INFINITY;
    poison[400] = 1e30;
    let (left, right) = render(effect.as_mut(), ctx, &poison, &poison, 64);
    assert!(all_finite(&left) && all_finite(&right), "{}", kind.id);
    assert!(peak(&left) <= 2.0 && peak(&right) <= 2.0, "{}", kind.id);
    let clean = noise(48_000, 0.3, 5);
    let (left, right) = render(effect.as_mut(), ctx, &clean, &clean, 64);
    assert!(all_finite(&left) && all_finite(&right), "{}", kind.id);
    assert!(rms(&left[24_000..]) > 1e-3, "{} went quiet", kind.id);
}

fn silence_after_sound_decays_to_silence(kind: &EffectKind) {
    let ctx = context(120.0);
    let mut effect = prepared(kind);
    let mut signal = noise(24_000, 0.5, 11);
    signal.resize(RATE as usize * 30, 0.0);
    let (left, right) = render(effect.as_mut(), ctx, &signal, &signal, 512);
    let tail = signal.len() - RATE as usize;
    assert!(
        peak(&left[tail..]) < 1e-4,
        "{} {}",
        kind.id,
        peak(&left[tail..])
    );
    assert!(
        peak(&right[tail..]) < 1e-4,
        "{} {}",
        kind.id,
        peak(&right[tail..])
    );
}

fn silence_in_gives_silence_out(kind: &EffectKind) {
    let ctx = context(120.0);
    let mut effect = prepared(kind);
    let quiet = vec![0.0; 48_000];
    let (left, right) = render(effect.as_mut(), ctx, &quiet, &quiet, 480);
    assert!(peak(&left) < 1e-4 && peak(&right) < 1e-4, "{}", kind.id);
}

fn every_parameter_at_its_ends_stays_finite_and_bounded(kind: &EffectKind) {
    let ctx = context(120.0);
    let signal = noise(12_000, 0.8, 17);
    for (index, spec) in kind.params.iter().enumerate() {
        for value in [spec.min, spec.max] {
            let mut effect = prepared(kind);
            effect.set_param(index, value);
            let (left, right) = render(effect.as_mut(), ctx, &signal, &signal, 256);
            assert!(
                all_finite(&left) && all_finite(&right),
                "{} {}",
                kind.id,
                spec.name
            );
            assert!(
                peak(&left) <= 2.0 && peak(&right) <= 2.0,
                "{} {}",
                kind.id,
                spec.name
            );
        }
    }
    let mut effect = prepared(kind);
    for (index, spec) in kind.params.iter().enumerate() {
        effect.set_param(index, spec.max);
    }
    let (left, _) = render(effect.as_mut(), ctx, &signal, &signal, 256);
    assert!(
        all_finite(&left) && peak(&left) <= 2.0,
        "{} all max",
        kind.id
    );
    for (index, spec) in kind.params.iter().enumerate() {
        effect.set_param(index, spec.min);
    }
    let (left, _) = render(effect.as_mut(), ctx, &signal, &signal, 256);
    assert!(
        all_finite(&left) && peak(&left) <= 2.0,
        "{} all min",
        kind.id
    );
}

fn feedback_at_its_ends_stays_bounded_for_a_minute(kind: &EffectKind) {
    // The output guard alone keeps every peak under 2.0, so being bounded
    // proves little: with freeze off, the loop must also die away.
    for index in feedback_params(kind) {
        let spec = kind.params[index];
        for value in [spec.min, spec.max] {
            if value.abs() < 0.5 {
                continue;
            }
            let (first_quiet, last) = loop_levels(kind, index, value, false, 60);
            assert!(
                last < (first_quiet * 0.05).max(1e-4),
                "{} {} {value}: {first_quiet} then {last}",
                kind.id,
                spec.name
            );
            if kind.params.iter().any(|other| other.name == "freeze") {
                loop_levels(kind, index, value, true, 20);
            }
        }
    }
}

/// Five seconds of loud noise into `kind` with parameter `index` at
/// `value` and everything that makes a loop hotter at its top (freeze on or
/// off as asked), then silence to `seconds` in all. Checks every block is
/// finite and bounded; returns the peak of the first silent second and of
/// the last.
fn loop_levels(
    kind: &EffectKind,
    index: usize,
    value: f32,
    freeze: bool,
    seconds: usize,
) -> (f32, f32) {
    let ctx = context(120.0);
    let block = noise(RATE as usize, 0.9, 23);
    let quiet = vec![0.0; block.len()];
    let mut effect = prepared(kind);
    effect.set_param(index, value);
    for (other, other_spec) in kind.params.iter().enumerate() {
        match other_spec.name {
            "drive" | "level" => effect.set_param(other, other_spec.max),
            "freeze" => effect.set_param(other, if freeze { 1.0 } else { 0.0 }),
            _ => {}
        }
    }
    let mut first_quiet = 0.0f32;
    let mut last = 0.0f32;
    for second in 0..seconds {
        let input = if second < 5 { &block } else { &quiet };
        let (left, right) = render(effect.as_mut(), ctx, input, input, 512);
        assert!(all_finite(&left) && all_finite(&right), "{}", kind.id);
        last = peak(&left).max(peak(&right));
        assert!(last <= 2.0, "{} {last}", kind.id);
        if second == 5 {
            first_quiet = last;
        }
    }
    (first_quiet, last)
}

fn bad_parameters_are_ignored(kind: &EffectKind) {
    let ctx = context(120.0);
    let signal = noise(9_600, 0.5, 29);
    let mut reference = prepared(kind);
    let mut tested = prepared(kind);
    tested.set_param(kind.params.len(), 0.5);
    tested.set_param(usize::MAX, 0.5);
    for index in 0..kind.params.len() {
        tested.set_param(index, f32::NAN);
        tested.set_param(index, f32::INFINITY);
        tested.set_param(index, f32::NEG_INFINITY);
    }
    let (a, _) = render(reference.as_mut(), ctx, &signal, &signal, 128);
    let (b, _) = render(tested.as_mut(), ctx, &signal, &signal, 128);
    assert_eq!(a, b, "{}", kind.id);
}

fn odd_blocks_are_handled(kind: &EffectKind) {
    let ctx = context(120.0);
    let mut effect = prepared(kind);
    let input = [0.25f32; 64];
    let mut left = [1.0f32; 64];
    let mut right = [1.0f32; 40];
    effect.process(
        &ctx,
        [&input[..0], &input[..0]],
        [&mut left[..0], &mut right[..0]],
    );
    effect.process(&ctx, [&input[..50], &input[..64]], [&mut left, &mut right]);
    assert!(left[40..].iter().all(|v| *v == 0.0), "{}", kind.id);
    assert!(all_finite(&left) && all_finite(&right), "{}", kind.id);
    let mut unprepared = (kind.build)();
    let mut out = [1.0f32; 16];
    let mut out_right = [1.0f32; 16];
    unprepared.process(
        &ctx,
        [&input[..16], &input[..16]],
        [&mut out, &mut out_right],
    );
    assert!(
        out.iter().chain(&out_right).all(|v| *v == 0.0),
        "{}",
        kind.id
    );
}

fn reset_forgets_the_tail(kind: &EffectKind) {
    let ctx = context(120.0);
    let mut effect = prepared(kind);
    let signal = noise(24_000, 0.5, 31);
    render(effect.as_mut(), ctx, &signal, &signal, 256);
    effect.reset();
    let quiet = vec![0.0; 4_800];
    let (left, right) = render(effect.as_mut(), ctx, &quiet, &quiet, 256);
    assert!(peak(&left) < 1e-4 && peak(&right) < 1e-4, "{}", kind.id);
}

fn an_impulse_is_heard(kind: &EffectKind) {
    let ctx = context(120.0);
    let mut effect: Box<dyn Effect> = prepared(kind);
    let input = impulse(96_000, 10);
    let (left, _) = render(effect.as_mut(), ctx, &input, &input, 333);
    assert!(peak(&left) > 1e-3, "{}", kind.id);
}

/// Every contract check, as its own test for each effect, so they run in
/// parallel.
macro_rules! contract {
    ($($effect:ident),* $(,)?) => {
        $(
            mod $effect {
                use crate::time::$effect::KIND;

                #[test]
                fn poison_in_gives_finite_out_and_leaves_no_trace() {
                    super::poison_in_gives_finite_out_and_leaves_no_trace(&KIND);
                }

                #[test]
                fn silence_after_sound_decays_to_silence() {
                    super::silence_after_sound_decays_to_silence(&KIND);
                }

                #[test]
                fn silence_in_gives_silence_out() {
                    super::silence_in_gives_silence_out(&KIND);
                }

                #[test]
                fn every_parameter_at_its_ends_stays_finite_and_bounded() {
                    super::every_parameter_at_its_ends_stays_finite_and_bounded(&KIND);
                }

                #[test]
                fn feedback_at_its_ends_stays_bounded_for_a_minute() {
                    super::feedback_at_its_ends_stays_bounded_for_a_minute(&KIND);
                }

                #[test]
                fn bad_parameters_are_ignored() {
                    super::bad_parameters_are_ignored(&KIND);
                }

                #[test]
                fn odd_blocks_are_handled() {
                    super::odd_blocks_are_handled(&KIND);
                }

                #[test]
                fn reset_forgets_the_tail() {
                    super::reset_forgets_the_tail(&KIND);
                }

                #[test]
                fn an_impulse_is_heard() {
                    super::an_impulse_is_heard(&KIND);
                }
            }
        )*

        #[test]
        fn every_effect_is_checked() {
            let checked = [$(stringify!($effect)),*];
            let listed: Vec<&str> = KINDS.iter().map(|kind| kind.id).collect();
            assert_eq!(listed, checked);
        }
    };
}

contract!(
    tape, bbd, digital, chorus, flanger, phaser, trem, shift, granular
);
