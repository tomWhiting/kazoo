//! User-facing parameters for the Juno-style synth.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChorusMode {
    Off,
    I,
    Ii,
    IPlusIi,
}

impl ChorusMode {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::I,
            Self::I => Self::Ii,
            Self::Ii => Self::IPlusIi,
            Self::IPlusIi => Self::Off,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::I => "I",
            Self::Ii => "II",
            Self::IPlusIi => "I+II",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcoParams {
    pub saw_level: f32,
    pub pulse_level: f32,
    pub sub_level: f32,
    pub noise_level: f32,
    pub pulse_width: f32,
    pub pwm_depth: f32,
    pub lfo_rate_hz: f32,
}

impl Default for DcoParams {
    fn default() -> Self {
        Self {
            saw_level: 0.75,
            pulse_level: 0.35,
            sub_level: 0.55,
            noise_level: 0.03,
            pulse_width: 0.5,
            pwm_depth: 0.18,
            lfo_rate_hz: 0.55,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterParams {
    pub hpf_amount: f32,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub envelope_amount: f32,
    pub key_track: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            hpf_amount: 0.12,
            cutoff_hz: 2600.0,
            resonance: 0.22,
            envelope_amount: 0.38,
            key_track: 0.35,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeParams {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for EnvelopeParams {
    fn default() -> Self {
        Self {
            attack: 0.012,
            decay: 0.65,
            sustain: 0.62,
            release: 0.9,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChorusParams {
    pub mode: ChorusMode,
    pub mix: f32,
    pub noise: f32,
}

impl Default for ChorusParams {
    fn default() -> Self {
        Self {
            mode: ChorusMode::I,
            mix: 0.55,
            noise: 0.015,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthParams {
    pub dco: DcoParams,
    pub filter: FilterParams,
    pub envelope: EnvelopeParams,
    pub chorus: ChorusParams,
    pub voice_drift_cents: f32,
    pub master_level: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self {
            dco: DcoParams::default(),
            filter: FilterParams::default(),
            envelope: EnvelopeParams::default(),
            chorus: ChorusParams::default(),
            voice_drift_cents: 1.2,
            master_level: 0.32,
        }
    }
}
