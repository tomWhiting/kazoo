//! Following the desk's transport to the frame.
//!
//! The desk never changes the transport "now": it schedules each change a
//! little in the future, and tells every instrument the frame of the
//! instrument's own audio stream on which it lands, with the song position
//! (in beats) at that frame. An instrument feeds those
//! [`TransportSyncMsg`]s to a [`TransportFollower`], asks it on every frame
//! it renders whether a change is due, and moves its sequencer to the beat it
//! is given. Every instrument then starts, stops and changes tempo on the
//! same studio frame as the desk's own metronome, whatever its buffer size.
//!
//! A change that arrives after its frame has already been rendered is applied
//! at once, with the beat advanced by the frames missed, so a late
//! instrument still lands on the desk's grid.
//!
//! Real-time safe: fixed capacity, no allocation.

use super::types::{
    SYNC_NOW, TRANSPORT_PAUSED, TRANSPORT_PLAYING, TRANSPORT_RECORDING, TRANSPORT_STOPPED,
    TransportSyncMsg,
};

/// Changes a follower can hold before they fall due.
pub const FOLLOWER_CAPACITY: usize = 16;

/// Beats that pass in `frames` at `bpm` and `sample_rate`.
///
/// Every part of the studio computes song position with this one formula, so
/// the desk and the instruments agree on it to the last bit.
#[must_use]
pub fn beats_in(frames: f64, bpm: f64, sample_rate: f64) -> f64 {
    frames * bpm / (60.0 * sample_rate)
}

/// What an instrument does on the frame a change falls due.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportChange {
    /// Tempo from this frame on, in beats per minute.
    pub bpm: f64,
    /// While playing, the song position on this frame, in beats from the
    /// start of the song (beat 0 is the first downbeat). `None` while
    /// stopped, or while the desk has not yet placed this instrument on its
    /// timeline (it plays once it has).
    pub beat: Option<f64>,
}

/// Why a sync was not scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowError {
    /// Not a transport state this build knows, or a tempo that is not a
    /// positive number.
    Invalid,
    /// [`FOLLOWER_CAPACITY`] changes are already waiting.
    Full,
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid transport sync",
            Self::Full => "too many transport changes waiting",
        })
    }
}

impl std::error::Error for FollowError {}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Scheduled {
    /// Stream frame, or [`SYNC_NOW`].
    at: u64,
    /// Arrival order: newer news wins.
    sequence: u64,
    playing: bool,
    bpm: f64,
    /// Song position at `at`; NaN when the desk has not placed the stream.
    beat: f64,
}

/// Schedules the desk's transport changes onto an instrument's stream.
#[derive(Debug, Clone)]
pub struct TransportFollower {
    sample_rate: f64,
    pending: [Option<Scheduled>; FOLLOWER_CAPACITY],
    /// Earliest frame anything is due, for a one-comparison fast path.
    earliest: Option<u64>,
    next_sequence: u64,
}

impl TransportFollower {
    /// A follower for a stream at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate.max(1)),
            pending: [None; FOLLOWER_CAPACITY],
            earliest: None,
            next_sequence: 0,
        }
    }

    /// Schedule a sync from the desk.
    ///
    /// A newer sync replaces older ones scheduled on or after its frame: the
    /// desk re-sends a change when it re-places the stream, and the newer
    /// frame is the right one.
    ///
    /// # Errors
    ///
    /// [`FollowError::Invalid`] for an unknown state or a tempo that is not
    /// a positive number; [`FollowError::Full`] when
    /// [`FOLLOWER_CAPACITY`] changes are already waiting.
    pub fn schedule(&mut self, sync: &TransportSyncMsg) -> Result<(), FollowError> {
        let playing = match sync.state {
            TRANSPORT_PLAYING | TRANSPORT_RECORDING => true,
            TRANSPORT_STOPPED | TRANSPORT_PAUSED => false,
            _ => return Err(FollowError::Invalid),
        };
        let bpm = f64::from(sync.bpm);
        if !(bpm.is_finite() && bpm > 0.0) || sync.beat.is_infinite() {
            return Err(FollowError::Invalid);
        }
        for slot in &mut self.pending {
            if slot.is_some_and(|old| at_or_after(old.at, sync.at_frame)) {
                *slot = None;
            }
        }
        let Some(free) = self.pending.iter_mut().find(|slot| slot.is_none()) else {
            return Err(FollowError::Full);
        };
        *free = Some(Scheduled {
            at: sync.at_frame,
            sequence: self.next_sequence,
            playing,
            bpm,
            beat: sync.beat,
        });
        self.next_sequence += 1;
        self.earliest = self.pending.iter().flatten().map(|s| due_frame(s.at)).min();
        Ok(())
    }

    /// Whether any change is waiting.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        self.earliest.is_none()
    }

    /// The change due on stream frame `frame`, if any.
    ///
    /// Call it for every frame rendered, in order. When several changes are
    /// due, the newest one applies (each is a whole transport state), and
    /// they are all consumed.
    pub fn due(&mut self, frame: u64) -> Option<TransportChange> {
        if self.earliest.is_none_or(|earliest| earliest > frame) {
            return None;
        }
        let mut newest: Option<Scheduled> = None;
        for slot in &mut self.pending {
            if let Some(change) = *slot {
                if due_frame(change.at) <= frame {
                    *slot = None;
                    if newest.is_none_or(|best| change.sequence > best.sequence) {
                        newest = Some(change);
                    }
                }
            }
        }
        self.earliest = self.pending.iter().flatten().map(|s| due_frame(s.at)).min();
        let change = newest?;
        let beat = (change.playing && !change.beat.is_nan()).then(|| {
            // Frames already rendered since the change's frame: catch up.
            let late = if change.at == SYNC_NOW {
                0
            } else {
                frame - change.at
            };
            // Frame counts far below 2^53: exact as f64.
            change.beat + beats_in(late as f64, change.bpm, self.sample_rate)
        });
        Some(TransportChange {
            bpm: change.bpm,
            beat,
        })
    }
}

