pub mod engine;
pub mod envelope;
pub mod filter;
pub mod noise;
pub mod oscillator;
pub mod params;
pub mod status;
pub mod voice;

/// Number of voices in the classic Prophet-5 architecture.
pub const NUM_VOICES: usize = 5;

pub use engine::ProphetSynth;
pub use params::{
    DriftParams, EnvelopeParams, FilterParams, MixerParams, OscillatorParams, PolyModParams,
    SynthParams,
};
pub use status::VoiceStatus;
