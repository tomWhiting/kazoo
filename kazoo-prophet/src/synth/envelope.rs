//! ADSR envelope generator.

/// Envelope stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Linear ADSR envelope.
#[derive(Debug, Clone)]
pub struct AdsrEnvelope {
    value: f32,
    release_start: f32,
    stage: EnvelopeStage,
    sample_rate: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl AdsrEnvelope {
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            value: 0.0,
            release_start: 0.0,
            stage: EnvelopeStage::Idle,
            sample_rate,
            attack: 0.01,
            decay: 0.25,
            sustain: 0.65,
            release: 0.4,
        }
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub const fn gate_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
    }

    pub const fn gate_off(&mut self) {
        self.release_start = self.value;
        self.stage = EnvelopeStage::Release;
    }

    pub const fn reset(&mut self) {
        self.value = 0.0;
        self.release_start = 0.0;
        self.stage = EnvelopeStage::Idle;
    }

    #[must_use]
    pub const fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        let sr = self.sample_rate.max(1.0);
        match self.stage {
            EnvelopeStage::Idle => self.value = 0.0,
            EnvelopeStage::Attack => {
                self.value += 1.0 / (self.attack.max(0.001) * sr);
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                let target = self.sustain.clamp(0.0, 1.0);
                self.value -= (1.0 - target) / (self.decay.max(0.001) * sr);
                if self.value <= target {
                    self.value = target;
                    self.stage = EnvelopeStage::Sustain;
                }
            }
            EnvelopeStage::Sustain => {
                self.value = self.sustain.clamp(0.0, 1.0);
            }
            EnvelopeStage::Release => {
                self.value -= self.release_start / (self.release.max(0.001) * sr);
                if self.value <= 0.0 {
                    self.value = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        if !self.value.is_finite() {
            self.value = 0.0;
            self.stage = EnvelopeStage::Idle;
        }
        self.value.clamp(0.0, 1.0)
    }
}
