use super::{BANDS, Bank, BankSample, Octaves, PARAMS, RATIOS, centre};
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

fn pure_chord(frequencies: &[f32]) -> Vec<f32> {
    (0..96_000)
        .map(|frame| {
            frequencies
                .iter()
                .map(|frequency| {
                    (std::f64::consts::TAU * f64::from(*frequency) * f64::from(frame)
                        / f64::from(RATE))
                    .sin() as f32
                })
                .sum::<f32>()
                * 0.25
                / frequencies.len() as f32
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

fn no_other_component(signal: &[f32], frequency: f64, margin_db: f64, upper: f64) {
    let segment = &signal[24_000..24_000 + 65_536];
    let power = spectrum(segment);
    let resolution = f64::from(RATE) / segment.len() as f64;
    let target = (frequency / resolution).round() as usize;
    let peak = power[target.saturating_sub(1)..=target + 1]
        .iter()
        .copied()
        .fold(0.0, f64::max);
    assert!(peak > 1e-9, "missing {frequency} Hz component");
    for (bin, value) in power.iter().enumerate().take((upper / resolution) as usize) {
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
        no_other_component(&output, frequency, 30.0, 1000.0);
    }
}

#[test]
fn up2_is_four_times() {
    let input = sine(440.0, 96_000);
    let output = render(isolated("up2").as_mut(), &input);
    pitch_is(&output, 1760.0, 10.0);
    let pitches = window_pitch(&output, RATE, 10.0);
    let steady: Vec<_> = pitches.iter().filter(|(time, _)| *time >= 0.25).collect();
    let mean = steady.iter().map(|sample| sample.1).sum::<f64>() / steady.len() as f64;
    assert!(cents(mean, 1760.0).abs() <= 1.0, "mean pitch {mean} Hz");
    no_other_component(&output, 1760.0, 30.0, 4000.0);
}

#[test]
fn every_octave_path_is_at_unity_across_the_bank() {
    let mut levels = Vec::new();
    for (path, ratio) in PATHS.into_iter().zip(RATIOS) {
        for position in [20.0, 40.0, 60.0] {
            let frequency = centre(60.0, 2000.0, position);
            let input = sine(frequency, 24_000);
            let output = render(isolated(path).as_mut(), &input);
            let level = 20.0
                * (magnitude(&output[12_000..], RATE, frequency * ratio)
                    / magnitude(&input[12_000..], RATE, frequency))
                .log10();
            levels.push((path, frequency, level));
        }
    }
    assert!(
        levels.iter().all(|(_, _, level)| level.abs() <= 1.0),
        "unity levels: {levels:?}"
    );
}

#[test]
fn the_down_paths_hold_their_level_whatever_came_before() {
    let frequency = centre(60.0, 2000.0, 40.0);
    let mut levels = Vec::new();
    for (path, ratio) in [("down1", 0.5), ("down2", 0.25)] {
        for history in ["hard start", "fade", "300 Hz", "semitone"] {
            let mut effect = isolated(path);
            let mut input = sine(frequency, 24_000);
            match history {
                "fade" => {
                    for (frame, sample) in input.iter_mut().take(960).enumerate() {
                        *sample *= frame as f32 / 960.0;
                    }
                }
                "300 Hz" | "semitone" => {
                    let before = if history == "300 Hz" {
                        300.0
                    } else {
                        frequency * 1.06
                    };
                    let preceding = render(effect.as_mut(), &sine(before, 12_000));
                    assert!(preceding.iter().all(|sample| sample.is_finite()));
                }
                "hard start" => {}
                _ => panic!("unknown history"),
            }
            let output = render(effect.as_mut(), &input);
            let level = 20.0
                * (magnitude(&output[12_000..], RATE, frequency * ratio)
                    / magnitude(&input[12_000..], RATE, frequency))
                .log10();
            eprintln!("history {path} {history}: {level:.6} dB");
            levels.push((path, history, level));
        }
    }
    assert!(
        levels
            .iter()
            .all(|(_, _, level)| level.is_finite() && level.abs() <= 1.0),
        "history levels: {levels:?}"
    );
}

#[test]
fn the_width_range_keeps_every_path_usable() {
    assert!((PARAMS[2].min - 0.9).abs() <= f32::EPSILON);
    assert!((PARAMS[2].max - 1.2).abs() <= f32::EPSILON);
    for step in 90..=120 {
        let width = step as f32 / 100.0;
        let mut bank = Bank::default();
        bank.prepare([60.0, 2000.0, width], RATE);
        let mut minimum = [f64::INFINITY; 4];
        let mut maximum = [0.0_f64; 4];
        for position in 0..=316 {
            let frequency = centre(60.0, 2000.0, f64::from(position) / 4.0);
            let mut sums = [(0.0_f64, 0.0_f64); 4];
            for band in &bank.bands {
                let (real, imag) = band.response(frequency, f64::from(RATE));
                let amplitude = 0.5 * real.hypot(imag);
                let phase = imag.atan2(real);
                for (sum, ratio) in sums.iter_mut().zip(RATIOS) {
                    let (sin, cos) = (ratio * phase).sin_cos();
                    sum.0 += amplitude * cos;
                    sum.1 += amplitude * sin;
                }
            }
            for (path, (real, imag)) in sums.into_iter().enumerate() {
                let amplitude = real.hypot(imag);
                minimum[path] = minimum[path].min(amplitude);
                maximum[path] = maximum[path].max(amplitude);
            }
        }
        for path in 0..4 {
            let spread = 20.0 * (maximum[path] / minimum[path]).log10();
            let limit = [4.0, 9.5, 3.0, 3.5][path];
            assert!(
                spread <= limit,
                "width {width}, {}: {spread} dB",
                PATHS[path]
            );
        }
    }
}

#[test]
fn a_chord_gets_its_octaves() {
    let frequencies = [110.0, 138.6, 164.8];
    let input = pure_chord(&frequencies);
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

fn delayed_pitch(source: &[(f64, f64)], time: f64) -> Option<f64> {
    let after = source.partition_point(|sample| sample.0 < time);
    if after == 0 || after >= source.len() {
        return None;
    }
    let (before, next) = (source[after - 1], source[after]);
    let share = (time - before.0) / (next.0 - before.0);
    Some(before.1 * ((next.1 / before.1).log2() * share).exp2())
}

fn follows(signal: &[f32], path: &str, ratio: f64, lowest: f64, maximum_error: f64) {
    let output = render(isolated(path).as_mut(), signal);
    let source = window_pitch(signal, RATE, 10.0);
    let heard = window_pitch(&output, RATE, 10.0);
    let spacing =
        10.0_f64.powf((super::erb(2000.0) - super::erb(60.0)) / (21.4 * (BANDS - 1) as f64));
    let below = (lowest + 1.0 / 0.00437) * (1.0 - 1.0 / spacing);
    let above = (lowest + 1.0 / 0.00437) * (spacing - 1.0);
    let bandwidth = 2.0 * below * above / (below + above);
    let maximum_lag = 1.2 * 2.4 / (std::f64::consts::PI * bandwidth);
    let steps = (maximum_lag / 0.0001).ceil() as usize;
    let mut best = (0, f64::INFINITY);
    for step in 0..=steps {
        let lag = maximum_lag * step as f64 / steps as f64;
        let errors: Vec<_> = heard
            .iter()
            .filter(|(time, _)| *time >= 0.2)
            .filter_map(|(time, pitch)| {
                delayed_pitch(&source, time - lag).map(|expected| cents(*pitch, expected * ratio))
            })
            .collect();
        assert!(!errors.is_empty(), "no matched windows at lag {lag}");
        let squared = errors.iter().map(|error| error * error).sum::<f64>() / errors.len() as f64;
        if squared < best.1 {
            best = (step, squared);
        }
    }
    let lag = maximum_lag * best.0 as f64 / steps as f64;
    let mut previous = None;
    let mut errors = Vec::new();
    let mut missing = 0;
    let mut largest = 0.0_f64;
    let mut worst_excess = 0.0_f64;
    for &(time, pitch) in &heard {
        if time < 0.2 {
            continue;
        }
        let Some(expected) = delayed_pitch(&source, time - lag).map(|pitch| pitch * ratio) else {
            missing += 1;
            continue;
        };
        errors.push(cents(pitch, expected));
        if let Some((last_pitch, last_expected)) = previous {
            let step = cents(pitch, last_pitch).abs();
            let input_step = cents(expected, last_expected).abs();
            largest = largest.max(step);
            worst_excess = worst_excess.max(step - input_step);
        }
        previous = Some((pitch, expected));
    }
    let count = errors.len();
    assert!(count >= 170, "only {count} windows");
    let mean = errors.iter().sum::<f64>() / count as f64;
    let rms_error = (errors.iter().map(|error| error * error).sum::<f64>() / count as f64).sqrt();
    let maximum = errors.iter().map(|error| error.abs()).fold(0.0, f64::max);
    eprintln!(
        "{path}: lag {:.6} ms (bound {:.6}), RMS {rms_error:.6}, mean {mean:.6}, max {maximum:.6} cents; largest pitch step {largest:.6}, step excess {worst_excess:.6}; {missing} unmatched of {}",
        lag * 1000.0,
        maximum_lag * 1000.0,
        count + missing
    );
    assert!(best.0 > 0 && best.0 < steps, "lag fit reached bound: {lag}");
    assert!(rms_error <= 8.0, "RMS {rms_error} cents");
    assert!(mean.abs() <= 2.0, "mean {mean} cents");
    assert!(maximum <= maximum_error, "max {maximum} cents");
    assert!(worst_excess <= 5.0, "step excess {worst_excess} cents");
    if path == "down1" {
        assert!(
            largest <= 6.109_905,
            "glide step grew by more than one cent: {largest}"
        );
    }
    assert!(
        missing * 20 < count + missing,
        "{missing} unmatched windows of {}",
        count + missing
    );
    let levels: Vec<_> = output[9600..].chunks_exact(480).map(rms).collect();
    let peak = levels.iter().copied().fold(0.0, f64::max);
    assert!(peak > 1e-5, "missing octave");
    for (window, level) in levels.into_iter().enumerate() {
        let dip_db = 20.0 * (level / peak).log10();
        assert!(dip_db >= -6.0, "window {window}: dip {dip_db} dB");
    }
}

#[test]
fn the_octave_follows_a_glide_without_steps() {
    follows(&glide_fourth(RATE), "down1", 0.5, 220.0, 20.0);
}

#[test]
fn the_octave_follows_a_vibrato_without_steps() {
    follows(
        &vibrato_30c_6hz(RATE),
        "up1",
        2.0,
        220.0 * (-30.0_f64 / 1200.0).exp2(),
        10.0,
    );
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
    assert!(milliseconds <= 30.0, "response: {milliseconds} ms");
}

fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_redesign_never_bangs_or_dips() {
    let kind = kind();
    let input = sine(1000.0, 48_000);
    for (name, value) in [("low", 150.0), ("high", 5000.0), ("width", 1.2)] {
        let mut effect = isolated("down1");
        let before = render(effect.as_mut(), &input);
        let reference = rms(&before[24_000..]);
        let index = kind
            .params
            .iter()
            .position(|param| param.name == name)
            .expect("knob exists");
        effect.set_param(index, value);
        let after = render(effect.as_mut(), &input[..9600]);
        for (window, samples) in after.chunks_exact(96).enumerate() {
            let change = 20.0 * (rms(samples) / reference).log10();
            assert!(change.abs() <= 3.0, "{name}, window {window}: {change} dB");
        }
    }
}

#[test]
fn the_wet_contains_only_octaves() {
    let mut effect = effect(&[("down1", 0.0)]);
    let input = sine(440.0, 128);
    let mut left = [0.0; 128];
    let mut right = [0.0; 128];
    let mut wet_left = [1.0; 129];
    let mut wet_right = [1.0; 130];
    effect.process_wet(
        &CONTEXT,
        [&input, &input],
        [&mut left, &mut right],
        [&mut wet_left, &mut wet_right],
    );
    assert_eq!(left.as_slice(), input);
    assert!(
        wet_left
            .iter()
            .chain(&wet_right)
            .all(|sample| sample.abs() <= f32::EPSILON)
    );
}

#[test]
fn turning_low_high_or_width_never_allocates_or_clicks() {
    let input = chord(RATE, &[110.0, 138.6, 164.8]);
    let kind = kind();
    for (name, low, high) in [
        ("low", 30.0, 150.0),
        ("high", 500.0, 5000.0),
        ("width", 0.9, 1.2),
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
                effect.set_param(2, 1.2);
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
            effect.set_param(2, 0.9 + 0.3 * frame as f32 / 4800.0);
        }
        assert!(effect.sample(0.1, 0.1).is_finite());
        assert_eq!(effect.designed.map(f32::to_bits), before.map(f32::to_bits));
    }
    effect.set_param(2, 1.2);
    for _ in 0..effect.hold - 1 {
        assert!(effect.sample(0.1, 0.1).is_finite());
        assert_eq!(effect.designed.map(f32::to_bits), before.map(f32::to_bits));
    }
    assert!(effect.sample(0.1, 0.1).is_finite());
    assert!((effect.designed[2] - 1.2).abs() <= f32::EPSILON);
    assert_eq!(effect.fade, 0);
    assert_eq!(effect.warming, effect.banks[1 - effect.active].settling - 1);
    let previous_bank = effect.active;
    for _ in 0..effect.warming - 1 {
        assert!(effect.sample(0.1, 0.1).is_finite());
        assert_eq!(effect.active, previous_bank);
        assert_eq!(effect.fade, 0);
    }
    assert!(effect.sample(0.1, 0.1).is_finite());
    assert_eq!(effect.fade, effect.fade_length);
    for _ in 0..effect.fade_length {
        assert!(effect.sample(0.1, 0.1).is_finite());
    }
    assert_eq!(effect.active, 1 - previous_bank);
    assert_eq!(effect.fade, 0);
    assert_eq!(effect.banks[0].bands.len(), BANDS);
    assert_eq!(effect.banks[1].bands.len(), BANDS);
}

#[test]
fn the_spare_bank_stays_idle_until_a_redesign() {
    let mut effect = Octaves::new();
    effect.prepare(RATE);
    for sample in sine(440.0, 4800) {
        assert!(effect.sample(sample, sample).is_finite());
    }
    let spare = effect.banks[1 - effect.active].sample(0.0, [true; 4], true);
    assert!(
        spare
            .paths
            .iter()
            .all(|sample| sample.abs() <= f32::EPSILON)
    );
}

#[test]
fn divided_phases_line_up_and_keep_their_orientation() {
    let mut bank = Bank {
        gains: [1.0; 4],
        down1_sign: 1.0,
        ..Bank::default()
    };
    let active = BankSample {
        down: [(1.0, 0.0); 2],
        ..BankSample::default()
    };
    let mut spare = BankSample {
        paths: [0.0, 0.0, -1.0, 0.0],
        down: [(-1.0, 0.0), (0.0, -1.0)],
    };
    bank.align(&active, &mut spare);
    assert!((bank.down1_sign + 1.0).abs() <= f32::EPSILON);
    assert_eq!(bank.down2_turn, 1);
    for (spare, active) in spare.down.into_iter().zip(active.down) {
        assert!((spare.0 - active.0).abs() <= f32::EPSILON);
        assert!((spare.1 - active.1).abs() <= f32::EPSILON);
    }
    assert!((spare.paths[2] - 1.0).abs() <= f32::EPSILON);
    assert!((spare.paths[3] - 1.0).abs() <= f32::EPSILON);
}
