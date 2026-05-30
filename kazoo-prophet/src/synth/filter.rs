//! Resonant low-pass filter inspired by the CEM3320-era polysynth sound.

/// Four one-pole stages with feedback resonance and soft saturation.
#[derive(Debug, Clone)]
pub struct CurtisLowPass {
    stages: [f32; 4],
    sample_rate: f32,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub key_track: f32,
}

impl CurtisLowPass {
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            stages: [0.0; 4],
            sample_rate,
            cutoff_hz: 1600.0,
            resonance: 0.3,
            key_track: 0.35,
        }
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub const fn reset(&mut self) {
        self.stages = [0.0; 4];
    }

    #[inline]
    pub fn process(&mut self, input: f32, cutoff_hz: f32, resonance: f32) -> f32 {
        let sr = self.sample_rate.max(1.0);
        let cutoff = cutoff_hz.clamp(20.0, sr * 0.45);
        let g = (1.0 - (-2.0 * std::f32::consts::PI * cutoff / sr).exp()).clamp(0.0, 1.0);
        let res = resonance.clamp(0.0, 0.92);
        let mut x = (self.stages[3] * res).mul_add(-3.8, input).tanh();

        for stage in &mut self.stages {
            *stage += g * (x - *stage);
            x = stage.tanh();
        }

        if self.stages[3].is_finite() {
            self.stages[3]
        } else {
            self.reset();
            0.0
        }
    }
}
