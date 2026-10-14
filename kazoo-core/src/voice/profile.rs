//! Input ranges and time constants for continuous pitch tracking.

use super::TrackSettings;
use crate::analysis::cycle::CycleSettings;
use crate::host::live::InputProfile;

#[cfg(test)]
mod tests;

impl InputProfile {
    /// Tune tracking without quantising: every frame keeps its continuous Hz,
    /// and `snapped_hz == hz` until a quantiser explicitly writes it.
    ///
    /// Range comes from `range_hz`, giving each input one range everywhere.
    /// Voice keeps the original cycle and fusion settings exactly.
    ///
    /// DC rejection sits 0.7 to 1.5 octaves below the lowest note: 30 Hz for
    /// voice, 20 Hz for guitar and 10 Hz for bass and line. A 30 Hz blocker
    /// would remove about 3 dB from a bass's low B.
    ///
    /// Peak release is long compared with the longest period: guitar's
    /// 16.7 ms fits 30 ms, while bass's 34.5 ms and line's 33 ms need 80 ms.
    /// Voice retains its 30 ms follower.
    ///
    /// The judge needs about two periods: 2048 samples at 48 kHz covers
    /// voice and guitar; bass and line need 4096 samples (85 ms). It only
    /// folds octaves and vetoes voicing, so the longer frame delays no pitch.
    /// Its worker budget is 4.2 ms per 10.7 ms hop for the larger frame.
    ///
    /// A preamplified string sits above its floor: opening at -48 dBFS and
    /// closing at -54 catches a soft pluck but excludes hum at -55 to -70.
    /// Voice and line keep -50/-56 dBFS and six dB of hysteresis.
    ///
    /// A pick over a ringing string rises less than a new sung syllable:
    /// guitar and bass use 6 dB, voice and line 9 dB. The valley follower and
    /// 60 ms refractory interval reject tremolo's 3 to 6 dB fluctuations.
    ///
    /// Guitar and bass retrigger at an 80-cent move within two frames
    /// (2.7 ms at 48 kHz): a string crossing, hammer-on or pull-off. A
    /// 12-semitone slide in 100 ms moves 16 cents per frame, a 200-cent bend
    /// 2.7 cents, and 30-cent vibrato at 6 Hz at most 1.5 cents. The threshold
    /// clears those continuous movements and catches fretted steps. Voice
    /// and line leave pitch onsets off; sung legato stays one articulation.
    ///
    /// Guitar and bass use early half-cycle readings: plucked strings start
    /// with their cleanest cycles, and low notes gain the most time. Voice
    /// keeps early readings off because breathy sung onsets have irregular
    /// lobes and the voice scoreboard was tuned on normal marks. Line keeps
    /// them off because its source is unknown.
    #[must_use]
    pub const fn tracking(self) -> TrackSettings {
        let (min_hz, max_hz) = self.range_hz();
        let (
            dc_hz,
            peak_release_ms,
            judge_frame_48k,
            gate_on_db,
            gate_off_db,
            onset_rise_db,
            jump_onset_cents,
        ) = match self {
            Self::Voice => return TrackSettings::VOICE,
            Self::Guitar => (20.0, 30.0, 2048, -48.0, -54.0, 6.0, Some(80.0)),
            Self::Bass => (10.0, 80.0, 4096, -48.0, -54.0, 6.0, Some(80.0)),
            Self::Line => (10.0, 80.0, 4096, -50.0, -56.0, 9.0, None),
        };
        TrackSettings {
            cycle: CycleSettings {
                min_hz,
                max_hz,
                dc_hz,
                peak_release_ms,
                early: matches!(self, Self::Guitar | Self::Bass),
            },
            judge_frame_48k,
            gate_on_db,
            gate_off_db,
            onset_rise_db,
            jump_onset_cents,
        }
    }
}
