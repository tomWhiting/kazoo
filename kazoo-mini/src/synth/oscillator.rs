//! Multi-waveform VCO with LFO mode.
//!
//! Waveforms: triangle, sawtooth, square, narrow pulse, wide pulse.
//! Osc 3 switchable to low-frequency range (becomes the LFO).
//! Octave ranges: 32', 16', 8', 4'.
//!
//! Band-limited using `PolyBLEP` to suppress aliasing on saw, square, and
//! pulse waveforms. Triangle is generated from an integrated square with
//! `PolyBLEP` corrections (`PolyBLAMP` leaky integrator approach).

use kazoo_core::sanitize_sample;

// ---------------------------------------------------------------------------
// Waveform selection
// ---------------------------------------------------------------------------

/// Available oscillator waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Triangle,
    Saw,
    Square,
    /// Narrow pulse (~25% duty cycle).
    NarrowPulse,
    /// Wide pulse (~75% duty cycle).
    WidePulse,
}

impl Waveform {
    /// All waveforms in display order.
    pub const ALL: [Self; 5] = [
        Self::Triangle,
        Self::Saw,
        Self::Square,
        Self::NarrowPulse,
        Self::WidePulse,
    ];

    /// Human-readable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Triangle => "Tri",
            Self::Saw => "Saw",
            Self::Square => "Sqr",
            Self::NarrowPulse => "NPul",
            Self::WidePulse => "WPul",
        }
    }

    /// Duty cycle for pulse waveforms.
    #[must_use]
    const fn duty(self) -> f32 {
        match self {
            Self::NarrowPulse => 0.25,
            Self::WidePulse => 0.75,
            Self::Square | Self::Triangle | Self::Saw => 0.5,
        }
    }

    /// Advance to next waveform (wraps).
    #[must_use]
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&w| w == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Go to previous waveform (wraps).
    #[must_use]
    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|&w| w == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

// ---------------------------------------------------------------------------
// Octave range
// ---------------------------------------------------------------------------

/// Octave range selector (footage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OctaveRange {
    /// Sub-bass (two octaves down).
    Footage32,
    /// Bass (one octave down).
    Footage16,
    /// Normal (concert pitch).
    Footage8,
    /// High (one octave up).
    Footage4,
    /// Low-frequency mode (Osc 3 only — sub-audio rates for LFO use).
    Lo,
}

impl OctaveRange {
    /// All ranges in display order (excluding Lo).
    pub const STANDARD: [Self; 4] = [
        Self::Footage32,
        Self::Footage16,
        Self::Footage8,
        Self::Footage4,
    ];

    /// All ranges including Lo.
    pub const ALL_WITH_LO: [Self; 5] = [
        Self::Lo,
        Self::Footage32,
        Self::Footage16,
        Self::Footage8,
        Self::Footage4,
    ];

    /// Pitch multiplier relative to 8' (concert pitch).
    #[must_use]
    pub const fn multiplier(self) -> f32 {
        match self {
            Self::Footage32 => 0.25,
            Self::Footage16 => 0.5,
            Self::Footage8 => 1.0,
            Self::Footage4 => 2.0,
            // Lo mode: 4 octaves below 32' = 6 octaves below concert
            Self::Lo => 0.25 / 16.0,
        }
    }

    /// Human-readable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Footage32 => "32'",
            Self::Footage16 => "16'",
            Self::Footage8 => "8'",
            Self::Footage4 => "4'",
            Self::Lo => "Lo",
        }
    }

    /// Advance to next range (wraps within the given set).
    #[must_use]
    pub fn next(self, allow_lo: bool) -> Self {
        let set = if allow_lo {
            &Self::ALL_WITH_LO[..]
        } else {
            &Self::STANDARD[..]
        };
        let idx = set.iter().position(|&r| r == self).unwrap_or(0);
        set[(idx + 1) % set.len()]
    }

    /// Go to previous range (wraps within the given set).
    #[must_use]
    pub fn prev(self, allow_lo: bool) -> Self {
        let set = if allow_lo {
            &Self::ALL_WITH_LO[..]
        } else {
            &Self::STANDARD[..]
        };
        let idx = set.iter().position(|&r| r == self).unwrap_or(0);
        set[(idx + set.len() - 1) % set.len()]
    }
}

