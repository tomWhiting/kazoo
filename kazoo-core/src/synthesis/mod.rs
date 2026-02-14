//! Synthesis engines: granular, wavetable, vocoder, phase vocoder, pitch-tracked resynthesis.
//!
//! Each engine implements [`crate::Processor`]. Voice audio arrives as `input`;
//! synthesized audio is written to `output`. The analysis thread provides
//! pitch and envelope data via setter methods called from the command channel.

pub mod granular;
pub mod phase_vocoder;
pub mod pitch_tracked;
pub mod vocoder;
pub mod wavetable;

pub use granular::{GrainEnvelope, GranularSynth};
pub use phase_vocoder::PhaseVocoder;
pub use pitch_tracked::{OscillatorShape, PitchTrackedSynth};
pub use vocoder::{Vocoder, VocoderCarrierMode};
pub use wavetable::{Wavetable, WavetableExtractor, WavetableOscillator};

/// Active synthesis mode selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthesisMode {
    /// Raw mic input passed through unprocessed (no synthesis).
    Passthrough,
    /// Voice pitch drives band-limited oscillators.
    PitchTracked,
    /// Extracted single-cycle waveforms played back with morphing.
    Wavetable,
    /// Voice decomposed into grains and reassembled as clouds.
    Granular,
    /// Voice spectral envelope applied to a carrier signal.
    Vocoder,
    /// STFT-based time stretching and pitch shifting.
    PhaseVocoder,
}

impl SynthesisMode {
    /// Human-readable name for this synthesis mode.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Passthrough => "Passthrough (Raw)",
            Self::PitchTracked => "Pitch Tracked Synth",
            Self::Wavetable => "Wavetable Oscillator",
            Self::Granular => "Granular Synth",
            Self::Vocoder => "Vocoder",
            Self::PhaseVocoder => "Phase Vocoder",
        }
    }

    /// Return parameter metadata for this synthesis mode.
    ///
    /// Creates a temporary synth instance to query its [`crate::ParamInfo`]
    /// entries. Only call this on mode changes, not in hot paths.
    #[must_use]
    pub fn param_infos(self, sample_rate: f32) -> Vec<crate::ParamInfo> {
        let synth = self.create_synth(sample_rate);
        (0..synth.param_count())
            .filter_map(|i| synth.param_info(i))
            .collect()
    }

    /// Return default parameter values for this synthesis mode.
    #[must_use]
    pub fn default_param_values(self, sample_rate: f32) -> Vec<f32> {
        let synth = self.create_synth(sample_rate);
        (0..synth.param_count())
            .filter_map(|i| synth.param_value(i))
            .collect()
    }

    /// Format a parameter value for display, using labels for enum params.
    #[must_use]
    pub fn format_param_value(self, param_index: usize, value: f32) -> String {
        match self {
            Self::PitchTracked if param_index == 0 => match value.round() as i32 {
                0 => "Sine".into(),
                1 => "Saw".into(),
                2 => "Square".into(),
                3 => "Triangle".into(),
                _ => format!("{value:.0}"),
            },
            Self::Granular if param_index == 6 => match value.round() as i32 {
                0 => "Hann".into(),
                1 => "Triangle".into(),
                2 => "Gaussian".into(),
                3 => "Tukey".into(),
                _ => format!("{value:.0}"),
            },
            Self::Vocoder if param_index == 0 => match value.round() as i32 {
                0 => "Saw".into(),
                1 => "Square".into(),
                2 => "Noise".into(),
                3 => "External".into(),
                _ => format!("{value:.0}"),
            },
            _ => {
                // Format numeric values: use integer display for whole numbers,
                // 1 decimal for values with fractional parts, 2 for very small.
                if value.fract().abs() < 0.005 {
                    format!("{value:.0}")
                } else if value.abs() >= 10.0 {
                    format!("{value:.1}")
                } else {
                    format!("{value:.2}")
                }
            }
        }
    }

    /// Create a boxed synth processor for this mode (internal helper).
    fn create_synth(self, sample_rate: f32) -> Box<dyn crate::Processor> {
        match self {
            Self::Passthrough => Box::new(PassthroughSynth),
            Self::PitchTracked => Box::new(PitchTrackedSynth::new(sample_rate)),
            Self::Wavetable => Box::new(WavetableOscillator::new(sample_rate)),
            Self::Granular => Box::new(GranularSynth::new(sample_rate)),
            Self::Vocoder => Box::new(Vocoder::new(sample_rate)),
            Self::PhaseVocoder => Box::new(PhaseVocoder::new(sample_rate)),
        }
    }
}

