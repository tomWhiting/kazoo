//! Deterministic mathematical noise source.
//!
//! This is not sample playback. It is a tiny xorshift generator used as a
//! white-noise voltage source, matching the role of noise in an analog synth.

/// White-noise generator with stable per-voice seeding.
#[derive(Debug, Clone)]
pub struct WhiteNoise {
    state: u32,
}

impl WhiteNoise {
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self { state: seed | 1 }
    }

    pub const fn reset(&mut self, seed: u32) {
        self.state = seed | 1;
    }

    #[inline]
    pub fn next_bipolar(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        let unit = (self.state >> 8) as f32 / 16_777_216.0;
        unit.mul_add(2.0, -1.0)
    }
}
