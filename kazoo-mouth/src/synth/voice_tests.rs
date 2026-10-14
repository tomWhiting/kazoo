use std::f32::consts::TAU;

use kazoo_core::voice::{VoiceFrame, volts_from_hz};

use super::{SynthSettings, VoiceSynth};
#[path = "legacy.rs"]
mod legacy;
use legacy::LegacyVoiceSynth;

const RATE: u32 = 48_000;
const BLOCK: usize = 128;

fn frame() -> VoiceFrame {
    VoiceFrame {
        at: 0,
        hz: 100.0,
        volts: volts_from_hz(100.0),
        snapped_hz: 100.0,
        snapped_volts: volts_from_hz(100.0),
        gate: true,
        onset: true,
        voiced: true,
        loudness: 0.8,
        breath: 0.2,
        confidence: 1.0,
    }
}

fn vowel(formants: [f32; 2]) -> Vec<f32> {
    (0..24_000)
        .map(|sample| {
            (1..=60)
                .map(|harmonic| {
                    let frequency = harmonic as f32 * 100.0;
                    let amplitude: f32 = formants
                        .iter()
                        .map(|center| (-0.5 * ((frequency - center) / 100.0).powi(2)).exp())
                        .sum();
                    0.12 * amplitude * (TAU * frequency * sample as f32 / RATE as f32).sin()
                })
                .sum()
        })
        .collect()
}

fn render(synth: &mut VoiceSynth, mic: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0; mic.len()];
    synth.render(mic, &mut out);
    out
}

fn power(samples: &[f32], frequency: f32) -> f32 {
    let mut real = 0.0;
    let mut imaginary = 0.0;
    for (index, sample) in samples.iter().enumerate() {
        let phase = TAU * frequency * index as f32 / RATE as f32;
        let window = 0.5_f32.mul_add(-(TAU * index as f32 / samples.len() as f32).cos(), 0.5);
        real = (sample * window).mul_add(phase.cos(), real);
        imaginary = (sample * window).mul_add(phase.sin(), imaginary);
    }
    real.mul_add(real, imaginary * imaginary)
}

fn peaks(samples: &[f32]) -> [f32; 2] {
    [(1..=9), (10..=30)].map(|range| {
        range
            .map(|harmonic| {
                (
                    harmonic as f32 * 100.0,
                    power(samples, harmonic as f32 * 100.0),
                )
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map_or(0.0, |peak| peak.0)
    })
}

fn assert_bits_match(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.to_bits(), expected.to_bits(), "sample {index}");
    }
}

#[test]
fn voice_zero_is_todays_sound() -> kazoo_core::ParamResult<()> {
    let mic = vowel([800.0, 1200.0]);
    let mut synth = VoiceSynth::new(RATE, BLOCK)?;
    let mut legacy = LegacyVoiceSynth::new(RATE, BLOCK)?;
    synth.control(&frame());
    legacy.control(&frame());
    let mut expected = vec![0.0; mic.len()];
    legacy.render(&mic, &mut expected);
    assert_bits_match(&render(&mut synth, &mic), &expected);
    synth.set(SynthSettings {
        voice: 1.0,
        ..SynthSettings::default()
    })?;
    legacy.render(&mic, &mut expected);
    assert!(
        render(&mut synth, &mic) != expected,
        "voice must engage before testing return to zero"
    );
    synth.set(SynthSettings::default())?;
    legacy.render(&mic, &mut expected);
    let actual = render(&mut synth, &mic);
    assert_bits_match(&actual[1000..], &expected[1000..]);
    Ok(())
}

