//! Effects processing: filters, reverb, delay, chorus, distortion, formant shifting.
//!
//! Each effect implements [`crate::Processor`]. The [`EffectChain`] struct
//! allows composing multiple effects in series with per-slot bypass.

pub mod chorus;
pub mod delay;
pub mod distortion;
pub mod filter;
pub mod formant_shift;
pub mod reverb;

pub use chorus::Chorus;
pub use delay::Delay;
pub use distortion::{Distortion, DistortionType};
pub use filter::{BiquadFilter, FilterType};
pub use formant_shift::FormantShift;
pub use reverb::Reverb;

use crate::{Processor, sanitize_buffer};

// ---------------------------------------------------------------------------
// EffectSlot
// ---------------------------------------------------------------------------

/// A single slot in the effect chain holding a processor and bypass state.
pub struct EffectSlot {
    /// The underlying effect processor.
    pub processor: Box<dyn Processor>,
    /// When `true`, this slot passes audio through unmodified.
    pub bypassed: bool,
}

impl std::fmt::Debug for EffectSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectSlot")
            .field("processor", &self.processor.name())
            .field("bypassed", &self.bypassed)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// EffectChain
// ---------------------------------------------------------------------------

/// A serial chain of effects with per-slot bypass.
///
/// Audio flows through each non-bypassed effect in order. An internal scratch
/// buffer avoids per-call allocation.
#[derive(Debug)]
pub struct EffectChain {
    effects: Vec<EffectSlot>,
    scratch_buffer: Vec<f32>,
}

impl EffectChain {
    /// Create an empty effect chain.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            effects: Vec::new(),
            scratch_buffer: Vec::new(),
        }
    }

    /// Create an empty effect chain with scratch buffer pre-allocated to
    /// `max_block_size` samples, avoiding runtime allocation in [`process`].
    #[must_use]
    pub fn new_with_capacity(max_block_size: usize) -> Self {
        Self {
            effects: Vec::new(),
            scratch_buffer: vec![0.0; max_block_size],
        }
    }

    /// Pre-allocate the internal scratch buffer for the given block size.
    ///
    /// Call this from [`crate::mixer::Mixer::prepare`] so that subsequent
    /// [`process`] calls never need to resize.
    pub fn prepare(&mut self, buffer_size: usize) {
        if self.scratch_buffer.len() < buffer_size {
            self.scratch_buffer.resize(buffer_size, 0.0);
        }
    }

    /// Append an effect to the end of the chain.
    pub fn push(&mut self, effect: Box<dyn Processor>) {
        self.effects.push(EffectSlot {
            processor: effect,
            bypassed: false,
        });
    }

    /// Remove and return the effect at `index`, or `None` if out of range.
    pub fn remove(&mut self, index: usize) -> Option<Box<dyn Processor>> {
        if index < self.effects.len() {
            Some(self.effects.remove(index).processor)
        } else {
            None
        }
    }

    /// Set the bypass state of the effect at `index`.
    ///
    /// Does nothing if the index is out of range.
    pub fn set_bypass(&mut self, index: usize, bypassed: bool) {
        if let Some(slot) = self.effects.get_mut(index) {
            slot.bypassed = bypassed;
        }
    }

    /// Process audio through the entire chain.
    ///
    /// Input is copied to output, then each non-bypassed effect is applied
    /// in order. Uses the internal scratch buffer to avoid allocation.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        if len == 0 {
            return;
        }

        // Ensure scratch buffer is large enough.
        if self.scratch_buffer.len() < len {
            self.scratch_buffer.resize(len, 0.0);
        }

        // Start with input in output.
        output[..len].copy_from_slice(&input[..len]);

        // Track which buffer currently holds the "current" audio.
        // We alternate between output and scratch to avoid unnecessary copies.
        let mut current_in_output = true;

        for slot in &mut self.effects {
            if slot.bypassed {
                continue;
            }

            if current_in_output {
                // Process output -> scratch.
                slot.processor
                    .process(&output[..len], &mut self.scratch_buffer[..len]);
                current_in_output = false;
            } else {
                // Process scratch -> output.
                slot.processor
                    .process(&self.scratch_buffer[..len], &mut output[..len]);
                current_in_output = true;
            }
        }

        // If the final result is in scratch, copy it to output.
        if !current_in_output {
            output[..len].copy_from_slice(&self.scratch_buffer[..len]);
        }

        sanitize_buffer(&mut output[..len]);
    }

    /// Set a parameter value on the effect at `effect_index`.
    ///
    /// Returns an error if the effect index is out of range or if
    /// `set_param` on the underlying processor fails.
    pub fn set_effect_param(
        &mut self,
        effect_index: usize,
        param_index: usize,
        value: f32,
    ) -> crate::Result<()> {
        let slot = self.effects.get_mut(effect_index).ok_or_else(|| {
            crate::Error::Config(format!("effect index {effect_index} out of range"))
        })?;
        slot.processor.set_param(param_index, value)
    }

    /// Number of effects in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Whether the chain has no effects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

