//! CS-80 VCO: sawtooth, variable-width pulse (PWM), sine.
//!
//! Sine output is post-filter (injected after the HPF/LPF chain).
//! Octave ranges: 32', 16', 8', 4'.

use std::f32::consts::TAU;

/// Waveform selection for the VCO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Waveform {
    Sawtooth,
    Pulse,
    Sine,
}

/// Octave footage — determines the octave transposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OctaveRange {
    /// 32' — two octaves down.
    ThirtyTwo,
    /// 16' — one octave down.
    Sixteen,
    /// 8' — concert pitch.
    Eight,
    /// 4' — one octave up.
    Four,
}

impl OctaveRange {
    /// Frequency multiplier for this footage.
    #[must_use]
    pub const fn multiplier(self) -> f32 {
        match self {
            Self::ThirtyTwo => 0.25,
            Self::Sixteen => 0.5,
            Self::Eight => 1.0,
            Self::Four => 2.0,
        }
    }
}

/// CS-80 voltage-controlled oscillator.
///
/// Generates sawtooth, variable-width pulse, and sine waveforms.
/// The sawtooth uses a polyBLEP anti-aliasing technique.
#[derive(Debug)]
pub struct Oscillator {
    /// Current phase [0, 1).
    phase: f32,
    /// Phase increment per sample.
    phase_inc: f32,
    /// Sample rate in Hz.
    sample_rate: f32,
    /// Active waveform.
    pub waveform: Waveform,
    /// Octave footage.
    pub octave_range: OctaveRange,
    /// Pulse width for PWM [0.5, 0.9].
    pub pulse_width: f32,
    /// Fine tune in cents [-100, +100].
    pub fine_tune_cents: f32,
}

