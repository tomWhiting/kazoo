//! The song clock: where the studio is in the song, frame by frame.
//!
//! The desk never starts, stops or changes tempo "now". The audio callback
//! schedules each change a short way ahead ([`SongClock::schedule`]), far
//! enough for every instrument to hear about it before it has rendered that
//! frame, and hands the resulting [`SongAnchor`] to the hub, which tells each
//! instrument the frame of its own stream the change lands on. The desk's
//! metronome and every instrument then move on the same studio frame.
//!
//! Song position is in beats from the first downbeat, computed with
//! [`beats_in`], the one formula the whole studio uses. Starting play always
//! starts the song at beat 0.
//!
//! Real-time safe: fixed capacity, no allocation.

use kazoo_core::ipc::follow::beats_in;

/// Changes the clock can hold before they land.
pub const SONG_PENDING: usize = 8;

/// A point on the song timeline: from studio frame `frame` on, the song is
/// at `beat` and moving at `bpm` (or standing still).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongAnchor {
    /// Studio frame the anchor takes effect on.
    pub frame: u64,
    /// Song position at `frame`, in beats.
    pub beat: f64,
    /// Tempo from `frame` on.
    pub bpm: f64,
    /// Whether the song moves.
    pub playing: bool,
}

impl SongAnchor {
    /// The stopped song at the start, before anything was scheduled.
    #[must_use]
    pub const fn stopped(bpm: f64) -> Self {
        Self {
            frame: 0,
            beat: 0.0,
            bpm,
            playing: false,
        }
    }

    /// Song position on `frame` (at or after the anchor), at `sample_rate`.
    #[must_use]
    pub fn beat_at(&self, frame: u64, sample_rate: f64) -> f64 {
        if !self.playing {
            return self.beat;
        }
        // Frame distances are far below 2^53: exact as f64.
        let elapsed = frame.saturating_sub(self.frame) as f64;
        self.beat + beats_in(elapsed, self.bpm, sample_rate)
    }
}

/// The song clock the audio callback owns.
#[derive(Debug, Clone)]
pub struct SongClock {
    sample_rate: f64,
    current: SongAnchor,
    /// Scheduled anchors, in frame order.
    pending: [Option<SongAnchor>; SONG_PENDING],
    len: usize,
}

impl SongClock {
    /// A stopped clock at `bpm`.
    #[must_use]
    pub fn new(sample_rate: u32, bpm: f64) -> Self {
        Self {
            sample_rate: f64::from(sample_rate.max(1)),
            current: SongAnchor::stopped(bpm),
            pending: [None; SONG_PENDING],
            len: 0,
        }
    }

    /// The anchor in force on the last frame advanced to.
    #[must_use]
    pub const fn current(&self) -> SongAnchor {
        self.current
    }

    /// Schedule a change of play state and tempo on `frame`, which must not
    /// be before a change already scheduled (the callback always schedules
    /// a fixed distance past the frame it renders next, so it never is; an
    /// earlier frame is moved up to the last scheduled one). Returns the
    /// anchor, for the hub to pass on.
    ///
    /// Starting play starts the song at beat 0; stopping holds the position
    /// reached; a tempo change keeps the position continuous.
    ///
    /// When [`SONG_PENDING`] changes are already waiting, the newest one is
    /// replaced: the song still ends up in the state asked for last.
    pub fn schedule(&mut self, frame: u64, playing: bool, bpm: f64) -> SongAnchor {
        let before = self.last_scheduled();
        let frame = frame.max(before.frame);
        let beat = if playing && !before.playing {
            0.0
        } else {
            before.beat_at(frame, self.sample_rate)
        };
        let anchor = SongAnchor {
            frame,
            beat,
            bpm,
            playing,
        };
        if self.len == SONG_PENDING {
            self.pending[SONG_PENDING - 1] = Some(anchor);
        } else {
            self.pending[self.len] = Some(anchor);
            self.len += 1;
        }
        anchor
    }

