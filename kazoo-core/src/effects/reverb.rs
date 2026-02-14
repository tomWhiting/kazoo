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
        let output = -input + buffered;

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
    width: f32,
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
    const PARAM_WIDTH: usize = 3;

    const ROOM_SIZE_MIN: f32 = 0.0;
    const ROOM_SIZE_MAX: f32 = 1.0;
    const ROOM_SIZE_DEFAULT: f32 = 0.5;

    const DAMPING_MIN: f32 = 0.0;
    const DAMPING_MAX: f32 = 1.0;
    const DAMPING_DEFAULT: f32 = 0.5;

    const MIX_MIN: f32 = 0.0;
    const MIX_MAX: f32 = 1.0;
    const MIX_DEFAULT: f32 = 0.33;

    const WIDTH_MIN: f32 = 0.0;
    const WIDTH_MAX: f32 = 1.0;
    const WIDTH_DEFAULT: f32 = 1.0;

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
            width: Self::WIDTH_DEFAULT,
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

    fn param_infos() -> [ParamInfo; 4] {
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
            ParamInfo {
                name: "Width".into(),
                min: Self::WIDTH_MIN,
                max: Self::WIDTH_MAX,
                default: Self::WIDTH_DEFAULT,
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

            // Pass through series allpass filters.
            let mut out = comb_sum;
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
        4
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
            Self::PARAM_WIDTH => Some(self.width),
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
            Self::PARAM_WIDTH => self.width = clamped,
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
        assert_eq!(reverb.param_count(), 4);
    }
}