// ---------------------------------------------------------------------------
// Passthrough "synth" — raw mic signal, no processing
// ---------------------------------------------------------------------------

/// A no-op synthesizer that copies input directly to output.
///
/// Used for monitoring the raw microphone signal without any synthesis
/// processing. The signal still passes through the track's effect chain,
/// volume, and pan — only the synthesis stage is bypassed.
#[derive(Debug)]
pub struct PassthroughSynth;

impl crate::Processor for PassthroughSynth {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        for i in 0..len {
            output[i] = crate::sanitize_sample(input[i]);
        }
        // Zero any remaining output if output is longer than input.
        for s in &mut output[len..] {
            *s = 0.0;
        }
    }

    fn reset(&mut self) {}

    fn name(&self) -> &'static str {
        "Passthrough"
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Processor;

    #[test]
    fn passthrough_copies_input_to_output() {
        let mut synth = PassthroughSynth;
        let input = [0.1, 0.5, -0.3, 0.0];
        let mut output = [0.0; 4];
        synth.process(&input, &mut output);
        assert_eq!(output, input);
    }

    #[test]
    fn passthrough_empty_buffers() {
        let mut synth = PassthroughSynth;
        let input: [f32; 0] = [];
        let mut output: [f32; 0] = [];
        synth.process(&input, &mut output);
    }

    #[test]
    fn passthrough_output_longer_than_input_zeros_remainder() {
        let mut synth = PassthroughSynth;
        let input = [0.5, -0.5];
        let mut output = [1.0; 5];
        synth.process(&input, &mut output);
        assert_eq!(output[0], 0.5);
        assert_eq!(output[1], -0.5);
        assert_eq!(output[2], 0.0);
        assert_eq!(output[3], 0.0);
        assert_eq!(output[4], 0.0);
    }

    #[test]
    fn passthrough_input_longer_than_output_truncates() {
        let mut synth = PassthroughSynth;
        let input = [0.1, 0.2, 0.3, 0.4, 0.5];
        let mut output = [0.0; 3];
        synth.process(&input, &mut output);
        assert_eq!(output, [0.1, 0.2, 0.3]);
    }

    #[test]
    fn passthrough_sanitizes_nan_and_inf() {
        let mut synth = PassthroughSynth;
        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        let mut output = [1.0; 4];
        synth.process(&input, &mut output);
        assert_eq!(output[0], 0.0);
        assert_eq!(output[1], 0.0);
        assert_eq!(output[2], 0.0);
        assert_eq!(output[3], 0.5);
    }

    #[test]
    fn passthrough_reset_does_not_panic() {
        let mut synth = PassthroughSynth;
        synth.reset();
    }

    #[test]
    fn passthrough_set_sample_rate_does_not_panic() {
        let mut synth = PassthroughSynth;
        synth.set_sample_rate(96000.0);
    }

    #[test]
    fn passthrough_set_pitch_does_not_panic() {
        let mut synth = PassthroughSynth;
        synth.set_pitch(440.0);
    }

    #[test]
    fn passthrough_name() {
        let synth = PassthroughSynth;
        assert_eq!(synth.name(), "Passthrough");
    }

    #[test]
    fn passthrough_has_zero_params() {
        let synth = PassthroughSynth;
        assert_eq!(synth.param_count(), 0);
        assert!(synth.param_info(0).is_none());
        assert!(synth.param_value(0).is_none());
    }

    #[test]
    fn passthrough_param_infos_empty() {
        let infos = SynthesisMode::Passthrough.param_infos(44100.0);
        assert!(infos.is_empty());
    }

    #[test]
    fn passthrough_default_param_values_empty() {
        let values = SynthesisMode::Passthrough.default_param_values(44100.0);
        assert!(values.is_empty());
    }

    #[test]
    fn passthrough_display_name() {
        assert_eq!(
            SynthesisMode::Passthrough.display_name(),
            "Passthrough (Raw)"
        );
    }

    #[test]
    fn all_modes_have_display_names() {
        for mode in [
            SynthesisMode::Passthrough,
            SynthesisMode::PitchTracked,
            SynthesisMode::Wavetable,
            SynthesisMode::Granular,
            SynthesisMode::Vocoder,
            SynthesisMode::PhaseVocoder,
        ] {
            assert!(
                !mode.display_name().is_empty(),
                "{mode:?} has empty display name"
            );
        }
    }
}