    /// Move to `frame`, landing any change scheduled on or before it, and
    /// return the song position there (`None` while stopped).
    pub fn advance(&mut self, frame: u64) -> Option<f64> {
        while self.len > 0 {
            match self.pending[0] {
                Some(next) if next.frame <= frame => {
                    self.current = next;
                    self.pending.rotate_left(1);
                    self.pending[SONG_PENDING - 1] = None;
                    self.len -= 1;
                }
                _ => break,
            }
        }
        self.current
            .playing
            .then(|| self.current.beat_at(frame, self.sample_rate))
    }

    /// Beats that pass in one frame at the current tempo.
    #[must_use]
    pub fn beats_per_frame(&self) -> f64 {
        beats_in(1.0, self.current.bpm, self.sample_rate)
    }

    /// The anchor the song will be on once everything scheduled has landed.
    fn last_scheduled(&self) -> SongAnchor {
        self.pending[..self.len]
            .iter()
            .flatten()
            .last()
            .copied()
            .unwrap_or(self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_f64_eq;

    const RATE: u32 = 48_000;

    #[test]
    fn play_lands_on_its_frame_at_beat_zero() {
        let mut clock = SongClock::new(RATE, 120.0);
        let anchor = clock.schedule(1_000, true, 120.0);
        assert_f64_eq(anchor.beat, 0.0);
        assert_eq!(clock.advance(999), None);
        assert_eq!(clock.advance(1_000), Some(0.0));
        // 120 BPM at 48 kHz: a beat every 24 000 frames.
        assert_eq!(clock.advance(25_000), Some(1.0));
    }

    #[test]
    fn tempo_changes_keep_the_position_continuous() {
        let mut clock = SongClock::new(RATE, 120.0);
        clock.schedule(0, true, 120.0);
        let change = clock.schedule(24_000, true, 60.0);
        assert_f64_eq(change.beat, 1.0);
        assert_eq!(clock.advance(24_000), Some(1.0));
        // Now a beat every 48 000 frames.
        assert_eq!(clock.advance(72_000), Some(2.0));
    }

    #[test]
    fn stopping_holds_the_position_and_playing_again_starts_over() {
        let mut clock = SongClock::new(RATE, 120.0);
        clock.schedule(0, true, 120.0);
        let stop = clock.schedule(36_000, false, 120.0);
        assert_f64_eq(stop.beat, 1.5);
        assert_eq!(clock.advance(36_000), None);
        assert_f64_eq(clock.current().beat, 1.5);
        let again = clock.schedule(50_000, true, 120.0);
        assert_f64_eq(again.beat, 0.0);
    }

    #[test]
    fn changes_scheduled_back_to_back_are_worked_out_from_each_other() {
        let mut clock = SongClock::new(RATE, 120.0);
        clock.schedule(1_000, true, 120.0);
        // Before the first change lands, a tempo change after it.
        let change = clock.schedule(25_000, true, 90.0);
        assert_f64_eq(change.beat, 1.0);
        assert_eq!(clock.advance(25_000), Some(1.0));
    }

    #[test]
    fn a_full_queue_keeps_the_newest_request() {
        let mut clock = SongClock::new(RATE, 120.0);
        for n in 0..SONG_PENDING as u64 {
            clock.schedule(100 + n, n % 2 == 0, 120.0);
        }
        clock.schedule(200, true, 150.0);
        clock.advance(1_000);
        assert!(clock.current().playing);
        assert_f64_eq(clock.current().bpm, 150.0);
    }

    #[test]
    fn an_early_frame_is_moved_up_to_the_last_change() {
        let mut clock = SongClock::new(RATE, 120.0);
        clock.schedule(5_000, true, 120.0);
        let late = clock.schedule(4_000, false, 120.0);
        assert_eq!(late.frame, 5_000);
    }
}
