//! Ring modulator: internal sine carrier * voice signal.
//! Own attack-decay envelope on modulation depth.
//! Creates metallic attack transients on bright patches.

use std::f32::consts::TAU;

use super::envelope::AdEnvelope;

/// Ring modulator with an internal sine carrier and AD envelope on depth.
///
/// The ring mod multiplies the voice signal by a sine carrier. The depth
/// of this multiplication is controlled by an AD envelope — metallic
/// attack on note-on that fades away.
#[derive(Debug)]
pub struct RingModulator {
    /// Internal sine carrier phase [0, 1).
    phase: f32,
    /// Carrier frequency in Hz.
    pub carrier_freq: f32,
    /// Maximum modulation depth [0, 1].
    pub depth: f32,
    /// AD envelope controlling the modulation depth over time.
    envelope: AdEnvelope,
    /// Sample rate.
    sample_rate: f32,
    /// Phase increment per sample.
    phase_inc: f32,
}

impl RingModulator {
    /// Create a new ring modulator.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let carrier_freq = 200.0;
        let phase_inc = carrier_freq / sample_rate.max(1.0);
        Self {
            phase: 0.0,
            carrier_freq,
            depth: 0.4,
            envelope: AdEnvelope::new(sample_rate),
            sample_rate: sample_rate.max(1.0),
            phase_inc,
        }
    }

    /// Set carrier frequency.
    pub fn set_carrier_freq(&mut self, hz: f32) {
        self.carrier_freq = hz.clamp(20.0, 5000.0);
        self.phase_inc = self.carrier_freq / self.sample_rate;
    }

    /// Set envelope attack time.
    pub fn set_attack(&mut self, seconds: f32) {
        self.envelope.set_attack(seconds);
    }

    /// Set envelope decay time.
    pub fn set_decay(&mut self, seconds: f32) {
        self.envelope.set_decay(seconds);
    }

    /// Update sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.phase_inc = self.carrier_freq / self.sample_rate;
        self.envelope.set_sample_rate(sample_rate);
    }

    /// Trigger the ring mod envelope (on note-on).
    pub const fn trigger(&mut self) {
        self.envelope.trigger();
    }

    /// Reset all state.
    pub const fn reset(&mut self) {
        self.phase = 0.0;
        self.envelope.reset();
    }

    /// Process one sample of voice signal through the ring modulator.
    ///
    /// Returns the mixed signal: `dry * (1 - wet_amount) + ring_mod * wet_amount`
    /// where `wet_amount = depth * envelope_value`.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };

        // Carrier sine
        let carrier = (self.phase * TAU).sin();

        // Advance carrier phase
        self.phase += self.phase_inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        if !self.phase.is_finite() || self.phase < 0.0 {
            self.phase = 0.0;
        }

        // Envelope controls how much ring mod is applied
        let env_val = self.envelope.tick();
        let wet_amount = self.depth * env_val;

        // Ring modulation = input * carrier
        let ring = input * carrier;

        // Crossfade between dry and ring-modulated signal
        let output = input * (1.0 - wet_amount) + ring * wet_amount;

        if output.is_finite() { output } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_mod_no_effect_when_depth_zero() {
        let mut rm = RingModulator::new(44100.0);
        rm.depth = 0.0;
        rm.trigger();

        let input = 0.5;
        let output = rm.process(input);
        assert!(
            (output - input).abs() < 0.01,
            "zero depth should pass through, got {output}"
        );
    }

    #[test]
    fn ring_mod_output_finite() {
        let mut rm = RingModulator::new(44100.0);
        rm.depth = 0.8;
        rm.set_carrier_freq(300.0);
        rm.trigger();

        for i in 0..44100 {
            let input = (i as f32 * 0.05).sin();
            let output = rm.process(input);
            assert!(output.is_finite(), "ring mod output not finite at {i}");
        }
    }

    #[test]
    fn ring_mod_handles_nan() {
        let mut rm = RingModulator::new(44100.0);
        rm.trigger();
        let output = rm.process(f32::NAN);
        assert!(output.is_finite());
    }

    #[test]
    fn ring_mod_envelope_decays() {
        let mut rm = RingModulator::new(44100.0);
        rm.depth = 1.0;
        rm.set_attack(0.001);
        rm.set_decay(0.01);
        rm.trigger();

        // After envelope decays, should be essentially dry
        for _ in 0..44100 {
            rm.process(0.5);
        }
        let output = rm.process(0.5);
        assert!(
            (output - 0.5).abs() < 0.05,
            "after decay, should be near dry signal, got {output}"
        );
    }
}