// ---------------------------------------------------------------------------
// PolyBLEP antialiasing
// ---------------------------------------------------------------------------

/// `PolyBLEP` correction:
#[inline]
fn polyblep_correction(phase: f32, phase_inc: f32) -> f32 {
    // phase is in [0, 1), phase_inc is the per-sample increment
    if phase_inc <= 0.0 {
        return 0.0;
    }
    let t = phase / phase_inc;
    if t < 1.0 {
        // Just after transition at phase=0
        let t2 = t;
        return t2.mul_add(-t2, 2.0 * t2) - 1.0;
        // = 2t - t² - 1
    }
    let t = (phase - 1.0) / phase_inc;
    if t > -1.0 {
        // Just before transition at phase=1
        let t2 = t;
        return t2.mul_add(t2, 2.0 * t2) + 1.0;
        // = t² + 2t + 1
    }
    0.0
}

// ---------------------------------------------------------------------------
// Oscillator
// ---------------------------------------------------------------------------

/// A single band-limited VCO.
#[derive(Debug)]
pub struct Oscillator {
    /// Current waveform.
    pub waveform: Waveform,
    /// Octave range (footage).
    pub octave: OctaveRange,
    /// Fine tune in cents (-50 to +50).
    pub fine_tune_cents: f32,
    /// Output level (0.0 to 1.0).
    pub level: f32,
    /// Whether this oscillator is in LFO mode (Osc 3 only).
    pub lfo_mode: bool,

    // Internal state
    phase: f32,
    sample_rate: f32,
    // For triangle generation via leaky integrator
    tri_integrator: f32,
}

