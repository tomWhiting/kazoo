//! Freeverb-style algorithmic reverb.
//!
//! Implements the classic Jezar Freeverb algorithm: 8 parallel Schroeder-Moorer
//! comb filters with low-pass damping, followed by 4 series allpass filters.
//! Tuning constants are from the original Freeverb source, scaled by
//! `sample_rate / 44100`.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

// ---------------------------------------------------------------------------
// Freeverb tuning constants (Jezar, at 44100 Hz)
// ---------------------------------------------------------------------------

const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const ALLPASS_FEEDBACK: f32 = 0.5;
const REFERENCE_SAMPLE_RATE: f32 = 44100.0;

// Freeverb uses these internal constants to map [0,1] params to coefficients.
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const SCALE_DAMP: f32 = 0.4;

// ---------------------------------------------------------------------------
// Internal comb filter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CombFilter {
    buffer: Vec<f32>,
    pos: usize,
    filter_store: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            pos: 0,
            filter_store: 0.0,
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.pos = 0;
        self.filter_store = 0.0;
    }

    #[inline]
    fn process(&mut self, input: f32, damp1: f32, damp2: f32, feedback: f32) -> f32 {
        let output = self.buffer[self.pos];

        // Low-pass filter in the feedback path (one-pole).
        self.filter_store = damp2.mul_add(self.filter_store, damp1 * output);
        // Flush tiny denormals.
        if self.filter_store.abs() < 1e-30 {
            self.filter_store = 0.0;
        }

        self.buffer[self.pos] = sanitize_sample(feedback.mul_add(self.filter_store, input));

        self.pos += 1;
        if self.pos >= self.buffer.len() {
            self.pos = 0;
        }

        output
    }
}

// ---------------------------------------------------------------------------
// Internal allpass filter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct AllpassFilter {
    buffer: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            pos: 0,
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.pos = 0;
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.pos];
        let output = sanitize_sample(-input + buffered);

        self.buffer[self.pos] = sanitize_sample(ALLPASS_FEEDBACK.mul_add(buffered, input));

        self.pos += 1;
        if self.pos >= self.buffer.len() {
            self.pos = 0;
        }

        output
    }
}

// ---------------------------------------------------------------------------
// Reverb
// ---------------------------------------------------------------------------

/// Freeverb-style algorithmic reverb processor.
#[derive(Debug)]
pub struct Reverb {
    sample_rate: f32,
    room_size: f32,
    damping: f32,
    mix: f32,
    combs: Vec<CombFilter>,
    allpasses: Vec<AllpassFilter>,
    // Derived coefficients cached for the audio loop.
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl Reverb {
    const PARAM_ROOM_SIZE: usize = 0;
    const PARAM_DAMPING: usize = 1;
    const PARAM_MIX: usize = 2;

    const ROOM_SIZE_MIN: f32 = 0.0;
    const ROOM_SIZE_MAX: f32 = 1.0;
    const ROOM_SIZE_DEFAULT: f32 = 0.5;

    const DAMPING_MIN: f32 = 0.0;
    const DAMPING_MAX: f32 = 1.0;
    const DAMPING_DEFAULT: f32 = 0.5;

    const MIX_MIN: f32 = 0.0;
    const MIX_MAX: f32 = 1.0;
    const MIX_DEFAULT: f32 = 0.33;

    /// Create a new Freeverb processor at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let scale = sr / REFERENCE_SAMPLE_RATE;

        let combs = COMB_TUNINGS
            .iter()
            .map(|&t| CombFilter::new((t as f32 * scale).round() as usize))
            .collect();

        let allpasses = ALLPASS_TUNINGS
            .iter()
            .map(|&t| AllpassFilter::new((t as f32 * scale).round() as usize))
            .collect();

        let mut rev = Self {
            sample_rate: sr,
            room_size: Self::ROOM_SIZE_DEFAULT,
            damping: Self::DAMPING_DEFAULT,
            mix: Self::MIX_DEFAULT,
            combs,
            allpasses,
            feedback: 0.0,
            damp1: 0.0,
            damp2: 0.0,
        };
        rev.update_coefficients();
        rev
    }

