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
        output[..len].copy_from_slice(&input[..len]);
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
