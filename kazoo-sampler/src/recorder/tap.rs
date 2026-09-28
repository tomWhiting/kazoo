//! The audio-thread end of the recorder, and the state it shares with the
//! control side and the writer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ringbuf::HeapProd;
use ringbuf::traits::{Observer, Producer};

/// Bit 0 of [`Shared::request`]: the control side wants this take recorded.
pub(crate) const WANT: u64 = 1;

/// Bit 1 of [`Shared::request`]: the tap has claimed the take and is
/// recording it (or has recorded it).
pub(crate) const CLAIMED: u64 = 2;

/// The take number in [`Shared::request`] sits above the two flag bits.
pub(crate) const GEN_SHIFT: u32 = 2;

/// State the control side, the tap and the writer share. Every field is an
/// atomic, so the tap never waits on anyone.
#[derive(Debug)]
pub(crate) struct Shared {
    /// `take << GEN_SHIFT | CLAIMED? | WANT?`. The control side stores a new
    /// take with `WANT`; the tap claims it by compare-and-swap; stopping
    /// clears `WANT` and keeps `CLAIMED`, so a take can never be claimed
    /// after it was stopped.
    pub(crate) request: AtomicU64,
    /// The longest the requested take may run, in frames. Written before
    /// the request that it belongs to.
    pub(crate) max_frames: AtomicU64,
    /// The last take the tap finished. Stored (release) after the take's
    /// last push, so a writer that sees it (acquire) sees every frame.
    pub(crate) stopped: AtomicU64,
    /// The last take that ended by running into its length limit.
    pub(crate) limited: AtomicU64,
    /// Frames of the current take handed to the ring.
    pub(crate) frames: AtomicU64,
    /// Frames of the current take lost because the ring was full.
    pub(crate) dropped: AtomicU64,
    /// Where the claimed take starts in the ring: the count of samples
    /// pushed before it, over every take. Stored before the claim, so a
    /// writer that sees the claim sees it.
    pub(crate) start_at: AtomicU64,
    /// Cleared when the tap is dropped.
    pub(crate) tap_alive: AtomicBool,
}

impl Shared {
    pub(crate) const fn new() -> Self {
        Self {
            request: AtomicU64::new(0),
            max_frames: AtomicU64::new(0),
            stopped: AtomicU64::new(0),
            limited: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            start_at: AtomicU64::new(0),
            tap_alive: AtomicBool::new(true),
        }
    }

    /// Stop take `take` if it is still the one requested: clear `WANT`,
    /// keep `CLAIMED`.
    pub(crate) fn stop(&self, take: u64) {
        let mut now = self.request.load(Ordering::Acquire);
        while now >> GEN_SHIFT == take && now & WANT != 0 {
            match self.request.compare_exchange_weak(
                now,
                now & !WANT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => now = actual,
            }
        }
    }

    /// Where take `take` starts in the ring, once the tap has claimed it.
    pub(crate) fn start_of(&self, take: u64) -> Option<u64> {
        let request = self.request.load(Ordering::Acquire);
        let claimed = request >> GEN_SHIFT == take && request & CLAIMED != 0;
        claimed.then(|| self.start_at.load(Ordering::Acquire))
    }

    /// Whether take `take` has been asked to stop.
    pub(crate) fn stop_requested(&self, take: u64) -> bool {
        let request = self.request.load(Ordering::Acquire);
        request >> GEN_SHIFT != take || request & WANT == 0
    }

    /// Whether take `take` is over from the tap's side: it finished it, it
    /// never claimed it before it was stopped, or the tap is gone.
    pub(crate) fn is_over(&self, take: u64) -> bool {
        if self.stopped.load(Ordering::Acquire) >= take || !self.tap_alive.load(Ordering::Acquire) {
            return true;
        }
        let request = self.request.load(Ordering::Acquire);
        request >> GEN_SHIFT != take || request & (WANT | CLAIMED) == 0
    }
}

