//! The studio clock as seen from outside the audio callback.
//!
//! The desk's callback publishes the next studio frame it will render. Any
//! thread feeding the desk (the instrument hub, the built-in demo source)
//! reads that through a [`DeskTimer`] and places its audio with a
//! [`StudioStamp`]: gap-free after the previous block, and a steady lead
//! ahead of the frame playing now whenever the stream starts or slips.

use std::time::Instant;

use crate::shared::SharedState;

/// Device callback length assumed before the desk's first callback.
pub const ASSUMED_DEVICE_FRAMES: u32 = 512;

/// Scheduling margin added to every stream's lead.
pub const MARGIN_SECONDS: f32 = 0.003;

/// Extra time a transport change is scheduled ahead.
///
/// It covers the hub thread or an instrument's threads being held up by the
/// operating system while the change travels. It costs this much latency
/// between pressing play and the downbeat, and buys every instrument hearing
/// of it in time.
pub const SCHEDULE_SLACK_SECONDS: f32 = 0.02;

/// Places an instrument's audio on the studio clock.
///
/// Instruments push audio as they render it, on their own schedule, each
/// block marked with its first frame in the instrument's own stream. Once
/// the stream is placed, every block lands at its stream frame plus a fixed
/// offset: consecutive blocks play gap-free, and audio the instrument lost on
/// the way leaves a silent gap rather than pulling the rest early. The first
/// block, and any block after the stream has fallen behind the desk or crept
/// too far ahead of it, re-places the stream `lead` frames after the frame
/// the desk is playing now. The lead absorbs the jitter of both callbacks.
///
/// The offset is also how the hub tells an instrument which frame of its
/// stream a studio frame is ([`Self::stream_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StudioStamp {
    /// Studio frame minus stream frame (wrapping), while placed.
    offset: Option<u64>,
    /// Stream frame the next block should start at.
    next_stream: u64,
    /// Largest block this instrument has sent, for sizing the lead.
    largest_block: u32,
    /// Lead beyond the usual that the instrument asked for, in frames: one
    /// that renders on a timer, whose blocks arrive less evenly than a
    /// device's.
    extra_lead: u32,
}

/// Where one block landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamped {
    /// Studio frame of the block's first sample.
    pub start_frame: u64,
    /// Whether the stream was re-placed on the clock for this block.
    pub resynced: bool,
}

impl StudioStamp {
    /// Smallest block size assumed before an instrument has sent any audio.
    const INITIAL_BLOCK: u32 = 256;

