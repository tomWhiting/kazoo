//! CS-80 envelopes.
//!
//! Filter envelope: IL (Initial Level), AL (Attack Level), Attack, Decay, Release.
//! NOT a standard ADSR. IL sets where the filter starts, AL sets where
//! the attack phase peaks. This allows non-zero starting brightness.
//!
//! VCA envelope: standard ADSR.

/// Envelope stage for both envelope types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

// ---------------------------------------------------------------------------
// IL/AL Filter Envelope
// ---------------------------------------------------------------------------

/// CS-80 filter envelope with Initial Level / Attack Level architecture.
///
/// On note-on, the output jumps to IL (Initial Level), then sweeps to
/// AL (Attack Level) during the attack phase. This is what gives CS-80
/// pads their characteristic non-zero starting brightness — the filter
/// doesn't start from silence.
///
/// - Attack: IL -> AL
/// - Decay: AL -> IL (returns toward initial level)
/// - Release: current -> IL
///
/// Output range is [0, 1] and is used as a modulation depth for the
/// filter cutoff frequency.
#[derive(Debug)]
pub struct FilterEnvelope {
    /// Current envelope value [0, 1].
    value: f32,
    /// Current stage.
    stage: EnvelopeStage,
    /// Initial Level [0, 1] — where the filter starts at note-on.
    pub initial_level: f32,
    /// Attack Level [0, 1] — where the attack phase peaks.
    pub attack_level: f32,
    /// Attack time coefficient (per-sample increment).
    attack_coeff: f32,
    /// Decay time coefficient.
    decay_coeff: f32,
    /// Release time coefficient.
    release_coeff: f32,
    /// Attack time in seconds.
    attack_time: f32,
    /// Decay time in seconds.
    decay_time: f32,
    /// Release time in seconds.
    release_time: f32,
    /// Sample rate.
    sample_rate: f32,
}