/// The take the tap is recording.
#[derive(Debug, Clone, Copy)]
struct Active {
    take: u64,
    max: u64,
    elapsed: u64,
    pushed: u64,
    dropped: u64,
}

/// The audio thread's end of a [`super::Recorder`]. Call [`Self::process`]
/// with every stereo block that might be recorded; it records only while a
/// take is running.
pub struct RecordTap {
    shared: Arc<Shared>,
    producer: HeapProd<f32>,
    active: Option<Active>,
    /// Samples pushed into the ring over every take.
    pushed_total: u64,
}

impl std::fmt::Debug for RecordTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordTap")
            .field("recording", &self.active.is_some())
            .finish_non_exhaustive()
    }
}

impl RecordTap {
    pub(crate) const fn new(shared: Arc<Shared>, producer: HeapProd<f32>) -> Self {
        Self {
            shared,
            producer,
            active: None,
            pushed_total: 0,
        }
    }

    /// Whether a take is being recorded right now.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// Offer one stereo block. While a take runs its frames go to the
    /// writer; if the ring is full the frames that do not fit are counted
    /// as dropped, never waited for. Channels of different lengths are cut
    /// to the shorter; NaN and infinity are recorded as silence.
    ///
    /// Real-time safe: no allocation, lock, I/O or panic.
    pub fn process(&mut self, left: &[f32], right: &[f32]) {
        self.follow_request();
        let Some(mut active) = self.active else {
            return;
        };
        let frames = left.len().min(right.len()) as u64;
        let wanted = frames.min(active.max.saturating_sub(active.elapsed));
        let fits = wanted.min(self.producer.vacant_len() as u64 / 2);
        let fits_len = fits as usize;
        let frames_in = left[..fits_len].iter().zip(&right[..fits_len]);
        let samples = frames_in.flat_map(|(&l, &r)| [clean(l), clean(r)]);
        let pushed_samples = self.producer.push_iter(samples) as u64;
        self.pushed_total += pushed_samples;
        let pushed = pushed_samples / 2;
        active.elapsed += wanted;
        active.pushed += pushed;
        active.dropped += wanted - pushed;
        self.shared.frames.store(active.pushed, Ordering::Relaxed);
        self.shared.dropped.store(active.dropped, Ordering::Relaxed);
        if active.elapsed >= active.max {
            self.shared.limited.store(active.take, Ordering::Release);
            self.finish(active.take);
        } else {
            self.active = Some(active);
        }
    }

    /// Start or stop as the control side asks.
    fn follow_request(&mut self) {
        let request = self.shared.request.load(Ordering::Acquire);
        let take = request >> GEN_SHIFT;
        if let Some(active) = self.active {
            if take == active.take && request & WANT != 0 {
                return;
            }
            // Ended or replaced: finish it, then see whether a new take
            // wants claiming in this same block.
            self.finish(active.take);
        }
        if request & (WANT | CLAIMED) != WANT {
            return;
        }
        self.shared
            .start_at
            .store(self.pushed_total, Ordering::Release);
        let claimed = self.shared.request.compare_exchange(
            request,
            request | CLAIMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if claimed.is_err() {
            // Stopped or replaced in the meantime: the next block looks
            // again.
            return;
        }
        self.shared.frames.store(0, Ordering::Relaxed);
        self.shared.dropped.store(0, Ordering::Relaxed);
        self.active = Some(Active {
            take,
            max: self.shared.max_frames.load(Ordering::Acquire),
            elapsed: 0,
            pushed: 0,
            dropped: 0,
        });
    }

    /// End the take: after this, the writer may drain the ring and trust
    /// that it holds all of it.
    fn finish(&mut self, take: u64) {
        self.active = None;
        self.shared.stopped.store(take, Ordering::Release);
    }
}

impl Drop for RecordTap {
    fn drop(&mut self) {
        if let Some(active) = self.active {
            self.finish(active.take);
        }
        self.shared.tap_alive.store(false, Ordering::Release);
    }
}

/// Silence for NaN and infinity.
const fn clean(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}
