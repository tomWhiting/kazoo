use std::f32::consts::TAU;

use super::{Vocoder, VocoderBands};
use crate::Processor;
use crate::synthesis::PitchTrackedSynth;

const RATE: f32 = 48_000.0;

fn sine(frequency: f32, amplitude: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| amplitude * (TAU * frequency * i as f32 / RATE).sin())
        .collect()
}

/// A band-limited saw at 100 Hz: every harmonic up to 12 kHz at 1/n.
fn saw(len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| {
            (1..=120)
                .map(|n| (TAU * 100.0 * n as f32 * i as f32 / RATE).sin() / n as f32)
                .sum::<f32>()
                * 0.5
        })
        .collect()
}

fn power(samples: &[f32], frequency: f32) -> f32 {
    let (mut real, mut imaginary) = (0.0_f32, 0.0_f32);
    for (i, sample) in samples.iter().enumerate() {
        let phase = TAU * frequency * i as f32 / RATE;
        real = sample.mul_add(phase.cos(), real);
        imaginary = sample.mul_add(phase.sin(), imaginary);
    }
    real.mul_add(real, imaginary * imaginary)
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

fn vocoder(bands: VocoderBands) -> Vocoder {
    let mut vocoder = Vocoder::with_bands(RATE, bands);
    vocoder.prepare(256);
    vocoder
}

#[test]
fn a_separate_carrier_takes_the_voices_bands() {
    let len = 24_000;
    let modulator = sine(1000.0, 0.3, len);
    let carrier = saw(len);
    for bands in [
        VocoderBands::Eight,
        VocoderBands::Sixteen,
        VocoderBands::TwentyFour,
    ] {
        let mut vocoder = vocoder(bands);
        let mut output = vec![0.0; len];
        vocoder.process_with_carrier(&modulator, &carrier, &mut output);
        let tail = &output[len / 2..];
        // The carrier's harmonics near the voice's 1 kHz, against each of
        // the carrier's harmonics far from it: 8 bands an octave apart
        // spread the voice over a wider skirt than 24, and let 10 dB through
        // where the others hold 15.
        let held = if bands == VocoderBands::Eight {
            10.0
        } else {
            30.0
        };
        let voiced = (6..=15)
            .map(|harmonic| power(tail, 100.0 * harmonic as f32))
            .fold(0.0_f32, f32::max);
        for elsewhere in [100.0, 200.0, 3000.0, 6000.0] {
            let ratio = voiced / power(tail, elsewhere);
            assert!(
                ratio > held,
                "{bands:?}: the carrier's {elsewhere} Hz is only {ratio} under the voice's 1 kHz"
            );
        }
        // Its level is the voice's in each band; how much of a voice the
        // bands catch between them is the mouth's level test, sung.
        assert!(rms(tail) > 0.01, "{bands:?}: the voice is lost");
    }
}

#[test]
fn a_silent_voice_or_carrier_is_silence() {
    let len = 4_800;
    let mut vocoder = vocoder(VocoderBands::Sixteen);
    let mut output = vec![1.0; len];
    vocoder.process_with_carrier(&vec![0.0; len], &saw(len), &mut output);
    assert!(output.iter().all(|x| *x == 0.0));
    let mut vocoder = self::vocoder(VocoderBands::Sixteen);
    vocoder.process_with_carrier(&sine(1000.0, 0.3, len), &vec![0.0; len], &mut output);
    assert!(output.iter().all(|x| *x == 0.0));
}

#[test]
fn nonsense_in_the_voice_or_the_carrier_is_read_as_silence() {
    let len = 9_600;
    let mut modulator = sine(1000.0, 0.3, len);
    let mut carrier = saw(len);
    for (i, (voice, tone)) in modulator.iter_mut().zip(&mut carrier).enumerate() {
        match i % 7 {
            0 => *voice = f32::NAN,
            1 => *tone = f32::INFINITY,
            2 => *voice = f32::NEG_INFINITY,
            3 => *tone = f32::NAN,
            _ => {}
        }
    }
    let mut vocoder = vocoder(VocoderBands::TwentyFour);
    let mut output = vec![0.0; len];
    vocoder.process_with_carrier(&modulator, &carrier, &mut output);
    assert!(output.iter().all(|x| x.is_finite()));
    assert!(output[len / 2..].iter().any(|x| x.abs() > 1e-3));
    let mut output = vec![0.0; len];
    vocoder.process_with_carrier(&[f32::NAN; 64], &[f32::INFINITY; 64], &mut output);
    assert!(output.iter().all(|x| x.is_finite()));
    vocoder.reset();
    vocoder.process_with_carrier(&[f32::NAN; 64], &[f32::INFINITY; 64], &mut output[..64]);
    assert!(output[..64].iter().all(|x| *x == 0.0));
}

#[test]
fn short_inputs_read_as_silence_and_long_blocks_render_in_pieces() {
    let len = 2_000;
    let modulator = sine(1000.0, 0.3, len);
    let carrier = saw(len);
    let mut whole = vocoder(VocoderBands::Sixteen);
    let mut pieces = vocoder(VocoderBands::Sixteen);
    let mut expected = vec![0.0; len];
    for (start, end) in [(0, 256), (256, 512), (512, 768), (768, 1024)] {
        whole.process_with_carrier(
            &modulator[start..end],
            &carrier[start..end],
            &mut expected[start..end],
        );
    }
    let mut actual = vec![0.0; 1024];
    pieces.process_with_carrier(&modulator[..1024], &carrier[..1024], &mut actual);
    assert_eq!(
        actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        expected[..1024]
            .iter()
            .map(|x| x.to_bits())
            .collect::<Vec<_>>()
    );
    let mut output = vec![1.0; 512];
    let mut silent = vocoder(VocoderBands::Eight);
    silent.process_with_carrier(&modulator[..512], &[], &mut output);
    assert!(output.iter().all(|x| *x == 0.0));
    let mut output = vec![1.0; 512];
    let mut silent = vocoder(VocoderBands::Eight);
    silent.process_with_carrier(&[], &carrier[..512], &mut output);
    assert!(output.iter().all(|x| *x == 0.0));
}

#[test]
fn a_separate_carrier_never_allocates() {
    let modulator = sine(1000.0, 0.3, 4_096);
    let carrier = saw(4_096);
    let mut vocoder = vocoder(VocoderBands::TwentyFour);
    let mut synth = PitchTrackedSynth::new(RATE);
    synth.prepare(256);
    let mut output = vec![0.0; 4_096];
    let mut tone = vec![0.0; 4_096];
    let mut raw = vec![0.0; 4_096];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        vocoder.process_with_carrier(&modulator, &carrier, &mut output);
        vocoder.reset();
        synth.process_with_carrier(&modulator, &mut tone, &mut raw);
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
}

#[test]
fn a_new_sample_rate_keeps_every_stage_on_its_band() {
    let mut vocoder = vocoder(VocoderBands::Sixteen);
    vocoder.set_sample_rate(RATE);
    let len = 24_000;
    let mut output = vec![0.0; len];
    vocoder.process_with_carrier(&sine(1000.0, 0.3, len), &saw(len), &mut output);
    let mut fresh = self::vocoder(VocoderBands::Sixteen);
    let mut expected = vec![0.0; len];
    fresh.process_with_carrier(&sine(1000.0, 0.3, len), &saw(len), &mut expected);
    assert_eq!(
        output.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn the_synths_carrier_render_is_its_plain_render() {
    let input = sine(220.0, 0.5, 4_096);
    let mut plain = PitchTrackedSynth::new(RATE);
    let mut with_carrier = PitchTrackedSynth::new(RATE);
    for synth in [&mut plain, &mut with_carrier] {
        synth.prepare(128);
        synth.set_target_frequency(220.0);
    }
    let mut expected = vec![0.0; 4_096];
    plain.process(&input, &mut expected);
    let mut actual = vec![0.0; 4_096];
    let mut carrier = vec![0.0; 4_096];
    with_carrier.process_with_carrier(&input, &mut actual, &mut carrier);
    assert_eq!(
        actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
    assert!(carrier.iter().all(|x| x.is_finite()));
    assert!(
        rms(&carrier) > 0.3,
        "the carrier is the oscillator at full level"
    );
    let mut short = vec![1.0; 100];
    let mut short_output = vec![1.0; 100];
    with_carrier.process_with_carrier(&input[..100], &mut short_output, &mut short[..40]);
    assert!(short_output[40..].iter().all(|x| *x == 0.0));
}

#[test]
fn on_sine_the_carrier_is_a_saw_and_the_output_stays_a_sine() {
    let input = sine(220.0, 0.5, 9_600);
    let mut plain = PitchTrackedSynth::new(RATE);
    let mut with_carrier = PitchTrackedSynth::new(RATE);
    for synth in [&mut plain, &mut with_carrier] {
        assert!(synth.set_param(0, 0.0).is_ok(), "sine is shape 0");
        synth.prepare(128);
        synth.set_target_frequency(200.0);
    }
    let mut expected = vec![0.0; 9_600];
    plain.process(&input, &mut expected);
    let mut actual = vec![0.0; 9_600];
    let mut raw = vec![0.0; 9_600];
    with_carrier.process_with_carrier(&input, &mut actual, &mut raw);
    assert_eq!(
        actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
    let tail = &raw[4_800..];
    let fundamental = power(tail, 200.0);
    for harmonic in [2.0_f32, 3.0, 5.0, 10.0] {
        let ratio = power(tail, 200.0 * harmonic) / fundamental;
        let expected_ratio = 1.0 / (harmonic * harmonic);
        assert!(
            (ratio / expected_ratio - 1.0).abs() < 0.25,
            "harmonic {harmonic}: {ratio} of the fundamental, a saw has {expected_ratio}"
        );
    }
    let mut unpitched = PitchTrackedSynth::new(RATE);
    assert!(unpitched.set_param(0, 0.0).is_ok(), "sine is shape 0");
    unpitched.prepare(128);
    raw[..256].fill(1.0);
    unpitched.process_with_carrier(&input[..256], &mut actual[..256], &mut raw[..256]);
    assert!(raw[..256].iter().all(|x| *x == 0.0), "no pitch, no carrier");
}
