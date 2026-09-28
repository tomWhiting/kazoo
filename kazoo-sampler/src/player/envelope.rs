//! A voice's amp envelope: linear attack, exponential decay and release.

use super::params::EnvRates;

/// Below this the release is over, about -80 dB.
const FLOOR: f32 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// Attack, decay, sustain, release.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Envelope {
    stage: Stage,
    level: f32,
}

impl Envelope {
    pub(crate) const fn new() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
        }
    }

    /// Start the attack from wherever the level is now, so a retrigger
    /// never jumps.
    pub(crate) const fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    /// Start the release.
    pub(crate) fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    /// Whether the envelope has finished.
    pub(crate) fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Silence at once.
    pub(crate) const fn reset(&mut self) {
        *self = Self::new();
    }

    /// The next level, 0 to 1.
    pub(crate) fn next(&mut self, rates: &EnvRates) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level += rates.attack_step;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            // Decay settles on the sustain level and then follows the
            // sustain knob smoothly at the same rate.
            Stage::Decay => {
                self.level = (self.level - rates.sustain).mul_add(rates.decay, rates.sustain);
            }
            Stage::Release => {
                self.level *= rates.release;
                if self.level < FLOOR {
                    self.reset();
                }
            }
        }
        if !self.level.is_finite() {
            self.reset();
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: EnvRates = EnvRates {
        attack_step: 0.01,
        decay: 0.99,
        sustain: 0.5,
        release: 0.99,
    };

    #[test]
    fn it_runs_through_its_stages() {
        let mut env = Envelope::new();
        assert!(env.is_idle());
        env.gate_on();
        let mut peak = 0.0f32;
        for _ in 0..100 {
            peak = peak.max(env.next(&RATES));
        }
        assert!((peak - 1.0).abs() < 1e-6);
        for _ in 0..2_000 {
            env.next(&RATES);
        }
        assert!((env.next(&RATES) - 0.5).abs() < 1e-3);
        env.gate_off();
        let mut last = 1.0;
        for _ in 0..2_000 {
            let now = env.next(&RATES);
            assert!(now <= last);
            last = now;
        }
        assert!(env.is_idle());
        assert!(env.next(&RATES).abs() < f32::EPSILON);
    }

    #[test]
    fn a_retrigger_starts_from_where_it_is() {
        let mut env = Envelope::new();
        env.gate_on();
        for _ in 0..50 {
            env.next(&RATES);
        }
        env.gate_off();
        let before = env.next(&RATES);
        env.gate_on();
        let after = env.next(&RATES);
        assert!((after - before).abs() <= RATES.attack_step + 1e-6);
    }
}
