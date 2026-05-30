//! Procedural BBD-style chorus approximation.

use std::f32::consts::TAU;

use super::params::{ChorusMode, ChorusParams};

#[derive(Debug, Clone)]
pub struct JunoChorus {
    sample_rate: f32,
    buffer: Vec<f32>,
    pos: usize,
    phase_a: f32,
    phase_b: f32,
    noise_state: u32,
}

impl JunoChorus {
    const MAX_DELAY_SECONDS: f32 = 0.04;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let len = (sample_rate.max(1.0) * Self::MAX_DELAY_SECONDS) as usize + 4;
        Self {
            sample_rate: sample_rate.max(1.0),
            buffer: vec![0.0; len],
            pos: 0,
            phase_a: 0.0,
            phase_b: 0.37,
            noise_state: 0x00c0_ffee,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        let len = (self.sample_rate * Self::MAX_DELAY_SECONDS) as usize + 4;
        self.buffer.resize(len, 0.0);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.pos = 0;
        self.phase_a = 0.0;
        self.phase_b = 0.37;
    }

    pub fn process(&mut self, input: f32, params: &ChorusParams) -> f32 {
        if matches!(params.mode, ChorusMode::Off) {
            return input;
        }

        self.buffer[self.pos] = input;
        let (rate_a, depth_a, rate_b, depth_b): (f32, f32, f32, f32) = match params.mode {
            ChorusMode::Off => (0.0, 0.0, 0.0, 0.0),
            ChorusMode::I => (0.38, 0.0035, 0.51, 0.0025),
            ChorusMode::Ii => (0.82, 0.0055, 1.03, 0.0045),
            ChorusMode::IPlusIi => (0.62, 0.0075, 0.93, 0.0065),
        };

        let delay_a = depth_a.mul_add((self.phase_a * TAU).sin(), 0.010);
        let delay_b = depth_b.mul_add((self.phase_b * TAU).sin(), 0.014);
        let wet = 0.5 * (self.read_delay(delay_a) + self.read_delay(delay_b));
        self.phase_a = (self.phase_a + rate_a / self.sample_rate).fract();
        self.phase_b = (self.phase_b + rate_b / self.sample_rate).fract();
        self.pos = (self.pos + 1) % self.buffer.len();

        let hiss = self.white_noise() * params.noise.clamp(0.0, 0.08);
        let mix = params.mix.clamp(0.0, 1.0);
        input.mul_add(1.0 - mix, wet * mix) + hiss
    }

    fn read_delay(&self, seconds: f32) -> f32 {
        let delay_samples = seconds * self.sample_rate;
        let read_pos = self.pos as f32 - delay_samples;
        let len = self.buffer.len() as f32;
        let wrapped = read_pos.rem_euclid(len);
        let len_usize = self.buffer.len();
        let idx0_float = wrapped.floor();
        let idx0 = (idx0_float as usize).min(len_usize - 1);
        let idx1 = (idx0 + 1) % len_usize;
        let frac = (wrapped - idx0_float).clamp(0.0, 1.0);
        self.buffer[idx0].mul_add(1.0 - frac, self.buffer[idx1] * frac)
    }

    fn white_noise(&mut self) -> f32 {
        self.noise_state = self
            .noise_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let unit = (self.noise_state >> 8) as f32 / 16_777_216.0;
        unit * 2.0 - 1.0
    }
}
