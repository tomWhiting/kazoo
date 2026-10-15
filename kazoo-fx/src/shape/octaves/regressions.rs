use super::{Octaves, centre};
use crate::Effect;

const RATE: f32 = 48_000.0;

fn down2(low: f32) -> Octaves {
    let mut effect = Octaves::new();
    effect.set_param(0, low);
    effect.set_param(3, 0.0);
    effect.set_param(6, 0.0);
    effect.set_param(7, 100.0);
    effect.set_param(8, 16_000.0);
    effect.prepare(RATE);
    effect
}

fn play(effect: &mut Octaves, frequency: f64, frames: usize) {
    for frame in 0..frames {
        let input = (std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE)).sin()
            as f32
            * 0.25;
        assert!(effect.sample(input, input).is_finite());
    }
}

fn semitone_history(effect: &mut Octaves) {
    let frequency = centre(60.0, 2000.0, 40.0);
    play(effect, frequency * 1.06, 12_000);
    play(effect, frequency, 48_000);
    let gain = effect.banks[effect.active].levels[1].gain_db();
    assert!(gain > 6.0, "history gain: {gain:.6} dB");
}

#[test]
fn a_low_down2_note_has_no_level_gain_ripple() {
    let mut effect = down2(30.0);
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    for frame in 0..48_000 {
        let input =
            (std::f64::consts::TAU * 41.2 * f64::from(frame) / f64::from(RATE)).sin() as f32 * 0.25;
        assert!(effect.sample(input, input).is_finite());
        if frame >= 24_000 {
            let gain = effect.banks[effect.active].levels[1].gain_db();
            minimum = minimum.min(gain);
            maximum = maximum.max(gain);
        }
    }
    let ripple = maximum - minimum;
    eprintln!("gain ripple: {ripple:.6} dB ({minimum:.6}..{maximum:.6})");
    assert!(
        ripple <= 0.05,
        "gain ripple: {ripple:.6} dB ({minimum:.6}..{maximum:.6})"
    );
}

#[test]
fn enabling_down2_discards_its_stale_gain() {
    let mut effect = down2(60.0);
    semitone_history(&mut effect);
    effect.set_param(7, 0.0);
    play(&mut effect, 1000.0, 96_000);
    effect.set_param(7, 100.0);
    let mut output = vec![0.0; 12_000];
    let mut maximum = f64::NEG_INFINITY;
    for (frame, sample) in output.iter_mut().enumerate() {
        let input =
            (std::f64::consts::TAU * 1000.0 * frame as f64 / f64::from(RATE)).sin() as f32 * 0.25;
        *sample = effect.sample(input, input);
        if frame == 0 {
            let gain = effect.banks[effect.active].levels[1].gain_db();
            assert!(
                gain <= 0.0025 + 1e-10,
                "re-enabled first-frame gain: {gain:.6} dB"
            );
        }
    }
    for samples in output.chunks_exact(96) {
        let power = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / 96.0;
        let level = 10.0 * (power / (0.25_f64.powi(2) * 0.5)).log10();
        maximum = maximum.max(level);
        assert!(level <= 1.0, "re-enabled output: {level:.6} dB");
    }
    eprintln!("re-enabled maximum output: {maximum:.6} dB");
}

#[test]
fn a_redesign_seeds_both_matchers_from_the_active_bank() {
    let mut effect = down2(60.0);
    effect.set_param(6, 100.0);
    semitone_history(&mut effect);
    let active = effect.active;
    let mut expected = effect.banks[active].levels;
    let before = expected.map(|level| level.gain_db());
    effect.set_param(2, 1.2);
    effect.quiet = effect.hold;
    effect.retune();
    assert!(effect.warming > 0);
    let after = effect.banks[1 - active].levels.map(|level| level.gain_db());
    eprintln!("matcher gains at redesign: active {before:?}, spare {after:?}");
    assert_eq!(
        before.map(f64::to_bits),
        after.map(f64::to_bits),
        "redesign lost matcher gain"
    );
    for frame in 0..480 {
        for (path, matcher) in expected.iter_mut().enumerate() {
            let expected_gain = matcher.sample(0.02, 0.01);
            let actual_gain = effect.banks[1 - active].levels[path].sample(0.02, 0.01);
            assert_eq!(
                expected_gain.to_bits(),
                actual_gain.to_bits(),
                "path {path}, frame {frame}: followers were not seeded"
            );
        }
    }
}

#[test]
fn down2_recovers_from_the_semitone_history_within_fifteen_ms() {
    let frequency = 440.0;
    let mut effect = down2(60.0);
    semitone_history(&mut effect);
    let mut output = vec![0.0; 9600];
    for (frame, sample) in output.iter_mut().enumerate() {
        let input = (std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE)).sin()
            as f32
            * 0.25;
        *sample = effect.sample(input, input);
    }
    let mut maximum = f64::NEG_INFINITY;
    let mut maximum_at = 0;
    for (window, samples) in output.windows(480).step_by(240).enumerate() {
        let power = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / 480.0;
        let level = 10.0 * (power / (0.25_f64.powi(2) * 0.5)).log10();
        if (1..=3).contains(&window) {
            eprintln!(
                "{frequency} Hz: gained output in 10 ms window starting at {} ms: {level:.6} dB",
                window * 5
            );
        }
        if window >= 3 && level > maximum {
            maximum = level;
            maximum_at = window * 5;
        }
    }
    eprintln!(
        "{frequency} Hz: largest gained output after fifteen ms {maximum:.6} dB at {maximum_at} ms"
    );
    assert!(
        maximum < 1.0,
        "in-phase gained output after fifteen ms: {maximum:.6} dB"
    );
}