impl FilterEnvelope {
    /// Create a new IL/AL filter envelope.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Self {
            value: 0.0,
            stage: EnvelopeStage::Idle,
            initial_level: 0.4,
            attack_level: 0.8,
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            attack_time: 0.05,
            decay_time: 0.3,
            release_time: 0.5,
            sample_rate: sample_rate.max(1.0),
        };
        env.recalc_coefficients();
        env
    }

    /// Set attack time in seconds.
    pub fn set_attack(&mut self, seconds: f32) {
        self.attack_time = seconds.clamp(0.001, 30.0);
        self.recalc_coefficients();
    }

    /// Set decay time in seconds.
    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.001, 30.0);
        self.recalc_coefficients();
    }

    /// Set release time in seconds.
    pub fn set_release(&mut self, seconds: f32) {
        self.release_time = seconds.clamp(0.001, 30.0);
        self.recalc_coefficients();
    }

    /// Update sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recalc_coefficients();
    }

    /// Trigger note-on: jump to IL and start attack toward AL.
    pub const fn note_on(&mut self) {
        self.value = self.initial_level.clamp(0.0, 1.0);
        self.stage = EnvelopeStage::Attack;
    }

    /// Trigger note-off: start release toward IL.
    pub fn note_off(&mut self) {
        if self.stage != EnvelopeStage::Idle {
            self.stage = EnvelopeStage::Release;
        }
    }

    /// Process one sample, returning the envelope value [0, 1].
    ///
    /// Uses a timing scale of 1.0 (no drift).
    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.tick_scaled(1.0)
    }

    /// Process one sample with a timing scale factor (from per-voice drift).
    ///
    /// `timing_scale` > 1.0 = slower envelopes, < 1.0 = faster envelopes.
    /// The per-sample coefficients are divided by the scale factor, making
    /// the envelope take longer (slower) or shorter (faster) to complete.
    #[inline]
    pub fn tick_scaled(&mut self, timing_scale: f32) -> f32 {
        let il = self.initial_level.clamp(0.0, 1.0);
        let al = self.attack_level.clamp(0.0, 1.0);
        let scale = timing_scale.clamp(0.5, 2.0);
        let inv_scale = 1.0 / scale;

        match self.stage {
            EnvelopeStage::Idle => {}
            EnvelopeStage::Attack => {
                // Move from IL toward AL
                let step = self.attack_coeff * inv_scale;
                if al >= il {
                    self.value += step;
                    if self.value >= al {
                        self.value = al;
                        self.stage = EnvelopeStage::Decay;
                    }
                } else {
                    self.value -= step;
                    if self.value <= al {
                        self.value = al;
                        self.stage = EnvelopeStage::Decay;
                    }
                }
            }
            EnvelopeStage::Decay => {
                // Move from AL toward IL
                let step = self.decay_coeff * inv_scale;
                if il <= al {
                    self.value -= step;
                    if self.value <= il {
                        self.value = il;
                        self.stage = EnvelopeStage::Sustain;
                    }
                } else {
                    self.value += step;
                    if self.value >= il {
                        self.value = il;
                        self.stage = EnvelopeStage::Sustain;
                    }
                }
            }
            EnvelopeStage::Sustain => {
                // Hold at IL while key is held
                self.value = il;
            }
            EnvelopeStage::Release => {
                // Move toward IL, then idle
                let step = self.release_coeff * inv_scale;
                let target = il;
                if self.value > target {
                    self.value -= step;
                    if self.value <= target {
                        self.value = target;
                        self.stage = EnvelopeStage::Idle;
                    }
                } else if self.value < target {
                    self.value += step;
                    if self.value >= target {
                        self.value = target;
                        self.stage = EnvelopeStage::Idle;
                    }
                } else {
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }

        // NaN/Inf defense
        if !self.value.is_finite() {
            self.value = 0.0;
        }
        self.value.clamp(0.0, 1.0)
    }

    /// Current stage.
    #[must_use]
    pub const fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    /// Current value.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Reset to idle state.
    pub const fn reset(&mut self) {
        self.value = 0.0;
        self.stage = EnvelopeStage::Idle;
    }

    /// Recalculate per-sample coefficients from time parameters.
    fn recalc_coefficients(&mut self) {
        let il = self.initial_level.clamp(0.0, 1.0);
        let al = self.attack_level.clamp(0.0, 1.0);
        let range = (al - il).abs().max(0.001);

        let attack_samples = (self.attack_time * self.sample_rate).max(1.0);
        self.attack_coeff = range / attack_samples;

        let decay_samples = (self.decay_time * self.sample_rate).max(1.0);
        self.decay_coeff = range / decay_samples;

        // Release covers the full range (worst case) from current to IL
        let release_samples = (self.release_time * self.sample_rate).max(1.0);
        self.release_coeff = 1.0 / release_samples;
    }
}

// ---------------------------------------------------------------------------
// Standard ADSR Envelope (for VCA)
// ---------------------------------------------------------------------------

/// Standard ADSR envelope for the VCA.
///
/// Attack: 0 -> 1
/// Decay: 1 -> `sustain_level`
/// Sustain: hold at `sustain_level`
/// Release: `sustain_level` -> 0
#[derive(Debug)]
pub struct AdsrEnvelope {
    /// Current envelope value [0, 1].
    value: f32,
    /// Current stage.
    stage: EnvelopeStage,
    /// Attack rate (per-sample increment).
    attack_rate: f32,
    /// Decay rate (per-sample decrement).
    decay_rate: f32,
    /// Sustain level [0, 1].
    pub sustain_level: f32,
    /// Release rate (per-sample decrement).
    release_rate: f32,
    /// Time parameters (seconds).
    attack_time: f32,
    decay_time: f32,
    release_time: f32,
    /// Sample rate.
    sample_rate: f32,
}

impl AdsrEnvelope {
    /// Create a new ADSR envelope.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Self {
            value: 0.0,
            stage: EnvelopeStage::Idle,
            attack_rate: 0.0,
            decay_rate: 0.0,
            sustain_level: 0.7,
            release_rate: 0.0,
            attack_time: 0.01,
            decay_time: 0.2,
            release_time: 0.4,
            sample_rate: sample_rate.max(1.0),
        };
        env.recalc_rates();
        env
    }

    pub fn set_attack(&mut self, seconds: f32) {
        self.attack_time = seconds.clamp(0.001, 30.0);
        self.recalc_rates();
    }

    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.001, 30.0);
        self.recalc_rates();
    }

    pub const fn set_sustain(&mut self, level: f32) {
        self.sustain_level = level.clamp(0.0, 1.0);
    }

    pub fn set_release(&mut self, seconds: f32) {
        self.release_time = seconds.clamp(0.001, 30.0);
        self.recalc_rates();
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recalc_rates();
    }

    /// Trigger note-on.
    pub const fn note_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
        // Don't reset value — allows retriggering from current position
    }

    /// Trigger note-off.
    pub fn note_off(&mut self) {
        if self.stage != EnvelopeStage::Idle {
            self.stage = EnvelopeStage::Release;
        }
    }

    /// Process one sample.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.tick_scaled(1.0)
    }

    /// Process one sample with a timing scale factor (from per-voice drift).
    ///
    /// `timing_scale` > 1.0 = slower envelopes, < 1.0 = faster envelopes.
    #[inline]
    pub fn tick_scaled(&mut self, timing_scale: f32) -> f32 {
        let scale = timing_scale.clamp(0.5, 2.0);
        let inv_scale = 1.0 / scale;

        match self.stage {
            EnvelopeStage::Idle => {}
            EnvelopeStage::Attack => {
                self.value += self.attack_rate * inv_scale;
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                self.value -= self.decay_rate * inv_scale;
                if self.value <= self.sustain_level {
                    self.value = self.sustain_level;
                    self.stage = EnvelopeStage::Sustain;
                }
            }
            EnvelopeStage::Sustain => {
                self.value = self.sustain_level;
            }
            EnvelopeStage::Release => {
                self.value -= self.release_rate * inv_scale;
                if self.value <= 0.0 {
                    self.value = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }

        if !self.value.is_finite() {
            self.value = 0.0;
        }
        self.value.clamp(0.0, 1.0)
    }

    #[must_use]
    pub const fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Whether the envelope has finished (returned to idle).
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        matches!(self.stage, EnvelopeStage::Idle)
    }

    pub const fn reset(&mut self) {
        self.value = 0.0;
        self.stage = EnvelopeStage::Idle;
    }

    fn recalc_rates(&mut self) {
        let attack_samples = (self.attack_time * self.sample_rate).max(1.0);
        self.attack_rate = 1.0 / attack_samples;

        let decay_samples = (self.decay_time * self.sample_rate).max(1.0);
        self.decay_rate = (1.0 - self.sustain_level).max(0.001) / decay_samples;

        let release_samples = (self.release_time * self.sample_rate).max(1.0);
        self.release_rate = 1.0 / release_samples;
    }
}

