use super::*;
use std::f32::consts::PI;

fn generate_sine(frequency: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
    (0..num_samples)
        .map(|i| {
            let t = i as f32 / sample_rate;
            (2.0 * PI * frequency * t).sin()
        })
        .collect()
}

#[test]
fn detects_440hz_sine() {
    let config = PitchDetectorConfig {
        min_frequency: 60.0,
        max_frequency: 1000.0,
        sample_rate: 44_100,
        frame_length: 2048,
        hop_length: 512,
        voiced_threshold: 0.15,
    };
    let mut detector = PitchDetector::new(config).expect("valid config");

    // Generate several frames worth of 440 Hz sine.
    let samples = generate_sine(440.0, 44100.0, 2048 * 4);

    let mut estimates = Vec::new();
    for chunk in samples.chunks(512) {
        if let Some(est) = detector.push_samples(chunk) {
            estimates.push(est);
        }
    }

    // We should have gotten at least one estimate.
    assert!(
        !estimates.is_empty(),
        "should have produced at least one pitch estimate"
    );

    // Find any voiced estimate.
    let voiced: Vec<_> = estimates.iter().filter(|e| e.frequency.is_some()).collect();
    assert!(
        !voiced.is_empty(),
        "at least one estimate should be voiced for a clean sine"
    );

    for est in &voiced {
        let freq = est.frequency.unwrap();
        // pYIN should be within 5% of the true frequency.
        let error_pct = ((freq - 440.0) / 440.0).abs() * 100.0;
        assert!(
            error_pct < 5.0,
            "detected {freq} Hz, expected ~440 Hz (error {error_pct:.1}%)"
        );
    }

    // Check MIDI note for the voiced estimate closest to 440.
    let best = voiced
        .iter()
        .min_by(|a, b| {
            let da = (a.frequency.unwrap() - 440.0).abs();
            let db = (b.frequency.unwrap() - 440.0).abs();
            da.partial_cmp(&db).unwrap()
        })
        .unwrap();
    if let Some(midi) = best.midi_note {
        // A4 = MIDI 69
        assert!(
            (67..=71).contains(&midi),
            "MIDI note should be near 69, got {midi}"
        );
    }
}

#[test]
fn silence_is_unvoiced() {
    let config = PitchDetectorConfig::default();
    let mut detector = PitchDetector::new(config).expect("valid config");

    let silence = vec![0.0_f32; 2048 * 4];
    let mut estimates = Vec::new();
    for chunk in silence.chunks(512) {
        if let Some(est) = detector.push_samples(chunk) {
            estimates.push(est);
        }
    }

    for est in &estimates {
        assert!(
            est.frequency.is_none(),
            "silence should be unvoiced, got freq {:?}",
            est.frequency
        );
    }
}

#[test]
fn reset_clears_buffer() {
    let config = PitchDetectorConfig::default();
    let mut detector = PitchDetector::new(config).expect("valid config");

    // Push partial frame.
    detector.push_samples(&[0.5; 100]);
    assert!(detector.buffer_pos > 0);

    detector.reset();
    assert_eq!(detector.buffer_pos, 0);
}

#[test]
fn invalid_config_returns_error() {
    // min >= max
    let config = PitchDetectorConfig {
        min_frequency: 1000.0,
        max_frequency: 100.0,
        ..PitchDetectorConfig::default()
    };
    assert!(PitchDetector::new(config).is_err());

    // max > Nyquist
    let config = PitchDetectorConfig {
        max_frequency: 30000.0,
        sample_rate: 44100,
        ..PitchDetectorConfig::default()
    };
    assert!(PitchDetector::new(config).is_err());
}

#[test]
fn nan_samples_are_sanitized() {
    let config = PitchDetectorConfig::default();
    let mut detector = PitchDetector::new(config).expect("valid config");

    let mut samples = vec![f32::NAN; 2048];
    samples.extend_from_slice(&[f32::INFINITY; 2048]);
    // Should not panic, and any estimate must be finite.
    for chunk in samples.chunks(512) {
        if let Some(estimate) = detector.push_samples(chunk) {
            assert!(estimate.voiced_probability.is_finite());
            assert!(estimate.frequency.is_none_or(f32::is_finite));
        }
    }
}

fn device_config() -> PitchDetectorConfig {
    PitchDetectorConfig::for_rate(48_000)
}

fn estimates_at_hops(samples: &[f32]) -> Vec<(usize, PitchEstimate)> {
    let mut detector = PitchDetector::new(device_config()).expect("valid device config");
    samples
        .chunks(128)
        .enumerate()
        .filter_map(|(index, chunk)| {
            detector
                .push_samples(chunk)
                .map(|estimate| ((index + 1) * 128, estimate))
        })
        .collect()
}

fn cents_error(frequency: f32, expected: f32) -> f32 {
    (1200.0 * (frequency / expected).log2()).abs()
}

#[test]
fn estimates_come_every_hop() {
    let samples = generate_sine(440.0, 48_000.0, 48_000);
    let estimates = estimates_at_hops(&samples);
    assert!(
        (93..=94).contains(&estimates.len()),
        "expected 93 or 94 estimates, got {}",
        estimates.len()
    );
    for pair in estimates.windows(2) {
        assert_eq!(pair[1].0 - pair[0].0, 512);
    }
}

