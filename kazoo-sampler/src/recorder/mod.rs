//! Recording takes into the store.
//!
//! A [`Recorder`] comes with a [`RecordTap`]. The tap goes to the audio
//! thread and is offered every stereo block that could be recorded (the
//! mic, the wall's master, a `rec` module's input); the host decides which,
//! and when to call [`Recorder::start`] and [`Recorder::stop`], so it can
//! quantise both to the beat. While a take runs the tap copies its blocks
//! into a lock-free ring, and a writer thread streams them to a hidden
//! temporary WAV in the store, trimming leading silence as it goes. When the
//! take ends the writer normalises it if asked, saves it under its name
//! atomically and sends a [`TakeReport`]: the finished sample, or exactly
//! what went wrong. Frames lost to a full ring are counted and reported with
//! the take, never hidden.

mod tap;
mod writer;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use ringbuf::HeapRb;
use ringbuf::traits::Split;

pub use tap::RecordTap;

use crate::{Error, Overwrite, Result, SampleInfo, SampleName, SampleStore, rate_is_valid};
use tap::{GEN_SHIFT, Shared, WANT};
use writer::{Command, Job, Writer};

/// The quietest trim threshold, in dBFS.
pub const TRIM_MIN_DB: f32 = -96.0;

/// The loudest trim threshold, in dBFS.
pub const TRIM_MAX_DB: f32 = -6.0;

/// Where normalising puts a take's peak, in dBFS: a little headroom so a
/// resampled or interpolated peak stays below full scale.
pub const NORMALISE_PEAK_DB: f32 = -1.0;

/// Audio kept before the point where a trimmed take crosses its threshold,
/// faded in so the attack survives and the cut is silent, in seconds.
pub const PREROLL_SECONDS: f64 = 0.005;

/// The shortest and longest ring the recorder accepts, in seconds.
const RING_SECONDS: std::ops::RangeInclusive<f64> = 0.05..=60.0;

/// A take's number, counting from 1 for each recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TakeId(pub u64);

/// How to record a take.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeRequest {
    /// The sample it becomes.
    pub name: SampleName,
    /// The take stops by itself after this long, in seconds; never longer
    /// than the store's cap.
    pub max_seconds: f64,
    /// Cut the silence before the first frame at or above this level, in
    /// dBFS ([`TRIM_MIN_DB`] to [`TRIM_MAX_DB`]); `None` keeps everything.
    pub trim_db: Option<f32>,
    /// Scale the take so its peak sits at [`NORMALISE_PEAK_DB`].
    pub normalise: bool,
    /// Whether a sample already under this name may be replaced.
    pub overwrite: Overwrite,
}

impl TakeRequest {
    /// A take as long as the store allows, untrimmed, unnormalised, that
    /// never replaces an existing sample.
    #[must_use]
    pub const fn new(name: SampleName) -> Self {
        Self {
            name,
            max_seconds: f64::INFINITY,
            trim_db: None,
            normalise: false,
            overwrite: Overwrite::Refuse,
        }
    }
}

/// A finished take.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// The sample as saved.
    pub info: SampleInfo,
    /// Frames lost because the writer fell behind and the ring filled.
    /// Anything above zero means the take has gaps.
    pub dropped_frames: u64,
    /// Frames of leading silence cut.
    pub trimmed_frames: u64,
    /// Whether the take stopped by reaching its length limit.
    pub hit_limit: bool,
    /// Gain applied by normalising, in dB (0 if not normalised).
    pub gain_db: f32,
}

/// How a take ended.
#[derive(Debug)]
pub struct TakeReport {
    /// Which take.
    pub id: TakeId,
    /// The name it was recorded under.
    pub name: SampleName,
    /// The saved sample, or why there is none.
    pub result: Result<Take>,
}

/// What the recorder is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderStatus {
    /// Ready for a take.
    Idle,
    /// A take is running.
    Recording {
        /// Which take.
        id: TakeId,
        /// Its name.
        name: SampleName,
        /// Seconds recorded so far.
        seconds: f64,
        /// Frames lost to a full ring so far.
        dropped_frames: u64,
    },
    /// The take has stopped and is being written out; its report is next.
    Finishing {
        /// Which take.
        id: TakeId,
        /// Its name.
        name: SampleName,
    },
}

