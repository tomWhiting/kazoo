//! Recording what the wall plays.
//!
//! While [`Shared::recording`] is set, the engine copies its master, stereo
//! interleaved, into its record ring (see [`crate::engine`]): the master
//! before the monitor, so a recording hears the wall even while it is
//! silent. A [`Recorder`] belongs to the control thread. It holds the
//! ring's reading end between recordings; for each recording it opens a new
//! file and starts a writer thread, which drains the ring every
//! [`DRAIN_EVERY`] into a 32-bit float stereo WAV at the engine's rate.
//!
//! Files are named for the local time they start,
//! `wall-2026-09-27-203001.wav`, in `~/Music/kazoo-wall/` (see
//! [`crate::paths::recordings_dir`]); a name already taken gets `-2`, `-3`
//! and so on, and no file is ever overwritten. The header is brought up to
//! date every [`HEADER_EVERY`], so a daemon that is killed outright leaves
//! a file that plays to within a second of the end. A file that reaches
//! [`MAX_FILE_FRAMES`] (the WAV format's 4 GiB: 2.9 hours at 48 kHz) ends
//! there, and the writer leaves the ring filling so the recording carries on
//! into a new file without a gap.
//!
//! Nothing here runs on the audio thread.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ringbuf::HeapCons;
use ringbuf::traits::{Consumer, Observer};

use crate::SUB_BLOCK;
use crate::engine::{EngineControl, Shared};

/// How often the writer drains the ring.
pub const DRAIN_EVERY: Duration = Duration::from_millis(50);

/// How often the writer brings the file's header up to date.
pub const HEADER_EVERY: Duration = Duration::from_secs(1);

/// Longest the writer waits, once told to stop, for a sub-block the engine
/// had under way to reach the ring. A running engine takes a millisecond
/// or two; one that has gone never does.
const SETTLE_LONGEST: Duration = Duration::from_millis(100);

/// Samples the writer takes from the ring at a time: whole stereo frames.
const DRAIN_CHUNK: usize = 8_192;

/// Most frames one file holds: 4 000 000 000 bytes of stereo f32, inside
/// the WAV format's 32-bit sizes with room for the header.
pub const MAX_FILE_FRAMES: u64 = 500_000_000;

/// Files that may share one start time's name before the wall gives up.
const NAME_TRIES: u32 = 100;

/// The writer's file.
type Wav = hound::WavWriter<BufWriter<File>>;

/// How a recording ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// It was asked to stop.
    Asked,
    /// The file reached [`MAX_FILE_FRAMES`]. The engine is still recording
    /// into the ring, for the next file to carry on from.
    Full,
    /// Writing failed; the words say how. The engine has stopped recording.
    Failed(String),
}

/// A recording that has ended, and its file finished.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    /// The file.
    pub path: PathBuf,
    /// Who started it.
    pub seat: String,
    /// How long it is, in seconds.
    pub seconds: f64,
    /// Samples lost because the writer fell behind.
    pub dropped: u64,
    /// The file's frames per second.
    pub sample_rate: u32,
    /// How it ended.
    pub ending: Ending,
}

/// A recording under way.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// The file.
    pub path: PathBuf,
    /// Who started it.
    pub seat: String,
    /// How much is written so far, in seconds.
    pub seconds: f64,
    /// Samples lost so far because the writer fell behind.
    pub dropped: u64,
    /// The file's frames per second.
    pub sample_rate: u32,
}

/// One recording's writer.
#[derive(Debug)]
struct Session {
    path: PathBuf,
    seat: String,
    sample_rate: u32,
    /// The engine's dropped count when the recording started.
    dropped_before: u64,
    /// Frames written so far.
    written: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    join: JoinHandle<Ending>,
}