    /// A clock for an instrument that has sent nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_extra_lead(0)
    }

    /// A clock for an instrument that asked for `extra_lead` frames of
    /// lead beyond the usual.
    #[must_use]
    pub const fn with_extra_lead(extra_lead: u32) -> Self {
        Self {
            offset: None,
            next_stream: 0,
            largest_block: Self::INITIAL_BLOCK,
            extra_lead,
        }
    }

    /// Frames of lead given to a stream placed on the clock, for a desk
    /// whose device callbacks are `device_frames` long: one device buffer
    /// (the render already under way), one instrument block, a small
    /// margin for scheduling jitter, and whatever extra the instrument
    /// asked for.
    #[must_use]
    pub fn lead(&self, device_frames: u32, margin_frames: u32) -> u64 {
        u64::from(device_frames)
            + u64::from(self.largest_block)
            + u64::from(margin_frames)
            + u64::from(self.extra_lead)
    }

    /// How far ahead of the frame the desk renders next a transport change
    /// must be scheduled for this instrument to hear of it before rendering
    /// that frame: as far ahead as its stream sits now (or would sit, once
    /// placed), plus two of its blocks (the one it is rendering while the
    /// change travels, and the one after), plus the margin and the slack.
    #[must_use]
    pub fn schedule_need(&self, clock: DeskClock) -> u64 {
        let ahead = self.next_frame().map_or_else(
            || {
                u64::from(clock.elapsed_frames)
                    + self.lead(clock.device_frames, clock.margin_frames)
            },
            |next| next.saturating_sub(clock.studio_now),
        );
        ahead
            + 2 * u64::from(self.largest_block)
            + u64::from(clock.margin_frames)
            + u64::from(clock.slack_frames)
    }

    /// Stamp a block of `frames` starting at `stream_frame` in the
    /// instrument's stream, against the desk's clock.
    pub fn stamp(&mut self, stream_frame: u64, frames: u32, clock: DeskClock) -> Stamped {
        self.largest_block = self.largest_block.max(frames);
        let lead = self.lead(clock.device_frames, clock.margin_frames);
        // Placing from the frame playing now, not the last callback's
        // boundary, keeps the lead the same whenever in the device's cycle
        // the block arrives.
        let playing = clock.playing_now();
        // A stream more than two leads ahead has crept away from the desk
        // (the instrument's clock runs fast, or it burst after a stall);
        // one that starts before the frame the desk renders next is late.
        // A stream that goes backwards has restarted.
        let latest = playing.saturating_add(lead.saturating_mul(3));
        let placed = self
            .offset
            .filter(|_| stream_frame >= self.next_stream)
            .map(|offset| stream_frame.wrapping_add(offset))
            .filter(|start| *start >= clock.studio_now && *start <= latest);
        let (start_frame, resynced) = if let Some(start) = placed {
            (start, false)
        } else {
            let start = playing.saturating_add(lead);
            self.offset = Some(start.wrapping_sub(stream_frame));
            (start, true)
        };
        self.next_stream = stream_frame.saturating_add(u64::from(frames));
        Stamped {
            start_frame,
            resynced,
        }
    }

    /// Studio frame the next block will start at if the stream stays on
    /// time; `None` before the first block or after a reset.
    #[must_use]
    pub const fn next_frame(&self) -> Option<u64> {
        match self.offset {
            Some(offset) => Some(self.next_stream.wrapping_add(offset)),
            None => None,
        }
    }

    /// The instrument's stream frame at studio frame `studio`, while
    /// placed.
    #[must_use]
    pub const fn stream_at(&self, studio: u64) -> Option<u64> {
        match self.offset {
            Some(offset) => Some(studio.wrapping_sub(offset)),
            None => None,
        }
    }

    /// Whether the stream is at least `lead` ahead of the frame playing
    /// now, so a feeder rendering ahead of time should wait.
    #[must_use]
    pub fn far_enough_ahead(&self, clock: DeskClock) -> bool {
        let lead = self.lead(clock.device_frames, clock.margin_frames);
        self.next_frame()
            .is_some_and(|next| next >= clock.playing_now().saturating_add(lead))
    }

    /// Forget the stream's place, so the next block is placed afresh.
    pub const fn reset(&mut self) {
        self.offset = None;
    }
}

impl Default for StudioStamp {
    fn default() -> Self {
        Self::new()
    }
}

/// The desk's clock, as read once by a thread feeding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeskClock {
    /// Frame the desk's callback renders next.
    pub studio_now: u64,
    /// Frames played since the callback published `studio_now`, at most
    /// one device buffer.
    pub elapsed_frames: u32,
    /// Device callback length, in frames.
    pub device_frames: u32,
    /// Jitter margin, in frames.
    pub margin_frames: u32,
    /// Allowance for threads held up while a transport change travels, in
    /// frames (see [`SCHEDULE_SLACK_SECONDS`]).
    pub slack_frames: u32,
}

impl DeskClock {
    /// The studio frame playing now, as near as the hub can tell.
    #[must_use]
    pub const fn playing_now(&self) -> u64 {
        self.studio_now.saturating_add(self.elapsed_frames as u64)
    }
}

