//! Cross-modulation routing.
//!
//! Osc 3 -> Osc 2 frequency (FM). Produces dirty, aggressive timbres.
//! Osc 2 -> Filter cutoff modulation.
//!
//! Also handles Osc 3 as LFO modulation source:
//! - Mod wheel -> Osc 1+2 pitch
//! - Mod wheel -> Filter cutoff
//! - Mod wheel -> Both

// ---------------------------------------------------------------------------
// Modulation wheel destination
// ---------------------------------------------------------------------------

/// Where the modulation wheel (Osc 3 LFO) routes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModWheelDest {
    /// Modulate Osc 1+2 pitch (vibrato).
    Pitch,
    /// Modulate filter cutoff (wah).
    Filter,
    /// Both pitch and filter.
    Both,
}

impl ModWheelDest {
    /// All destinations in display order.
    pub const ALL: [Self; 3] = [Self::Pitch, Self::Filter, Self::Both];

    /// Human-readable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pitch => "Pitch",
            Self::Filter => "Filter",
            Self::Both => "Both",
        }
    }

    /// Cycle to next.
    #[must_use]
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&d| d == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }
}

// ---------------------------------------------------------------------------
// Cross-modulation state
// ---------------------------------------------------------------------------

/// Cross-modulation parameters.
///
/// Controls the FM and filter modulation routing between oscillators.
#[derive(Debug)]
pub struct CrossMod {
    /// Osc 3 -> Osc 2 FM depth (0.0 = off, 1.0 = max FM).
    pub osc3_to_osc2_fm: f32,
    /// Osc 2 -> Filter cutoff modulation depth (0.0 = off, 1.0 = max).
    pub osc2_to_filter: f32,
    /// Modulation wheel amount (0.0 to 1.0).
    pub mod_wheel: f32,
    /// Modulation wheel destination.
    pub mod_wheel_dest: ModWheelDest,
}

impl CrossMod {
    /// Create with all modulation off.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            osc3_to_osc2_fm: 0.0,
            osc2_to_filter: 0.0,
            mod_wheel: 0.0,
            mod_wheel_dest: ModWheelDest::Pitch,
        }
    }

    /// Compute the FM offset for Osc 2 given the current Osc 3 output.
    ///
    /// Returns a frequency multiplier to apply to Osc 2's base frequency.
    /// FM depth of 0 returns 1.0 (no modulation).
    #[inline]
    #[must_use]
    pub fn osc2_fm_multiplier(&self, osc3_output: f32) -> f32 {
        if self.osc3_to_osc2_fm <= 0.0 {
            return 1.0;
        }
        // FM synthesis: frequency deviation proportional to modulator output
        // Scale: at max depth (1.0), osc3 can deviate osc2 by ±1 octave
        let deviation = osc3_output * self.osc3_to_osc2_fm;
        // Convert to frequency ratio: 2^deviation gives octave-scale FM
        deviation.exp2()
    }

    /// Compute the filter cutoff modulation from Osc 2.
    ///
    /// Returns an additive offset in Hz to apply to the filter cutoff.
    #[inline]
    #[must_use]
    pub fn filter_mod_hz(&self, osc2_output: f32, base_cutoff: f32) -> f32 {
        if self.osc2_to_filter <= 0.0 {
            return 0.0;
        }
        // Osc 2 modulates cutoff: at max depth, ±2 octaves of cutoff sweep
        osc2_output * self.osc2_to_filter * base_cutoff
    }

    /// Compute pitch modulation from the mod wheel (LFO via Osc 3).
    ///
    /// Returns a frequency multiplier for Osc 1+2 pitch.
    #[inline]
    #[must_use]
    pub fn mod_wheel_pitch_multiplier(&self, osc3_lfo_output: f32) -> f32 {
        if self.mod_wheel <= 0.0 {
            return 1.0;
        }
        let applies = matches!(
            self.mod_wheel_dest,
            ModWheelDest::Pitch | ModWheelDest::Both
        );
        if !applies {
            return 1.0;
        }
        // Vibrato: ±1 semitone at max mod wheel
        let deviation = osc3_lfo_output * self.mod_wheel * (1.0 / 12.0);
        deviation.exp2()
    }

    /// Compute filter cutoff modulation from the mod wheel (LFO via Osc 3).
    ///
    /// Returns an additive offset in Hz.
    #[inline]
    #[must_use]
    pub fn mod_wheel_filter_hz(&self, osc3_lfo_output: f32, base_cutoff: f32) -> f32 {
        if self.mod_wheel <= 0.0 {
            return 0.0;
        }
        let applies = matches!(
            self.mod_wheel_dest,
            ModWheelDest::Filter | ModWheelDest::Both
        );
        if !applies {
            return 0.0;
        }
        // Filter wah: ±1 octave of cutoff at max
        osc3_lfo_output * self.mod_wheel * base_cutoff
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fm_returns_unity() {
        let xmod = CrossMod::new();
        assert!(
            (xmod.osc2_fm_multiplier(0.5) - 1.0).abs() < f32::EPSILON,
            "zero FM depth should return 1.0"
        );
    }

    #[test]
    fn fm_modulates_frequency() {
        let mut xmod = CrossMod::new();
        xmod.osc3_to_osc2_fm = 1.0;

        let up = xmod.osc2_fm_multiplier(1.0);
        assert!(
            up > 1.5,
            "positive modulator should increase freq, got {up}"
        );

        let down = xmod.osc2_fm_multiplier(-1.0);
        assert!(
            down < 0.6,
            "negative modulator should decrease freq, got {down}"
        );
    }

    #[test]
    fn filter_mod_zero_when_off() {
        let xmod = CrossMod::new();
        let m = xmod.filter_mod_hz(0.5, 1000.0);
        assert!(m.abs() < f32::EPSILON, "zero depth should produce no mod");
    }

    #[test]
    fn mod_wheel_pitch_off_when_filter_only() {
        let mut xmod = CrossMod::new();
        xmod.mod_wheel = 1.0;
        xmod.mod_wheel_dest = ModWheelDest::Filter;
        let mult = xmod.mod_wheel_pitch_multiplier(1.0);
        assert!(
            (mult - 1.0).abs() < f32::EPSILON,
            "filter-only should not modulate pitch"
        );
    }
}
