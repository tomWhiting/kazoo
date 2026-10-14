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

/// A tape and its record, boxed on the control side to cross the rings whole.
///
/// The loop swaps it for what it held and sends the same box back, so the
/// audio thread never allocates or frees.
#[derive(Debug)]
pub struct LoopTape {
    /// Interleaved stereo including the splice tail and guard frame.
    pub samples: Box<[f32]>,
    /// Recording geometry, or blank tape.
    pub splice: Option<Splice>,
    /// Most recent written pass of each bar.
    pub bar_pass: [i64; MAX_LOOP_BARS],
    /// Stored level bins.
    pub bins: [u8; LOOP_BINS],
}

/// A snapshot's box: the destination going in, the copy coming out.
#[derive(Debug)]
pub struct LoopSnapshot {
    /// Interleaved stereo copy.
    pub tape: Box<[f32]>,
    /// Beginning of the snapshot.
    pub at: TakeMark,
    /// Geometry at the beginning of the snapshot.
    pub splice: Option<Splice>,
    /// Written passes at the beginning of the snapshot.
    pub bar_pass: [i64; MAX_LOOP_BARS],
}

/// Control-side requests; buffers are allocated before entering the ring.
///
/// The tape and its record travel boxed: the brief's inline arrays made
/// every ring slot over 600 bytes, which clippy's `large_enum_variant` and
/// `result_large_err` refuse, and an unboxed copy cannot cross back without
/// the audio thread freeing its box.
#[derive(Debug)]
pub enum LoopIn {
    /// Replace the tape; the box comes back as `Retired` holding the old one.
    Install {
        /// The tape to put on, with its geometry, passes and bins.
        tape: Box<LoopTape>,
        /// Refuse a late restore after the module has spliced.
        only_if_blank: bool,
    },
    /// Copy the tape as it exists at the next sub-block boundary.
    Snapshot {
        /// Destination allocation, prepared by the control side.
        into: Box<LoopSnapshot>,
    },
}

/// Returned ownership and completed punch marks.
#[derive(Debug)]
pub enum LoopOut {
    /// The install's box, now holding the tape the loop held before.
    Retired(Box<LoopTape>),
    /// A request the processor could not accept, handed back whole.
    Refused(LoopIn),
    /// A completed recording punch.
    Took(TakeMark),
    /// A completed copy at a precise engine frame.
    Snapped(Box<LoopSnapshot>),
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
    /// Last write pass per bar; the minimum integer means never written.
    pub bar_pass: [AtomicI64; MAX_LOOP_BARS],
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
    /// Items leaked rather than freed on the audio thread because no slot
    /// was free to hand them back. Never expected: admission keeps slots.
    pub leaked: AtomicU64,
}

impl Default for LoopGauges {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(LoopState::Blank as u8),
            frames: AtomicU64::new(0),
            beats: AtomicU64::new(0),
            anchor: AtomicU64::new(0),
            pass: AtomicI64::new(0),
            bar_pass: std::array::from_fn(|_| AtomicI64::new(i64::MIN)),
            rate: AtomicU32::new(0),
            written: std::array::from_fn(|_| AtomicU64::new(0)),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            bins: std::array::from_fn(|_| AtomicU8::new(0)),
            shortened: AtomicBool::new(false),
            refused: AtomicU64::new(0),
            leaked: AtomicU64::new(0),
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
        let finite = tape.iter().all(|sample| sample.is_finite());
        let request = LoopIn::Install {
            tape: Box::new(LoopTape {
                samples: tape,
                splice,
                bar_pass,
                bins,
            }),
            only_if_blank,
        };
        // Validate samples before ownership reaches the render thread.
        if !finite {
            return Err(request);
        }
        self.inbox.try_push(request)
    }

    /// Queue a snapshot, returning its allocation when the ring is full.
    pub fn snapshot(&mut self, mut into: Box<[f32]>) -> Result<(), Box<[f32]>> {
        // Tape is finite, so a NaN destination identifies a cell not yet preserved.
        // Preparing the destination here keeps both page writes and bookkeeping off the renderer.
        into.fill(f32::NAN);
        let request = LoopIn::Snapshot {
            into: Box::new(LoopSnapshot {
                tape: into,
                at: TakeMark {
                    frame: 0,
                    beat: 0.0,
                    pass: 0,
                    bars: [false; MAX_LOOP_BARS],
                },
                splice: None,
                bar_pass: [i64::MIN; MAX_LOOP_BARS],
            }),
        };
        match self.inbox.try_push(request) {
            Ok(()) => Ok(()),
            Err(LoopIn::Snapshot { into }) => Err(into.tape),
            Err(LoopIn::Install { tape, .. }) => Err(tape.samples),
        }
    }

    /// Collect every returned item on the control side.
    pub fn collect(&mut self) -> Vec<LoopOut> {
        self.outbox
            .pop_iter()
            .map(|mut item| {
                if let LoopOut::Snapped(snapshot) = &mut item {
                    let len = snapshot
                        .splice
                        .map_or(0, |geometry| 2 * (geometry.frames + geometry.tail + 1));
                    // Extra destination capacity is not tape; remove its copy markers here.
                    snapshot.tape[len..].fill(0.0);
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
