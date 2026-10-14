/// Middle C: the nearest f32 to 440 × 2^(−9/12), the wall's zero volts.
pub const C4_HZ: f32 = 261.625_58;
/// A fixed 64-sample hop gives a steady frame clock across callback sizes,
/// finer than a cycle below 750 Hz at 48 kHz.
pub const VOICE_HOP: usize = 64;
/// −60 dBFS maps to zero loudness; full scale maps to one.
pub const LOUDNESS_FLOOR_DB: f32 = -60.0;

/// Convert hertz to volts, one volt per octave above middle C.
#[must_use]
pub fn volts_from_hz(hz: f32) -> f32 {
    if hz.is_finite() && hz > 0.0 {
        // Separate logarithms keep subnormal pitches from underflowing the ratio.
        hz.log2() - C4_HZ.log2()
    } else {
        0.0
    }
}

/// Convert volts to hertz, bounded to eight octaves either side of middle C.
#[must_use]
pub fn hz_from_volts(volts: f32) -> f32 {
    let volts = if volts.is_nan() {
        0.0
    } else {
        volts.clamp(-8.0, 8.0)
    };
    C4_HZ * volts.exp2()
}

/// What the voice is doing, once per [`VOICE_HOP`] samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceFrame {
    /// Producer sample count at this frame's end, on the audio stream's clock.
    pub at: u64,
    /// Latest voiced pitch, held through silence; zero before the first note.
    pub hz: f32,
    /// One volt per octave above middle C.
    pub volts: f32,
    /// Direct pitch until a quantiser writes it.
    pub snapped_hz: f32,
    /// Direct voltage until a quantiser writes it.
    pub snapped_volts: f32,
    pub events: VoiceEvents,
    /// A pitch is available in this frame, independent of the gate hold.
    pub voiced: bool,
    /// A worker's partial-cycle pitch awaiting normal acquisition.
    pub provisional: bool,
    /// Normalised dBFS above the loudness floor, clamped to 0..=1.
    pub loudness: f32,
    /// Smoothed breath relative to tone, 0..=1.
    pub breath: f32,
    /// Pitch certainty, 0..=1.
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceEvents {
    /// A note is sounding, with level hysteresis and a release hold.
    pub gate: bool,
    /// A gate rise or re-sung syllable in this frame.
    pub onset: bool,
}

impl std::ops::Deref for VoiceFrame {
    type Target = VoiceEvents;

    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl std::ops::DerefMut for VoiceFrame {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.events
    }
}
