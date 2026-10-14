use super::{BANDS, Octaves, PARAMS, centre};
use crate::shape::testkit::{
    chord, glide_fourth, magnitude, spectrum, vibrato_30c_6hz, window_pitch,
};
use crate::{Context, Effect};

const RATE: f32 = 48_000.0;
const CONTEXT: Context = Context { bpm: 120.0 };
const PATHS: [&str; 4] = ["up1", "up2", "down1", "down2"];

fn kind() -> &'static crate::EffectKind {
    let found = crate::find("octaves");
    assert!(found.is_some(), "octaves must exist in the catalogue");
    found.expect("catalogue assertion above")
}

fn effect(settings: &[(&str, f32)]) -> Box<dyn Effect> {
    let kind = kind();
    let mut effect = (kind.build)();
    for &(name, value) in settings {
        let index = kind.params.iter().position(|param| param.name == name);
        effect.set_param(index.expect("parameter exists"), value);
    }
    effect.prepare(RATE);
    effect
}

fn isolated(path: &str) -> Box<dyn Effect> {
    let mut settings = vec![("dry", 0.0), ("tone", 16_000.0)];
    settings.extend(PATHS.map(|name| (name, if name == path { 100.0 } else { 0.0 })));
    effect(&settings)
}

fn sine(frequency: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            (std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE)).sin() as f32 * 0.25
        })
        .collect()
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

fn cents(actual: f64, expected: f64) -> f64 {
    1200.0 * (actual / expected).log2()
}

fn pitch_is(signal: &[f32], expected: f64, tolerance: f64) {
    let pitches = window_pitch(signal, RATE, 10.0);
    let steady: Vec<_> = pitches.iter().filter(|(time, _)| *time >= 0.25).collect();
    assert!(steady.len() > 150, "{} steady windows", steady.len());
    for &&(time, pitch) in &steady {
        let error = cents(pitch, expected);
        assert!(error.abs() <= tolerance, "{time}: {pitch}, {error} cents");
    }
}

fn no_other_component(signal: &[f32], frequency: f64, margin_db: f64) {
    let segment = &signal[24_000..24_000 + 65_536];
    let power = spectrum(segment);
    let resolution = f64::from(RATE) / segment.len() as f64;
    let target = (frequency / resolution).round() as usize;
    let peak = power[target.saturating_sub(1)..=target + 1]
        .iter()
        .copied()
        .fold(0.0, f64::max);
    assert!(peak > 1e-9, "missing {frequency} Hz component");
    for (bin, value) in power
        .iter()
        .enumerate()
        .take((1000.0 / resolution) as usize)
    {
        if bin.abs_diff(target) > 3 {
            let db = 10.0 * (value / peak).log10();
            assert!(db < -margin_db, "{} Hz: {db} dB", bin as f64 * resolution);
        }
    }
}

#[test]
fn one_sine_in_one_band_comes_out_at_unity() {
    let frequency = centre(60.0, 2000.0, 40.0);
    let input = sine(frequency, 96_000);
    let output = render(isolated("up1").as_mut(), &input);
    pitch_is(&output, 2.0 * frequency, 1.0);
    let source = magnitude(&input[24_000..], RATE, frequency);
    let heard = magnitude(&output[24_000..], RATE, 2.0 * frequency);
    let level_db = 20.0 * (heard / source).log10();
    assert!(level_db.abs() <= 1.0, "unity error: {level_db} dB");
}

#[test]
fn down1_and_down2_are_half_and_quarter() {
    let input = sine(440.0, 96_000);
    for (path, frequency) in [("down1", 220.0), ("down2", 110.0)] {
        let output = render(isolated(path).as_mut(), &input);
        pitch_is(&output, frequency, 2.0);
        no_other_component(&output, frequency, 30.0);
    }
}

#[test]
fn up2_is_four_times() {
    let input = sine(440.0, 96_000);
    let output = render(isolated("up2").as_mut(), &input);
    pitch_is(&output, 1760.0, 1.0);
}

