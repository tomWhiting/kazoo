//! The amplifier: `bias + gain × cv` (held to 0..1) when the cv input is
//! plugged in, plain `gain` when it is not.
//!
//! The cv input acts sample by sample, unsmoothed, so an envelope keeps its
//! attack and an audio-rate cv makes true amplitude or ring modulation.
//! The knobs follow their glides and jacks sample by sample too; a knob
//! turned "at once" still takes a millisecond (see the engine), so nothing
//! here needs smoothing of its own.

use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;

const GAIN: usize = 0;
const BIAS: usize = 1;

const IN_AUDIO: usize = 0;
const IN_CV: usize = 1;

#[derive(Debug)]
pub struct Vca;

impl Vca {
    pub const fn new() -> Self {
        Self
    }
}

impl Module for Vca {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for frame in 0..SUB_BLOCK {
            let gain = io.knob_at(GAIN, frame);
            let amp = if io.connected[IN_CV] {
                let bias = io.knob_at(BIAS, frame);
                gain.mul_add(finite(io.inputs[IN_CV][frame]), bias)
                    .clamp(0.0, 1.0)
            } else {
                gain
            };
            let x = finite(io.inputs[IN_AUDIO][frame]).clamp(-16.0, 16.0);
            io.outputs[0][frame] = x * amp;
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, db, stays_in_range, survives_nonsense, tone};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_indices_match_the_catalogue() {
        let spec = Kind::VCA.spec();
        assert_eq!(spec.knobs[GAIN].name, "gain");
        assert_eq!(spec.knobs[BIAS].name, "bias");
        assert_eq!(spec.inputs[IN_AUDIO].name, "in");
        assert_eq!(spec.inputs[IN_CV].name, "cv");
    }

    fn settled(bench: &mut Bench) -> f32 {
        bench.render(0, 9_600);
        bench.render(0, 32)[31]
    }

    #[test]
    fn gain_alone_without_cv_and_bias_plus_gain_with_it() {
        let mut bench = Bench::new(Kind::VCA);
        bench.knob("gain", 0.5).hold("in", 1.0);
        assert!((settled(&mut bench) - 0.5).abs() < 1e-3);

        bench.knob("bias", 0.25).hold("cv", 0.5);
        assert!((settled(&mut bench) - 0.5).abs() < 1e-3);

        bench.hold("cv", 0.0);
        assert!((settled(&mut bench) - 0.25).abs() < 1e-3);

        // The sum is held to 0..1.
        bench.hold("cv", 10.0);
        assert!((settled(&mut bench) - 1.0).abs() < 1e-3);
        bench.hold("cv", -10.0);
        assert!(settled(&mut bench).abs() < 1e-3);
    }

    /// 10 % to 90 % rise and 90 % to 10 % fall of `samples` after `from`,
    /// in frames.
    fn edges(samples: &[f32], on: usize, off: usize) -> (usize, usize) {
        let first = |from: usize, test: &dyn Fn(f32) -> bool| {
            from + samples[from..].iter().position(|s| test(*s)).unwrap()
        };
        let rise = first(on, &|s| s >= 0.9) - first(on, &|s| s >= 0.1);
        let fall = first(off, &|s| s <= 0.1) - first(off, &|s| s <= 0.9);
        (rise, fall)
    }

