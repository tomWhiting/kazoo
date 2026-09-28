//! The recorder's writer thread: drains the ring to a temporary WAV, trims
//! leading silence as the audio arrives, normalises when the take ends and
//! saves the result into the store.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use hound::WavReader;
use kazoo_fx::dsp::{db_to_gain, gain_to_db};
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use super::tap::Shared;
use super::{NORMALISE_PEAK_DB, PREROLL_SECONDS, Take, TakeId, TakeReport, TakeRequest};
use crate::store::wav::TempWav;
use crate::{Error, Result, SampleStore};

/// Interleaved samples read from the ring at a time.
const CHUNK: usize = 8_192;

/// How long the writer sleeps when there is nothing to do.
const IDLE: Duration = Duration::from_millis(2);

/// How long a stopped take waits for the tap to confirm. A tap that has not
/// run for this long belongs to an audio thread that has stopped; the take
/// is finished with what arrived.
const STOP_GRACE: Duration = Duration::from_millis(250);

/// Orders for the writer thread.
#[derive(Debug)]
pub(crate) enum Command {
    /// A take has been requested.
    Begin(Job),
    /// Finish whatever is running and exit.
    Shutdown,
}

/// One take to write.
#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) id: TakeId,
    pub(crate) request: TakeRequest,
}

/// Everything the thread owns.
pub(crate) struct Writer {
    pub(crate) shared: Arc<Shared>,
    pub(crate) consumer: HeapCons<f32>,
    pub(crate) commands: Receiver<Command>,
    pub(crate) reports: Sender<TakeReport>,
    pub(crate) store: SampleStore,
    pub(crate) rate: u32,
    /// Samples popped from the ring over every take.
    pub(crate) consumed: u64,
}

impl Writer {
    /// The thread's body: runs until told to shut down or until the
    /// control side is gone, finishing any take first.
    pub(crate) fn run(mut self) {
        let mut chunk = vec![0.0f32; CHUNK];
        let mut shutting_down = false;
        loop {
            let command = if shutting_down {
                None
            } else {
                match self.commands.recv_timeout(IDLE) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => Some(Command::Shutdown),
                }
            };
            match command {
                Some(Command::Begin(job)) => self.record(job, &mut chunk, &mut shutting_down),
                Some(Command::Shutdown) => shutting_down = true,
                None => {}
            }
            // The ring is empty between takes: the tap only pushes into a
            // take it claimed, and `record` drains every claimed take to
            // the end before reporting it.
            if shutting_down {
                return;
            }
        }
    }

    /// Write one take from start to finish and report it.
    fn record(&mut self, job: Job, chunk: &mut [f32], shutting_down: &mut bool) {
        let take = job.id.0;
        let mut sink = Sink::open(&self.store, &job.request, self.rate);
        let mut stop_seen: Option<Instant> = None;
        loop {
            let over = self.shared.is_over(take)
                || stop_seen.is_some_and(|since| since.elapsed() > STOP_GRACE);
            if stop_seen.is_none() && self.shared.stop_requested(take) {
                stop_seen = Some(Instant::now());
            }
            // Everything the tap pushed before it said the take was over is
            // visible now; drain it all, skipping anything older than the
            // take's start.
            let moved = match self.shared.start_of(take) {
                Some(start) => self.drain(start, chunk, &mut sink, take),
                None => 0,
            };
            if over {
                break;
            }
            if moved == 0 {
                let closing = match self.commands.recv_timeout(IDLE) {
                    Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => true,
                    Ok(Command::Begin(other)) => {
                        self.refuse(other);
                        false
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                };
                if closing {
                    *shutting_down = true;
                    self.shared.stop(take);
                }
            }
        }
        let dropped = self
            .shared
            .dropped
            .load(std::sync::atomic::Ordering::Relaxed);
        let hit_limit = self
            .shared
            .limited
            .load(std::sync::atomic::Ordering::Acquire)
            == take;
        let result = sink
            .finish(&self.store, &job.request, self.rate)
            .map(|mut done| {
                done.dropped_frames = dropped;
                done.hit_limit = hit_limit;
                done
            });
        self.report(TakeReport {
            id: job.id,
            name: job.request.name,
            result,
        });
    }

    /// Pop everything in the ring into `sink`, discarding samples pushed
    /// before `start` (left by a take that ended before its tap stopped).
    /// Returns the samples popped.
    fn drain(&mut self, start: u64, chunk: &mut [f32], sink: &mut Sink, take: u64) -> usize {
        let mut moved = 0;
        loop {
            let stale = usize::try_from(start.saturating_sub(self.consumed)).unwrap_or(usize::MAX);
            let want = if stale > 0 {
                stale.min(chunk.len())
            } else {
                chunk.len()
            };
            let count = self.consumer.pop_slice(&mut chunk[..want]);
            if count == 0 {
                return moved;
            }
            moved += count;
            self.consumed += count as u64;
            if stale > 0 {
                continue;
            }
            sink.feed(&chunk[..count]);
            if sink.failed() {
                // Tell the tap to stop feeding a take that cannot be saved;
                // keep draining until it has.
                self.shared.stop(take);
            }
        }
    }

    /// A take asked for while another runs (the control side never does
    /// this, but the answer is still a report, never silence).
    fn refuse(&self, job: Job) {
        self.report(TakeReport {
            id: job.id,
            name: job.request.name,
            result: Err(Error::Busy),
        });
    }

    fn report(&self, report: TakeReport) {
        if let Err(unsent) = self.reports.send(report) {
            // The control side is gone; say what became of the take.
            let report = unsent.0;
            match report.result {
                Ok(take) => eprintln!(
                    "kazoo-sampler: take '{}' saved after its recorder closed ({} frames)",
                    report.name, take.info.frames
                ),
                Err(e) => eprintln!(
                    "kazoo-sampler: take '{}' failed after its recorder closed: {e}",
                    report.name
                ),
            }
        }
    }
}