/// Records the wall: holds the engine's record ring and the recording under
/// way, if there is one.
pub struct Recorder {
    dir: PathBuf,
    /// The record ring's reading end: here between recordings, locked by
    /// the writer for the whole of each. `None` if the engine gave none
    /// (it had been taken already).
    ring: Option<Arc<Mutex<HeapCons<f32>>>>,
    shared: Arc<Shared>,
    sample_rate: u32,
    limit_frames: u64,
    session: Option<Session>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("dir", &self.dir)
            .field("has_ring", &self.ring.is_some())
            .field("sample_rate", &self.sample_rate)
            .field("limit_frames", &self.limit_frames)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Recorder {
    /// A recorder for `engine`, writing into `dir`.
    #[must_use]
    pub fn new(dir: PathBuf, engine: &mut EngineControl) -> Self {
        Self {
            dir,
            ring: engine.take_record().map(|ring| Arc::new(Mutex::new(ring))),
            shared: Arc::clone(engine.shared()),
            sample_rate: engine.sample_rate(),
            limit_frames: MAX_FILE_FRAMES,
            session: None,
        }
    }

    /// Where the files go.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Put the files in `dir` from the next recording on.
    pub fn set_dir(&mut self, dir: PathBuf) {
        self.dir = dir;
    }

    /// End each file at `frames` rather than [`MAX_FILE_FRAMES`] (at least
    /// one sub-block), from the next recording on.
    pub fn set_limit_frames(&mut self, frames: u64) {
        // SUB_BLOCK is 32: lossless.
        self.limit_frames = frames.clamp(SUB_BLOCK as u64, MAX_FILE_FRAMES);
    }

    /// The recording under way, if there is one.
    #[must_use]
    pub fn status(&self) -> Option<Status> {
        self.session.as_ref().map(|session| Status {
            path: session.path.clone(),
            seat: session.seat.clone(),
            seconds: seconds(session.written.load(Ordering::Relaxed), session.sample_rate),
            dropped: self
                .shared
                .record_dropped()
                .saturating_sub(session.dropped_before),
            sample_rate: session.sample_rate,
        })
    }

    /// Start recording for `seat` into a new file, and return it. `fresh`
    /// empties the ring first (a new recording); otherwise what the engine
    /// put in it since the last file ended goes first, so a recording
    /// carries on without a gap.
    ///
    /// # Errors
    ///
    /// Fails, recording nothing and leaving no file, if a recording is
    /// under way, the engine gave no ring, the directory or the file cannot
    /// be made, or the writer thread cannot start.
    pub fn start(&mut self, seat: &str, fresh: bool) -> Result<PathBuf, String> {
        if let Some(session) = &self.session {
            return Err(format!("already recording {}", session.path.display()));
        }
        let Some(ring) = self.ring.clone() else {
            self.shared.set_recording(false);
            return Err("the engine has no record ring to read".to_string());
        };
        let (path, file) = match self.open() {
            Ok(opened) => opened,
            Err(why) => {
                self.shared.set_recording(false);
                return Err(why);
            }
        };
        let disk = match file.try_clone() {
            Ok(disk) => disk,
            Err(err) => {
                self.shared.set_recording(false);
                return Err(discard(
                    &path,
                    format!("{} cannot be held open: {err}", path.display()),
                ));
            }
        };
        let wav = match wav(&path, file, self.sample_rate) {
            Ok(wav) => wav,
            Err(why) => {
                self.shared.set_recording(false);
                return Err(discard(&path, why));
            }
        };
        if fresh {
            ring.lock().unwrap_or_else(PoisonError::into_inner).clear();
        }
        let dropped_before = self.shared.record_dropped();
        let written = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let job = Job {
            ring,
            wav,
            disk,
            shared: Arc::clone(&self.shared),
            stop: Arc::clone(&stop),
            written: Arc::clone(&written),
            limit: self.limit_frames,
        };
        self.shared.set_recording(true);
        let join = match thread::Builder::new()
            .name("kazoo-wall-record".to_string())
            .spawn(move || job.run())
        {
            Ok(join) => join,
            Err(err) => {
                self.shared.set_recording(false);
                return Err(discard(
                    &path,
                    format!("the recording's writer could not start: {err}"),
                ));
            }
        };
        self.session = Some(Session {
            path: path.clone(),
            seat: seat.to_string(),
            sample_rate: self.sample_rate,
            dropped_before,
            written,
            stop,
            join,
        });
        Ok(path)
    }

    /// Stop the recording under way and finish its file; `None` when there
    /// is none. Returns once the file is complete on disk.
    pub fn stop(&mut self) -> Option<Finished> {
        let session = self.session.take()?;
        session.stop.store(true, Ordering::Release);
        session.join.thread().unpark();
        let mut finished = self.finish(session);
        // Asked to stop just as the file filled: it stops all the same.
        self.shared.set_recording(false);
        if finished.ending == Ending::Full {
            finished.ending = Ending::Asked;
        }
        Some(finished)
    }

    /// The recording, if it ended on its own (its file filled, or writing
    /// failed) since the last call, with its file finished.
    pub fn poll(&mut self) -> Option<Finished> {
        if !self.session.as_ref()?.join.is_finished() {
            return None;
        }
        let session = self.session.take()?;
        Some(self.finish(session))
    }

    /// Move to a rebuilt engine: stop the recording under way on the old
    /// one, finishing its file (the new engine's ring starts empty and its
    /// rate may differ, so no file spans the two), and take the new
    /// engine's ring. Returns the recording it stopped, to carry on.
    pub fn attach(&mut self, engine: &mut EngineControl) -> Option<Finished> {
        let finished = self.stop();
        self.ring = engine.take_record().map(|ring| Arc::new(Mutex::new(ring)));
        self.shared = Arc::clone(engine.shared());
        self.sample_rate = engine.sample_rate();
        finished
    }

    /// The engine's frames per second.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Wait for `session`'s writer and say how it went.
    fn finish(&self, session: Session) -> Finished {
        let ending = session.join.join().unwrap_or_else(|_| {
            self.shared.set_recording(false);
            Ending::Failed(format!(
                "the recording's writer panicked; {} may be short",
                session.path.display()
            ))
        });
        Finished {
            seconds: seconds(session.written.load(Ordering::Relaxed), session.sample_rate),
            dropped: self
                .shared
                .record_dropped()
                .saturating_sub(session.dropped_before),
            path: session.path,
            seat: session.seat,
            sample_rate: session.sample_rate,
            ending,
        }
    }

    /// Make the directory and a new file in it, named for now.
    fn open(&self) -> Result<(PathBuf, File), String> {
        fs::create_dir_all(&self.dir).map_err(|err| {
            format!(
                "the recordings directory {} cannot be made: {err}",
                self.dir.display()
            )
        })?;
        let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string();
        new_file(&self.dir, &stamp)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(finished) = self.stop() {
            eprintln!(
                "kazoo-wall: the recording {} was finished as the wall let it go",
                finished.path.display()
            );
            if let Ending::Failed(why) = finished.ending {
                eprintln!("kazoo-wall: {why}");
            }
        }
    }
}

/// Frames at `rate`, in seconds.
fn seconds(frames: u64, rate: u32) -> f64 {
    // Frame counts are far below 2^53: exact.
    frames as f64 / f64::from(rate.max(1))
}

/// A new file in `dir` named `wall-<stamp>.wav`, or `wall-<stamp>-2.wav`
/// and on if that is taken: never one that is there already.
///
/// # Errors
///
/// Fails if the file cannot be made, or every name is taken.
pub fn new_file(dir: &Path, stamp: &str) -> Result<(PathBuf, File), String> {
    for attempt in 1..=NAME_TRIES {
        let name = if attempt == 1 {
            format!("wall-{stamp}.wav")
        } else {
            format!("wall-{stamp}-{attempt}.wav")
        };
        let path = dir.join(name);
        let opened = OpenOptions::new().write(true).create_new(true).open(&path);
        let taken = matches!(&opened, Err(err) if err.kind() == io::ErrorKind::AlreadyExists);
        if !taken {
            return opened
                .map_err(|err| format!("{} cannot be made: {err}", path.display()))
                .map(|file| (path, file));
        }
    }
    Err(format!(
        "{NAME_TRIES} recordings named wall-{stamp} are in {} already",
        dir.display()
    ))
}

/// A 32-bit float stereo WAV at `rate` on `file` (just made at `path`).
///
/// # Errors
///
/// Fails if the header cannot be written.
fn wav(path: &Path, file: File, rate: u32) -> Result<Wav, String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    hound::WavWriter::new(BufWriter::new(file), spec)
        .map_err(|err| format!("{} cannot be written: {err}", path.display()))
}

