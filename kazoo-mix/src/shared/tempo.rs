use super::{AtomicF32, DEFAULT_BPM, SharedState, clamp_bpm};

impl SharedState {
    pub(crate) const fn with_tempo(bpm: f32) -> Self {
        let mut shared = Self::new();
        shared.tempo_bpm = AtomicF32::new(clamp_bpm(bpm, DEFAULT_BPM));
        shared
    }
}