impl Default for EffectChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trivial gain processor for testing the chain.
    #[derive(Debug)]
    struct TestGain {
        gain: f32,
    }

    impl TestGain {
        fn new(gain: f32) -> Self {
            Self { gain }
        }
    }

    impl Processor for TestGain {
        fn process(&mut self, input: &[f32], output: &mut [f32]) {
            let len = input.len().min(output.len());
            for i in 0..len {
                output[i] = sanitize_sample(input[i] * self.gain);
            }
        }

        fn reset(&mut self) {}

        fn name(&self) -> &str {
            "TestGain"
        }

        fn set_sample_rate(&mut self, _sample_rate: f32) {}
    }

    use crate::sanitize_sample;

    #[test]
    fn chain_empty_passes_through() {
        let mut chain = EffectChain::new();
        let input = [0.5, -0.3, 0.0, 1.0];
        let mut output = [0.0_f32; 4];
        chain.process(&input, &mut output);

        for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (inp - out).abs() < f32::EPSILON,
                "empty chain should pass through: [{i}] {inp} != {out}"
            );
        }
    }

    #[test]
    fn chain_single_effect() {
        let mut chain = EffectChain::new();
        chain.push(Box::new(TestGain::new(0.5)));

        let input = [1.0, -1.0, 0.5, 0.0];
        let mut output = [0.0_f32; 4];
        chain.process(&input, &mut output);

        for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
            let expected = inp * 0.5;
            assert!(
                (expected - out).abs() < 1e-6,
                "single effect: [{i}] expected {expected}, got {out}"
            );
        }
    }

    #[test]
    fn chain_two_effects_compose() {
        let mut chain = EffectChain::new();
        chain.push(Box::new(TestGain::new(0.5)));
        chain.push(Box::new(TestGain::new(2.0)));

        let input = [1.0, -1.0, 0.5];
        let mut output = [0.0_f32; 3];
        chain.process(&input, &mut output);

        // 0.5 * 2.0 = 1.0 overall gain.
        for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (inp - out).abs() < 1e-6,
                "two effects: [{i}] expected {inp}, got {out}"
            );
        }
    }

    #[test]
    fn chain_bypass_skips_effect() {
        let mut chain = EffectChain::new();
        chain.push(Box::new(TestGain::new(0.0))); // would silence everything
        chain.push(Box::new(TestGain::new(2.0)));

        // Bypass the silencing effect.
        chain.set_bypass(0, true);

        let input = [0.5; 4];
        let mut output = [0.0_f32; 4];
        chain.process(&input, &mut output);

        // Only the 2x gain should apply.
        for (i, &out) in output.iter().enumerate() {
            assert!(
                (out - 1.0).abs() < 1e-6,
                "bypass: [{i}] expected 1.0, got {out}"
            );
        }
    }

    #[test]
    fn chain_remove() {
        let mut chain = EffectChain::new();
        chain.push(Box::new(TestGain::new(0.5)));
        chain.push(Box::new(TestGain::new(3.0)));
        assert_eq!(chain.len(), 2);

        let removed = chain.remove(0);
        assert!(removed.is_some());
        assert_eq!(chain.len(), 1);
        assert_eq!(removed.unwrap().name(), "TestGain");

        // Out of range returns None.
        assert!(chain.remove(10).is_none());
    }

    #[test]
    fn chain_len_and_is_empty() {
        let mut chain = EffectChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);

        chain.push(Box::new(TestGain::new(1.0)));
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn chain_handles_empty_buffers() {
        let mut chain = EffectChain::new();
        chain.push(Box::new(TestGain::new(1.0)));
        chain.process(&[], &mut []);
    }

    #[test]
    fn chain_with_real_effects() {
        // Ensure real effects compose without panicking.
        let mut chain = EffectChain::new();
        chain.push(Box::new(BiquadFilter::new(FilterType::LowPass, 44100.0)));
        chain.push(Box::new(Delay::new(44100.0)));

        let input = [0.5_f32; 256];
        let mut output = [0.0_f32; 256];
        chain.process(&input, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "chain output[{i}] = {s}");
        }
    }

    #[test]
    fn chain_new_with_capacity_preallocates() {
        let mut chain = EffectChain::new_with_capacity(512);
        chain.push(Box::new(TestGain::new(0.5)));

        let input = [1.0_f32; 256];
        let mut output = [0.0_f32; 256];
        chain.process(&input, &mut output);

        for (i, &out) in output.iter().enumerate() {
            assert!(
                (out - 0.5).abs() < 1e-6,
                "new_with_capacity: [{i}] expected 0.5, got {out}"
            );
        }
    }

    #[test]
    fn chain_prepare_resizes_scratch() {
        let mut chain = EffectChain::new();
        chain.prepare(1024);
        chain.push(Box::new(TestGain::new(2.0)));

        let input = [0.25_f32; 512];
        let mut output = [0.0_f32; 512];
        chain.process(&input, &mut output);

        for (i, &out) in output.iter().enumerate() {
            assert!(
                (out - 0.5).abs() < 1e-6,
                "prepare: [{i}] expected 0.5, got {out}"
            );
        }
    }
}
