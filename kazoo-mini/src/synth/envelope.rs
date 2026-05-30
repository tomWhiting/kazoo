//! ADSR envelope with Amount control.
//!
//! Filter Contour: ADSR + Amount (scales envelope-to-cutoff modulation).
//! Loudness Contour: ADSR (with optional release).
//! Legato: new note while key held does NOT retrigger — pitch changes,
//! amplitude stays in sustain phase.

// ---------------------------------------------------------------------------
// Envelope stages
// ---------------------------------------------------------------------------

/// Current stage of the ADSR envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

// ---------------------------------------------------------------------------
// ADSR Envelope
// ---------------------------------------------------------------------------

/// ADSR envelope generator.
///
/// All time parameters are in seconds. The envelope output ranges from 0.0
/// to 1.0. An external Amount parameter scales how much the envelope
/// modulates its target (cutoff, amplitude, etc.).
///
/// Attack, decay, and release use exponential curves for natural-sounding
/// contours. Attack approaches a value above 1.0 so that the exponential
/// actually reaches 1.0 in finite time, then decay begins.
#[derive(Debug)]
pub struct AdsrEnvelope {
    /// Attack time in seconds (0.001 to 10.0).
    pub attack: f32,
    /// Decay time in seconds (0.001 to 10.0).
    pub decay: f32,
    /// Sustain level (0.0 to 1.0).
    pub sustain: f32,
    /// Release time in seconds (0.001 to 10.0).
    pub release: f32,

    // Internal state
    stage: Stage,
    value: f32,
    sample_rate: f32,
    /// Coefficient for exponential attack curve.
    attack_coeff: f32,
    /// Coefficient for exponential decay curve.
    decay_coeff: f32,
    /// Coefficient for exponential release curve.
    release_coeff: f32,
    /// Level at which release began (for smooth release from any level).
    release_level: f32,
}

/// Overshoot target for attack phase — exponential towards ~1.37 means
/// it passes through 1.0 at approximately the specified attack time.
const ATTACK_TARGET: f32 = 1.37;

/// Time constant multiplier. A coefficient of exp(-1/(tau*sr)) gives a
/// ~63% approach in `tau` seconds.
fn exp_coeff(time_secs: f32, sample_rate: f32) -> f32 {
    if time_secs <= 0.0 || sample_rate <= 0.0 {
        return 0.0; // instant
    }
    (-1.0 / (time_secs * sample_rate)).exp()
}

impl AdsrEnvelope {
    /// Create a new envelope with sensible defaults.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Self {
            attack: 0.005,
            decay: 0.2,
            sustain: 0.6,
            release: 0.1,
            stage: Stage::Idle,
            value: 0.0,
            sample_rate: sample_rate.max(1.0),
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            release_level: 0.0,
        };
        env.recompute_coefficients();
        env
    }

    /// Update sample rate and recompute coefficients.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recompute_coefficients();
    }

    /// Recompute exponential coefficients from current time parameters.
    pub fn recompute_coefficients(&mut self) {
        self.attack_coeff = exp_coeff(self.attack, self.sample_rate);
        self.decay_coeff = exp_coeff(self.decay, self.sample_rate);
        self.release_coeff = exp_coeff(self.release, self.sample_rate);
    }

    /// Trigger the envelope (note on).
    ///
    /// If `retrigger` is false and the envelope is already in Attack/Decay/Sustain,
    /// the envelope is NOT retriggered (legato behavior). The pitch changes
    /// but the envelope continues from its current phase.
    pub fn gate_on(&mut self, retrigger: bool) {
        if !retrigger && self.stage != Stage::Idle && self.stage != Stage::Release {
            // Legato: already active, don't retrigger
            return;
        }
        self.stage = Stage::Attack;
        // Don't reset value — glide from current level for click-free retriggering
    }

    /// Release the envelope (note off).
    pub fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.release_level = self.value;
            self.stage = Stage::Release;
        }
    }

    /// Force the envelope to idle (used on voice steal).
    pub const fn force_off(&mut self) {
        self.stage = Stage::Idle;
        self.value = 0.0;
        self.release_level = 0.0;
    }

    /// Current stage.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }

    /// Whether the envelope is active (not idle).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// Current raw envelope value (0.0 to 1.0).
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Generate the next envelope sample.
    ///
    /// Returns the envelope level (0.0 to 1.0).
    pub fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => {
                self.value = 0.0;
            }
            Stage::Attack => {
                // Exponential approach towards ATTACK_TARGET (>1.0)
                self.value = self
                    .attack_coeff
                    .mul_add(self.value - ATTACK_TARGET, ATTACK_TARGET);

                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                // Exponential decay towards sustain level
                let target = self.sustain;
                self.value = self.decay_coeff.mul_add(self.value - target, target);

                // Close enough — snap to sustain
                if (self.value - target).abs() < 1e-5 {
                    self.value = target;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                self.value = self.sustain;
            }
            Stage::Release => {
                // Exponential decay towards 0
                self.value *= self.release_coeff;

                if self.value < 1e-5 {
                    self.value = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }

        self.value.clamp(0.0, 1.0)
    }

    /// Reset to idle.
    pub const fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.value = 0.0;
        self.release_level = 0.0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_idle_is_zero() {
        let mut env = AdsrEnvelope::new(44100.0);
        assert!((env.tick() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn envelope_attack_reaches_one() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.attack = 0.01; // 10ms
        env.recompute_coefficients();
        env.gate_on(true);

        let mut reached = false;
        for _ in 0..44100 {
            let v = env.tick();
            if (v - 1.0).abs() < 0.01 {
                reached = true;
                break;
            }
        }
        assert!(reached, "envelope should reach ~1.0 during attack");
    }

    #[test]
    fn envelope_sustain_holds() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.attack = 0.001;
        env.decay = 0.001;
        env.sustain = 0.5;
        env.recompute_coefficients();
        env.gate_on(true);

        // Run through attack + decay
        for _ in 0..4410 {
            env.tick();
        }

        // Should be near sustain
        let v = env.tick();
        assert!(
            (v - 0.5).abs() < 0.05,
            "sustain should hold at 0.5, got {v}"
        );
    }

    #[test]
    fn envelope_release_reaches_zero() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.attack = 0.001;
        env.decay = 0.001;
        env.sustain = 0.8;
        env.release = 0.01;
        env.recompute_coefficients();

        env.gate_on(true);
        for _ in 0..4410 {
            env.tick();
        }

        env.gate_off();
        for _ in 0..44100 {
            env.tick();
        }

        assert!(
            env.value() < 0.001,
            "envelope should reach near-zero after release"
        );
        assert_eq!(env.stage(), Stage::Idle);
    }

    #[test]
    fn legato_does_not_retrigger() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.attack = 0.001;
        env.decay = 0.01;
        env.sustain = 0.5;
        env.recompute_coefficients();

        env.gate_on(true);
        for _ in 0..4410 {
            env.tick();
        }

        // Now in sustain — legato gate_on should NOT retrigger
        let stage_before = env.stage();
        env.gate_on(false); // legato
        assert_eq!(env.stage(), stage_before);
    }

    #[test]
    fn output_always_finite() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.gate_on(true);
        for _ in 0..88200 {
            let v = env.tick();
            assert!(v.is_finite(), "envelope produced non-finite value");
            assert!(v >= 0.0 && v <= 1.0, "envelope out of range: {v}");
        }
    }
}