/// Where a take's audio goes as it arrives.
struct Sink {
    temp: Result<TempWav>,
    /// Frames still waiting for the signal to cross the trim threshold,
    /// kept so the attack is not cut: the pre-roll.
    waiting: VecDeque<(f32, f32)>,
    /// The threshold as a linear level, while still waiting for it.
    threshold: Option<f32>,
    preroll: usize,
    received: u64,
    trimmed: u64,
    peak: f32,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Sink {
    fn open(store: &SampleStore, request: &TakeRequest, rate: u32) -> Self {
        let preroll = (PREROLL_SECONDS * f64::from(rate)).round() as usize;
        Self {
            temp: store
                .refuse_non_file(&request.name, &store.path_of(&request.name))
                .and_then(|()| TempWav::create(store.dir(), &request.name, rate)),
            waiting: VecDeque::with_capacity(preroll + 1),
            threshold: request.trim_db.map(db_to_gain),
            preroll,
            received: 0,
            trimmed: 0,
            peak: 0.0,
            left: Vec::with_capacity(CHUNK / 2),
            right: Vec::with_capacity(CHUNK / 2),
        }
    }

    const fn failed(&self) -> bool {
        self.temp.is_err()
    }

    /// Take interleaved stereo samples from the ring.
    fn feed(&mut self, interleaved: &[f32]) {
        self.received += (interleaved.len() / 2) as u64;
        self.left.clear();
        self.right.clear();
        for pair in interleaved.chunks_exact(2) {
            self.push_frame(pair[0], pair[1]);
        }
        let Ok(temp) = self.temp.as_mut() else {
            return;
        };
        if let Err(e) = temp.write(&self.left, &self.right) {
            self.temp = Err(e);
        }
    }

    /// One frame through the trim gate into the block to write.
    fn push_frame(&mut self, l: f32, r: f32) {
        let Some(threshold) = self.threshold else {
            self.keep(l, r);
            return;
        };
        if l.abs().max(r.abs()) < threshold {
            if self.waiting.len() == self.preroll {
                self.waiting.pop_front();
                self.trimmed += 1;
            }
            if self.preroll > 0 {
                self.waiting.push_back((l, r));
            } else {
                self.trimmed += 1;
            }
            return;
        }
        // The signal has arrived: fade the pre-roll in so the cut is
        // silent, then let everything through.
        self.threshold = None;
        let count = self.waiting.len();
        let held: Vec<(f32, f32)> = self.waiting.drain(..).collect();
        for (i, (wl, wr)) in held.into_iter().enumerate() {
            let gain = i as f32 / count as f32;
            self.keep(wl * gain, wr * gain);
        }
        self.keep(l, r);
    }

    fn keep(&mut self, l: f32, r: f32) {
        self.peak = self.peak.max(l.abs()).max(r.abs());
        self.left.push(l);
        self.right.push(r);
    }

    /// Normalise if asked, then save into the store.
    fn finish(self, store: &SampleStore, request: &TakeRequest, rate: u32) -> Result<Take> {
        let mut temp = self.temp?;
        let name = &request.name;
        if self.received == 0 {
            return Err(Error::EmptyTake {
                name: name.to_string(),
            });
        }
        if let (Some(threshold_db), Some(_)) = (request.trim_db, self.threshold) {
            return Err(Error::SilentTake {
                name: name.to_string(),
                threshold_db,
            });
        }
        let mut gain_db = 0.0;
        if request.normalise && self.peak > 0.0 {
            let gain = db_to_gain(NORMALISE_PEAK_DB) / self.peak;
            gain_db = gain_to_db(gain);
            temp = rescale(temp, store, request, rate, gain)?;
        }
        temp.commit(&store.path_of(name), name, request.overwrite)?;
        let info = store.info(name)?;
        Ok(Take {
            info,
            dropped_frames: 0,
            trimmed_frames: self.trimmed,
            hit_limit: false,
            gain_db,
        })
    }
}

/// A copy of the finished temporary WAV with every sample times `gain`.
fn rescale(
    mut temp: TempWav,
    store: &SampleStore,
    request: &TakeRequest,
    rate: u32,
    gain: f32,
) -> Result<TempWav> {
    temp.finish()?;
    let context = |e: hound::Error| match e {
        hound::Error::IoError(e) => Error::io(format!("normalising '{}'", request.name), e),
        other => Error::BadAudio {
            reason: format!("normalising '{}': {other}", request.name),
        },
    };
    let mut reader = WavReader::open(temp.path()).map_err(context)?;
    let mut scaled = TempWav::create(store.dir(), &request.name, rate)?;
    let mut left = Vec::with_capacity(CHUNK);
    let mut right = Vec::with_capacity(CHUNK);
    let mut samples = reader.samples::<f32>();
    loop {
        left.clear();
        right.clear();
        while left.len() < CHUNK {
            let (Some(l), Some(r)) = (samples.next(), samples.next()) else {
                break;
            };
            left.push(l.map_err(context)? * gain);
            right.push(r.map_err(context)? * gain);
        }
        if left.is_empty() {
            break;
        }
        scaled.write(&left, &right)?;
    }
    Ok(scaled)
}