    #[test]
    fn an_envelope_keeps_its_edges() {
        // Before: a 1 ms attack came out of the VCA as an 11.4 ms rise at
        // 48 kHz, because the cv went through a 5 ms smoother.
        for rate in [48_000.0, 96_000.0, 192_000.0] {
            for (attack, release) in [(0.001, 0.001), (0.005, 0.005), (0.02, 0.05)] {
                let mut env = Bench::at(Kind::ENV, rate);
                env.knob("attack", attack)
                    .knob("decay", 1.0)
                    .knob("sustain", 1.0)
                    .knob("release", release);
                let mut vca = Bench::at(Kind::VCA, rate);
                vca.knob("gain", 1.0).knob("bias", 0.0).hold("in", 1.0);
                let on = (rate * 0.1) as usize;
                let off = (rate * 0.35) as usize;
                let (mut envelope, mut amplified) = (Vec::new(), Vec::new());
                while envelope.len() < (rate * 0.6) as usize {
                    let start = envelope.len();
                    let gate = env.jack("gate");
                    for (i, sample) in gate.iter_mut().enumerate() {
                        *sample = if (on..off).contains(&(start + i)) {
                            1.0
                        } else {
                            0.0
                        };
                    }
                    env.step();
                    *vca.jack("cv") = env.outputs[0];
                    vca.step();
                    envelope.extend_from_slice(&env.outputs[0]);
                    amplified.extend_from_slice(&vca.outputs[0]);
                }
                let (env_rise, env_fall) = edges(&envelope, on, off);
                let (rise, fall) = edges(&amplified, on, off);
                // Within 1.2× of the envelope's own edges (and a frame).
                assert!(
                    rise * 5 <= env_rise * 6 + 5,
                    "{rate} {attack}: {rise} vs {env_rise}"
                );
                assert!(
                    fall * 5 <= env_fall * 6 + 5,
                    "{rate} {release}: {fall} vs {env_fall}"
                );
            }
        }
    }

    #[test]
    fn audio_rate_cv_makes_full_sidebands() {
        // 1 kHz through the VCA, its cv a 440 Hz sine at full depth: each
        // sideband should sit 6 dB under the carrier. Before: −29 dB.
        let rate = 48_000.0_f64;
        let mut vca = Bench::new(Kind::VCA);
        vca.knob("gain", 1.0).knob("bias", 0.0);
        let mut out = Vec::new();
        let mut frame = 0;
        while out.len() < 48_000 {
            let input = vca.jack("in");
            for (i, sample) in input.iter_mut().enumerate() {
                *sample =
                    (std::f64::consts::TAU * 1_000.0 * (frame + i) as f64 / rate).sin() as f32;
            }
            let cv = vca.jack("cv");
            for (i, sample) in cv.iter_mut().enumerate() {
                let t = (frame + i) as f64 / rate;
                *sample = 0.5f64.mul_add((std::f64::consts::TAU * 440.0 * t).sin(), 0.5) as f32;
            }
            vca.step();
            out.extend_from_slice(&vca.outputs[0]);
            frame += SUB_BLOCK;
        }
        let carrier = tone(&out, 1_000.0, rate);
        let sideband = db(tone(&out, 1_440.0, rate) / carrier);
        assert!((sideband + 6.02).abs() < 0.1, "{sideband:.2} dB");
    }

    #[test]
    fn modulating_the_gain_jack_does_not_zip() {
        // 440 Hz into the gain jack of a 1 kHz tone: only the two sidebands,
        // nothing at the sub-block rate. Before: −47 dBc spurs at 1.5 kHz ±.
        let rate = 48_000.0_f64;
        let mut vca = Bench::new(Kind::VCA);
        vca.knob("gain", 0.5).knob("bias", 0.0);
        let mut out = Vec::new();
        let mut frame = 0;
        while out.len() < 48_000 {
            let input = vca.jack("in");
            for (i, sample) in input.iter_mut().enumerate() {
                *sample =
                    (std::f64::consts::TAU * 1_000.0 * (frame + i) as f64 / rate).sin() as f32;
            }
            let jack = vca.jack("gain");
            for (i, sample) in jack.iter_mut().enumerate() {
                *sample = (std::f64::consts::TAU * 440.0 * (frame + i) as f64 / rate).sin() as f32;
            }
            vca.step();
            out.extend_from_slice(&vca.outputs[0]);
            frame += SUB_BLOCK;
        }
        let carrier = tone(&out, 1_000.0, rate);
        // Where the sub-block steps used to put their images.
        for spur in [500.0, 940.0, 1_060.0, 2_500.0, 2_940.0, 3_060.0] {
            let level = db(tone(&out, spur, rate) / carrier);
            assert!(level < -120.0, "{spur} Hz: {level:.1} dBc");
        }
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::VCA, 16.0);
        survives_nonsense(Kind::VCA);
    }
}