/// Remove the file at `path`, made for a recording that could not start,
/// and return `why` with what became of it.
fn discard(path: &Path, why: String) -> String {
    match fs::remove_file(path) {
        Ok(()) => why,
        Err(err) => format!(
            "{why}; and the empty {} could not be removed: {err}",
            path.display()
        ),
    }
}

/// Everything a writer thread needs.
struct Job {
    ring: Arc<Mutex<HeapCons<f32>>>,
    wav: Wav,
    /// The file under `wav`, to make sure it is on the disk at the end.
    disk: File,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    written: Arc<AtomicU64>,
    limit: u64,
}

/// What one drain of the ring came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drained {
    /// Everything that was there is written.
    Caught,
    /// The file is full; what is left stays in the ring.
    Full,
}

impl Job {
    /// Drain the ring into the file until told to stop, the file fills or
    /// writing fails; then finish the file and see it onto the disk.
    fn run(self) -> Ending {
        let Self {
            ring,
            mut wav,
            disk,
            shared,
            stop,
            written,
            limit,
        } = self;
        let mut ring = ring.lock().unwrap_or_else(PoisonError::into_inner);
        let mut buffer = vec![0.0_f32; DRAIN_CHUNK];
        let mut header_at = Instant::now();
        let ending = loop {
            let asked = stop.load(Ordering::Acquire);
            if asked {
                shared.set_recording(false);
                settle(&shared);
            }
            match drain(&mut ring, &mut wav, &mut buffer, &written, limit) {
                Ok(Drained::Full) => break Ending::Full,
                Ok(Drained::Caught) if asked => break Ending::Asked,
                Ok(Drained::Caught) => {}
                Err(why) => break Ending::Failed(why),
            }
            if header_at.elapsed() >= HEADER_EVERY {
                if let Err(err) = wav.flush() {
                    break Ending::Failed(format!("the recording could not be written: {err}"));
                }
                header_at = Instant::now();
            }
            thread::park_timeout(DRAIN_EVERY);
        };
        if matches!(ending, Ending::Failed(_)) {
            shared.set_recording(false);
        }
        let finished = wav
            .finalize()
            .map_err(|err| format!("the recording could not be finished: {err}"))
            .and_then(|()| {
                disk.sync_all()
                    .map_err(|err| format!("the recording could not be saved to the disk: {err}"))
            });
        match (ending, finished) {
            (ending, Ok(())) => ending,
            (Ending::Failed(why), Err(also)) => Ending::Failed(format!("{why}; {also}")),
            (Ending::Asked | Ending::Full, Err(why)) => {
                shared.set_recording(false);
                Ending::Failed(why)
            }
        }
    }
}

