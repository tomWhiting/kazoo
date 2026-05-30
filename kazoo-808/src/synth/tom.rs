//! TR-808 tom synthesis.
//!
//! Bridged-T oscillator, same architecture as kick at different
//! frequencies and shorter fixed decays. Subtle downward pitch
//! sweep during decay per the original circuit behaviour.

use super::Voice;

/// 808 tom voice (parameterised for hi/mid/lo).
#[derive(Debug)]
pub struct Tom {
    sample_rate: f32,
    /// Resting frequency (Hz).
    base_freq: f32,
    /// Current instantaneous frequency.
    current_freq: f32,
    /// Pitch at trigger (slightly above base for the initial punch).
    trigger_freq: f32,
    phase: f32,
    amplitude: f32,
    amp_decay: f32,
    /// Per-sample pitch sweep coefficient.
    pitch_decay: f32,
    active: bool,
    decay_time: f32,
}

impl Tom {
    /// Create a tom at the given frequency and decay time.
    #[must_use]
    pub fn new(sample_rate: f32, freq: f32, decay_seconds: f32) -> Self {
        // Trigger frequency 30% above base for the subtle pitch sweep.
        let trigger_freq = freq * 1.3;
        Self {
            sample_rate,
            base_freq: freq,
            current_freq: freq,
            trigger_freq,
            phase: 0.0,
            amplitude: 0.0,
            amp_decay: Self::compute_amp_decay(sample_rate, decay_seconds),
            pitch_decay: Self::compute_pitch_decay(sample_rate, decay_seconds),
            active: false,
            decay_time: decay_seconds,
        }
    }

    fn compute_amp_decay(sample_rate: f32, seconds: f32) -> f32 {
        let samples = sample_rate * seconds;
        if samples > 0.0 {
            (-6.9 / samples).exp()
        } else {
            0.0
        }
    }

    fn compute_pitch_decay(sample_rate: f32, decay_seconds: f32) -> f32 {
        // Pitch sweep completes in ~20% of the decay time.
        let sweep_samples = sample_rate * decay_seconds * 0.2;
        if sweep_samples > 0.0 {
            (-1.0 / sweep_samples).exp()
        } else {
            0.0
        }
    }

    /// High tom preset (~200 Hz, 100 ms decay).
    #[must_use]
    pub fn high(sample_rate: f32) -> Self {
        Self::new(sample_rate, 200.0, 0.1)
    }

    /// Mid tom preset (~150 Hz, 130 ms decay).
    #[must_use]
    pub fn mid(sample_rate: f32) -> Self {
        Self::new(sample_rate, 150.0, 0.13)
    }

    /// Low tom preset (~100 Hz, 200 ms decay).
    #[must_use]
    pub fn low(sample_rate: f32) -> Self {
        Self::new(sample_rate, 100.0, 0.2)
    }

    /// Set base frequency (50-400 Hz).
    pub fn set_tune(&mut self, freq: f32) {
        self.base_freq = freq.clamp(50.0, 400.0);
        self.trigger_freq = self.base_freq * 1.3;
    }

    /// Set decay time in seconds (0.05-0.5).
    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.05, 0.5);
        self.amp_decay = Self::compute_amp_decay(self.sample_rate, self.decay_time);
        self.pitch_decay = Self::compute_pitch_decay(self.sample_rate, self.decay_time);
    }
}

impl Voice for Tom {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
        self.current_freq = self.trigger_freq;
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        // Pitch sweep: exponential decay toward base frequency.
        let freq_excess = self.current_freq - self.base_freq;
        self.current_freq = freq_excess.mul_add(self.pitch_decay, self.base_freq);

        self.phase += self.current_freq / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let sample = (self.phase * std::f32::consts::TAU).sin();

        self.amplitude *= self.amp_decay;
        if self.amplitude < 1e-6 {
            self.active = false;
        }

        kazoo_core::sanitize_sample(sample * self.amplitude)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tom_presets_produce_sound() {
        for mut tom in [Tom::high(44100.0), Tom::mid(44100.0), Tom::low(44100.0)] {
            tom.trigger(1.0);
            let mut had_nonzero = false;
            for _ in 0..2000 {
                let s = tom.process();
                assert!(s.is_finite());
                if s.abs() > 1e-6 {
                    had_nonzero = true;
                }
            }
            assert!(had_nonzero, "tom should produce audible output");
        }
    }

    #[test]
    fn tom_decays_to_silence() {
        let mut tom = Tom::high(44100.0);
        tom.trigger(1.0);
        // Run for 500ms (well past 100ms decay).
        for _ in 0..22050 {
            tom.process();
        }
        assert!(!tom.is_active());
    }

    #[test]
    fn tom_pitch_sweep_starts_higher() {
        let tom = Tom::high(44100.0);
        assert!(
            tom.trigger_freq > tom.base_freq,
            "trigger freq should be above base"
        );
    }
}