#[test]
fn voice_one_carries_the_vowels() -> kazoo_core::ParamResult<()> {
    let mut wet = VoiceSynth::new(RATE, BLOCK)?;
    let mut dry = VoiceSynth::new(RATE, BLOCK)?;
    wet.set(SynthSettings {
        voice: 1.0,
        sibilance: 0.0,
        air: 0.0,
        ..SynthSettings::default()
    })?;
    dry.set(SynthSettings {
        air: 0.0,
        ..SynthSettings::default()
    })?;
    wet.control(&frame());
    dry.control(&frame());
    let mut dry_peaks = Vec::new();
    for formants in [[800.0, 1200.0], [300.0, 2400.0]] {
        let mic = vowel(formants);
        let input_peaks = peaks(&mic[12_000..]);
        let wet_out = render(&mut wet, &mic);
        let wet_peaks = peaks(&wet_out[12_000..]);
        for (actual, expected) in wet_peaks.into_iter().zip(input_peaks) {
            assert!(
                (actual - expected).abs() / expected <= 0.15,
                "wet formant {actual} Hz, input {expected} Hz, pair {formants:?}"
            );
        }
        dry_peaks.push(peaks(&render(&mut dry, &mic)[12_000..]));
    }
    assert_eq!(
        dry_peaks[0].map(f32::to_bits),
        dry_peaks[1].map(f32::to_bits),
        "plain oscillator must not follow vowels"
    );
    Ok(())
}

#[test]
fn changing_bands_never_allocates_or_clicks() -> kazoo_core::ParamResult<()> {
    let mic = vowel([800.0, 1200.0]);
    let mut synth = VoiceSynth::new(RATE, BLOCK)?;
    let settings = SynthSettings {
        voice: 1.0,
        bands: 8,
        air: 0.0,
        sibilance: 0.0,
        ..SynthSettings::default()
    };
    let mut unchanged = VoiceSynth::new(RATE, BLOCK)?;
    synth.set(settings)?;
    unchanged.set(settings)?;
    unchanged.control(&frame());
    synth.control(&frame());
    let steady = render(&mut synth, &mic);
    assert_eq!(steady, render(&mut unchanged, &mic));
    let maximum_step = steady[12_000..]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0_f32, f32::max);
    let mut changed = vec![0.0; mic.len()];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| -> kazoo_core::ParamResult<()> {
        synth.set(SynthSettings {
            bands: 24,
            ..settings
        })?;
        synth.render(&mic, &mut changed);
        let mut fade = [0.0; BLOCK];
        synth.set(SynthSettings {
            voice: 0.0,
            bands: 24,
            ..settings
        })?;
        synth.render(&mic[..BLOCK], &mut fade);
        synth.set(SynthSettings {
            bands: 24,
            ..settings
        })?;
        synth.render(&mic[..BLOCK], &mut fade);
        assert!(fade.iter().all(|sample| sample.is_finite()));
        Ok(())
    })?;
    assert_eq!(assert_no_alloc::violation_count(), before);
    let same_bands = render(&mut unchanged, &mic);
    assert!(
        changed[12_000..] != same_bands[12_000..],
        "band count must change the spectral bank"
    );
    let transition_step = changed
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold((changed[0] - steady[steady.len() - 1]).abs(), f32::max);
    assert!(
        transition_step <= maximum_step * 2.0,
        "step {transition_step}, steady {maximum_step}"
    );
    Ok(())
}

#[test]
fn nonsense_mic_is_survived() -> kazoo_core::ParamResult<()> {
    let mut wet = VoiceSynth::new(RATE, BLOCK)?;
    let mut dry = VoiceSynth::new(RATE, BLOCK)?;
    wet.set(SynthSettings {
        voice: 1.0,
        ..SynthSettings::default()
    })?;
    wet.control(&frame());
    dry.control(&frame());
    let mut mic = vowel([800.0, 1200.0]);
    for (index, sample) in mic.iter_mut().enumerate() {
        if index % 97 == 0 {
            *sample = f32::NAN;
        }
        if index % 101 == 0 {
            *sample = f32::INFINITY;
        }
        if index % 103 == 0 {
            *sample = f32::NEG_INFINITY;
        }
    }
    let output = render(&mut wet, &mic);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(
        output[12_000..].iter().any(|sample| sample.abs() > 0.0001),
        "valid mic samples must remain audible"
    );
    assert!(
        output != render(&mut dry, &mic),
        "finite output must come from the engaged vocoder"
    );
    Ok(())
}
