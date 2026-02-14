//! Chorus / flanger effect via a sine-modulated delay line.
//!
//! An LFO (sine wave) modulates the read position within a circular delay
//! buffer. Linear interpolation is used for fractional delay positions.
//! Feedback allows self-oscillation for flanging effects.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};
use std::f32::consts::PI;

/// Chorus / flanger processor.
#[derive(Debug)]
pub struct Chorus {
    sample_rate: f32,
    rate_hz: f32,
    depth_ms: f32,
    mix: f32,
    feedback: f32,
    buffer: Vec<f32>,
    write_pos: usize,
    lfo_phase: f32,
}

impl Chorus {
    const PARAM_RATE: usize = 0;
    const PARAM_DEPTH: usize = 1;
    const PARAM_MIX: usize = 2;
    const PARAM_FEEDBACK: usize = 3;

    const RATE_MIN: f32 = 0.1;
    const RATE_MAX: f32 = 10.0;
    const RATE_DEFAULT: f32 = 1.5;

    const DEPTH_MIN: f32 = 0.5;
    const DEPTH_MAX: f32 = 20.0;
    const DEPTH_DEFAULT: f32 = 5.0;

    const MIX_MIN: f32 = 0.0;
    const MIX_MAX: f32 = 1.0;
    const MIX_DEFAULT: f32 = 0.5;

    const FEEDBACK_MIN: f32 = 0.0;
    const FEEDBACK_MAX: f32 = 0.7;
    const FEEDBACK_DEFAULT: f32 = 0.0;

    /// Create a new chorus effect at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        // Buffer must accommodate the maximum depth plus some margin.
        let max_delay_samples = (Self::DEPTH_MAX / 1000.0 * sr).ceil() as usize + 2;
        Self {
            sample_rate: sr,
            rate_hz: Self::RATE_DEFAULT,
            depth_ms: Self::DEPTH_DEFAULT,
            mix: Self::MIX_DEFAULT,
            feedback: Self::FEEDBACK_DEFAULT,
            buffer: vec![0.0; max_delay_samples],
            write_pos: 0,
            lfo_phase: 0.0,
        }
    }

    /// Read from the delay buffer at a fractional position using linear interpolation.
    fn read_interpolated(&self, delay_samples: f32) -> f32 {
        let buf_len = self.buffer.len();
        if buf_len == 0 {
            return 0.0;
        }

        // Compute the fractional read position relative to write_pos.
        let read_f = self.write_pos as f32 - delay_samples;
        let read_floor = read_f.floor();
        let frac = read_f - read_floor;

        let idx0 = read_floor.rem_euclid(buf_len as f32) as usize;
        let idx1 = (idx0 + 1) % buf_len;

        let s0 = self.buffer[idx0];
        let s1 = self.buffer[idx1];

        // Linear interpolation.
        (1.0 - frac).mul_add(s0, frac * s1)
    }

    fn param_infos() -> [ParamInfo; 4] {
        [
            ParamInfo {
                name: "Rate".into(),
                min: Self::RATE_MIN,
                max: Self::RATE_MAX,
                default: Self::RATE_DEFAULT,
                unit: "Hz".into(),
            },
            ParamInfo {
                name: "Depth".into(),
                min: Self::DEPTH_MIN,
                max: Self::DEPTH_MAX,
                default: Self::DEPTH_DEFAULT,
                unit: "ms".into(),
            },
            ParamInfo {
                name: "Mix".into(),
                min: Self::MIX_MIN,
                max: Self::MIX_MAX,
                default: Self::MIX_DEFAULT,
                unit: String::new(),
            },
            ParamInfo {
                name: "Feedback".into(),
                min: Self::FEEDBACK_MIN,
                max: Self::FEEDBACK_MAX,
                default: Self::FEEDBACK_DEFAULT,
                unit: String::new(),
            },
        ]
    }
}

impl Processor for Chorus {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        if self.buffer.is_empty() {
            for i in 0..len {
                output[i] = sanitize_sample(input[i]);
            }
            return;
        }