#[test]
fn a_note_change_reads_the_new_note_not_an_average() {
    let mut phase = 0.0_f64;
    let samples: Vec<_> = (0..48_000)
        .map(|index| {
            let frequency = if index < 24_000 { 220.0 } else { 330.0 };
            let sample = phase.sin() as f32 * 0.5;
            phase = (phase + std::f64::consts::TAU * frequency / 48_000.0) % std::f64::consts::TAU;
            sample
        })
        .collect();
    let estimates = estimates_at_hops(&samples);
    assert_eq!(
        estimates.len(),
        93,
        "the note-change take must be judged every hop"
    );
    let mut settled = 0;
    for (end, estimate) in estimates {
        if let Some(frequency) = estimate.frequency {
            assert!(
                !(240.0..310.0).contains(&frequency),
                "run ending at {end} averaged the notes to {frequency} Hz"
            );
            if end >= 24_000 + 4096 {
                settled += 1;
                assert!(
                    cents_error(frequency, 330.0) <= 10.0,
                    "run ending at {end} read {frequency} Hz after the change"
                );
            }
        }
    }
    assert!(settled > 0, "the new note must produce voiced estimates");
}

fn assert_voiced_take(samples: &[f32], expected: f32) {
    let estimates = estimates_at_hops(samples);
    let settled: Vec<_> = estimates.iter().filter(|(end, _)| *end > 4096).collect();
    assert!(!settled.is_empty());
    let accurate = settled
        .iter()
        .filter(|(_, estimate)| {
            estimate
                .frequency
                .is_some_and(|frequency| cents_error(frequency, expected) <= 10.0)
        })
        .count();
    assert!(
        accurate * 10 >= settled.len() * 9,
        "{accurate}/{} settled estimates voiced within 10 cents of {expected} Hz",
        settled.len()
    );
}

#[test]
fn a_whistle_is_voiced() {
    let mut seed = 0x1234_5678_u32;
    let samples: Vec<_> = generate_sine(2500.0, 48_000.0, 48_000)
        .into_iter()
        .map(|sample| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (f64::from(seed) / f64::from(u32::MAX)).mul_add(0.02, -0.01) as f32;
            sample.mul_add(0.5, noise)
        })
        .collect();
    assert_voiced_take(&samples, 2500.0);
}

#[test]
fn a_low_hum_is_voiced() {
    let fundamental = generate_sine(55.0, 48_000.0, 48_000);
    let harmonic = generate_sine(110.0, 48_000.0, 48_000);
    let samples: Vec<_> = fundamental
        .iter()
        .zip(harmonic)
        .map(|(base, second)| second.mul_add(0.25, base * 0.5))
        .collect();
    assert_voiced_take(&samples, 55.0);
}

#[test]
fn config_for_rate_scales_the_window() {
    for (rate, frame_length, hop_length) in [
        (44_100, 2048, 512),
        (48_000, 2048, 512),
        (88_200, 4096, 1024),
        (96_000, 4096, 1024),
        (176_400, 8192, 2048),
        (192_000, 8192, 2048),
        (384_000, 16_384, 4096),
    ] {
        let config = PitchDetectorConfig::for_rate(rate);
        assert_eq!(config.sample_rate, rate);
        assert_eq!(config.frame_length, frame_length);
        assert_eq!(config.hop_length, hop_length);
        assert!((config.min_frequency - 50.0).abs() < f32::EPSILON);
        assert!((config.max_frequency - 4000.0).abs() < f32::EPSILON);
    }
}

#[test]
fn a_bad_hop_is_refused() {
    let config = PitchDetectorConfig::for_rate(48_000);
    for hop_length in [0, config.frame_length + 1] {
        let asked = PitchDetectorConfig {
            hop_length,
            ..config.clone()
        };
        assert!(matches!(
            PitchDetector::new(asked),
            Err(crate::Error::Config(_))
        ));
    }
}

/// Each of these passed `new`'s checks and then panicked inside pyin's
/// `PYINExecutor::new`, taking the engine's start down with it: a hop so long
/// that pyin's transition band is wider than the pitch grid, a range too narrow
/// to hold two pitch bins, and a frame too short to hold the range's periods.
#[test]
fn a_config_pyin_cannot_hold_is_refused() {
    let long_hop = PitchDetectorConfig {
        sample_rate: 8000,
        hop_length: 2048,
        ..PitchDetectorConfig::for_rate(8000)
    };
    let narrow_range = PitchDetectorConfig {
        min_frequency: 440.0,
        max_frequency: 441.0,
        ..PitchDetectorConfig::for_rate(48_000)
    };
    let short_frame = PitchDetectorConfig {
        frame_length: 4,
        hop_length: 1,
        ..PitchDetectorConfig::for_rate(48_000)
    };
    for config in [long_hop, narrow_range, short_frame] {
        let shown = format!("{config:?}");
        let built = std::panic::catch_unwind(|| PitchDetector::new(config));
        assert!(
            matches!(built, Ok(Err(crate::Error::Config(_)))),
            "{shown} must be refused with Error::Config, not accepted or panicking"
        );
    }
}