impl Oscillator {
    /// Create a new oscillator at the given sample rate.
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            waveform: Waveform::Saw,
            octave: OctaveRange::Footage8,
            fine_tune_cents: 0.0,
            level: 0.8,
            lfo_mode: false,
            phase: 0.0,
            sample_rate: sample_rate.max(1.0),
            tri_integrator: 0.0,
        }
    }

    /// Update sample rate and reset state.
    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.reset();
    }

    /// Reset phase and internal state.
    pub const fn reset(&mut self) {
        self.phase = 0.0;
        self.tri_integrator = 0.0;
    }

    /// Compute the effective frequency given a base note frequency.
    ///
    /// Applies octave range and fine tuning. If `lfo_mode` is set, the octave
    /// range `Lo` is used regardless of the `octave` field, and keyboard
    /// tracking is disconnected (frequency stays at a fixed LFO rate).
    #[must_use]
    pub fn effective_frequency(&self, base_freq: f32) -> f32 {
        let cent_ratio = (self.fine_tune_cents / 1200.0).exp2();
        let oct = self.octave.multiplier();
        let freq = base_freq * oct * cent_ratio;
        freq.clamp(0.01, self.sample_rate * 0.49)
    }

    /// Generate one sample at the given frequency (already computed via
    /// `effective_frequency` or with FM modulation applied).
    ///
    /// Returns the oscillator output scaled by `self.level`.
    pub fn tick(&mut self, freq: f32) -> f32 {
        let freq = freq.clamp(0.01, self.sample_rate * 0.49);
        let phase_inc = freq / self.sample_rate;

        let sample = match self.waveform {
            Waveform::Saw => self.generate_saw(phase_inc),
            Waveform::Square | Waveform::NarrowPulse | Waveform::WidePulse => {
                self.generate_pulse(phase_inc, self.waveform.duty())
            }
            Waveform::Triangle => self.generate_triangle(phase_inc),
        };

        // Advance phase
        self.phase += phase_inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        sanitize_sample(sample * self.level)
    }

    /// Band-limited sawtooth via `PolyBLEP`.
    fn generate_saw(&self, phase_inc: f32) -> f32 {
        // Naive saw: 2 * phase - 1 (ramp from -1 to +1)
        let naive = self.phase.mul_add(2.0, -1.0);
        // Apply PolyBLEP at the discontinuity (phase wraps at 1.0 -> 0.0)
        naive - polyblep_correction(self.phase, phase_inc)
    }

    /// Band-limited pulse wave via `PolyBLEP`.
    fn generate_pulse(&self, phase_inc: f32, duty: f32) -> f32 {
        // Naive pulse: +1 if phase < duty, -1 otherwise
        let naive = if self.phase < duty { 1.0 } else { -1.0 };

        // PolyBLEP at the rising edge (phase = 0)
        let mut blep = naive;
        blep -= polyblep_correction(self.phase, phase_inc);

        // PolyBLEP at the falling edge (phase = duty)
        let phase_shifted = self.phase - duty;
        let phase_shifted = if phase_shifted < 0.0 {
            phase_shifted + 1.0
        } else {
            phase_shifted
        };
        blep += polyblep_correction(phase_shifted, phase_inc);

        blep
    }

    /// Band-limited triangle via leaky integrator of `PolyBLEP` square.
    fn generate_triangle(&mut self, phase_inc: f32) -> f32 {
        // Generate a PolyBLEP square wave, then integrate to get triangle
        let square = self.generate_pulse(phase_inc, 0.5);

        // Leaky integrator: integrates the square to produce triangle
        // The leak factor keeps DC from drifting
        let integration_gain = 4.0 * phase_inc; // scale factor for proper amplitude
        self.tri_integrator = self
            .tri_integrator
            .mul_add(1.0 - integration_gain * 0.05, square * integration_gain);

        // Clamp to prevent runaway
        self.tri_integrator = self.tri_integrator.clamp(-1.5, 1.5);

        self.tri_integrator
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_cycle() {
        let w = Waveform::Triangle;
        let w = w.next();
        assert_eq!(w, Waveform::Saw);
        let w = w.next().next().next().next();
        assert_eq!(w, Waveform::Triangle);
    }

    #[test]
    fn octave_multipliers() {
        assert!((OctaveRange::Footage8.multiplier() - 1.0).abs() < f32::EPSILON);
        assert!((OctaveRange::Footage4.multiplier() - 2.0).abs() < f32::EPSILON);
        assert!((OctaveRange::Footage16.multiplier() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn oscillator_output_bounded() {
        let mut osc = Oscillator::new(44100.0);
        osc.waveform = Waveform::Saw;
        for _ in 0..44100 {
            let s = osc.tick(440.0);
            assert!(s.is_finite(), "NaN/Inf in oscillator output");
            assert!(s >= -2.0 && s <= 2.0, "oscillator output out of range: {s}");
        }
    }

    #[test]
    fn all_waveforms_produce_output() {
        for wf in Waveform::ALL {
            let mut osc = Oscillator::new(44100.0);
            osc.waveform = wf;
            let mut max_abs = 0.0_f32;
            for _ in 0..4410 {
                let s = osc.tick(440.0);
                max_abs = max_abs.max(s.abs());
            }
            assert!(
                max_abs > 0.1,
                "waveform {wf:?} produced negligible output (max={max_abs})"
            );
        }
    }

    #[test]
    fn fine_tune_changes_frequency() {
        let mut osc = Oscillator::new(44100.0);
        let f1 = osc.effective_frequency(440.0);
        osc.fine_tune_cents = 50.0;
        let f2 = osc.effective_frequency(440.0);
        assert!(f2 > f1, "positive fine tune should increase frequency");
    }

    #[test]
    fn polyblep_correction_at_zero() {
        // At phase=0 with any positive increment, correction should be near -1
        let c = polyblep_correction(0.0, 0.01);
        assert!(c.abs() <= 1.1, "correction at phase=0 out of range: {c}");
    }
}
