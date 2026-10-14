use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::{VoiceFrame, volts_from_hz};

use super::{SynthSettings, VoiceSynth};

#[test]
fn controls_are_held_to_finite_ranges() {
    let held = SynthSettings {
        cutoff_hz: f32::INFINITY,
        air: f32::NEG_INFINITY,
        volume: f32::NAN,
        ..SynthSettings::default()
    }
    .held();
    assert!((held.cutoff_hz - 12_000.0).abs() < f32::EPSILON);
    assert!(held.air.abs() < f32::EPSILON);
    assert!((held.volume - 0.7).abs() < f32::EPSILON);
}

#[test]
fn unvoiced_frames_hold_pitch_and_closed_gate_releases() -> kazoo_core::ParamResult<()> {
    let mut synth = VoiceSynth::new(48_000, 128)?;
    synth.set(SynthSettings {
        shape: OscillatorShape::Sine,
        air: 0.0,
        ..SynthSettings::default()
    })?;
    let mut frame = VoiceFrame {
        at: 64,
        hz: 220.0,
        volts: volts_from_hz(220.0),
        snapped_hz: 220.0,
        snapped_volts: volts_from_hz(220.0),
        events: kazoo_core::voice::VoiceEvents {
            gate: true,
            onset: true,
        },
        voiced: true,
        provisional: false,
        loudness: 1.0,
        breath: 0.0,
        confidence: 1.0,
    };
    synth.control(&frame);
    let input = [0.5; 128];
    let mut output = [0.0; 128];
    for _ in 0..100 {
        synth.render(&input, &mut output);
    }
    assert!((synth.tone.current_frequency() - 220.0).abs() < 0.01);
    frame.voiced = false;
    frame.snapped_hz = 880.0;
    frame.gate = false;
    synth.control(&frame);
    for _ in 0..400 {
        synth.render(&input, &mut output);
    }
    assert!((synth.tone.current_frequency() - 220.0).abs() < 0.01);
    assert!(output.iter().all(|sample| sample.abs() < 0.0001));
    Ok(())
}