        let buf_len = self.buffer.len();
        let depth_samples = (self.depth_ms / 1000.0) * self.sample_rate;
        let phase_inc = 2.0 * PI * self.rate_hz / self.sample_rate;
        let feedback = self.feedback;
        let mix = self.mix;

        for i in 0..len {
            let x = sanitize_sample(input[i]);

            // LFO produces a value in [0, 1] controlling the delay offset.
            let lfo = (self.lfo_phase.sin() + 1.0) * 0.5;
            self.lfo_phase += phase_inc;
            if self.lfo_phase >= 2.0 * PI {
                self.lfo_phase -= 2.0 * PI;
            }

            // Read modulated delay.
            let delay = lfo.mul_add(depth_samples, 1.0);
            let delayed = self.read_interpolated(delay);

            // Write to buffer with optional feedback.
            self.buffer[self.write_pos] = sanitize_sample(feedback.mul_add(delayed, x));

            self.write_pos += 1;
            if self.write_pos >= buf_len {
                self.write_pos = 0;
            }

            // Mix dry/wet.
            output[i] = sanitize_sample(x.mul_add(1.0 - mix, delayed * mix));
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.lfo_phase = 0.0;
    }

    fn name(&self) -> &'static str {
        "Chorus"
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
            Self::PARAM_RATE => Some(self.rate_hz),
            Self::PARAM_DEPTH => Some(self.depth_ms),
            Self::PARAM_MIX => Some(self.mix),
            Self::PARAM_FEEDBACK => Some(self.feedback),
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
            Self::PARAM_RATE => self.rate_hz = clamped,
            Self::PARAM_DEPTH => self.depth_ms = clamped,
            Self::PARAM_MIX => self.mix = clamped,
            Self::PARAM_FEEDBACK => self.feedback = clamped,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        let max_delay_samples = (Self::DEPTH_MAX / 1000.0 * sr).ceil() as usize + 2;
        self.buffer = vec![0.0; max_delay_samples];
        self.write_pos = 0;
        self.lfo_phase = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chorus_modifies_sine() {
        let sr = 44100.0;
        let mut chorus = Chorus::new(sr);
        chorus.set_param(Chorus::PARAM_MIX, 0.5).unwrap();

        let len = 2048;
        let freq = 440.0;
        let input: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; len];

        chorus.process(&input, &mut output);

        // The output should differ from a pure sine (chorus adds modulation).
        let diff_energy: f32 = input
            .iter()
            .zip(output.iter())
            .skip(256) // skip transient
            .map(|(a, b)| {
                let d = a - b;
                d * d
            })
            .sum();

        assert!(
            diff_energy > 0.01,
            "chorus should modify the signal, diff_energy={diff_energy}"
        );
    }

    #[test]
    fn chorus_handles_nan() {
        let mut chorus = Chorus::new(44100.0);
        let input = [f32::NAN, f32::INFINITY, 0.5, 0.0];
        let mut output = [0.0_f32; 4];
        chorus.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn chorus_reset() {
        let mut chorus = Chorus::new(44100.0);
        let input = [1.0; 128];
        let mut output = [0.0_f32; 128];
        chorus.process(&input, &mut output);
        chorus.reset();

        let silence = [0.0_f32; 512];
        let mut out2 = [0.0_f32; 512];
        chorus.set_param(Chorus::PARAM_MIX, 1.0).unwrap();
        chorus.set_param(Chorus::PARAM_FEEDBACK, 0.0).unwrap();
        chorus.process(&silence, &mut out2);

        for (i, &s) in out2.iter().enumerate() {
            assert!(
                s.abs() < 1e-6,
                "after reset, output[{i}] should be ~0, got {s}"
            );
        }
    }

    #[test]
    fn chorus_empty_buffers() {
        let mut chorus = Chorus::new(44100.0);
        chorus.process(&[], &mut []);
    }

    #[test]
    fn chorus_param_count() {
        let chorus = Chorus::new(44100.0);
        assert_eq!(chorus.param_count(), 4);
    }
}
