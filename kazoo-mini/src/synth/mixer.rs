//! Oscillator mixer: Osc 1-3 levels + noise + external input.
//! The mix feeds the ladder filter.

use kazoo_core::sanitize_sample;

// ---------------------------------------------------------------------------
// Noise generator
// ---------------------------------------------------------------------------

/// Simple white noise generator using xorshift32.
///
/// Produces uniform white noise in [-1.0, 1.0]. No allocations.
#[derive(Debug)]
pub struct NoiseGenerator {
    state: u32,
}

impl NoiseGenerator {
    /// Create a new noise generator with a non-zero seed.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self {
            // Ensure non-zero state (xorshift requirement)
            state: if seed == 0 { 0x5EED_BEEF } else { seed },
        }
    }

    /// Generate the next noise sample in [-1.0, 1.0].
    #[inline]
    pub fn tick(&mut self) -> f32 {
        // xorshift32
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;

        // Convert to float in [-1, 1]
        // Map u32 to [-1.0, 1.0] uniformly
        (self.state as f32 / (u32::MAX as f32 / 2.0)) - 1.0
    }
}

// ---------------------------------------------------------------------------
// Oscillator Mixer
// ---------------------------------------------------------------------------

/// Mixes the three oscillator outputs plus noise and external input.
///
/// Each source has an independent level control (0.0 to 1.0).
#[derive(Debug)]
pub struct OscMixer {
    /// Osc 1 level (0.0 to 1.0).
    pub osc1_level: f32,
    /// Osc 2 level (0.0 to 1.0).
    pub osc2_level: f32,
    /// Osc 3 level (0.0 to 1.0).
    pub osc3_level: f32,
    /// Noise level (0.0 to 1.0).
    pub noise_level: f32,
    /// External input level (0.0 to 1.0).
    pub ext_level: f32,

    noise: NoiseGenerator,
}

impl OscMixer {
    /// Create a new mixer with default levels.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            osc1_level: 0.8,
            osc2_level: 0.75,
            osc3_level: 0.0,
            noise_level: 0.0,
            ext_level: 0.0,
            noise: NoiseGenerator::new(0xDEAD_CAFE),
        }
    }

    /// Mix oscillator outputs, noise, and external input into a single sample.
    ///
    /// `osc1`, `osc2`, `osc3` are raw oscillator outputs (pre-level).
    /// `external` is the external audio input (if any).
    #[inline]
    pub fn mix(&mut self, osc1: f32, osc2: f32, osc3: f32, external: f32) -> f32 {
        let noise = self.noise.tick();

        let mixed = external.mul_add(
            self.ext_level,
            osc1.mul_add(self.osc1_level, osc2 * self.osc2_level)
                + osc3.mul_add(self.osc3_level, noise * self.noise_level),
        );

        sanitize_sample(mixed)
    }

    /// Reset noise generator state.
    pub const fn reset(&mut self) {
        self.noise = NoiseGenerator::new(0xDEAD_CAFE);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_output_bounded() {
        let mut ng = NoiseGenerator::new(12345);
        for _ in 0..10000 {
            let s = ng.tick();
            assert!(
                s.is_finite() && s >= -1.1 && s <= 1.1,
                "noise out of range: {s}"
            );
        }
    }

    #[test]
    fn noise_not_constant() {
        let mut ng = NoiseGenerator::new(12345);
        let s1 = ng.tick();
        let s2 = ng.tick();
        assert!(
            (s1 - s2).abs() > f32::EPSILON,
            "noise should vary between samples"
        );
    }

    #[test]
    fn mixer_zero_levels() {
        let mut mixer = OscMixer::new();
        mixer.osc1_level = 0.0;
        mixer.osc2_level = 0.0;
        mixer.osc3_level = 0.0;
        mixer.noise_level = 0.0;
        mixer.ext_level = 0.0;

        let s = mixer.mix(1.0, 1.0, 1.0, 1.0);
        assert!(
            s.abs() < f32::EPSILON,
            "all-zero levels should produce silence, got {s}"
        );
    }

    #[test]
    fn mixer_passes_osc1() {
        let mut mixer = OscMixer::new();
        mixer.osc1_level = 1.0;
        mixer.osc2_level = 0.0;
        mixer.osc3_level = 0.0;
        mixer.noise_level = 0.0;
        mixer.ext_level = 0.0;

        let s = mixer.mix(0.5, 0.0, 0.0, 0.0);
        assert!(
            (s - 0.5).abs() < f32::EPSILON,
            "osc1 at unity should pass through, got {s}"
        );
    }

    #[test]
    fn mixer_output_finite() {
        let mut mixer = OscMixer::new();
        let s = mixer.mix(f32::NAN, 0.5, f32::INFINITY, 0.0);
        assert!(s.is_finite(), "mixer should sanitize non-finite inputs");
    }
}
