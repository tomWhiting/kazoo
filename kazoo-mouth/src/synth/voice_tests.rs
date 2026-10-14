use std::f32::consts::TAU;

use kazoo_core::synthesis::OscillatorShape;
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

fn vowel_at(fundamental: f32, formants: [f32; 2], samples: usize) -> Vec<f32> {
    let harmonics = (6000.0 / fundamental) as usize;
    (0..samples)
        .map(|sample| {
            (1..=harmonics)
                .map(|harmonic| {
                    let frequency = harmonic as f32 * fundamental;
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

fn frame_at(hz: f32) -> VoiceFrame {
    VoiceFrame {
        hz,
        volts: volts_from_hz(hz),
        snapped_hz: hz,
        snapped_volts: volts_from_hz(hz),
        ..frame()
    }
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

fn formant_peaks(samples: &[f32], pitch: f32) -> [f32; 2] {
    let peak = |low: f32, high: f32| {
        ((low / pitch).ceil().max(1.0) as usize..=(high / pitch) as usize)
            .map(|harmonic| {
                let frequency = harmonic as f32 * pitch;
                (frequency, power(samples, frequency))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map_or(0.0, |found| found.0)
    };
    [peak(100.0, 900.0), peak(1000.0, 3000.0)]
}

fn sung(shape: OscillatorShape, bands: u8, voice: f32) -> SynthSettings {
    SynthSettings {
        shape,
        bands,
        voice,
        air: 0.0,
        sibilance: 0.0,
        ..SynthSettings::default()
    }
}

/// Singing comes through on every shape with harmonics to carry it, and on
/// a pitch other than the one sung: the output's first two formants sit
/// within 15% of the voice's, for /a/ and /i/.
#[test]
fn the_vowels_come_through_on_every_bright_shape() -> kazoo_core::ParamResult<()> {
    let mut misses = Vec::new();
    for (shape, pitch) in [
        (OscillatorShape::Saw, 100.0),
        (OscillatorShape::Saw, 150.0),
        (OscillatorShape::Square, 100.0),
        (OscillatorShape::Triangle, 100.0),
    ] {
        for formants in [[800.0, 1200.0], [300.0, 2400.0]] {
            let mic = vowel_at(100.0, formants, 24_000);
            let mut synth = VoiceSynth::new(RATE, BLOCK)?;
            synth.set(sung(shape, 16, 1.0))?;
            synth.control(&frame_at(pitch));
            let found = formant_peaks(&render(&mut synth, &mic)[12_000..], pitch);
            for (actual, expected) in found.into_iter().zip(formants) {
                if (actual - expected).abs() / expected > 0.15 {
                    misses.push(format!(
                        "{shape:?} at {pitch} Hz, vowel {formants:?}: formant {actual} Hz for {expected} Hz"
                    ));
                }
            }
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
    Ok(())
}

/// Turning the voice up keeps the mouth as loud as it was: the vocoder at
/// voice 1 sits within 3 dB of the plain saw at voice 0, at every band
/// count, for /a/ and /i/.
#[test]
fn the_voice_keeps_the_synths_level() -> kazoo_core::ParamResult<()> {
    let mut misses = Vec::new();
    for formants in [[800.0, 1200.0], [300.0, 2400.0]] {
        let mic = vowel_at(100.0, formants, 24_000);
        let mut plain = VoiceSynth::new(RATE, BLOCK)?;
        plain.set(sung(OscillatorShape::Saw, 16, 0.0))?;
        plain.control(&frame());
        let plain_level = rms(&render(&mut plain, &mic)[12_000..]);
        for bands in [8, 16, 24] {
            let mut wet = VoiceSynth::new(RATE, BLOCK)?;
            wet.set(sung(OscillatorShape::Saw, bands, 1.0))?;
            wet.control(&frame());
            let level = rms(&render(&mut wet, &mic)[12_000..]);
            let decibels = 20.0 * (level / plain_level).log10();
            if decibels.abs() > 3.0 {
                misses.push(format!(
                    "{bands} bands, vowel {formants:?}: {decibels:.1} dB from the plain synth"
                ));
            }
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
    Ok(())
}

/// The vocoder rests while the voice is at 0; turned up again, it starts
/// from silence, not from the last vowel sung before it rested.
#[test]
fn the_voice_returns_without_the_last_vowel() -> kazoo_core::ParamResult<()> {
    let mut synth = VoiceSynth::new(RATE, BLOCK)?;
    let on = sung(OscillatorShape::Saw, 16, 1.0);
    synth.set(on)?;
    synth.control(&frame());
    render(&mut synth, &vowel([800.0, 1200.0]));
    synth.set(SynthSettings { voice: 0.0, ..on })?;
    let silence = vec![0.0; RATE as usize];
    render(&mut synth, &silence);
    synth.set(on)?;
    let peak = render(&mut synth, &silence[..9600])
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak < 1e-4, "the last vowel came back at {peak}");
    Ok(())
}

/// What the vocoder costs the output callback: microseconds per 128-frame
/// block at 48 kHz, rendered in the engine's 64-frame hops, against the
/// block's 2,666.667 us deadline. Run optimised for the number that counts.
#[test]
fn voice_cost_per_block() -> kazoo_core::ParamResult<()> {
    use std::time::Instant;
    let mic = vowel([800.0, 1200.0]);
    let blocks: u32 = if cfg!(debug_assertions) { 32 } else { 2000 };
    for (voice, bands) in [(0.0, 16), (1.0, 8), (1.0, 16), (1.0, 24), (0.5, 16)] {
        let mut synth = VoiceSynth::new(RATE, BLOCK)?;
        synth.set(sung(OscillatorShape::Saw, bands, voice))?;
        synth.control(&frame());
        let mut out = [0.0; BLOCK];
        for block in mic.chunks_exact(BLOCK).take(100) {
            synth.render(&block[..64], &mut out[..64]);
            synth.render(&block[64..], &mut out[64..]);
        }
        let start = Instant::now();
        for block in mic.chunks_exact(BLOCK).cycle().take(blocks as usize) {
            synth.render(&block[..64], &mut out[..64]);
            synth.render(&block[64..], &mut out[64..]);
        }
        let synth_us = start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(blocks);
        println!(
            "voice={voice} bands={bands} synth_us={synth_us:.3} block_us=2666.667 synth_percent={:.2}",
            synth_us / 2666.667 * 100.0
        );
        assert!(out.iter().all(|sample| sample.is_finite()));
    }
    Ok(())
}