    /// Map user-facing [0,1] params to internal Freeverb coefficients.
    fn update_coefficients(&mut self) {
        self.feedback = self.room_size.mul_add(SCALE_ROOM, OFFSET_ROOM);
        self.damp1 = self.damping * SCALE_DAMP;
        self.damp2 = 1.0 - self.damp1;
    }

    fn param_infos() -> [ParamInfo; 3] {
        [
            ParamInfo {
                name: "Room Size".into(),
                min: Self::ROOM_SIZE_MIN,
                max: Self::ROOM_SIZE_MAX,
                default: Self::ROOM_SIZE_DEFAULT,
                unit: String::new(),
            },
            ParamInfo {
                name: "Damping".into(),
                min: Self::DAMPING_MIN,
                max: Self::DAMPING_MAX,
                default: Self::DAMPING_DEFAULT,
                unit: String::new(),
            },
            ParamInfo {
                name: "Mix".into(),
                min: Self::MIX_MIN,
                max: Self::MIX_MAX,
                default: Self::MIX_DEFAULT,
                unit: String::new(),
            },
        ]
    }
}

impl Processor for Reverb {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        let feedback = self.feedback;
        let damp1 = self.damp1;
        let damp2 = self.damp2;
        let mix = self.mix;

        for i in 0..len {
            let x = sanitize_sample(input[i]);

            // Sum output from all parallel comb filters.
            let mut comb_sum = 0.0_f32;
            for comb in &mut self.combs {
                comb_sum += comb.process(x, damp1, damp2, feedback);
            }

            // Sanitize comb sum before feeding to allpass chain.
            let mut out = sanitize_sample(comb_sum);
            for ap in &mut self.allpasses {
                out = ap.process(out);
            }

            // Mix dry/wet.
            output[i] = sanitize_sample(x.mul_add(1.0 - mix, out * mix));
        }
    }

    fn reset(&mut self) {
        for comb in &mut self.combs {
            comb.reset();
        }
        for ap in &mut self.allpasses {
            ap.reset();
        }
    }

    fn name(&self) -> &'static str {
        "Reverb"
    }

    fn param_count(&self) -> usize {
        3
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        let infos = Self::param_infos();
        infos.get(index).cloned()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_ROOM_SIZE => Some(self.room_size),
            Self::PARAM_DAMPING => Some(self.damping),
            Self::PARAM_MIX => Some(self.mix),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) -> Result<()> {
        let infos = Self::param_infos();
        let info = infos
            .get(index)
            .ok_or_else(|| Error::Config(format!("invalid param index {index}")))?;
        let clamped = info.clamp(value);

        match index {
            Self::PARAM_ROOM_SIZE => self.room_size = clamped,
            Self::PARAM_DAMPING => self.damping = clamped,
            Self::PARAM_MIX => self.mix = clamped,
            _ => unreachable!(),
        }

        self.update_coefficients();
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        let scale = sr / REFERENCE_SAMPLE_RATE;

        self.combs = COMB_TUNINGS
            .iter()
            .map(|&t| CombFilter::new((t as f32 * scale).round() as usize))
            .collect();

        self.allpasses = ALLPASS_TUNINGS
            .iter()
            .map(|&t| AllpassFilter::new((t as f32 * scale).round() as usize))
            .collect();

        self.update_coefficients();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverb_impulse_has_tail() {
        let mut reverb = Reverb::new(44100.0);
        reverb.set_param(Reverb::PARAM_ROOM_SIZE, 0.8).unwrap();
        reverb.set_param(Reverb::PARAM_MIX, 1.0).unwrap();

        let len = 8192;
        let mut input = vec![0.0_f32; len];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; len];

        reverb.process(&input, &mut output);

        // The reverb tail should have energy well beyond the initial impulse.
        let tail_energy: f32 = output[2048..].iter().map(|s| s * s).sum();
        assert!(
            tail_energy > 1e-6,
            "reverb tail should have energy, got {tail_energy}"
        );
    }

    #[test]
    fn reverb_handles_nan() {
        let mut reverb = Reverb::new(44100.0);
        let input = [f32::NAN, f32::INFINITY, 0.5, 0.0];
        let mut output = [0.0_f32; 4];
        reverb.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn reverb_reset_silences_tail() {
        let mut reverb = Reverb::new(44100.0);
        reverb.set_param(Reverb::PARAM_MIX, 1.0).unwrap();

        let mut input = vec![0.0_f32; 1024];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; 1024];
        reverb.process(&input, &mut output);

        reverb.reset();

        let silence = vec![0.0_f32; 4096];
        let mut out2 = vec![0.0_f32; 4096];
        reverb.process(&silence, &mut out2);

        let energy: f32 = out2.iter().map(|s| s * s).sum();
        assert!(
            energy < 1e-10,
            "after reset + silence, output energy should be ~0, got {energy}"
        );
    }

    #[test]
    fn reverb_empty_buffers() {
        let mut reverb = Reverb::new(44100.0);
        reverb.process(&[], &mut []);
    }

    #[test]
    fn reverb_param_count() {
        let reverb = Reverb::new(44100.0);
        assert_eq!(reverb.param_count(), 3);
    }

    #[test]
    fn reverb_fully_dry_passes_input() {
        let mut reverb = Reverb::new(44100.0);
        reverb.set_param(Reverb::PARAM_MIX, 0.0).unwrap();

        let input = [0.5, -0.3, 0.8, -0.1, 0.0];
        let mut output = [0.0_f32; 5];
        reverb.process(&input, &mut output);

        // mix=0 means output = dry * 1.0 + wet * 0.0 = input.
        for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (inp - out).abs() < 1e-6,
                "dry pass: [{i}] expected {inp}, got {out}"
            );
        }
    }

    #[test]
    fn reverb_room_size_extremes() {
        let sr = 44100.0;
        let input: Vec<f32> = {
            let mut v = vec![0.0_f32; 4096];
            v[0] = 1.0;
            v
        };

        for room_size in [0.0, 1.0] {
            let mut reverb = Reverb::new(sr);
            reverb
                .set_param(Reverb::PARAM_ROOM_SIZE, room_size)
                .unwrap();
            reverb.set_param(Reverb::PARAM_MIX, 1.0).unwrap();

            let mut output = vec![0.0_f32; 4096];
            reverb.process(&input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() < 100.0,
                    "room_size={room_size}: output[{i}] = {s}"
                );
            }
        }
    }

    #[test]
    fn reverb_damping_extremes() {
        let sr = 44100.0;
        let input: Vec<f32> = {
            let mut v = vec![0.0_f32; 4096];
            v[0] = 1.0;
            v
        };

        for damping in [0.0, 1.0] {
            let mut reverb = Reverb::new(sr);
            reverb.set_param(Reverb::PARAM_DAMPING, damping).unwrap();
            reverb.set_param(Reverb::PARAM_MIX, 1.0).unwrap();

            let mut output = vec![0.0_f32; 4096];
            reverb.process(&input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() < 100.0,
                    "damping={damping}: output[{i}] = {s}"
                );
            }
        }
    }

    #[test]
    fn reverb_stability_with_noise() {
        let mut reverb = Reverb::new(44100.0);
        reverb.set_param(Reverb::PARAM_ROOM_SIZE, 0.9).unwrap();
        reverb.set_param(Reverb::PARAM_MIX, 0.5).unwrap();

        let mut rng: u32 = 0xFACE_FEED;
        let noise: Vec<f32> = (0..8192)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();
        let mut output = vec![0.0_f32; 8192];
        reverb.process(&noise, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.is_finite() && s.abs() < 100.0,
                "noise stability: output[{i}] = {s}"
            );
        }
    }

    #[test]
    fn reverb_sample_rate_change() {
        let mut reverb = Reverb::new(44100.0);
        let mut input = vec![0.0_f32; 1024];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; 1024];
        reverb.process(&input, &mut output);

        // Change to 96 kHz — should rebuild comb/allpass buffers.
        reverb.set_sample_rate(96000.0);

        let silence = vec![0.0_f32; 2048];
        let mut out2 = vec![0.0_f32; 2048];
        reverb.set_param(Reverb::PARAM_MIX, 1.0).unwrap();
        reverb.process(&silence, &mut out2);

        // After SR change (which rebuilds buffers), silence in = silence out.
        for (i, &s) in out2.iter().enumerate() {
            assert!(
                s.abs() < 1e-10,
                "after SR change, output[{i}] should be ~0, got {s}"
            );
        }
    }

    #[test]
    fn reverb_low_sample_rate() {
        // 8 kHz — comb/allpass buffers should scale correctly.
        let mut reverb = Reverb::new(8000.0);
        reverb.set_param(Reverb::PARAM_MIX, 0.5).unwrap();

        let mut input = vec![0.0_f32; 2048];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; 2048];
        reverb.process(&input, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "8 kHz reverb: output[{i}] = {s}");
        }
    }

    #[test]
    fn reverb_all_param_info_names() {
        let reverb = Reverb::new(44100.0);
        for i in 0..reverb.param_count() {
            let info = reverb.param_info(i).unwrap();
            assert!(!info.name.is_empty(), "param {i} has empty name");
        }
    }

    #[test]
    fn reverb_invalid_param_index() {
        let mut reverb = Reverb::new(44100.0);
        assert!(reverb.set_param(99, 0.0).is_err());
        assert!(reverb.param_value(99).is_none());
        assert!(reverb.param_info(99).is_none());
    }

    #[test]
    fn reverb_param_values_roundtrip() {
        let mut reverb = Reverb::new(44100.0);
        reverb.set_param(Reverb::PARAM_ROOM_SIZE, 0.7).unwrap();
        reverb.set_param(Reverb::PARAM_DAMPING, 0.3).unwrap();
        reverb.set_param(Reverb::PARAM_MIX, 0.8).unwrap();

        assert!((reverb.param_value(0).unwrap() - 0.7).abs() < f32::EPSILON);
        assert!((reverb.param_value(1).unwrap() - 0.3).abs() < f32::EPSILON);
        assert!((reverb.param_value(2).unwrap() - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn reverb_name() {
        let reverb = Reverb::new(44100.0);
        assert_eq!(reverb.name(), "Reverb");
    }

    #[test]
    fn reverb_param_clamping() {
        let mut reverb = Reverb::new(44100.0);

        // Room size above max (1.0) should clamp.
        reverb.set_param(Reverb::PARAM_ROOM_SIZE, 5.0).unwrap();
        assert!((reverb.param_value(Reverb::PARAM_ROOM_SIZE).unwrap() - 1.0).abs() < f32::EPSILON);

        // Mix below min (0.0) should clamp.
        reverb.set_param(Reverb::PARAM_MIX, -1.0).unwrap();
        assert!(reverb.param_value(Reverb::PARAM_MIX).unwrap().abs() < f32::EPSILON);
    }

    #[test]
    fn reverb_long_sustained_processing() {
        let mut reverb = Reverb::new(44100.0);
        reverb.set_param(Reverb::PARAM_ROOM_SIZE, 1.0).unwrap();
        reverb.set_param(Reverb::PARAM_MIX, 0.5).unwrap();

        let input: Vec<f32> = (0..256)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut output = vec![0.0_f32; 256];

        // Process 100 blocks at max room size — should stay stable.
        for _ in 0..100 {
            reverb.process(&input, &mut output);
            for &s in &output {
                assert!(s.is_finite() && s.abs() < 100.0);
            }
        }
    }
}
