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

use crate::{ParamError, Processor, sanitize_buffer};

/// Why a parameter change on an effect chain was refused. `Copy`, so it is
/// safe to create on the audio thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EffectParamError {
    /// The chain has no effect at this position.
    #[error("no effect {index} (the chain has {count})")]
    NoSuchEffect {
        /// The requested effect position.
        index: usize,
        /// How many effects the chain holds.
        count: usize,
    },
    /// The effect refused the parameter change.
    #[error(transparent)]
    Param(#[from] ParamError),
}

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
/// Audio flows through each non-bypassed effect in order. The chain holds at
/// most [`crate::MAX_EFFECTS_PER_TRACK`] effects, with room for all of them
/// reserved up front, and an internal scratch buffer, so adding, removing
/// and processing never allocate or free: [`EffectChain::push`] refuses an
/// effect it has no room for and [`EffectChain::remove`] hands the removed
/// effect back, leaving the caller to dispose of it.
#[derive(Debug)]
pub struct EffectChain {
    effects: Vec<EffectSlot>,
    /// Never empty, so [`EffectChain::process`] can always make progress.
    scratch_buffer: Vec<f32>,
}

impl EffectChain {
    /// Create an empty effect chain with a scratch buffer for blocks of up
    /// to [`crate::DEFAULT_BUFFER_SIZE`] samples.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_capacity(crate::DEFAULT_BUFFER_SIZE)
    }

    /// Create an empty effect chain with its scratch buffer pre-allocated to
    /// `max_block_size` samples (at least one) and room reserved for
    /// [`crate::MAX_EFFECTS_PER_TRACK`] effects.
    #[must_use]
    pub fn new_with_capacity(max_block_size: usize) -> Self {
        Self {
            effects: Vec::with_capacity(crate::MAX_EFFECTS_PER_TRACK),
            scratch_buffer: vec![0.0; max_block_size.max(1)],
        }
    }

    /// Grow the internal scratch buffer to the given block size. Allocates:
    /// call it when preparing, not on the audio thread.
    ///
    /// Blocks longer than the scratch buffer are still processed correctly,
    /// in scratch-sized pieces.
    pub fn prepare(&mut self, buffer_size: usize) {
        if self.scratch_buffer.len() < buffer_size {
            self.scratch_buffer.resize(buffer_size, 0.0);
        }
    }

    /// Append an effect to the end of the chain. Never allocates.
    ///
    /// # Errors
    ///
    /// Hands the effect back if the chain already holds
    /// [`crate::MAX_EFFECTS_PER_TRACK`] effects.
    pub fn push(&mut self, effect: Box<dyn Processor>) -> Result<(), Box<dyn Processor>> {
        if self.effects.len() >= crate::MAX_EFFECTS_PER_TRACK {
            return Err(effect);
        }
        self.effects.push(EffectSlot {
            processor: effect,
            bypassed: false,
        });
        Ok(())
    }

    /// Remove and return the effect at `index`, or `None` if out of range.
    /// Never frees: the caller disposes of the effect.
    #[must_use = "the removed effect must be disposed of by the caller"]
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
    /// in order. Uses the internal scratch buffer, never allocating: a block
    /// longer than the scratch buffer is processed in scratch-sized pieces.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        if len == 0 {
            return;
        }
        let piece = self.scratch_buffer.len().max(1);
        for (input, output) in input[..len]
            .chunks(piece)
            .zip(output[..len].chunks_mut(piece))
        {
            self.process_piece(input, output);
        }
    }

    /// Process one piece no longer than the scratch buffer (`input` and
    /// `output` have the same length).
    fn process_piece(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len()).min(self.scratch_buffer.len());

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
    /// Real-time safe: validation never allocates.
    ///
    /// # Errors
    ///
    /// [`EffectParamError::NoSuchEffect`] if the chain has no effect at
    /// `effect_index`; [`EffectParamError::Param`] if the effect refuses the
    /// parameter change.
    pub fn set_effect_param(
        &mut self,
        effect_index: usize,
        param_index: usize,
        value: f32,
    ) -> Result<(), EffectParamError> {
        let count = self.effects.len();
        let slot = self
            .effects
            .get_mut(effect_index)
            .ok_or(EffectParamError::NoSuchEffect {
                index: effect_index,
                count,
            })?;
        slot.processor.set_param(param_index, value)?;
        Ok(())
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

        fn name(&self) -> &'static str {
            "TestGain"
        }

        fn set_sample_rate(&mut self, _sample_rate: f32) {}
    }

    use crate::sanitize_sample;

    #[test]
    fn set_effect_param_applies_valid_value() {
        let mut chain = EffectChain::new_with_capacity(64);
        assert!(chain.push(Box::new(Delay::new(48_000.0))).is_ok());
        chain.set_effect_param(0, 2, 0.25).unwrap();
        let mix = chain.effects[0].processor.param_value(2).unwrap();
        assert!((mix - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn set_effect_param_rejects_missing_effect_and_bad_param() {
        let mut chain = EffectChain::new_with_capacity(64);
        assert!(chain.push(Box::new(Delay::new(48_000.0))).is_ok());
        assert_eq!(
            chain.set_effect_param(3, 0, 0.5),
            Err(EffectParamError::NoSuchEffect { index: 3, count: 1 })
        );
        assert_eq!(
            chain.set_effect_param(0, 9, 0.5),
            Err(EffectParamError::Param(ParamError::UnknownIndex {
                index: 9,
                count: 3
            }))
        );
        assert_eq!(
            chain.set_effect_param(0, 2, f32::NAN),
            Err(EffectParamError::Param(ParamError::NotFinite { index: 2 }))
        );
        // A parameterless processor refuses every index.
        assert!(chain.push(Box::new(TestGain::new(1.0))).is_ok());
        assert_eq!(
            chain.set_effect_param(1, 0, 0.5),
            Err(EffectParamError::Param(ParamError::UnknownIndex {
                index: 0,
                count: 0
            }))
        );
    }

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
        assert!(chain.push(Box::new(TestGain::new(0.5))).is_ok());

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
        assert!(chain.push(Box::new(TestGain::new(0.5))).is_ok());
        assert!(chain.push(Box::new(TestGain::new(2.0))).is_ok());

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
        assert!(chain.push(Box::new(TestGain::new(0.0))).is_ok()); // would silence everything
        assert!(chain.push(Box::new(TestGain::new(2.0))).is_ok());

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
        assert!(chain.push(Box::new(TestGain::new(0.5))).is_ok());
        assert!(chain.push(Box::new(TestGain::new(3.0))).is_ok());
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

        assert!(chain.push(Box::new(TestGain::new(1.0))).is_ok());
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn chain_handles_empty_buffers() {
        let mut chain = EffectChain::new();
        assert!(chain.push(Box::new(TestGain::new(1.0))).is_ok());
        chain.process(&[], &mut []);
    }

    #[test]
    fn chain_with_real_effects() {
        // Ensure real effects compose without panicking.
        let mut chain = EffectChain::new();
        assert!(
            chain
                .push(Box::new(BiquadFilter::new(FilterType::LowPass, 44100.0)))
                .is_ok()
        );
        assert!(chain.push(Box::new(Delay::new(44100.0))).is_ok());

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
        assert!(chain.push(Box::new(TestGain::new(0.5))).is_ok());

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
    fn chain_refuses_effects_beyond_the_limit_and_hands_them_back() {
        let mut chain = EffectChain::new_with_capacity(16);
        for _ in 0..crate::MAX_EFFECTS_PER_TRACK {
            assert!(chain.push(Box::new(TestGain::new(1.0))).is_ok());
        }
        let refused = chain.push(Box::new(TestGain::new(3.0)));
        let refused = refused.map_err(|e| e.name().to_owned()).unwrap_err();
        assert_eq!(refused, "TestGain");
        assert_eq!(chain.len(), crate::MAX_EFFECTS_PER_TRACK);
    }

    #[test]
    fn chain_never_reallocates_up_to_the_limit() {
        let mut chain = EffectChain::new_with_capacity(16);
        let slots = chain.effects.as_ptr();
        for _ in 0..crate::MAX_EFFECTS_PER_TRACK {
            assert!(chain.push(Box::new(TestGain::new(1.0))).is_ok());
        }
        assert_eq!(chain.effects.as_ptr(), slots, "effect slots reallocated");
    }

    #[test]
    fn chain_processes_blocks_longer_than_its_scratch_without_growing() {
        let mut chain = EffectChain::new_with_capacity(8);
        assert!(chain.push(Box::new(TestGain::new(0.5))).is_ok());
        assert!(chain.push(Box::new(TestGain::new(3.0))).is_ok());
        let scratch = chain.scratch_buffer.as_ptr();

        let input: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        let mut output = vec![0.0_f32; 100];
        chain.process(&input, &mut output);

        assert_eq!(chain.scratch_buffer.len(), 8, "scratch grew");
        assert_eq!(chain.scratch_buffer.as_ptr(), scratch, "scratch moved");
        for (i, (&inp, &out)) in input.iter().zip(&output).enumerate() {
            assert!(
                inp.mul_add(-1.5, out).abs() < 1e-6,
                "[{i}] {out} != {}",
                inp * 1.5
            );
        }
    }

    #[test]
    fn chain_prepare_resizes_scratch() {
        let mut chain = EffectChain::new();
        chain.prepare(1024);
        assert!(chain.push(Box::new(TestGain::new(2.0))).is_ok());

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
