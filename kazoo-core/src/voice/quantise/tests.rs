use super::{HYSTERESIS_MAX, QuantiseSettings, Quantiser, SCALES, SNAP_MS_MAX, nearest};
use crate::voice::{VOICE_HOP, VoiceFrame, hz_from_volts};
use std::f32::consts::TAU;

const RATE: u32 = 48_000;

fn frame(semitones: f32, onset: bool) -> VoiceFrame {
    let volts = semitones / 12.0;
    let hz = hz_from_volts(volts);
    VoiceFrame {
        at: 0,
        hz,
        volts,
        snapped_hz: hz,
        snapped_volts: volts,
        gate: true,
        onset,
        voiced: true,
        loudness: 0.8,
        breath: 0.0,
        confidence: 1.0,
    }
}

fn instant(keep_vibrato: bool) -> QuantiseSettings {
    QuantiseSettings {
        snap_ms: 0.0,
        keep_vibrato,
        blend: 1.0,
        ..QuantiseSettings::default()
    }
}

fn cents(hz: f32, reference: f32) -> f32 {
    1200.0 * (hz / reference).log2()
}

#[test]
fn off_is_hertz_for_hertz() {
    let mut quantiser = Quantiser::new(RATE);
    let mut random = 37_u16;
    let mut pitch = 9.0;
    for index in 0..1000 {
        random = random.wrapping_mul(25173).wrapping_add(13849);
        pitch = (f32::from(random) / f32::from(u16::MAX) - 0.5).mul_add(0.13, pitch);
        let mut input = frame(pitch, index == 0);
        quantiser.process(&mut input);
        assert_eq!(input.snapped_volts.to_bits(), input.volts.to_bits());
        assert_eq!(input.snapped_hz.to_bits(), input.hz.to_bits());
    }
    let mut input = frame(0.0, false);
    input.volts = -0.0;
    quantiser.process(&mut input);
    assert_eq!(input.snapped_volts.to_bits(), input.volts.to_bits());
}

#[test]
fn chromatic_snaps_a_flat_note() {
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(instant(false));
    for index in 0..100 {
        let mut input = frame(8.7, index == 0);
        quantiser.process(&mut input);
        assert!(cents(input.snapped_hz, 440.0).abs() < 0.01);
    }
}

#[test]
fn snap_speed_is_a_time_constant() {
    assert_eq!(VOICE_HOP, 64);
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(QuantiseSettings {
        snap_ms: 100.0,
        ..instant(false)
    });
    quantiser.process(&mut frame(9.0, true));
    for index in 1..=225 {
        let mut input = frame(8.6, false);
        quantiser.process(&mut input);
        let fraction = (input.snapped_volts - input.volts) * 1200.0 / 40.0;
        if index == 75 {
            assert!((fraction - 0.63).abs() < 0.03, "{fraction}");
        }
        if index == 225 {
            assert!((fraction - 0.95).abs() < 0.02, "{fraction}");
        }
    }
}

#[test]
fn keep_vibrato_keeps_the_wobble() {
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(instant(true));
    let mut sum = 0.0;
    let mut low = f32::MAX;
    let mut high = f32::MIN;
    for index in 0_u16..1500 {
        let phase = TAU * 6.0 * f32::from(index) / 750.0;
        let mut input = frame(0.5_f32.mul_add(phase.sin(), 9.2), index == 0);
        quantiser.process(&mut input);
        if index >= 750 {
            let output = cents(input.snapped_hz, 440.0);
            sum += output;
            low = low.min(output);
            high = high.max(output);
        }
    }
    assert!((sum / 750.0).abs() < 4.0, "mean {}", sum / 750.0);
    assert!((high - low - 100.0).abs() < 20.0, "span {}", high - low);

    quantiser.reset();
    quantiser.set(instant(false));
    for index in 0_u16..1500 {
        let phase = TAU * 6.0 * f32::from(index) / 750.0;
        let mut input = frame(0.3_f32.mul_add(phase.sin(), 9.2), index == 0);
        quantiser.process(&mut input);
        assert!(cents(input.snapped_hz, 440.0).abs() < 1.0);
    }
}

#[test]
fn hysteresis_holds_then_lets_go() {
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(instant(false));
    for (pitch, expected) in [(9.0, 9.0), (9.55, 9.0), (9.66, 10.0), (9.45, 10.0)] {
        let mut input = frame(pitch, false);
        quantiser.process(&mut input);
        assert!(input.snapped_volts.mul_add(12.0, -expected).abs() < 1e-5);
    }
}

#[test]
fn scale_and_root_follow_the_wall_rule() {
    let cases = [
        (0.0, 0, 0.0, 0.0),
        (1.0, 1, 0.0, 0.0),
        (1.4, 1, 0.0, 2.0),
        (6.0, 1, 0.0, 5.0),
        (-1.1, 1, 0.0, -1.0),
        (0.2, 8, 9.0, 0.0),
        (2.6, 0, 0.0, 3.0),
        (11.9, 1, 0.0, 12.0),
        (1.1, 1, 2.0, 1.0),
        (5.0, 1, 2.0, 4.0),
        (1.0, 9, 0.0, 0.0),
        (0.0, 9, 1.0, -1.0),
        (108.0, 0, 0.0, 96.0),
        (-108.0, 0, 0.0, -96.0),
        (3.4, 2, 0.0, 3.0),
        (4.0, 2, 0.0, 3.0),
        (10.5, 3, 0.0, 10.0),
        (1.5, 4, 0.0, 1.0),
        (5.5, 5, 0.0, 6.0),
        (10.6, 6, 0.0, 10.0),
        (6.0, 7, 0.0, 7.0),
        (8.0, 7, 0.0, 7.0),
        (-2.0, 8, 0.0, -2.0),
        (24.9, 1, 2.0, 25.0),
    ];
    for (input, scale, root, expected) in cases {
        let output = nearest(input / 12.0, SCALES[scale], root) * 12.0;
        assert!(
            (output - expected).abs() < 1e-5,
            "{input} {scale} {root}: {output}"
        );
    }
}