/// The frame a change scheduled `at` falls due: [`SYNC_NOW`] is due at once.
const fn due_frame(at: u64) -> u64 {
    if at == SYNC_NOW { 0 } else { at }
}

/// Whether a change `old_at` lands on or after `new_at`, so a newer change
/// at `new_at` replaces it.
const fn at_or_after(old_at: u64, new_at: u64) -> bool {
    new_at == SYNC_NOW || (old_at != SYNC_NOW && old_at >= new_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sync(state: u8, bpm: f32, at_frame: u64, beat: f64) -> TransportSyncMsg {
        TransportSyncMsg {
            state,
            bpm,
            at_frame,
            beat,
        }
    }

    #[test]
    fn a_change_lands_exactly_on_its_frame() {
        let mut follower = TransportFollower::new(RATE);
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 120.0, 1_000, 0.0))
            .unwrap();
        assert_eq!(follower.due(999), None);
        assert_eq!(
            follower.due(1_000),
            Some(TransportChange {
                bpm: 120.0,
                beat: Some(0.0)
            })
        );
        assert_eq!(follower.due(1_001), None);
        assert!(follower.is_idle());
    }

    #[test]
    fn a_late_change_catches_up_to_the_grid() {
        let mut follower = TransportFollower::new(RATE);
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 120.0, 1_000, 4.0))
            .unwrap();
        // 120 BPM at 48 kHz: 24 000 frames a beat. 12 000 frames late is
        // half a beat.
        let change = follower.due(13_000).unwrap();
        assert_eq!(change.beat, Some(4.5));
    }

    #[test]
    fn stopping_and_unplaced_streams_have_no_position() {
        let mut follower = TransportFollower::new(RATE);
        follower
            .schedule(&sync(TRANSPORT_STOPPED, 90.0, 10, 8.0))
            .unwrap();
        assert_eq!(
            follower.due(10),
            Some(TransportChange {
                bpm: 90.0,
                beat: None
            })
        );
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 90.0, SYNC_NOW, f64::NAN))
            .unwrap();
        assert_eq!(follower.due(11).unwrap().beat, None);
    }

    #[test]
    fn now_is_due_at_once_and_on_time() {
        let mut follower = TransportFollower::new(RATE);
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 100.0, SYNC_NOW, 2.0))
            .unwrap();
        assert_eq!(follower.due(123_456).unwrap().beat, Some(2.0));
    }

    #[test]
    fn a_re_sent_change_replaces_the_old_plan() {
        let mut follower = TransportFollower::new(RATE);
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 120.0, 2_000, 0.0))
            .unwrap();
        // The desk re-placed the stream: the same start now lands earlier.
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 120.0, 1_500, 0.0))
            .unwrap();
        assert_eq!(follower.due(1_500).unwrap().beat, Some(0.0));
        // The old plan is gone: nothing restarts the song at 2 000.
        for frame in 1_501..=2_000 {
            assert_eq!(follower.due(frame), None);
        }
    }

    #[test]
    fn changes_keep_their_order_and_the_newest_due_one_wins() {
        let mut follower = TransportFollower::new(RATE);
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 120.0, 100, 0.0))
            .unwrap();
        follower
            .schedule(&sync(TRANSPORT_STOPPED, 120.0, 200, 0.0))
            .unwrap();
        assert!(follower.due(100).unwrap().beat.is_some());
        assert_eq!(follower.due(200).unwrap().beat, None);

        // Both due by the time the frame is checked: the newer one applies.
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 120.0, 300, 0.0))
            .unwrap();
        follower
            .schedule(&sync(TRANSPORT_PLAYING, 140.0, 350, 1.0))
            .unwrap();
        let change = follower.due(400).unwrap();
        assert!((change.bpm - 140.0).abs() < f64::EPSILON);
    }

    #[test]
    fn nonsense_is_refused_and_a_full_follower_says_so() {
        let mut follower = TransportFollower::new(RATE);
        assert_eq!(
            follower.schedule(&sync(9, 120.0, 0, 0.0)),
            Err(FollowError::Invalid)
        );
        assert_eq!(
            follower.schedule(&sync(TRANSPORT_PLAYING, f32::NAN, 0, 0.0)),
            Err(FollowError::Invalid)
        );
        assert_eq!(
            follower.schedule(&sync(TRANSPORT_PLAYING, 120.0, 0, f64::INFINITY)),
            Err(FollowError::Invalid)
        );
        // Changes at later and later frames each keep the ones before.
        for n in 0..FOLLOWER_CAPACITY as u64 {
            follower
                .schedule(&sync(TRANSPORT_PLAYING, 120.0, 1_000 + n, 0.0))
                .unwrap();
        }
        assert_eq!(
            follower.schedule(&sync(TRANSPORT_PLAYING, 120.0, 5_000, 0.0)),
            Err(FollowError::Full)
        );
        // A change that replaces some of them still fits.
        follower
            .schedule(&sync(TRANSPORT_STOPPED, 120.0, 1_010, 0.0))
            .unwrap();
    }

    #[test]
    fn the_beat_formula_is_the_studio_standard() {
        assert!((beats_in(24_000.0, 120.0, 48_000.0) - 1.0).abs() < f64::EPSILON);
        assert!((beats_in(44_100.0, 60.0, 44_100.0) - 1.0).abs() < f64::EPSILON);
    }
}
