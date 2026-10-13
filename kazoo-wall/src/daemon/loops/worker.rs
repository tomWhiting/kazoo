//! Disk reads and resident replacement buffers belong to the tape thread.
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;

use crate::dsp::looper::{LOOP_BINS, MAX_LOOP_BARS, Splice};
use crate::patch::TapeRecord;
use kazoo_core::reels::{ReelId, ReelStore};

#[derive(Debug)]
pub enum TapeJob {
    Fetch {
        module: String,
        record: TapeRecord,
        capacity_frames: usize,
        tail_cap: usize,
    },
    Blank {
        module: String,
        capacity_frames: usize,
        tail_cap: usize,
    },
}
pub type FetchedTape = (Box<[f32]>, Splice, [i64; MAX_LOOP_BARS], [u8; LOOP_BINS]);
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
#[derive(Debug)]
pub struct TapeWorker {
    pub jobs: SyncSender<(u64, Box<TapeJob>)>,
    pub done: Receiver<(u64, TapeDone)>,
}
impl TapeWorker {
    pub fn start(dir: PathBuf) -> Result<Self, String> {
        let (jobs, incoming) = mpsc::sync_channel::<(u64, Box<TapeJob>)>(64);
        let (answers, done) = mpsc::channel();
        let (ready, opened) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("kazoo-wall tape".into())
            .spawn(move || {
                let store = match ReelStore::open(&dir) {
                    Ok(store) => store,
                    Err(error) => {
                        if let Err(send) = ready.send(Err(error.to_string())) {
                            eprintln!("kazoo-wall: tape startup reply lost: {send}");
                        }
                        return;
                    }
                };
                if let Err(error) = ready.send(Ok(())) {
                    eprintln!("kazoo-wall: tape startup reply lost: {error}");
                    return;
                }
                while let Ok((ticket, job)) = incoming.recv() {
                    let answer = match *job {
                        TapeJob::Fetch {
                            module,
                            record,
                            capacity_frames,
                            tail_cap,
                        } => TapeDone::Fetched {
                            module,
                            result: fetch(&store, &record, capacity_frames, tail_cap).map(Box::new),
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
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        opened.recv().map_err(|error| error.to_string())??;
        Ok(Self { jobs, done })
    }
    pub fn submit(
        &self,
        ticket: u64,
        job: Box<TapeJob>,
    ) -> Result<(), TrySendError<(u64, Box<TapeJob>)>> {
        self.jobs.try_send((ticket, job))
    }
}
fn resident(capacity: usize, tail: usize) -> Result<Box<[f32]>, String> {
    let len = capacity
        .checked_add(tail)
        .and_then(|n| n.checked_add(1))
        .and_then(|n| n.checked_mul(2))
        .ok_or("tape capacity overflow")?;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(len)
        .map_err(|error| format!("no memory for tape: {error}"))?;
    samples.resize(len, 0.0);
    Ok(samples.into_boxed_slice())
}
fn fetch(
    store: &ReelStore,
    record: &TapeRecord,
    capacity: usize,
    tail_cap: usize,
) -> Result<FetchedTape, String> {
    record.check()?;
    let id: ReelId = record
        .reel
        .parse()
        .map_err(|error: kazoo_core::reels::ReelError| error.to_string())?;
    let reel = store.get(&id).map_err(|error| error.to_string())?;
    let frames = usize::try_from(record.frames).map_err(|error| error.to_string())?;
    let tail = usize::try_from(record.tail).map_err(|error| error.to_string())?;
    if reel.rate != record.rate
        || reel.frames() != frames + tail
        || frames > capacity
        || tail > tail_cap
    {
        return Err("reel geometry does not fit the saved tape or this device".into());
    }
    let mut samples = resident(capacity, tail_cap)?;
    samples[..reel.samples.len()].copy_from_slice(&reel.samples);
    if frames > 0 {
        samples[2 * (frames + tail)] = samples[0];
        samples[2 * (frames + tail) + 1] = samples[1];
    }
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
        *into = if peak <= 0.001 {
            0
        } else {
            ((20.0_f32.mul_add(peak.log10(), 60.0)) * 255.0 / 60.0)
                .round()
                .clamp(0.0, 255.0) as u8
        };
    }
    Ok((
        samples,
        Splice {
            beats: record.beats,
            frames,
            tail,
            anchor: record.anchor,
            rate: record.rate as f32,
        },
        bars,
        bins,
    ))
}
