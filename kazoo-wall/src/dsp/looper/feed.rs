//! Ownership crosses bounded rings; only the control side frees returned tape.
use super::{FEED_SLOTS, LOOP_BINS, MAX_LOOP_BARS, Splice};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64},
};

/// The recorder's public state, sampled once per sub-block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LoopState {
    /// No recording has been spliced.
    Blank,
    /// Waiting for a bar line.
    Armed,
    /// Writing the incoming signal.
    Rolling,
    /// Completing the punch fade.
    PostRoll,
    /// Playing an existing recording.
    Holding,
    /// Tape allocation failed or no capacity was supplied.
    NoTape,
}

/// A frame-stamped set of bars written by a punch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakeMark {
    /// Engine frame at the mark.
    pub frame: u64,
    /// Song beat at the mark.
    pub beat: f64,
    /// Tape pass at the mark.
    pub pass: i64,
    /// Bars written since the previous completed punch.
    pub bars: [bool; MAX_LOOP_BARS],
}

/// Control-side requests; buffers are allocated before entering the ring.
#[derive(Debug)]
pub enum LoopIn {
    /// Replace the tape, returning the previous allocation.
    Install {
        /// Interleaved stereo including the splice tail and guard frame.
        tape: Box<[f32]>,
        /// Recording geometry, or blank tape.
        splice: Option<Splice>,
        /// Most recent written pass of each bar.
        bar_pass: [i64; MAX_LOOP_BARS],
        /// Stored level bins.
        bins: [u8; LOOP_BINS],
        /// Refuse a late restore after the module has spliced.
        only_if_blank: bool,
    },
    /// Copy the tape as it exists at the next sub-block boundary.
    Snapshot {
        /// Destination allocation, prepared by the control side.
        into: Box<[f32]>,
    },
}

/// Returned ownership and completed punch marks.
#[derive(Debug)]
pub enum LoopOut {
    /// An allocation replaced by an install.
    Retired(Box<[f32]>),
    /// A request the processor could not accept.
    Refused(Box<[f32]>),
    /// A completed recording punch.
    Took(TakeMark),
    /// A completed copy at a precise engine frame.
    Snapped {
        /// Interleaved stereo copy.
        tape: Box<[f32]>,
        /// Beginning of the snapshot.
        at: TakeMark,
        /// Geometry at the beginning of the snapshot.
        splice: Option<Splice>,
        /// Written passes at the beginning of the snapshot.
        bar_pass: [i64; MAX_LOOP_BARS],
    },
}

/// Atomic observations; readers may see adjacent sub-blocks together.
#[derive(Debug)]
pub struct LoopGauges {
    /// A discriminant of [`LoopState`].
    pub state: AtomicU8,
    /// Frames in the spliced loop.
    pub frames: AtomicU64,
    /// Loop beats as f64 bits.
    pub beats: AtomicU64,
    /// Anchor beat as f64 bits.
    pub anchor: AtomicU64,
    /// Current tape pass.
    pub pass: AtomicI64,
    /// Splice sample rate as f32 bits.
    pub rate: AtomicU32,
    /// Bars written during the current punch.
    pub written: [AtomicU64; 2],
    /// Left peak as f32 bits; the reader swaps zero to reset it.
    pub peak_left: AtomicU32,
    /// Right peak as f32 bits; the reader swaps zero to reset it.
    pub peak_right: AtomicU32,
    /// Level per sixty-fourth of the loop.
    pub bins: [AtomicU8; LOOP_BINS],
    /// The requested length did not fit.
    pub shortened: AtomicBool,
    /// Number of refused requests.
    pub refused: AtomicU64,
}

impl Default for LoopGauges {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(LoopState::Blank as u8),
            frames: AtomicU64::new(0),
            beats: AtomicU64::new(0),
            anchor: AtomicU64::new(0),
            pass: AtomicI64::new(0),
            rate: AtomicU32::new(0),
            written: std::array::from_fn(|_| AtomicU64::new(0)),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            bins: std::array::from_fn(|_| AtomicU8::new(0)),
            shortened: AtomicBool::new(false),
            refused: AtomicU64::new(0),
        }
    }
}

/// The control-side handle; collection may allocate and free.
pub struct LoopFeed {
    pub(super) inbox: HeapProd<LoopIn>,
    pub(super) outbox: HeapCons<LoopOut>,
    pub(super) gauges: Arc<LoopGauges>,
}

impl LoopFeed {
    /// Queue a replacement, returning the whole request if the ring is full.
    pub fn install(
        &mut self,
        tape: Box<[f32]>,
        splice: Option<Splice>,
        bar_pass: [i64; MAX_LOOP_BARS],
        bins: [u8; LOOP_BINS],
        only_if_blank: bool,
    ) -> Result<(), LoopIn> {
        // Validate samples before ownership reaches the render thread.
        if tape.iter().any(|sample| !sample.is_finite()) {
            return Err(LoopIn::Install {
                tape,
                splice,
                bar_pass,
                bins,
                only_if_blank,
            });
        }
        self.inbox.try_push(LoopIn::Install {
            tape,
            splice,
            bar_pass,
            bins,
            only_if_blank,
        })
    }

    /// Queue a snapshot, returning its allocation when the ring is full.
    pub fn snapshot(&mut self, mut into: Box<[f32]>) -> Result<(), Box<[f32]>> {
        // Tape is finite, so a NaN destination identifies a cell not yet preserved.
        // Preparing the destination here keeps both page writes and bookkeeping off the renderer.
        into.fill(f32::NAN);
        match self.inbox.try_push(LoopIn::Snapshot { into }) {
            Ok(()) => Ok(()),
            Err(LoopIn::Snapshot { into }) => Err(into),
            Err(LoopIn::Install { tape, .. }) => Err(tape),
        }
    }

    /// Collect every returned item on the control side.
    pub fn collect(&mut self) -> Vec<LoopOut> {
        self.outbox
            .pop_iter()
            .map(|mut item| {
                if let LoopOut::Snapped { tape, splice, .. } = &mut item {
                    let len =
                        splice.map_or(0, |geometry| 2 * (geometry.frames + geometry.tail + 1));
                    // Extra destination capacity is not tape; remove its copy markers here.
                    tape[len..].fill(0.0);
                }
                item
            })
            .collect()
    }

    /// Observe the most recently rendered sub-block.
    #[must_use]
    pub fn gauges(&self) -> &LoopGauges {
        &self.gauges
    }
}

pub(super) fn rings() -> (HeapCons<LoopIn>, HeapProd<LoopOut>, LoopFeed) {
    let (inbox, incoming) = HeapRb::new(FEED_SLOTS).split();
    let (outgoing, outbox) = HeapRb::new(FEED_SLOTS).split();
    let gauges = Arc::new(LoopGauges::default());
    (
        incoming,
        outgoing,
        LoopFeed {
            inbox,
            outbox,
            gauges,
        },
    )
}

impl std::fmt::Debug for LoopFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopFeed")
            .field("gauges", &self.gauges)
            .finish_non_exhaustive()
    }
}