/// The control side of the recorder. Lives off the audio thread.
pub struct Recorder {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    reports: Receiver<TakeReport>,
    thread: Option<JoinHandle<()>>,
    store: SampleStore,
    rate: u32,
    last_take: u64,
    active: Option<(TakeId, SampleName)>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("rate", &self.rate)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Recorder {
    /// A recorder saving into `store` at `rate`, with a ring holding
    /// `ring_seconds` of audio between the tap and the writer (a quarter
    /// to one second is plenty; a larger ring rides out slower disks).
    /// Starts the writer thread; hand the returned tap to the audio thread.
    pub fn new(store: SampleStore, rate: u32, ring_seconds: f64) -> Result<(Self, RecordTap)> {
        if !rate_is_valid(rate) {
            return Err(Error::BadAudio {
                reason: format!("recording rate {rate} Hz is out of range"),
            });
        }
        if !RING_SECONDS.contains(&ring_seconds) {
            return Err(Error::BadAudio {
                reason: format!("a ring of {ring_seconds} s is outside 0.05 to 60 s"),
            });
        }
        let capacity = ((ring_seconds * f64::from(rate)).ceil() as usize).max(64) * 2;
        let (producer, consumer) = HeapRb::<f32>::new(capacity).split();
        let shared = Arc::new(Shared::new());
        let (commands, command_rx) = mpsc::channel();
        let (report_tx, reports) = mpsc::channel();
        let writer = Writer {
            shared: Arc::clone(&shared),
            consumer,
            commands: command_rx,
            reports: report_tx,
            store: store.clone(),
            rate,
            consumed: 0,
        };
        let thread = std::thread::Builder::new()
            .name("kazoo-sampler-writer".to_owned())
            .spawn(move || writer.run())
            .map_err(|e| Error::io("starting the recorder's writer thread", e))?;
        let tap = RecordTap::new(Arc::clone(&shared), producer);
        let recorder = Self {
            shared,
            commands,
            reports,
            thread: Some(thread),
            store,
            rate,
            last_take: 0,
            active: None,
        };
        Ok((recorder, tap))
    }

    /// Start a take. The tap begins with the next block it is offered.
    /// Refused while another take is still running or being finished.
    pub fn start(&mut self, request: TakeRequest) -> Result<TakeId> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        let mut request = request;
        if request.max_seconds.is_nan() || request.max_seconds <= 0.0 {
            return Err(Error::BadAudio {
                reason: format!(
                    "a take's length limit must be positive, not {}",
                    request.max_seconds
                ),
            });
        }
        let max_seconds = request.max_seconds.min(self.store.limits().max_seconds);
        request.max_seconds = max_seconds;
        request.trim_db = request.trim_db.map(|db| {
            if db.is_nan() {
                TRIM_MIN_DB
            } else {
                db.clamp(TRIM_MIN_DB, TRIM_MAX_DB)
            }
        });
        if request.overwrite == Overwrite::Refuse && self.store.path_of(&request.name).exists() {
            return Err(Error::Exists {
                name: request.name.to_string(),
            });
        }
        let max_frames = ((max_seconds * f64::from(self.rate)).floor() as u64).max(1);
        self.last_take += 1;
        let id = TakeId(self.last_take);
        let name = request.name.clone();
        self.commands
            .send(Command::Begin(Job { id, request }))
            .map_err(|_| Error::WriterStopped)?;
        self.shared.max_frames.store(max_frames, Ordering::Release);
        self.shared
            .request
            .store(id.0 << GEN_SHIFT | WANT, Ordering::Release);
        self.active = Some((id, name));
        Ok(id)
    }

    /// Stop the running take. Its report follows from [`Self::poll`] or
    /// [`Self::wait`] once the writer has saved it.
    pub fn stop(&mut self) -> Result<TakeId> {
        let Some((id, _)) = &self.active else {
            return Err(Error::NotRecording);
        };
        self.shared.stop(id.0);
        Ok(*id)
    }

    /// The next finished take's report, if there is one, without waiting.
    pub fn poll(&mut self) -> Option<TakeReport> {
        match self.reports.try_recv() {
            Ok(report) => Some(self.settle(report)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => self.writer_gone(),
        }
    }

    /// The next finished take's report, waiting up to `timeout` for it.
    pub fn wait(&mut self, timeout: Duration) -> Option<TakeReport> {
        match self.reports.recv_timeout(timeout) {
            Ok(report) => Some(self.settle(report)),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => self.writer_gone(),
        }
    }

    /// What the recorder is doing.
    #[must_use]
    pub fn status(&self) -> RecorderStatus {
        let Some((id, name)) = &self.active else {
            return RecorderStatus::Idle;
        };
        let request = self.shared.request.load(Ordering::Acquire);
        let wanted = request >> GEN_SHIFT == id.0 && request & WANT != 0;
        if !wanted || self.shared.is_over(id.0) {
            return RecorderStatus::Finishing {
                id: *id,
                name: name.clone(),
            };
        }
        RecorderStatus::Recording {
            id: *id,
            name: name.clone(),
            seconds: self.shared.frames.load(Ordering::Relaxed) as f64 / f64::from(self.rate),
            dropped_frames: self.shared.dropped.load(Ordering::Relaxed),
        }
    }

    /// The rate takes are recorded at.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// Clear the running take once its report arrives.
    fn settle(&mut self, report: TakeReport) -> TakeReport {
        if self.active.as_ref().is_some_and(|(id, _)| *id == report.id) {
            self.active = None;
        }
        report
    }

    /// The writer thread has ended: report the take it was writing, once.
    fn writer_gone(&mut self) -> Option<TakeReport> {
        let (id, name) = self.active.take()?;
        Some(TakeReport {
            id,
            name,
            result: Err(Error::WriterStopped),
        })
    }
}

impl Drop for Recorder {
    /// Stop any take, let the writer save it, and wait for the thread.
    fn drop(&mut self) {
        if let Some((id, _)) = &self.active {
            self.shared.stop(id.0);
        }
        let told = self.commands.send(Command::Shutdown).is_ok();
        let Some(thread) = self.thread.take() else {
            return;
        };
        match thread.join() {
            Ok(()) if told => {}
            Ok(()) => eprintln!("kazoo-sampler: the recorder's writer thread had already stopped"),
            Err(_) => eprintln!("kazoo-sampler: the recorder's writer thread panicked"),
        }
    }
}

#[cfg(test)]
mod tests;
