//! Public Prophet synthesis parameters.

use super::oscillator::{OctaveRange, Waveform};

/// Per-oscillator controls.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct OscillatorParams {
    pub waveform: Waveform,
    pub octave: OctaveRange,
    pub fine_tune_cents: f32,
    pub pulse_width: f32,
    pub level: f32,
}

impl OscillatorParams {
    #[must_use]
    pub const fn oscillator_a_default() -> Self {
        Self {
            waveform: Waveform::Saw,
            octave: OctaveRange::Footage8,
            fine_tune_cents: -2.0,
            pulse_width: 0.5,
            level: 0.82,
        }
    }

    #[must_use]
    pub const fn oscillator_b_default() -> Self {
        Self {
            waveform: Waveform::Pulse,
            octave: OctaveRange::Footage8,
            fine_tune_cents: 7.0,
            pulse_width: 0.42,
            level: 0.72,
        }
    }
}

/// Mix stage before the low-pass filter.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct MixerParams {
    pub noise_level: f32,
    pub pre_filter_gain: f32,
}

impl Default for MixerParams {
    fn default() -> Self {
        Self {
            noise_level: 0.02,
            pre_filter_gain: 0.45,
        }
    }
}

/// Four-pole low-pass filter controls.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct FilterParams {
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub key_track: f32,
    pub envelope_amount: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 950.0,
            resonance: 0.38,
            key_track: 0.35,
            envelope_amount: 0.65,
        }
    }
}

/// ADSR envelope controls in seconds and normalized sustain level.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct EnvelopeParams {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl EnvelopeParams {
    #[must_use]
    pub const fn filter_default() -> Self {
        Self {
            attack: 0.012,
            decay: 0.42,
            sustain: 0.38,
            release: 0.75,
        }
    }

    #[must_use]
    pub const fn amplifier_default() -> Self {
        Self {
            attack: 0.008,
            decay: 0.35,
            sustain: 0.72,
            release: 0.55,
        }
    }
}

/// Prophet poly-mod section.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PolyModParams {
    pub osc_b_to_osc_a_cents: f32,
    pub osc_b_to_filter_hz: f32,
    pub filter_env_to_osc_a_cents: f32,
    pub filter_env_to_filter_hz: f32,
    pub oscillator_sync: bool,
}

impl Default for PolyModParams {
    fn default() -> Self {
        Self {
            osc_b_to_osc_a_cents: 0.0,
            osc_b_to_filter_hz: 0.0,
            filter_env_to_osc_a_cents: 0.0,
            filter_env_to_filter_hz: 1000.0,
            oscillator_sync: false,
        }
    }
}

/// Analog instability controls.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct DriftParams {
    pub voice_detune_cents: f32,
    pub oscillator_b_detune_scale: f32,
}

impl Default for DriftParams {
    fn default() -> Self {
        Self {
            voice_detune_cents: 4.5,
            oscillator_b_detune_scale: -0.5,
        }
    }
}

/// Complete parameter snapshot shared by all voices.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SynthParams {
    pub oscillator_a: OscillatorParams,
    pub oscillator_b: OscillatorParams,
    pub mixer: MixerParams,
    pub filter: FilterParams,
    pub filter_envelope: EnvelopeParams,
    pub amplifier_envelope: EnvelopeParams,
    pub poly_mod: PolyModParams,
    pub drift: DriftParams,
    pub master_level: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self {
            oscillator_a: OscillatorParams::oscillator_a_default(),
            oscillator_b: OscillatorParams::oscillator_b_default(),
            mixer: MixerParams::default(),
            filter: FilterParams::default(),
            filter_envelope: EnvelopeParams::filter_default(),
            amplifier_envelope: EnvelopeParams::amplifier_default(),
            poly_mod: PolyModParams::default(),
            drift: DriftParams::default(),
            master_level: 0.72,
        }
    }
}
