//! TR-808 kick drum synthesis.
//!
//! Bridged-T bandpass filter excited into self-oscillation.
//! Base frequency ~49 Hz, pitch sweep from ~130 Hz over 6ms on trigger.
//! Exponential decay, 50-800ms range.

use super::Voice;

/// 808 kick drum voice.
#[derive(Debug)]
pub struct Kick {
    sample_rate: f32,
    /// Base resonant frequency in Hz.
    base_freq: f32,
    /// Current oscillator phase (0.0..1.0).
    phase: f32,
    /// Current instantaneous frequency (sweeps from trigger freq to base).
    current_freq: f32,
    /// Trigger frequency (pitch burst peak).
    trigger_freq: f32,
    /// Pitch sweep decay rate per sample.
    pitch_decay: f32,
    /// Amplitude envelope value.
    amplitude: f32,
    /// Amplitude decay rate per sample.
    amp_decay: f32,
    /// Whether currently sounding.
    active: bool,
    /// Decay time in seconds (user parameter).
    decay_time: f32,
}

impl Kick {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let decay_time = 0.3;
        Self {
            sample_rate,
            base_freq: 49.0,
            phase: 0.0,
            current_freq: 49.0,
            trigger_freq: 130.0,
            pitch_decay: Self::compute_pitch_decay(sample_rate),
            amplitude: 0.0,
            amp_decay: Self::compute_amp_decay(sample_rate, decay_time),
            active: false,
            decay_time,
        }
    }

    fn compute_pitch_decay(sample_rate: f32) -> f32 {
        // Pitch sweep completes in ~6ms.
        let sweep_samples = sample_rate * 0.006;
        if sweep_samples > 0.0 {
            (-1.0 / sweep_samples).exp()
        } else {
            0.0
        }
    }

    fn compute_amp_decay(sample_rate: f32, decay_time: f32) -> f32 {
        let decay_samples = sample_rate * decay_time;
        if decay_samples > 0.0 {
            // -60 dB in decay_time seconds.
            (-6.9 / decay_samples).exp()
        } else {
            0.0
        }
    }

    /// Set the decay time in seconds (0.05..0.8).
    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.05, 0.8);
        self.amp_decay = Self::compute_amp_decay(self.sample_rate, self.decay_time);
    }

    /// Set the base (resting) frequency in Hz.
    pub const fn set_tune(&mut self, freq: f32) {
        self.base_freq = freq.clamp(20.0, 100.0);
    }
}

impl Voice for Kick {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
        self.current_freq = self.trigger_freq;
        // Don't reset phase — allows retriggering mid-decay without click.
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        // Pitch sweep: exponential decay toward base frequency.
        let freq_excess = self.current_freq - self.base_freq;
        self.current_freq = freq_excess.mul_add(self.pitch_decay, self.base_freq);

        // Sine oscillator at current frequency.
        let increment = self.current_freq / self.sample_rate;
        self.phase += increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let sample = (self.phase * std::f32::consts::TAU).sin();

        // Amplitude decay.
        self.amplitude *= self.amp_decay;
        if self.amplitude < 1e-6 {
            self.active = false;
            self.amplitude = 0.0;
        }

        kazoo_core::sanitize_sample(sample * self.amplitude)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}