impl Oscillator {
    /// Create a new oscillator at the given sample rate.
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            phase_inc: 0.0,
            sample_rate: sample_rate.max(1.0),
            waveform: Waveform::Sawtooth,
            octave_range: OctaveRange::Eight,
            pulse_width: 0.5,
            fine_tune_cents: 0.0,
        }
    }

    /// Set the base frequency (before octave range and fine tune).
    pub fn set_frequency(&mut self, freq_hz: f32) {
        let cents_ratio = (self.fine_tune_cents / 1200.0).exp2();
        let actual_freq = freq_hz * self.octave_range.multiplier() * cents_ratio;
        self.phase_inc = actual_freq / self.sample_rate;
    }

    /// Update sample rate and recalculate phase increment.
    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    /// Reset phase to zero.
    pub const fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generate one sample. Returns (`filtered_output`, `sine_output`).
    ///
    /// `filtered_output` goes into the HPF -> LPF chain.
    /// `sine_output` is injected post-filter (sine has no harmonics to filter).
    #[inline]
    pub fn tick(&mut self) -> (f32, f32) {
        let sample = match self.waveform {
            Waveform::Sawtooth => self.saw_polyblep(),
            Waveform::Pulse => self.pulse_polyblep(),
            Waveform::Sine => 0.0, // sine goes post-filter only
        };

        let sine = if self.waveform == Waveform::Sine {
            (self.phase * TAU).sin()
        } else {
            0.0
        };

        // Advance phase
        self.phase += self.phase_inc;
        // Wrap without branching on normal values
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        // Safety: clamp phase to valid range
        if !self.phase.is_finite() || self.phase < 0.0 {
            self.phase = 0.0;
        }

        (sample, sine)
    }

    /// Naive sawtooth with polyBLEP anti-aliasing.
    #[inline]
    fn saw_polyblep(&self) -> f32 {
        // Naive saw: maps [0,1) phase to [-1,1)
        let naive = 2.0f32.mul_add(self.phase, -1.0);
        naive - Self::poly_blep(self.phase, self.phase_inc)
    }

    /// Variable-width pulse with polyBLEP on both edges.
    #[inline]
    fn pulse_polyblep(&self) -> f32 {
        let pw = self.pulse_width.clamp(0.05, 0.95);
        let naive = if self.phase < pw { 1.0 } else { -1.0 };

        // Apply polyBLEP correction at both discontinuities
        let mut out = naive;
        out -= Self::poly_blep(self.phase, self.phase_inc);

        let phase2 = self.phase - pw;
        let phase2 = if phase2 < 0.0 { phase2 + 1.0 } else { phase2 };
        out += Self::poly_blep(phase2, self.phase_inc);

        out
    }

    /// `PolyBLEP` residual for anti-aliasing discontinuities.
    ///
    /// `t` is phase position [0,1), `dt` is phase increment per sample.
    #[inline]
    fn poly_blep(t: f32, dt: f32) -> f32 {
        if dt <= 0.0 || !dt.is_finite() {
            return 0.0;
        }
        if t < dt {
            // Rising edge at phase = 0
            let t_norm = t / dt;
            return t_norm.mul_add(-t_norm, t_norm + t_norm) - 1.0;
        }
        if t > 1.0 - dt {
            // Falling edge at phase = 1
            let t_norm = (t - 1.0) / dt;
            return t_norm * t_norm + t_norm + t_norm + 1.0;
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oscillator_creates_at_sample_rate() {
        let osc = Oscillator::new(44100.0);
        assert!((osc.sample_rate - 44100.0).abs() < f32::EPSILON);
        assert!((osc.phase - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn saw_output_in_range() {
        let mut osc = Oscillator::new(44100.0);
        osc.waveform = Waveform::Sawtooth;
        osc.set_frequency(440.0);
        for _ in 0..44100 {
            let (filtered, sine) = osc.tick();
            assert!(filtered.is_finite(), "saw sample not finite: {filtered}");
            assert!(
                filtered >= -2.0 && filtered <= 2.0,
                "saw sample out of range: {filtered}"
            );
            assert!(
                (sine - 0.0).abs() < f32::EPSILON,
                "saw should have no sine output"
            );
        }
    }

    #[test]
    fn pulse_output_in_range() {
        let mut osc = Oscillator::new(44100.0);
        osc.waveform = Waveform::Pulse;
        osc.pulse_width = 0.5;
        osc.set_frequency(440.0);
        for _ in 0..44100 {
            let (filtered, sine) = osc.tick();
            assert!(filtered.is_finite(), "pulse sample not finite: {filtered}");
            assert!(
                filtered >= -2.0 && filtered <= 2.0,
                "pulse sample out of range: {filtered}"
            );
            assert!((sine - 0.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn sine_output_in_range() {
        let mut osc = Oscillator::new(44100.0);
        osc.waveform = Waveform::Sine;
        osc.set_frequency(440.0);
        for _ in 0..44100 {
            let (filtered, sine) = osc.tick();
            assert!(
                (filtered - 0.0).abs() < f32::EPSILON,
                "sine waveform should have no filtered output"
            );
            assert!(sine.is_finite());
            assert!(sine >= -1.01 && sine <= 1.01);
        }
    }

    #[test]
    fn octave_range_multipliers() {
        assert!((OctaveRange::ThirtyTwo.multiplier() - 0.25).abs() < f32::EPSILON);
        assert!((OctaveRange::Sixteen.multiplier() - 0.5).abs() < f32::EPSILON);
        assert!((OctaveRange::Eight.multiplier() - 1.0).abs() < f32::EPSILON);
        assert!((OctaveRange::Four.multiplier() - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn reset_clears_phase() {
        let mut osc = Oscillator::new(44100.0);
        osc.set_frequency(440.0);
        for _ in 0..100 {
            osc.tick();
        }
        assert!(osc.phase > 0.0);
        osc.reset();
        assert!((osc.phase - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nan_frequency_safe() {
        let mut osc = Oscillator::new(44100.0);
        osc.set_frequency(f32::NAN);
        for _ in 0..100 {
            let (f, s) = osc.tick();
            assert!(f.is_finite());
            assert!(s.is_finite());
        }
    }
}