#[test]
fn blend_mixes_direct_and_snapped() {
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(QuantiseSettings {
        blend: 0.5,
        ..instant(false)
    });
    let mut input = frame(8.7, true);
    quantiser.process(&mut input);
    assert!((cents(input.snapped_hz, 440.0) + 15.0).abs() < 0.1);
}

#[test]
fn a_new_note_starts_in_tune() {
    for keep_vibrato in [false, true] {
        let mut quantiser = Quantiser::new(RATE);
        quantiser.set(QuantiseSettings {
            snap_ms: 400.0,
            ..instant(keep_vibrato)
        });
        quantiser.process(&mut frame(9.0, true));
        let mut input = frame(3.7, true);
        quantiser.process(&mut input);
        assert!(cents(input.snapped_hz, hz_from_volts(4.0 / 12.0)).abs() < 0.1);
    }
}

#[test]
fn unvoiced_frames_hold() {
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(instant(false));
    let mut input = frame(9.0, true);
    quantiser.process(&mut input);
    let expected = (input.snapped_hz.to_bits(), input.snapped_volts.to_bits());
    for index in 0..50 {
        input.voiced = false;
        input.onset = true;
        input.gate = false;
        input.volts = if index % 2 == 0 {
            f32::NAN
        } else {
            f32::INFINITY
        };
        quantiser.process(&mut input);
        assert_eq!(
            (input.snapped_hz.to_bits(), input.snapped_volts.to_bits()),
            expected
        );
    }
    quantiser.reset();
    quantiser.process(&mut input);
    assert_eq!(input.snapped_hz.to_bits(), 0.0_f32.to_bits());
    assert_eq!(input.snapped_volts.to_bits(), 0.0_f32.to_bits());
    input = frame(9.55, false);
    quantiser.process(&mut input);
    assert!(input.snapped_volts.mul_add(12.0, -10.0).abs() < 1e-5);
}

#[test]
fn settings_are_held_to_their_ranges() {
    let held = QuantiseSettings {
        scale: 12,
        root: 14,
        snap_ms: -5.0,
        keep_vibrato: false,
        hysteresis_cents: 900.0,
        blend: 2.0,
    }
    .held();
    assert_eq!(
        held,
        QuantiseSettings {
            scale: 9,
            root: 11,
            snap_ms: 0.0,
            keep_vibrato: false,
            hysteresis_cents: HYSTERESIS_MAX,
            blend: 1.0,
        }
    );
    assert_eq!(
        QuantiseSettings {
            snap_ms: 900.0,
            ..held
        }
        .held()
        .snap_ms
        .to_bits(),
        SNAP_MS_MAX.to_bits()
    );
    assert_eq!(
        QuantiseSettings {
            snap_ms: f32::NAN,
            hysteresis_cents: f32::NAN,
            blend: f32::NAN,
            ..QuantiseSettings::default()
        }
        .held(),
        QuantiseSettings::default()
    );
    for value in [f32::NEG_INFINITY, f32::INFINITY] {
        let settings = QuantiseSettings {
            snap_ms: value,
            hysteresis_cents: value,
            blend: value,
            ..held
        }
        .held();
        assert!(settings.snap_ms.is_finite());
        assert!(settings.hysteresis_cents.is_finite());
        assert!(settings.blend.is_finite());
    }
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(held);
    assert_eq!(quantiser.settings(), held);
}

#[test]
fn processing_never_allocates() {
    let mut quantiser = Quantiser::new(RATE);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for index in 0_u16..10_000 {
            quantiser.set(QuantiseSettings {
                scale: usize::from(index % 10),
                root: u8::try_from(index % 12).unwrap(),
                snap_ms: f32::from(index % 401),
                keep_vibrato: index % 2 == 0,
                blend: f32::from(index % 101) / 100.0,
                ..QuantiseSettings::default()
            });
            let mut input = frame(f32::from(index % 1200) / 100.0, index % 100 == 0);
            quantiser.process(&mut input);
            std::hint::black_box(input);
        }
        quantiser.reset();
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
}

#[test]
fn new_note_centre_recovers_after_two_frames() {
    let mut quantiser = Quantiser::new(RATE);
    quantiser.set(instant(true));
    quantiser.process(&mut frame(9.0, true));
    quantiser.process(&mut frame(3.7, false));
    let mut input = frame(3.7, false);
    quantiser.process(&mut input);
    assert!(cents(input.snapped_hz, hz_from_volts(4.0 / 12.0)).abs() < 0.1);
}

#[test]
fn non_finite_pitch_resets_without_poisoning_the_next_note() {
    for rate in [0, 1, RATE, u32::MAX] {
        let mut quantiser = Quantiser::new(rate);
        quantiser.set(instant(true));
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            quantiser.process(&mut frame(9.0, true));
            let mut input = frame(9.0, false);
            input.volts = value;
            input.hz = value;
            quantiser.process(&mut input);
            assert!(input.snapped_volts.is_finite());
            assert!(input.snapped_hz.is_finite());
            let mut next = frame(3.7, true);
            quantiser.process(&mut next);
            assert!(cents(next.snapped_hz, hz_from_volts(4.0 / 12.0)).abs() < 0.1);
        }
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(nearest(value, SCALES[0], 0.0).is_finite());
        assert!(nearest(0.0, SCALES[0], value).is_finite());
    }
}