/// Estimates how far the desk has played since its callback last published
/// the studio frame.
///
/// The callback publishes once per device buffer, so the published frame
/// alone jumps a whole buffer at a time. The hub notes when it first sees
/// each new value and counts the time since, capped at one buffer (a
/// stalled stream does not run on).
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameWatch {
    seen: Option<(u64, Instant)>,
}

impl FrameWatch {
    /// Frames elapsed since `frame` was published, observed at `now`.
    pub fn elapsed(
        &mut self,
        frame: u64,
        now: Instant,
        sample_rate: u32,
        device_frames: u32,
    ) -> u32 {
        match self.seen {
            Some((seen_frame, since)) if seen_frame == frame => {
                let frames =
                    now.saturating_duration_since(since).as_secs_f64() * f64::from(sample_rate);
                // Capped to one buffer, so the conversion cannot overflow.
                frames.min(f64::from(device_frames)).round() as u32
            }
            _ => {
                self.seen = Some((frame, now));
                0
            }
        }
    }
}

/// Reads the desk's clock for one feeding thread.
#[derive(Debug, Clone, Copy)]
pub struct DeskTimer {
    watch: FrameWatch,
    sample_rate: u32,
    margin_frames: u32,
    slack_frames: u32,
}

impl DeskTimer {
    /// A timer for a desk running at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            watch: FrameWatch::default(),
            sample_rate,
            // Milliseconds of margin at audio rates: far inside u32.
            margin_frames: (sample_rate as f32 * MARGIN_SECONDS).round() as u32,
            slack_frames: (sample_rate as f32 * SCHEDULE_SLACK_SECONDS).round() as u32,
        }
    }

    /// The desk's clock now.
    pub fn read(&mut self, shared: &SharedState) -> DeskClock {
        let studio_now = shared.studio_frame();
        let device_frames = shared.callback_frames().unwrap_or(ASSUMED_DEVICE_FRAMES);
        DeskClock {
            studio_now,
            elapsed_frames: self.watch.elapsed(
                studio_now,
                Instant::now(),
                self.sample_rate,
                device_frames,
            ),
            device_frames,
            margin_frames: self.margin_frames,
            slack_frames: self.slack_frames,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DEVICE: u32 = 512;
    const MARGIN: u32 = 144;
    const SLACK: u32 = 960;

    /// The desk about to render `studio_now`, at a callback boundary.
    const fn at(studio_now: u64) -> DeskClock {
        DeskClock {
            studio_now,
            elapsed_frames: 0,
            device_frames: DEVICE,
            margin_frames: MARGIN,
            slack_frames: SLACK,
        }
    }

    #[test]
    fn first_block_is_placed_one_lead_ahead_then_follows_on() {
        let mut stamp = StudioStamp::new();
        let first = stamp.stamp(0, 256, at(10_000));
        assert_eq!(
            first,
            Stamped {
                start_frame: 10_000 + 512 + 256 + 144,
                resynced: true
            }
        );
        let second = stamp.stamp(256, 256, at(10_100));
        assert_eq!(second.start_frame, first.start_frame + 256);
        assert!(!second.resynced);
        assert_eq!(stamp.stream_at(first.start_frame + 300), Some(300));
    }

    #[test]
    fn lost_audio_leaves_a_gap_and_keeps_the_rest_in_place() {
        let mut stamp = StudioStamp::new();
        let first = stamp.stamp(0, 256, at(10_000));
        // Blocks at 256 and 512 never arrived.
        let later = stamp.stamp(768, 256, at(10_100));
        assert!(!later.resynced);
        assert_eq!(later.start_frame, first.start_frame + 768);
    }

    #[test]
    fn a_stream_that_goes_backwards_is_placed_afresh() {
        let mut stamp = StudioStamp::new();
        stamp.stamp(10_000, 256, at(0));
        assert!(stamp.stamp(0, 256, at(0)).resynced);
    }

    #[test]
    fn placement_counts_from_the_frame_playing_now() {
        let mut stamp = StudioStamp::new();
        let lead = stamp.lead(DEVICE, MARGIN);
        let midway = DeskClock {
            elapsed_frames: 300,
            ..at(10_000)
        };
        assert_eq!(stamp.stamp(0, 256, midway).start_frame, 10_300 + lead);
    }

    #[test]
    fn a_stream_that_falls_behind_the_desk_is_placed_afresh() {
        let mut stamp = StudioStamp::new();
        let first = stamp.stamp(0, 256, at(0));
        // The desk has already passed where the next block would go.
        let late = first.start_frame + 256 + 1;
        let placed = stamp.stamp(256, 256, at(late));
        assert!(placed.resynced);
        assert_eq!(placed.start_frame, late + stamp.lead(DEVICE, MARGIN));
        assert_eq!(stamp.stream_at(placed.start_frame), Some(256));
    }

    #[test]
    fn a_stream_that_creeps_ahead_is_pulled_back() {
        let mut stamp = StudioStamp::new();
        let lead = stamp.lead(DEVICE, MARGIN);
        stamp.stamp(0, 256, at(0));
        // Many blocks with the desk standing still: the stream runs ahead.
        let mut resynced = false;
        for n in 1..=40 {
            resynced |= stamp.stamp(n * 256, 256, at(0)).resynced;
        }
        assert!(resynced);
        let next = stamp.stamp(41 * 256, 256, at(0));
        assert!(next.start_frame <= 3 * lead);
    }

    #[test]
    fn the_lead_grows_with_the_largest_block_seen() {
        let mut stamp = StudioStamp::new();
        let small = stamp.lead(DEVICE, MARGIN);
        stamp.stamp(0, 2048, at(0));
        assert_eq!(stamp.lead(DEVICE, MARGIN), small - 256 + 2048);
        // Placed one lead (plus the block itself) ahead of the desk.
        assert_eq!(
            stamp.schedule_need(at(0)),
            stamp.lead(DEVICE, MARGIN) + 2048 + 2 * 2048 + u64::from(MARGIN) + u64::from(SLACK)
        );
        // However far ahead the stream really sits is what counts.
        assert_eq!(
            stamp.schedule_need(at(1_000)),
            stamp.lead(DEVICE, MARGIN) + 2048 - 1_000
                + 2 * 2048
                + u64::from(MARGIN)
                + u64::from(SLACK)
        );
        assert_eq!(
            StudioStamp::new().schedule_need(at(0)),
            StudioStamp::new().lead(DEVICE, MARGIN)
                + 2 * 256
                + u64::from(MARGIN)
                + u64::from(SLACK)
        );
    }

    #[test]
    fn a_feeder_waits_once_a_lead_ahead() {
        let mut stamp = StudioStamp::new();
        assert!(!stamp.far_enough_ahead(at(0)));
        let first = stamp.stamp(0, 256, at(0));
        // Placed one lead ahead, plus its own length: time to wait.
        assert!(stamp.far_enough_ahead(at(0)));
        assert_eq!(stamp.next_frame(), Some(first.start_frame + 256));
        // Once the desk has played a block's worth, render the next.
        assert!(!stamp.far_enough_ahead(at(257)));
    }

    #[test]
    fn the_frame_watch_counts_time_since_each_new_frame_up_to_a_buffer() {
        let mut watch = FrameWatch::default();
        let start = Instant::now();
        assert_eq!(watch.elapsed(1_000, start, 48_000, 512), 0);
        // 5 ms at 48 kHz is 240 frames.
        let later = start + Duration::from_millis(5);
        assert_eq!(watch.elapsed(1_000, later, 48_000, 512), 240);
        // A stalled desk does not run on past one buffer.
        let stalled = start + Duration::from_secs(1);
        assert_eq!(watch.elapsed(1_000, stalled, 48_000, 512), 512);
        // A new frame starts the count again.
        assert_eq!(watch.elapsed(1_512, stalled, 48_000, 512), 0);
    }
}
