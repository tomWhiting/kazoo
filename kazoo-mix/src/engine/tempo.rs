use super::{MixerEngine, MixerEngineError, SongClock};
use crate::shared::{DEFAULT_BPM, clamp_bpm};

impl MixerEngine {
    pub(crate) fn with_tempo(
        channel_slots: usize,
        sample_rate: u32,
        bpm: f32,
    ) -> Result<Self, MixerEngineError> {
        let mut engine = Self::new(channel_slots, sample_rate)?;
        engine.song = SongClock::new(sample_rate, f64::from(clamp_bpm(bpm, DEFAULT_BPM)));
        Ok(engine)
    }
}