#[test]
fn a_chord_gets_its_octaves() {
    let frequencies = [110.0, 138.6, 164.8];
    let input = chord(RATE, &frequencies);
    let output = render(isolated("down1").as_mut(), &input);
    let steady = &output[24_000..];
    let octaves = frequencies.map(|frequency| magnitude(steady, RATE, f64::from(frequency) * 0.5));
    let weakest = octaves.into_iter().fold(f64::INFINITY, f64::min);
    let strongest = octaves.into_iter().fold(0.0, f64::max);
    assert!(weakest > 1e-5, "missing chord octave: {octaves:?}");
    let spread_db = 20.0 * (strongest / weakest).log10();
    assert!(spread_db <= 6.0, "chord octave spread: {spread_db} dB");
    for frequency in frequencies {
        let original = magnitude(steady, RATE, f64::from(frequency));
        let leakage_db = 20.0 * (original / weakest).log10();
        assert!(
            leakage_db < -20.0,
            "{frequency} Hz leakage: {leakage_db} dB"
        );
    }
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

fn follows(signal: &[f32], path: &str, ratio: f64) {
    let output = render(isolated(path).as_mut(), signal);
    let source = window_pitch(signal, RATE, 10.0);
    let heard = window_pitch(&output, RATE, 10.0);
    let mut previous = None;
    let mut count = 0;
    for &(time, pitch) in &heard {
        if time < 0.1 {
            continue;
        }
        let expected = source
            .iter()
            .find(|sample| (sample.0 - time).abs() < 1e-6)
            .expect("matching input window")
            .1
            * ratio;
        let error = cents(pitch, expected);
        assert!(error.abs() <= 10.0, "{time}: {error} cents");
        if let Some((last_pitch, last_expected)) = previous {
            let step = cents(pitch, last_pitch).abs();
            let input_step = cents(expected, last_expected).abs();
            assert!(
                step <= input_step + 5.0,
                "{time}: step {step}, input {input_step}"
            );
        }
        previous = Some((pitch, expected));
        count += 1;
    }
    assert!(count > 180, "only {count} windows");
    let levels: Vec<_> = output[4800..].chunks_exact(480).map(rms).collect();
    let peak = levels.iter().copied().fold(0.0, f64::max);
    assert!(peak > 1e-5, "missing octave");
    for (window, level) in levels.into_iter().enumerate() {
        let dip_db = 20.0 * (level / peak).log10();
        assert!(dip_db >= -6.0, "window {window}: dip {dip_db} dB");
    }
}

#[test]
fn the_octave_follows_a_glide_without_steps() {
    follows(&glide_fourth(RATE), "down1", 0.5);
}

#[test]
fn the_octave_follows_a_vibrato_without_steps() {
    follows(&vibrato_30c_6hz(RATE), "up1", 2.0);
}

#[test]
fn the_dry_is_not_delayed() {
    let mut effect = effect(&[
        ("dry", 100.0),
        ("up1", 0.0),
        ("up2", 0.0),
        ("down1", 0.0),
        ("down2", 0.0),
    ]);
    let input = chord(RATE, &[110.0, 138.6, 164.8]);
    let output = render(effect.as_mut(), &input);
    assert_eq!(effect.latency(), 0);
    assert!(input.iter().zip(output).all(|(x, y)| (x - y).abs() <= 1e-6));
}

#[test]
fn the_response_time_is_as_stated() {
    let input = sine(1000.0, 24_000);
    let output = render(isolated("up1").as_mut(), &input);
    let steady = magnitude(&output[12_000..], RATE, 2000.0);
    assert!(steady > 1e-5, "missing steady octave");
    // A two-cycle window resolves the 2 kHz output without following its phase.
    let threshold = rms(&output[12_000..]) * 10.0_f64.powf(-3.0 / 20.0);
    let reached = output
        .windows(48)
        .position(|window| rms(window) >= threshold)
        .expect("burst reaches steady level");
    let milliseconds = (reached + 48) as f64 / f64::from(RATE) * 1000.0;
    assert!(milliseconds <= 12.0, "response: {milliseconds} ms");
}

fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn turning_low_high_or_width_never_allocates_or_clicks() {
    let input = chord(RATE, &[110.0, 138.6, 164.8]);
    let kind = kind();
    for (name, low, high) in [
        ("low", 30.0, 150.0),
        ("high", 500.0, 5000.0),
        ("width", 0.5, 2.0),
    ] {
        let mut effect = effect(&[
            ("dry", 0.0),
            ("up1", 100.0),
            ("up2", 100.0),
            ("down1", 100.0),
            ("down2", 100.0),
            (name, low),
        ]);
        let index = kind
            .params
            .iter()
            .position(|param| param.name == name)
            .expect("knob exists");
        let mut output = vec![0.0; input.len()];
        let mut right = [0.0; 48];
        assert_no_alloc::assert_no_alloc(|| {
            for (block, (input, left)) in input.chunks(48).zip(output.chunks_mut(48)).enumerate() {
                // Settle for 500 ms, sweep for one second, then exercise the redesign.
                if (500..1500).contains(&block) {
                    let travel = (block - 500) as f32 / 999.0;
                    effect.set_param(index, (high - low).mul_add(travel, low));
                }
                effect.process(&CONTEXT, [input, input], [left, &mut right]);
            }
        });
        assert!(
            output.iter().all(|sample| sample.is_finite()),
            "{name}: non-finite sample"
        );
        let before = largest_step(&output[19_200..24_000]);
        let during = largest_step(&output[24_000..]);
        assert!(during <= 3.0 * before, "{name}: {during} vs {before}");
    }
}

#[test]
fn survives_nonsense() {
    let mut effect = effect(&[]);
    let signal = sine(440.0, 4096);
    for rate in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -1.0,
        0.0,
        8000.0,
        192_000.0,
        768_000.0,
    ] {
        effect.prepare(rate);
        for index in 0..NONSENSE_INDICES {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                effect.set_param(index, value);
            }
        }
        let output = render(
            effect.as_mut(),
            &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
        );
        assert!(output.iter().all(|sample| sample.abs() <= f32::EPSILON));
        let recovered = render(effect.as_mut(), &signal);
        assert!(recovered.iter().all(|sample| sample.is_finite()));
        assert!(
            rms(&recovered) > 1e-3,
            "finite input did not recover at {rate}"
        );
        let input = [0.25; 17];
        let mut left = [1.0; 23];
        let mut right = [1.0; 31];
        effect.process(&CONTEXT, [&input, &input[..11]], [&mut left, &mut right]);
        assert!(
            left[11..]
                .iter()
                .chain(&right[11..])
                .all(|sample| sample.abs() <= f32::EPSILON)
        );
    }
}

