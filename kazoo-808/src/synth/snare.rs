//! TR-808 snare drum synthesis.
//!
//! Two bridged-T oscillators (476 Hz + 238 Hz) summed with
//! HP-filtered white noise. Independent tone/snappy controls.

use super::Voice;

/// 808 snare drum voice.
#[derive(Debug)]
pub struct Snare {
    sample_rate: f32,
    // Tonal body: two sine oscillators.
    phase_1: f32,
    phase_2: f32,
    freq_1: f32,
    freq_2: f32,
    body_amplitude: f32,
    body_decay: f32,
    // Noise: HP-filtered white noise.
    noise_amplitude: f32,
    noise_decay: f32,
    /// Simple 1-pole HP filter state for noise.
    hp_state: f32,
    hp_coeff: f32,
    active: bool,
    /// Tone balance between the two oscillators (0.0..1.0).
    tone: f32,
    /// Noise decay time in seconds.
    snappy: f32,
}

impl Snare {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let body_decay_time = 0.15;
        let snappy = 0.15;
        // HP filter cutoff ~2 kHz for noise path.
        let hp_coeff = (-std::f32::consts::TAU * 2000.0 / sample_rate).exp();
        Self {
            sample_rate,
            phase_1: 0.0,
            phase_2: 0.0,
            freq_1: 476.0,
            freq_2: 238.0,
            body_amplitude: 0.0,
            body_decay: (-6.9 / (sample_rate * body_decay_time)).exp(),
            noise_amplitude: 0.0,
            noise_decay: (-6.9 / (sample_rate * snappy)).exp(),
            hp_state: 0.0,
            hp_coeff,
            active: false,
            tone: 0.5,
            snappy,
        }
    }

    pub const fn set_tone(&mut self, tone: f32) {
        self.tone = tone.clamp(0.0, 1.0);
    }

    pub fn set_snappy(&mut self, seconds: f32) {
        self.snappy = seconds.clamp(0.02, 0.5);
        self.noise_decay = (-6.9 / (self.sample_rate * self.snappy)).exp();
    }

    /// Set tuning ratio (0.5-2.0). Scales both oscillator frequencies
    /// proportionally from their base values (476 Hz, 238 Hz).
    pub fn set_tune(&mut self, ratio: f32) {
        let r = ratio.clamp(0.5, 2.0);
        self.freq_1 = 476.0 * r;
        self.freq_2 = 238.0 * r;
    }

    /// Set body decay time in seconds (0.05-0.5).
    pub fn set_decay(&mut self, seconds: f32) {
        let time = seconds.clamp(0.05, 0.5);
        self.body_decay = (-6.9 / (self.sample_rate * time)).exp();
    }
}

impl Voice for Snare {
    fn trigger(&mut self, velocity: f32) {
        let vel = velocity.clamp(0.0, 1.0);
        self.active = true;
        self.body_amplitude = vel;
        self.noise_amplitude = vel;
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        // Tonal body: two sines.
        self.phase_1 += self.freq_1 / self.sample_rate;
        if self.phase_1 >= 1.0 {
            self.phase_1 -= 1.0;
        }
        self.phase_2 += self.freq_2 / self.sample_rate;
        if self.phase_2 >= 1.0 {
            self.phase_2 -= 1.0;
        }

        let osc_1 = (self.phase_1 * std::f32::consts::TAU).sin();
        let osc_2 = (self.phase_2 * std::f32::consts::TAU).sin();
        let body = osc_1.mul_add(self.tone, osc_2 * (1.0 - self.tone));

        // Noise: simple white noise through 1-pole HP.
        let noise_raw = rand::random::<f32>().mul_add(2.0, -1.0);
        let hp_out = noise_raw - self.hp_state;
        self.hp_state = (1.0 - self.hp_coeff).mul_add(noise_raw - self.hp_state, self.hp_state);

        // Mix and apply envelopes.
        let output = body * self.body_amplitude + hp_out * self.noise_amplitude;

        self.body_amplitude *= self.body_decay;
        self.noise_amplitude *= self.noise_decay;

        if self.body_amplitude < 1e-6 && self.noise_amplitude < 1e-6 {
            self.active = false;
        }

        kazoo_core::sanitize_sample(output)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}
