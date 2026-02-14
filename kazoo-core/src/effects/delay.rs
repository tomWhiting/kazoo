//! Delay effect with feedback and dry/wet mix.
//!
//! Uses a circular buffer delay line. The buffer is pre-allocated for the
//! maximum delay time and resized on sample rate changes.

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

/// Circular buffer delay line with feedback and dry/wet mixing.
#[derive(Debug)]
pub struct Delay {
    sample_rate: f32,
    time_ms: f32,
    feedback: f32,
    mix: f32,
    buffer: Vec<f32>,
    write_pos: usize,
}

impl Delay {
    const PARAM_TIME_MS: usize = 0;
    const PARAM_FEEDBACK: usize = 1;
    const PARAM_MIX: usize = 2;

    const TIME_MIN: f32 = 0.0;
    const TIME_MAX: f32 = 2000.0;
    const TIME_DEFAULT: f32 = 250.0;

    const FEEDBACK_MIN: f32 = 0.0;
    const FEEDBACK_MAX: f32 = 0.95;
    const FEEDBACK_DEFAULT: f32 = 0.3;

    const MIX_MIN: f32 = 0.0;
    const MIX_MAX: f32 = 1.0;
    const MIX_DEFAULT: f32 = 0.5;

    /// Create a new delay effect at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let buf_size = Self::buffer_size_for(Self::TIME_MAX, sr);
        Self {
            sample_rate: sr,
            time_ms: Self::TIME_DEFAULT,
            feedback: Self::FEEDBACK_DEFAULT,
            mix: Self::MIX_DEFAULT,
            buffer: vec![0.0; buf_size],
            write_pos: 0,
        }
    }

    fn buffer_size_for(max_time_ms: f32, sample_rate: f32) -> usize {
        ((max_time_ms / 1000.0) * sample_rate).ceil().max(1.0) as usize
    }

    /// Compute the current delay in samples.
    fn delay_samples(&self) -> usize {
        let samples = (self.time_ms / 1000.0) * self.sample_rate;
        (samples.round() as usize).min(self.buffer.len().saturating_sub(1))
    }

    fn param_infos() -> [ParamInfo; 3] {
        [
            ParamInfo {
                name: "Delay Time".into(),
                min: Self::TIME_MIN,
                max: Self::TIME_MAX,
                default: Self::TIME_DEFAULT,
                unit: "ms".into(),
            },
            ParamInfo {
                name: "Feedback".into(),
                min: Self::FEEDBACK_MIN,
                max: Self::FEEDBACK_MAX,
                default: Self::FEEDBACK_DEFAULT,
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

impl Processor for Delay {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        if self.buffer.is_empty() {
            for i in 0..len {
                output[i] = sanitize_sample(input[i]);
            }
            return;
        }

        let buf_len = self.buffer.len();
        let delay = self.delay_samples();
        let feedback = self.feedback;
        let mix = self.mix;

        for i in 0..len {
            let x = sanitize_sample(input[i]);

            // Read from the delay line.
            let read_pos = if self.write_pos >= delay {
                self.write_pos - delay
            } else {
                buf_len - (delay - self.write_pos)
            };

            let delayed = self.buffer[read_pos];

            // Write new sample into the delay line (input + feedback * delayed).
            let write_val = sanitize_sample(feedback.mul_add(delayed, x));
            self.buffer[self.write_pos] = write_val;

            // Advance write position.
            self.write_pos += 1;
            if self.write_pos >= buf_len {
                self.write_pos = 0;
            }

            // Mix dry/wet: output = dry * (1 - mix) + wet * mix
            let dry = x;
            let wet = delayed;
            output[i] = sanitize_sample(dry.mul_add(1.0 - mix, wet * mix));
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }

    fn name(&self) -> &'static str {
        "Delay"
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
            Self::PARAM_TIME_MS => Some(self.time_ms),
            Self::PARAM_FEEDBACK => Some(self.feedback),
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
            Self::PARAM_TIME_MS => self.time_ms = clamped,
            Self::PARAM_FEEDBACK => self.feedback = clamped,
            Self::PARAM_MIX => self.mix = clamped,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        let new_size = Self::buffer_size_for(Self::TIME_MAX, sr);
        self.buffer = vec![0.0; new_size];
        self.write_pos = 0;
    }

    fn latency_samples(&self) -> usize {
        self.delay_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_appears_at_correct_delay() {
        let sr = 44100.0;
        let mut delay = Delay::new(sr);
        // Set delay to 10ms, no feedback, full wet.
        delay.set_param(Delay::PARAM_TIME_MS, 10.0).unwrap();
        delay.set_param(Delay::PARAM_FEEDBACK, 0.0).unwrap();
        delay.set_param(Delay::PARAM_MIX, 1.0).unwrap();

        let expected_delay_samples = (0.01 * sr).round() as usize;

        // Create an impulse followed by silence.
        let len = expected_delay_samples + 64;
        let mut input = vec![0.0_f32; len];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; len];

        delay.process(&input, &mut output);

        // At 100% wet, the delayed copy of the impulse should appear.
        assert!(
            output[expected_delay_samples].abs() > 0.5,
            "impulse should appear at sample {expected_delay_samples}, got {}",
            output[expected_delay_samples]
        );
        // First sample should be near zero (no input yet in delay line).
        assert!(
            output[0].abs() < 1e-6,
            "first sample should be ~0 (nothing in delay line yet)"
        );
    }

    #[test]
    fn feedback_creates_repeated_echoes() {
        let sr = 44100.0;
        let mut delay = Delay::new(sr);
        delay.set_param(Delay::PARAM_TIME_MS, 10.0).unwrap();
        delay.set_param(Delay::PARAM_FEEDBACK, 0.5).unwrap();
        delay.set_param(Delay::PARAM_MIX, 1.0).unwrap();

        let d = (0.01 * sr).round() as usize;
        let len = d * 4;
        let mut input = vec![0.0_f32; len];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; len];

        delay.process(&input, &mut output);

        // First echo at d, second at 2*d (attenuated by feedback).
        let first_echo = output[d].abs();
        let second_echo = output[2 * d].abs();
        assert!(first_echo > 0.5);
        assert!(second_echo > 0.1);
        assert!(second_echo < first_echo);
    }

    #[test]
    fn delay_handles_nan_input() {
        let mut delay = Delay::new(44100.0);
        let input = [f32::NAN, f32::INFINITY, 0.5, 0.0];
        let mut output = [0.0_f32; 4];
        delay.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn delay_reset_clears_buffer() {
        let mut delay = Delay::new(44100.0);
        let input = [1.0; 64];
        let mut output = [0.0_f32; 64];
        delay.process(&input, &mut output);
        delay.reset();

        // After reset, the delay line should be empty.
        let silence = [0.0_f32; 512];
        let mut out2 = [0.0_f32; 512];
        delay.set_param(Delay::PARAM_MIX, 1.0).unwrap();
        delay.process(&silence, &mut out2);

        for (i, &s) in out2.iter().enumerate() {
            assert!(
                s.abs() < 1e-10,
                "after reset, output[{i}] should be ~0, got {s}"
            );
        }
    }

    #[test]
    fn delay_empty_buffers_no_panic() {
        let mut delay = Delay::new(44100.0);
        delay.process(&[], &mut []);
    }
}