const NONSENSE_INDICES: usize = 16;

#[test]
fn stays_bounded() {
    let input: Vec<_> = (0..48_000)
        .map(|frame| if frame % 2 == 0 { 1e30 } else { -1e30 })
        .collect();
    for maximum in [false, true] {
        let settings: Vec<_> = PARAMS
            .iter()
            .map(|param| (param.name, if maximum { param.max } else { param.min }))
            .collect();
        let output = render(effect(&settings).as_mut(), &input);
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 4.0)
        );
    }
}

#[test]
fn processing_never_allocates() {
    let mut effect = effect(&[
        ("up1", 100.0),
        ("up2", 100.0),
        ("down1", 100.0),
        ("down2", 100.0),
    ]);
    let input = [0.1; 512];
    let mut left = [0.0; 512];
    let mut right = [0.0; 512];
    assert_no_alloc::assert_no_alloc(|| {
        for block in 0..64 {
            if block == 4 {
                effect.set_param(2, 2.0);
            }
            effect.process(&CONTEXT, [&input, &input], [&mut left, &mut right]);
        }
        for index in 0..16 {
            effect.set_param(index, 50.0);
        }
        effect.reset();
        effect.process_wet(
            &CONTEXT,
            [&input, &input],
            [&mut left, &mut right],
            [&mut [0.0; 512], &mut [0.0; 512]],
        );
    });
}

#[test]
fn bank_design_waits_for_rest_and_crossfades_once() {
    assert_eq!(kind().id, "octaves");
    let mut effect = Octaves::new();
    effect.prepare(RATE);
    let before = effect.designed;
    for frame in 0..4800 {
        if frame % 48 == 0 {
            effect.set_param(2, 0.5 + frame as f32 / 4800.0);
        }
        assert!(effect.sample(0.1, 0.1).is_finite());
        assert_eq!(effect.designed, before);
    }
    effect.set_param(2, 2.0);
    for _ in 0..effect.hold - 1 {
        assert!(effect.sample(0.1, 0.1).is_finite());
        assert_eq!(effect.designed, before);
    }
    assert!(effect.sample(0.1, 0.1).is_finite());
    assert!((effect.designed[2] - 2.0).abs() <= f32::EPSILON);
    assert_eq!(effect.fade, effect.fade_length - 1);
    let previous_bank = effect.active;
    for _ in 0..effect.fade_length - 1 {
        assert!(effect.sample(0.1, 0.1).is_finite());
    }
    assert_eq!(effect.active, 1 - previous_bank);
    assert_eq!(effect.fade, 0);
    assert_eq!(effect.banks[0].bands.len(), BANDS);
    assert_eq!(effect.banks[1].bands.len(), BANDS);
}