// ---------------------------------------------------------------------------
// Attack-Decay Envelope (for Ring Mod)
// ---------------------------------------------------------------------------

/// Simple attack-decay envelope for the ring modulator depth.
#[derive(Debug)]
pub struct AdEnvelope {
    value: f32,
    stage: EnvelopeStage,
    attack_rate: f32,
    decay_rate: f32,
    attack_time: f32,
    decay_time: f32,
    sample_rate: f32,
}

impl AdEnvelope {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Self {
            value: 0.0,
            stage: EnvelopeStage::Idle,
            attack_rate: 0.0,
            decay_rate: 0.0,
            attack_time: 0.005,
            decay_time: 0.2,
            sample_rate: sample_rate.max(1.0),
        };
        env.recalc_rates();
        env
    }

    pub fn set_attack(&mut self, seconds: f32) {
        self.attack_time = seconds.clamp(0.0005, 10.0);
        self.recalc_rates();
    }

    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.001, 30.0);
        self.recalc_rates();
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recalc_rates();
    }

    pub const fn trigger(&mut self) {
        self.value = 0.0;
        self.stage = EnvelopeStage::Attack;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => {}
            EnvelopeStage::Attack => {
                self.value += self.attack_rate;
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                self.value -= self.decay_rate;
                if self.value <= 0.0 {
                    self.value = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
            // AD envelope only uses Attack and Decay
            EnvelopeStage::Sustain | EnvelopeStage::Release => {
                self.stage = EnvelopeStage::Idle;
            }
        }

        if !self.value.is_finite() {
            self.value = 0.0;
        }
        self.value.clamp(0.0, 1.0)
    }

    pub const fn reset(&mut self) {
        self.value = 0.0;
        self.stage = EnvelopeStage::Idle;
    }

    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    fn recalc_rates(&mut self) {
        let attack_samples = (self.attack_time * self.sample_rate).max(1.0);
        self.attack_rate = 1.0 / attack_samples;

        let decay_samples = (self.decay_time * self.sample_rate).max(1.0);
        self.decay_rate = 1.0 / decay_samples;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Filter Envelope tests --

    #[test]
    fn filter_env_starts_at_il() {
        let mut env = FilterEnvelope::new(44100.0);
        env.initial_level = 0.4;
        env.attack_level = 0.8;
        env.note_on();
        let val = env.tick();
        // Should be very close to IL on first tick
        assert!(
            (val - 0.4).abs() < 0.05,
            "filter env should start near IL=0.4, got {val}"
        );
    }

    #[test]
    fn filter_env_reaches_al() {
        let mut env = FilterEnvelope::new(44100.0);
        env.initial_level = 0.2;
        env.attack_level = 0.9;
        env.set_attack(0.01); // 10ms attack
        env.note_on();

        let mut max_val = 0.0_f32;
        for _ in 0..44100 {
            let val = env.tick();
            max_val = max_val.max(val);
        }
        assert!(
            (max_val - 0.9).abs() < 0.01,
            "filter env should reach AL=0.9, max was {max_val}"
        );
    }

    #[test]
    fn filter_env_decays_toward_il() {
        let mut env = FilterEnvelope::new(44100.0);
        env.initial_level = 0.3;
        env.attack_level = 0.8;
        env.set_attack(0.001);
        env.set_decay(0.05);
        env.note_on();

        // Run through attack + most of decay
        for _ in 0..44100 {
            env.tick();
        }
        let val = env.value();
        assert!(
            (val - 0.3).abs() < 0.05,
            "filter env should decay to IL=0.3, got {val}"
        );
    }

    #[test]
    fn filter_env_inverted_sweep() {
        // IL > AL: filter starts bright and closes
        let mut env = FilterEnvelope::new(44100.0);
        env.initial_level = 0.8;
        env.attack_level = 0.2;
        env.set_attack(0.01);
        env.note_on();

        let first = env.tick();
        assert!(
            first > 0.5,
            "inverted envelope should start high, got {first}"
        );

        // Run through attack
        for _ in 0..4410 {
            env.tick();
        }
        let after_attack = env.value();
        assert!(
            after_attack < 0.5,
            "inverted envelope should sweep down, got {after_attack}"
        );
    }

    #[test]
    fn filter_env_output_always_finite() {
        let mut env = FilterEnvelope::new(44100.0);
        env.note_on();
        for _ in 0..44100 {
            let val = env.tick();
            assert!(val.is_finite());
            assert!((0.0..=1.0).contains(&val));
        }
        env.note_off();
        for _ in 0..44100 {
            let val = env.tick();
            assert!(val.is_finite());
            assert!((0.0..=1.0).contains(&val));
        }
    }

    // -- ADSR tests --

    #[test]
    fn adsr_starts_at_zero() {
        let env = AdsrEnvelope::new(44100.0);
        assert!((env.value() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn adsr_reaches_peak() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.set_attack(0.01);
        env.note_on();

        let mut max_val = 0.0_f32;
        for _ in 0..44100 {
            let val = env.tick();
            max_val = max_val.max(val);
        }
        assert!(
            (max_val - 1.0).abs() < 0.01,
            "ADSR should reach 1.0, max was {max_val}"
        );
    }

    #[test]
    fn adsr_sustain_holds() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.set_attack(0.001);
        env.set_decay(0.01);
        env.set_sustain(0.6);
        env.note_on();

        // Run past attack + decay
        for _ in 0..44100 {
            env.tick();
        }
        let val = env.value();
        assert!(
            (val - 0.6).abs() < 0.01,
            "ADSR should sustain at 0.6, got {val}"
        );
    }

    #[test]
    fn adsr_release_to_zero() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.set_attack(0.001);
        env.set_decay(0.001);
        env.set_release(0.01);
        env.note_on();

        for _ in 0..4410 {
            env.tick();
        }
        env.note_off();
        for _ in 0..44100 {
            env.tick();
        }
        assert!(env.is_idle(), "ADSR should be idle after release");
        assert!(
            env.value() < 0.001,
            "ADSR should be near zero after release"
        );
    }

    #[test]
    fn adsr_output_always_valid() {
        let mut env = AdsrEnvelope::new(44100.0);
        env.note_on();
        for _ in 0..88200 {
            let val = env.tick();
            assert!(val.is_finite());
            assert!((0.0..=1.0).contains(&val));
        }
    }

    // -- AD envelope tests --

    #[test]
    fn ad_trigger_and_decay() {
        let mut env = AdEnvelope::new(44100.0);
        env.set_attack(0.001);
        env.set_decay(0.01);
        env.trigger();

        let mut max_val = 0.0_f32;
        for _ in 0..44100 {
            let val = env.tick();
            max_val = max_val.max(val);
        }
        assert!(max_val > 0.9, "AD should reach near 1.0");
        assert!(env.value() < 0.01, "AD should decay to near zero");
    }
}
