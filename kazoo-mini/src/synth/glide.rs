//! Rate-based portamento (RC slew).
//!
//! NOT time-based: an octave takes 12x longer than a semitone at the
//! same rate. This is the analog RC slew characteristic — logarithmic
//! curve that decelerates approaching the target pitch.
//!
//! Glide only activates in legato (new note while previous held).

// ---------------------------------------------------------------------------
// Glide (rate-based portamento)
// ---------------------------------------------------------------------------

/// Rate-based pitch glide (RC slew limiter).
///
/// The rate is in semitones per second. An octave (12 semitones) takes
/// 12x longer than a single semitone at the same rate. This matches the
/// analog Minimoog behavior — a constant voltage-per-second slew rate
/// applied to a 1V/octave pitch CV.
///
/// Internally we work in semitone space (log2 frequency * 12) so that
/// the slew is linear in pitch perception.
#[derive(Debug)]
pub struct Glide {
    /// Glide rate in semitones per second. 0 = instant (no glide).
    pub rate: f32,
    /// Whether glide is enabled.
    pub enabled: bool,

    // Internal state — all in semitone space
    current_semitone: f32,
    target_semitone: f32,
    sample_rate: f32,
    /// True when we have a valid current pitch (prevents glide from silence).
    has_note: bool,
}

impl Glide {
    /// Create a new glide processor.
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            rate: 60.0, // 60 semitones/sec default (~1 octave in 200ms)
            enabled: true,
            current_semitone: 0.0,
            target_semitone: 0.0,
            sample_rate: sample_rate.max(1.0),
            has_note: false,
        }
    }

    /// Update sample rate.
    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    /// Set a new target pitch.
    ///
    /// If `legato` is true and we already have a note, the glide activates.
    /// If `legato` is false (first note or non-legato), pitch jumps instantly.
    pub fn set_target(&mut self, frequency: f32, legato: bool) {
        if !frequency.is_finite() || frequency <= 0.0 {
            return;
        }

        let target_semi = freq_to_semitone(frequency);
        self.target_semitone = target_semi;

        if !self.has_note || !legato || !self.enabled || self.rate <= 0.0 {
            // Jump instantly
            self.current_semitone = target_semi;
            self.has_note = true;
        }
        // Otherwise: glide will happen in tick()

        self.has_note = true;
    }

    /// Generate the next glide sample, returning the current frequency in Hz.
    ///
    /// Call once per audio sample. The returned frequency smoothly approaches
    /// the target at the configured rate.
    #[must_use]
    pub fn tick(&mut self) -> f32 {
        if !self.has_note {
            return 0.0;
        }

        if !self.enabled || self.rate <= 0.0 {
            self.current_semitone = self.target_semitone;
            return semitone_to_freq(self.current_semitone);
        }

        let diff = self.target_semitone - self.current_semitone;

        if diff.abs() < 0.001 {
            // Close enough — snap
            self.current_semitone = self.target_semitone;
        } else {
            // Move at fixed rate (semitones per sample)
            let max_step = self.rate / self.sample_rate;
            let step = diff.signum() * max_step.min(diff.abs());
            self.current_semitone += step;
        }

        semitone_to_freq(self.current_semitone)
    }

    /// Get the current output frequency without advancing.
    #[must_use]
    pub fn current_frequency(&self) -> f32 {
        if !self.has_note {
            return 0.0;
        }
        semitone_to_freq(self.current_semitone)
    }

    /// Whether the glide is currently moving (not yet at target).
    #[must_use]
    pub fn is_gliding(&self) -> bool {
        self.has_note && (self.target_semitone - self.current_semitone).abs() > 0.001
    }

    /// Reset all state.
    pub const fn reset(&mut self) {
        self.current_semitone = 0.0;
        self.target_semitone = 0.0;
        self.has_note = false;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a frequency in Hz to absolute semitone number.
/// A4 (440 Hz) = semitone 69.
#[inline]
fn freq_to_semitone(freq: f32) -> f32 {
    if !freq.is_finite() || freq <= 0.0 {
        return 0.0;
    }
    12.0f32.mul_add((freq / 440.0).log2(), 69.0)
}

/// Convert an absolute semitone number to frequency in Hz.
#[inline]
fn semitone_to_freq(semitone: f32) -> f32 {
    if !semitone.is_finite() {
        return 0.0;
    }
    440.0 * ((semitone - 69.0) / 12.0).exp2()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freq_semitone_roundtrip() {
        let freq = 440.0;
        let semi = freq_to_semitone(freq);
        let recovered = semitone_to_freq(semi);
        assert!(
            (freq - recovered).abs() < 0.01,
            "roundtrip failed: {freq} -> {semi} -> {recovered}"
        );
    }

    #[test]
    fn instant_on_first_note() {
        let mut glide = Glide::new(44100.0);
        glide.set_target(440.0, false); // first note, not legato
        let f = glide.tick();
        assert!(
            (f - 440.0).abs() < 1.0,
            "first note should jump instantly, got {f}"
        );
    }

    #[test]
    fn glide_between_notes() {
        let mut glide = Glide::new(44100.0);
        glide.rate = 120.0; // 120 semitones/sec = 10 octaves/sec

        // First note: instant
        glide.set_target(440.0, false);
        let _ = glide.tick();

        // Second note: legato, should glide
        glide.set_target(880.0, true); // one octave up
        let f = glide.tick();
        // Should be slightly above 440 but not yet at 880
        assert!(
            f > 440.0 && f < 880.0,
            "should be gliding between 440 and 880, got {f}"
        );
    }

    #[test]
    fn glide_reaches_target() {
        let mut glide = Glide::new(44100.0);
        glide.rate = 1200.0; // very fast

        glide.set_target(440.0, false);
        let _ = glide.tick();

        glide.set_target(880.0, true);
        for _ in 0..44100 {
            let _ = glide.tick();
        }

        let f = glide.current_frequency();
        assert!(
            (f - 880.0).abs() < 1.0,
            "should have reached target 880, got {f}"
        );
    }

    #[test]
    fn disabled_glide_jumps() {
        let mut glide = Glide::new(44100.0);
        glide.enabled = false;

        glide.set_target(440.0, false);
        let _ = glide.tick();

        glide.set_target(880.0, true);
        let f = glide.tick();
        assert!(
            (f - 880.0).abs() < 1.0,
            "disabled glide should jump, got {f}"
        );
    }
}
