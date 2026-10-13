//! Disk reads and resident replacement buffers belong to the tape thread.
//!
//! The thread runs whether or not the reel store opens: a blank tape (an
//! erase) needs no store, so only a fetch fails when there is none.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;

use crate::dsp::looper::{
    LOOP_BINS, MAX_LOOP_BARS, Splice, bin_thresholds, level_bin, splice_fits,
};
use crate::patch::TapeRecord;
use kazoo_core::reels::{ReelError, ReelId, ReelStore};

/// Jobs waiting for the tape thread at most; a full queue is retried on
/// the next tick, never waited on.
pub const TAPE_QUEUE: usize = 64;

/// Work for the tape thread. Both jobs allocate a whole tape there.
#[derive(Debug)]
pub enum TapeJob {
    /// Read a saved reel into a tape of the loop's length.
    Fetch {
        module: String,
        record: TapeRecord,
        capacity_frames: usize,
        tail_cap: usize,
    },
    /// A zeroed tape of the loop's length.
    Blank {
        module: String,
        capacity_frames: usize,
        tail_cap: usize,
    },
}

/// A fetched tape: samples, splice, the pass of each bar and level bins.
pub type FetchedTape = (Box<[f32]>, Splice, [i64; MAX_LOOP_BARS], [u8; LOOP_BINS]);

/// The tape thread's answers. An error is the end of a sentence that
/// begins "loop1's reel 9f3c01ab" (a fetch) or "loop1" (a blank).
#[derive(Debug)]
pub enum TapeDone {
    Fetched {
        module: String,
        result: Result<Box<FetchedTape>, String>,
    },
    Blanked {
        module: String,
        result: Result<Box<[f32]>, String>,
    },
}

/// The control side's handle on the tape thread.
#[derive(Debug)]
pub struct TapeWorker {
    jobs: SyncSender<(u64, Box<TapeJob>)>,
    pub done: Receiver<(u64, TapeDone)>,
    /// Why the reel store did not open, if it did not.
    pub store_error: Option<String>,
}

impl TapeWorker {
    /// Start the tape thread on the reel store at `dir`, or with no store
    /// (`dir` an error) when there is none to open.
    ///
    /// # Errors
    /// The thread could not be started, or stopped before it said whether
    /// the store opened.
    pub fn start(dir: Result<PathBuf, String>) -> Result<Self, String> {
        let (jobs, incoming) = mpsc::sync_channel::<(u64, Box<TapeJob>)>(TAPE_QUEUE);
        let (answers, done) = mpsc::channel();
        let (ready, opened) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("kazoo-wall tape".into())
            .spawn(move || {
                let store = dir.and_then(|dir| {
                    ReelStore::open(&dir)
                        .map(|store| (store, dir.clone()))
                        .map_err(|error| {
                            format!("the reel store at {} cannot open ({error})", dir.display())
                        })
                });
                let said = store.as_ref().map(|_| ()).map_err(Clone::clone);
                if let Err(error) = ready.send(said) {
                    eprintln!("kazoo-wall: tape startup reply lost: {error}");
                    return;
                }
                serve(&store, &incoming, &answers);
            })
            .map_err(|error| format!("the tape thread could not start ({error})"))?;
        let store_error = opened
            .recv()
            .map_err(|error| format!("the tape thread stopped as it started ({error})"))?
            .err();
        Ok(Self {
            jobs,
            done,
            store_error,
        })
    }

    /// Queue `job`, handing it back if the queue is full or the thread
    /// has gone.
    ///
    /// # Errors
    /// The queue is full, or the thread has stopped.
    pub fn submit(
        &self,
        ticket: u64,
        job: Box<TapeJob>,
    ) -> Result<(), TrySendError<(u64, Box<TapeJob>)>> {
        self.jobs.try_send((ticket, job))
    }
}

