use super::super::testkit::{null_db, render, render_mono, rms, sine};
use super::*;

const HOT: &[(usize, f32)] = &[(GAIN, 100.0), (MASTER, 100.0), (CAB, 0.0)];

#[test]
fn the_null_against_the_old_model_holds() {
    let defaults = null_db(&mut Amp::reference(), &mut Amp::new(), &[]);
    println!("default null {defaults:.2} dB");
    assert!(defaults < -40.0, "default null {defaults:.2} dB");
    let hot = null_db(&mut Amp::reference(), &mut Amp::new(), HOT);
    println!("hot null {hot:.2} dB");
    assert!(hot < -30.0, "hot null {hot:.2} dB");
}

#[test]
fn switching_the_voice_does_not_click() {
    let input = sine(220.0, 10f32.powf(-12.0 / 20.0), 0.4);
    let step = |samples: &[f32]| {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let mut amp = Amp::new();
    amp.prepare(48_000.0);
    amp.reset();
    let mut before = render_mono(&mut amp, &input);
    for (voice, fender_live, marshall_live) in [(1.0, false, true), (0.0, true, false)] {
        let rest = step(&before[before.len() - 4_800..]);
        amp.set_param(VOICE, voice);
        let after = render_mono(&mut amp, &input);
        let mut window = vec![before[before.len() - 1]];
        window.extend_from_slice(&after[..1_440]);
        let during = step(&window);
        assert!(
            during <= 3.0 * rest,
            "voice {voice}: step {during} against {rest}"
        );
        for circuit in amp.pair.circuits() {
            assert_eq!(circuit.fender_live, fender_live);
            assert_eq!(circuit.marshall_live, marshall_live);
        }
        before = after;
    }
}

#[test]
fn flat_out_square_wave_stays_bounded_and_the_tail_dies() {
    let mut amp = Amp::new();
    amp.prepare(48_000.0);
    assert_eq!(amp.pair.rate().to_bits(), 384_000.0f64.to_bits());
    for &(index, value) in HOT {
        amp.set_param(index, value);
    }
    amp.set_param(LEVEL, 12.0);
    amp.reset();
    let mut input: Vec<f32> = (0..48_000)
        .map(|frame| if frame % 960 < 480 { 16.0 } else { -16.0 })
        .collect();
    input.resize(96_000, 0.0);
    let outputs: [Vec<f32>; 2] = render(&mut amp, &input, &input).into();
    for output in outputs {
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 2.0)
        );
        let tail = rms(&output[91_200..]);
        assert!(tail < 1e-7, "tail RMS {tail}");
    }
}