/// Wait, at most [`SETTLE_LONGEST`], until the engine has rendered two
/// sub-blocks since recording was cleared: the one under way then may have
/// pushed, and the next saw the flag down.
fn settle(shared: &Shared) {
    let from = shared.frames();
    let until = Instant::now() + SETTLE_LONGEST;
    // SUB_BLOCK is 32: lossless.
    while shared.frames().wrapping_sub(from) < 2 * SUB_BLOCK as u64 && Instant::now() < until {
        thread::sleep(Duration::from_millis(1));
    }
}

/// Write what is in the ring now to `wav`, in whole frames, up to `limit`
/// frames in the file.
///
/// # Errors
///
/// Fails if a sample cannot be written.
fn drain(
    ring: &mut HeapCons<f32>,
    wav: &mut Wav,
    buffer: &mut [f32],
    written: &AtomicU64,
    limit: u64,
) -> Result<Drained, String> {
    // Only what is there now: an engine outpacing the disk cannot keep the
    // writer here for ever.
    let mut left = ring.occupied_len() / 2 * 2;
    while left > 0 {
        let room = limit.saturating_sub(written.load(Ordering::Relaxed));
        if room == 0 {
            return Ok(Drained::Full);
        }
        let room = usize::try_from(room.saturating_mul(2)).unwrap_or(usize::MAX);
        let take = left.min(buffer.len() / 2 * 2).min(room);
        let got = ring.pop_slice(&mut buffer[..take]) / 2 * 2;
        if got == 0 {
            break;
        }
        for sample in &buffer[..got] {
            wav.write_sample(kazoo_core::sanitize_sample(*sample))
                .map_err(|err| format!("the recording could not be written: {err}"))?;
        }
        // At most DRAIN_CHUNK / 2: lossless.
        written.fetch_add((got / 2) as u64, Ordering::Relaxed);
        left -= got;
    }
    if written.load(Ordering::Relaxed) >= limit {
        Ok(Drained::Full)
    } else {
        Ok(Drained::Caught)
    }
}

#[cfg(test)]
mod tests;