fn serve(
    store: &Result<(ReelStore, PathBuf), String>,
    incoming: &Receiver<(u64, Box<TapeJob>)>,
    answers: &mpsc::Sender<(u64, TapeDone)>,
) {
    let thresholds = bin_thresholds();
    while let Ok((ticket, job)) = incoming.recv() {
        let answer = match *job {
            TapeJob::Fetch {
                module,
                record,
                capacity_frames,
                tail_cap,
            } => TapeDone::Fetched {
                module,
                result: store
                    .as_ref()
                    .map_err(|error| format!("cannot be fetched: {error}"))
                    .and_then(|(store, dir)| {
                        fetch(store, dir, &record, capacity_frames, tail_cap, &thresholds)
                    })
                    .map(Box::new),
            },
            TapeJob::Blank {
                module,
                capacity_frames,
                tail_cap,
            } => TapeDone::Blanked {
                module,
                result: resident(capacity_frames, tail_cap),
            },
        };
        if let Err(error) = answers.send((ticket, answer)) {
            eprintln!("kazoo-wall: tape reply lost: {error}");
            return;
        }
    }
}

/// A zeroed tape of `2 × (capacity + tail + 1)` samples, every page
/// written here so none is first touched by the render thread.
fn resident(capacity: usize, tail: usize) -> Result<Box<[f32]>, String> {
    let len = capacity
        .checked_add(tail)
        .and_then(|n| n.checked_add(1))
        .and_then(|n| n.checked_mul(2))
        .ok_or("has a tape too long to hold")?;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(len)
        .map_err(|error| format!("has no memory for its tape ({error})"))?;
    samples.resize(len, 0.0);
    Ok(samples.into_boxed_slice())
}

fn fetch(
    store: &ReelStore,
    dir: &Path,
    record: &TapeRecord,
    capacity: usize,
    tail_cap: usize,
    thresholds: &[f32; 256],
) -> Result<FetchedTape, String> {
    record
        .check()
        .map_err(|why| format!("does not read as a tape ({why})"))?;
    let id: ReelId = record
        .reel
        .parse()
        .map_err(|error: ReelError| format!("does not read as a tape ({error})"))?;
    let reel = store.get(&id).map_err(|error| match error {
        ReelError::Missing(_) => format!("is missing from {}", dir.display()),
        other => format!("could not be read from {} ({other})", dir.display()),
    })?;
    let frames = usize::try_from(record.frames).map_err(|error| error.to_string())?;
    let tail = usize::try_from(record.tail).map_err(|error| error.to_string())?;
    if reel.rate != record.rate || reel.frames() != frames + tail {
        return Err(format!(
            "holds {} frames at {} Hz where the save says {} at {} Hz",
            reel.frames(),
            reel.rate,
            frames + tail,
            record.rate
        ));
    }
    if frames > capacity || tail > tail_cap {
        return Err(format!(
            "holds {frames} frames and a {tail}-frame seam, more than this loop's {capacity} and {tail_cap}"
        ));
    }
    let splice = Splice {
        beats: record.beats,
        frames,
        tail,
        anchor: record.anchor,
        rate: record.rate as f32,
    };
    let mut samples = resident(capacity, tail_cap)?;
    if !splice_fits(splice, samples.len()) {
        return Err(format!(
            "has a splice no loop plays ({} beats, {frames} frames, a {tail}-frame seam at {} Hz)",
            record.beats, record.rate
        ));
    }
    samples[..reel.samples.len()].copy_from_slice(&reel.samples);
    // The reel ends with its seam's last frame; the guard after it holds
    // that frame, so the seam's last interpolation runs level.
    let last = 2 * (frames + tail);
    samples[last] = samples[last - 2];
    samples[last + 1] = samples[last - 1];
    drop(reel);
    let mut bars = [i64::MIN; MAX_LOOP_BARS];
    for (into, pass) in bars.iter_mut().zip(&record.bars) {
        *into = pass.unwrap_or(i64::MIN);
    }
    let mut bins = [0; LOOP_BINS];
    for (bin, into) in bins.iter_mut().enumerate() {
        let start = bin * frames / LOOP_BINS;
        let end = (bin + 1) * frames / LOOP_BINS;
        let peak = samples[2 * start..2 * end]
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        *into = level_bin(thresholds, peak);
    }
    Ok((samples, splice, bars, bins))
}
