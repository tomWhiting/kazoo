//! ADSR envelope with exponential per-sample smoothing.

use super::params::EnvelopeParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone)]
pub struct AdsrEnvelope {
    sample_rate: f32,
    value: f32,
    stage: EnvelopeStage,
}

impl AdsrEnvelope {
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            value: 0.0,
            stage: EnvelopeStage::Idle,
        }
    }

    pub const fn note_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
    }

    pub const fn note_off(&mut self) {
        if !matches!(self.stage, EnvelopeStage::Idle) {
            self.stage = EnvelopeStage::Release;
        }
    }

    pub const fn reset(&mut self) {
        self.value = 0.0;
        self.stage = EnvelopeStage::Idle;
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.stage, EnvelopeStage::Idle)
    }

    pub const fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub fn process(&mut self, params: &EnvelopeParams) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => {
                self.value = 0.0;
            }
            EnvelopeStage::Attack => {
                let coeff = time_coeff(params.attack.max(0.001), self.sample_rate);
                self.value += (1.0 - self.value) * coeff;
                if self.value >= 0.995 {
                    self.value = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                let target = params.sustain.clamp(0.0, 1.0);
                let coeff = time_coeff(params.decay.max(0.001), self.sample_rate);
                self.value += (target - self.value) * coeff;
                if (self.value - target).abs() < 0.001 {
                    self.value = target;
                    self.stage = EnvelopeStage::Sustain;
                }
            }
            EnvelopeStage::Sustain => {
                self.value = params.sustain.clamp(0.0, 1.0);
            }
            EnvelopeStage::Release => {
                let coeff = time_coeff(params.release.max(0.001), self.sample_rate);
                self.value += (0.0 - self.value) * coeff;
                if self.value <= 0.0005 {
                    self.value = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        self.value
    }
}

fn time_coeff(seconds: f32, sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (seconds * sample_rate.max(1.0))).exp()
}
