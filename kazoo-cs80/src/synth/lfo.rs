//! Multi-waveform LFO: sine, sawtooth, ramp, pulse, noise.
//! Independent depth controls per destination (pitch, filter, VCA).
//! Rate range extends into audio for FM effects.

use std::f32::consts::TAU;

use rand::Rng;

/// LFO waveform shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LfoWaveform {
    Sine,
    Sawtooth,
    Ramp,
    Pulse,
    Noise,
}

/// LFO modulation routing depths.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct LfoRouting {
    /// Pitch modulation depth in cents [0, 100].
    pub pitch_cents: f32,
    /// Filter cutoff modulation depth [0, 1] (fraction of cutoff range).
    pub filter_depth: f32,
    /// VCA amplitude modulation depth [0, 1].
    pub vca_depth: f32,
}

impl Default for LfoRouting {
    fn default() -> Self {
        Self {
            pitch_cents: 0.0,
            filter_depth: 0.0,
            vca_depth: 0.0,
        }
    }
}

/// Multi-waveform low-frequency oscillator with per-destination routing.
#[derive(Debug)]
pub struct Lfo {
    /// Current phase [0, 1).
    phase: f32,
    /// Phase increment per sample.
    phase_inc: f32,
    /// Rate in Hz.
    pub rate: f32,
    /// Waveform shape.
    pub waveform: LfoWaveform,
    /// Per-destination modulation depths.
    pub routing: LfoRouting,
    /// Sample rate.
    sample_rate: f32,
    /// Noise: current held value (sample-and-hold noise).
    noise_value: f32,
    /// Noise: previous phase for edge detection.
    noise_prev_phase: f32,
}

impl Lfo {
    /// Create a new LFO.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            phase_inc: 0.0,
            rate: 2.5,
            waveform: LfoWaveform::Sine,
            routing: LfoRouting::default(),
            sample_rate: sample_rate.max(1.0),
            noise_value: 0.0,
            noise_prev_phase: 0.0,
        }
    }

    /// Set LFO rate in Hz [0.01, 100].
    pub fn set_rate(&mut self, hz: f32) {
        self.rate = hz.clamp(0.01, 100.0);
        self.phase_inc = self.rate / self.sample_rate;
    }

    /// Update sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.phase_inc = self.rate / self.sample_rate;
    }

    /// Reset phase to zero.
    pub const fn reset(&mut self) {
        self.phase = 0.0;
        self.noise_value = 0.0;
        self.noise_prev_phase = 0.0;
    }

    /// Process one sample, returning modulation outputs.
    ///
    /// Returns `(pitch_mod_ratio, filter_mod, vca_mod)`:
    /// - `pitch_mod_ratio`: frequency multiplier (1.0 = no mod)
    /// - `filter_mod`: additive cutoff offset [-1, 1] scaled by depth
    /// - `vca_mod`: amplitude multiplier [0, 1] (1.0 = no mod)
    #[inline]
    pub fn tick(&mut self) -> (f32, f32, f32) {
        let raw = self.raw_value();

        // Advance phase
        self.noise_prev_phase = self.phase;
        self.phase += self.phase_inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        if !self.phase.is_finite() || self.phase < 0.0 {
            self.phase = 0.0;
        }

        // Pitch modulation: raw [-1,1] * depth_cents -> frequency ratio
        let pitch_cents = raw * self.routing.pitch_cents;
        let pitch_ratio = (pitch_cents / 1200.0).exp2();

        // Filter modulation: raw [-1,1] * depth
        let filter_mod = raw * self.routing.filter_depth;

        // VCA modulation: convert bipolar to unipolar tremolo
        // depth=0: output=1.0 (no tremolo). depth=1: full tremolo
        let vca_mod = (self.routing.vca_depth * 0.5).mul_add(-(1.0 - raw), 1.0);

        (
            if pitch_ratio.is_finite() {
                pitch_ratio
            } else {
                1.0
            },
            if filter_mod.is_finite() {
                filter_mod
            } else {
                0.0
            },
            if vca_mod.is_finite() {
                vca_mod.clamp(0.0, 1.0)
            } else {
                1.0
            },
        )
    }

    /// Get raw waveform value [-1, 1].
    fn raw_value(&mut self) -> f32 {
        match self.waveform {
            LfoWaveform::Sine => (self.phase * TAU).sin(),
            LfoWaveform::Sawtooth => 2.0f32.mul_add(self.phase, -1.0),
            LfoWaveform::Ramp => 2.0f32.mul_add(-self.phase, 1.0),
            LfoWaveform::Pulse => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWaveform::Noise => {
                // Sample-and-hold: new random value each cycle
                if self.phase < self.noise_prev_phase {
                    // Phase wrapped — new cycle
                    self.noise_value = rand::rng().random_range(-1.0_f32..=1.0);
                }
                self.noise_value
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfo_sine_output_range() {
        let mut lfo = Lfo::new(44100.0);
        lfo.set_rate(5.0);
        lfo.routing.pitch_cents = 10.0;
        lfo.routing.filter_depth = 0.5;
        lfo.routing.vca_depth = 0.3;

        for _ in 0..44100 {
            let (pitch, filter, vca) = lfo.tick();
            assert!(pitch.is_finite() && pitch > 0.0);
            assert!(filter.is_finite());
            assert!(vca.is_finite() && vca >= 0.0 && vca <= 1.0);
        }
    }

    #[test]
    fn lfo_no_modulation_when_depth_zero() {
        let mut lfo = Lfo::new(44100.0);
        lfo.set_rate(5.0);
        // All depths at zero
        lfo.routing = LfoRouting::default();

        for _ in 0..4410 {
            let (pitch, filter, vca) = lfo.tick();
            assert!(
                (pitch - 1.0).abs() < 0.001,
                "zero-depth pitch should be ~1.0, got {pitch}"
            );
            assert!(
                filter.abs() < 0.001,
                "zero-depth filter should be ~0, got {filter}"
            );
            assert!(
                (vca - 1.0).abs() < 0.001,
                "zero-depth vca should be ~1.0, got {vca}"
            );
        }
    }

    #[test]
    fn lfo_all_waveforms_finite() {
        for wf in [
            LfoWaveform::Sine,
            LfoWaveform::Sawtooth,
            LfoWaveform::Ramp,
            LfoWaveform::Pulse,
            LfoWaveform::Noise,
        ] {
            let mut lfo = Lfo::new(44100.0);
            lfo.set_rate(5.0);
            lfo.waveform = wf;
            lfo.routing.pitch_cents = 10.0;
            lfo.routing.filter_depth = 0.5;

            for _ in 0..44100 {
                let (p, f, v) = lfo.tick();
                assert!(p.is_finite(), "{wf:?} pitch not finite");
                assert!(f.is_finite(), "{wf:?} filter not finite");
                assert!(v.is_finite(), "{wf:?} vca not finite");
            }
        }
    }

    #[test]
    fn lfo_reset_clears_phase() {
        let mut lfo = Lfo::new(44100.0);
        lfo.set_rate(5.0);
        for _ in 0..1000 {
            lfo.tick();
        }
        lfo.reset();
        assert!((lfo.phase - 0.0).abs() < f32::EPSILON);
    }
}
