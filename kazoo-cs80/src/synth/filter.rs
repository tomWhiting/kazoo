//! 12 dB/oct state-variable filter.
//!
//! Provides simultaneous HPF and LPF outputs from the same filter state.
//! Each has its own resonance control. Resonance deliberately limited —
//! no self-oscillation. This is a different topology from the Moog ladder.
//!
//! Uses the Chamberlin SVF (two-integrator) structure:
//!   hp = input - lp - q * bp
//!   bp += f * hp
//!   lp += f * bp
//!
//! Where f = 2 * sin(π * cutoff / `sample_rate`) and q = 1/Q.

use std::f32::consts::PI;

/// State-variable filter providing simultaneous HP and LP outputs.
///
/// 12 dB/oct (2-pole). The CS-80 uses one HPF and one LPF per layer,
/// each as a separate SVF instance.
#[derive(Debug)]
pub struct StateVariableFilter {
    /// Low-pass state.
    lp: f32,
    /// Band-pass state.
    bp: f32,
    /// Filter coefficient (derived from cutoff frequency).
    f_coeff: f32,
    /// Damping coefficient (derived from resonance).
    q_coeff: f32,
    /// Cutoff frequency in Hz.
    cutoff_hz: f32,
    /// Resonance [0, 1]. Capped below self-oscillation.
    resonance: f32,
    /// Sample rate.
    sample_rate: f32,
}

impl StateVariableFilter {
    /// Maximum resonance — deliberately below self-oscillation.
    const MAX_RESONANCE: f32 = 0.95;

    /// Create a new SVF at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut svf = Self {
            lp: 0.0,
            bp: 0.0,
            f_coeff: 0.0,
            q_coeff: 1.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            sample_rate: sample_rate.max(1.0),
        };
        svf.update_coefficients();
        svf
    }

    /// Set cutoff frequency in Hz [20, 20000].
    pub fn set_cutoff(&mut self, hz: f32) {
        self.cutoff_hz = if hz.is_finite() {
            hz.clamp(20.0, 20_000.0)
        } else {
            20_000.0
        };
        self.update_coefficients();
    }

    /// Set resonance [0, 0.95]. No self-oscillation allowed.
    pub fn set_resonance(&mut self, r: f32) {
        self.resonance = if r.is_finite() {
            r.clamp(0.0, Self::MAX_RESONANCE)
        } else {
            0.0
        };
        self.update_coefficients();
    }

    /// Update sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.update_coefficients();
    }

    /// Reset filter state (clear delay elements).
    pub const fn reset(&mut self) {
        self.lp = 0.0;
        self.bp = 0.0;
    }

    /// Process one sample. Returns `(hp_out, lp_out)`.
    ///
    /// Both outputs are available simultaneously — the CS-80 signal path
    /// uses the HP output from the first SVF, then feeds into a second
    /// SVF where the LP output is taken.
    #[inline]
    pub fn tick(&mut self, input: f32) -> (f32, f32) {
        let input = if input.is_finite() { input } else { 0.0 };

        // Chamberlin SVF: two-integrator structure
        let hp = self.q_coeff.mul_add(-self.bp, input - self.lp);
        self.bp += self.f_coeff * hp;
        self.lp += self.f_coeff * self.bp;

        // NaN/Inf defense on state variables
        if !self.bp.is_finite() {
            self.bp = 0.0;
        }
        if !self.lp.is_finite() {
            self.lp = 0.0;
        }

        let hp_out = if hp.is_finite() { hp } else { 0.0 };
        (hp_out, self.lp)
    }

    /// Recalculate coefficients from cutoff and resonance.
    fn update_coefficients(&mut self) {
        // f = 2 * sin(π * cutoff / sample_rate)
        // Clamped to prevent instability at high frequencies
        let normalized = (PI * self.cutoff_hz / self.sample_rate).min(PI * 0.49);
        self.f_coeff = 2.0 * normalized.sin();

        // q = 2 - 2 * resonance (damping: high resonance = low damping)
        // At resonance=0: q=2 (overdamped). At resonance=0.95: q=0.1 (near oscillation).
        self.q_coeff = 2.0 * (1.0 - self.resonance);
        // Ensure minimum damping to prevent blowup
        if self.q_coeff < 0.05 {
            self.q_coeff = 0.05;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svf_passes_signal() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_cutoff(1000.0);
        svf.set_resonance(0.0);

        // Feed a DC signal — LP should pass it, HP should reject it
        let mut lp_sum = 0.0;
        for _ in 0..4410 {
            let (_, lp) = svf.tick(1.0);
            lp_sum += lp;
        }
        // LP should accumulate significant energy from DC
        assert!(lp_sum > 100.0, "LP should pass DC, got sum={lp_sum}");
    }

    #[test]
    fn svf_hp_rejects_dc() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_cutoff(1000.0);
        svf.set_resonance(0.0);

        // After settling, HP output of DC should be near zero
        for _ in 0..4410 {
            svf.tick(1.0);
        }
        let (hp, _) = svf.tick(1.0);
        assert!(
            hp.abs() < 0.01,
            "HP should reject DC after settling, got {hp}"
        );
    }

    #[test]
    fn svf_outputs_finite() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_cutoff(5000.0);
        svf.set_resonance(0.9);

        for i in 0..44100 {
            let input = (i as f32 * 0.1).sin();
            let (hp, lp) = svf.tick(input);
            assert!(hp.is_finite(), "HP not finite at sample {i}");
            assert!(lp.is_finite(), "LP not finite at sample {i}");
        }
    }

    #[test]
    fn svf_handles_nan_input() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_cutoff(1000.0);
        let (hp, lp) = svf.tick(f32::NAN);
        assert!(hp.is_finite());
        assert!(lp.is_finite());
    }

    #[test]
    fn svf_reset_clears_state() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_cutoff(1000.0);
        for _ in 0..100 {
            svf.tick(1.0);
        }
        svf.reset();
        assert!((svf.lp - 0.0).abs() < f32::EPSILON);
        assert!((svf.bp - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn svf_resonance_clamped() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_resonance(2.0);
        assert!(svf.resonance <= StateVariableFilter::MAX_RESONANCE);
        svf.set_resonance(-1.0);
        assert!(svf.resonance >= 0.0);
    }

    #[test]
    fn svf_stable_at_high_resonance() {
        let mut svf = StateVariableFilter::new(44100.0);
        svf.set_cutoff(2000.0);
        svf.set_resonance(0.95);

        // Run a burst then silence — should not blow up
        for _ in 0..100 {
            svf.tick(1.0);
        }
        for _ in 0..44100 {
            let (hp, lp) = svf.tick(0.0);
            assert!(hp.is_finite() && hp.abs() < 100.0, "HP unstable: {hp}");
            assert!(lp.is_finite() && lp.abs() < 100.0, "LP unstable: {lp}");
        }
    }
}
