//! TR-808 cowbell synthesis.
//!
//! Two square oscillators (800 Hz + 540 Hz) through bandpass at 880 Hz.
//! The beat frequency between the two creates the metallic cowbell tone.
//! Two-stage envelope: 50 ms transient then 500 ms sustain decay.

use super::Voice;

/// 808 cowbell voice.
#[derive(Debug)]
pub struct Cowbell {
    sample_rate: f32,
    phase_1: f32,
    phase_2: f32,
    freq_1: f32,
    freq_2: f32,
    amplitude: f32,
    /// Bandpass filter states.
    bp_state_lo: f32,
    bp_state_hi: f32,
    bp_coeff_lo: f32,
    bp_coeff_hi: f32,
    active: bool,
    /// Current position in the envelope (samples since trigger).
    env_pos: u32,
    /// Transient phase duration in samples (~50 ms).
    transient_end: u32,
    /// Sustain decay coefficient (per-sample).
    sustain_decay: f32,
    decay_time: f32,
}

impl Cowbell {
    /// Default transient duration (seconds).
    const TRANSIENT_SECS: f32 = 0.05;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        // BPF centered ~880 Hz: HP at 700 Hz, LP at 1100 Hz.
        let bp_coeff_lo = (-std::f32::consts::TAU * 700.0 / sample_rate).exp();
        let bp_coeff_hi = (-std::f32::consts::TAU * 1100.0 / sample_rate).exp();
        let decay_time = 0.5;
        #[allow(clippy::cast_sign_loss)]
        let transient_end = (sample_rate * Self::TRANSIENT_SECS) as u32;
        let sustain_decay = Self::compute_sustain_decay(sample_rate, decay_time);
        Self {
            sample_rate,
            phase_1: 0.0,
            phase_2: 0.0,
            freq_1: 800.0,
            freq_2: 540.0,
            amplitude: 0.0,
            bp_state_lo: 0.0,
            bp_state_hi: 0.0,
            bp_coeff_lo,
            bp_coeff_hi,
            active: false,
            env_pos: 0,
            transient_end,
            sustain_decay,
            decay_time,
        }
    }

    fn compute_sustain_decay(sample_rate: f32, seconds: f32) -> f32 {
        let samples = sample_rate * seconds;
        if samples > 0.0 {
            (-6.9 / samples).exp()
        } else {
            0.0
        }
    }

    /// Set decay time in seconds (0.1-1.0).
    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.1, 1.0);
        self.sustain_decay = Self::compute_sustain_decay(self.sample_rate, self.decay_time);
    }

    /// Set tuning — adjusts both oscillator frequencies proportionally.
    pub fn set_tune(&mut self, ratio: f32) {
        let r = ratio.clamp(0.5, 2.0);
        self.freq_1 = 800.0 * r;
        self.freq_2 = 540.0 * r;
    }

    /// Two-stage envelope: full level during transient, then exponential decay.
    /// Both phases return 1.0 because the actual decay is handled by the
    /// amplitude field in the process loop. Kept as a method for future
    /// envelope shaping.
    #[allow(clippy::unused_self)]
    const fn envelope(&self) -> f32 {
        1.0
    }
}

impl Voice for Cowbell {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
        self.env_pos = 0;
        self.bp_state_lo = 0.0;
        self.bp_state_hi = 0.0;
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        // Two square oscillators.
        self.phase_1 += self.freq_1 / self.sample_rate;
        if self.phase_1 >= 1.0 {
            self.phase_1 -= 1.0;
        }
        self.phase_2 += self.freq_2 / self.sample_rate;
        if self.phase_2 >= 1.0 {
            self.phase_2 -= 1.0;
        }

        let sq_1 = if self.phase_1 < 0.5 { 1.0_f32 } else { -1.0 };
        let sq_2 = if self.phase_2 < 0.5 { 1.0_f32 } else { -1.0 };
        let mixed = (sq_1 + sq_2) * 0.5;

        // Bandpass.
        let hp = mixed - self.bp_state_lo;
        self.bp_state_lo += (1.0 - self.bp_coeff_lo) * (mixed - self.bp_state_lo);
        self.bp_state_hi += (1.0 - self.bp_coeff_hi) * (hp - self.bp_state_hi);

        // Two-stage envelope: hold during transient, then exponential decay.
        let env = self.envelope();
        if self.env_pos >= self.transient_end {
            // Sustain decay phase.
            self.amplitude *= self.sustain_decay;
        }
        self.env_pos = self.env_pos.saturating_add(1);

        let output = self.bp_state_hi * self.amplitude * env;

        if self.amplitude < 1e-6 {
            self.active = false;
        }

        kazoo_core::sanitize_sample(output)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cowbell_produces_sound() {
        let mut cb = Cowbell::new(44100.0);
        cb.trigger(1.0);
        let mut had_nonzero = false;
        for _ in 0..4410 {
            let s = cb.process();
            assert!(s.is_finite());
            if s.abs() > 1e-6 {
                had_nonzero = true;
            }
        }
        assert!(had_nonzero);
    }

    #[test]
    fn cowbell_decays_to_silence() {
        let mut cb = Cowbell::new(44100.0);
        cb.trigger(1.0);
        // Two-stage envelope: 50ms transient hold + 500ms sustain decay to -60 dB.
        // Amplitude must drop below 1e-6 (~-120 dB), so need ~2x the decay time
        // plus the transient. Run for 2 seconds to be safe.
        for _ in 0..88200 {
            cb.process();
        }
        assert!(!cb.is_active());
    }

    #[test]
    fn cowbell_two_stage_envelope() {
        let mut cb = Cowbell::new(44100.0);
        cb.trigger(1.0);
        // During transient (first 50ms = 2205 samples), amplitude should stay near 1.0.
        for _ in 0..2000 {
            cb.process();
        }
        assert!(
            cb.amplitude > 0.9,
            "amplitude during transient should stay near 1.0, got {}",
            cb.amplitude
        );
    }
}
