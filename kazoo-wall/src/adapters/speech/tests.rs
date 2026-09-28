//! Speech tests: the vocoder speaks, the speaker plays what its feed gives
//! it on its gate, and neither allocates.

use kazoo_speech::Phrase;

use super::*;
use crate::catalogue::Kind;
use crate::dsp::testing::{Bench, RATE, survives_nonsense};
use crate::{MAX_INPUTS, MAX_OUTPUTS};

fn on_audio_thread<T>(f: impl FnOnce() -> T) -> T {
    let before = assert_no_alloc::violation_count();
    let result = assert_no_alloc::assert_no_alloc(f);
    assert_eq!(
        assert_no_alloc::violation_count(),
        before,
        "the audio thread allocated or freed memory"
    );
    result
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
}

#[test]
fn the_vocoder_speaks_the_carrier_through_the_modulator() {
    let kind = Kind::from_name("vocoder").unwrap();
    assert_eq!(
        kind.spec().jack_names()[..3],
        ["carrier_left", "carrier_right", "modulator"]
    );
    let mut bench = Bench::new(kind);
    // A bright carrier, and a modulator that speaks for half a second.
    let mut phase = 0.0_f32;
    let mut carrier = Vec::new();
    for _ in 0..48_000 {
        phase = (phase + 110.0 / RATE).fract();
        carrier.push(phase.mul_add(2.0, -1.0) * 0.5);
    }
    let mut out = Vec::new();
    let mut right = Vec::new();
    let mut frame = 0;
    while frame < 48_000 {
        for i in 0..SUB_BLOCK {
            bench.inputs[IN_CARRIER_LEFT][i] = carrier[frame + i];
            let talking = frame < 24_000;
            bench.inputs[IN_MODULATOR][i] = if talking {
                ((frame + i) as f32 * 0.07).sin() * 0.5
            } else {
                0.0
            };
        }
        bench.connected[IN_CARRIER_LEFT] = true;
        bench.connected[IN_MODULATOR] = true;
        on_audio_thread(|| bench.step());
        out.extend_from_slice(&bench.outputs[0]);
        right.extend_from_slice(&bench.outputs[1]);
        frame += SUB_BLOCK;
    }
    let talking = peak(&out[12_000..24_000]);
    let quiet = peak(&out[40_000..]);
    assert!(talking > 0.01, "the vocoder is silent: {talking}");
    assert!(
        quiet < talking * 0.1,
        "it speaks without a modulator: {quiet}"
    );
    // The right carrier follows the left when unplugged: stereo out.
    assert!(peak(&right[12_000..24_000]) > 0.01);
    survives_nonsense(kind);
}

/// Render a speaker for `blocks` sub-blocks with its gate at `gate`.
fn play(module: &mut SpeakModule, gate: f32, blocks: usize) -> Vec<f32> {
    let spec = Kind::from_name("speak").unwrap().spec();
    let mut knobs = [0.0; MAX_KNOBS];
    for (knob, value) in knobs.iter_mut().zip(spec.defaults()) {
        *knob = value;
    }
    let knob_cv = [[0.0; SUB_BLOCK]; MAX_KNOBS];
    let mut inputs = [[0.0; SUB_BLOCK]; MAX_INPUTS];
    inputs[IN_GATE] = [gate; SUB_BLOCK];
    let mut outputs = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
    let tick = Tick::new(RATE, 120.0, 0.0);
    let mut out = Vec::new();
    for _ in 0..blocks {
        on_audio_thread(|| {
            module.process(
                &tick,
                Io {
                    spec,
                    knobs: &knobs,
                    sweeps: &[crate::dsp::Sweep::REST; MAX_KNOBS],
                    knob_cv: &knob_cv,
                    knob_patched: [false; MAX_KNOBS],
                    inputs: &inputs,
                    connected: [true, false, false, false],
                    outputs: &mut outputs,
                },
            );
        });
        out.extend_from_slice(&outputs[0]);
    }
    out
}

#[test]
fn the_speaker_plays_its_phrase_on_the_gate() {
    let (mut module, mut feed) = SpeakModule::new(RATE);
    // No phrase yet: silence, gate or not.
    assert!(play(&mut module, 1.0, 10).iter().all(|s| *s == 0.0));
    let tone: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    feed.load(Phrase::new(tone, 48_000), false).unwrap();
    // Waiting for the gate.
    assert!(play(&mut module, 0.0, 20).iter().all(|s| s.abs() < 1e-6));
    let spoken = play(&mut module, 1.0, 200);
    assert!(peak(&spoken) > 0.2, "{}", peak(&spoken));
    // A new phrase replaces it; the old one comes back to be freed here.
    feed.load(Phrase::new(vec![0.1; 4_800], 48_000), true)
        .unwrap();
    play(&mut module, 1.0, 10);
    assert!(feed.collect() >= 1);
    survives_nonsense(Kind::from_name("speak").unwrap());
}
