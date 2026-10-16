//! The wall's loops on the control side: each loop's feed, and the tape
//! thread that moves reels between the store and the loops.
//!
//! Every disk read and every whole-tape allocation happens on the tape
//! thread (`worker`), never on the control thread or the render thread.
//! A loop's tape is allocated there or in `LoopModule::new` (built on the
//! control side), and freed here in [`Loops::collect`] (what an install
//! hands back) or in the engine's retire drain (a module taken out).
//!
//! Finished recordings and long overdubs are preserved as durable takes.
pub mod pins;
pub mod takes;
pub use pins::SAVE_WAIT;
pub use takes::CHECKPOINT_SECONDS;
pub mod meet;
#[cfg(test)]
pub(crate) mod tests;
mod worker;

use crate::dsp::looper::{self, LoopFeed, LoopGauges, LoopIn, LoopOut, LoopTape, MAX_LOOP_BARS};
use crate::patch::TapeRecord;
use crate::protocol::{Change, LoopState, LoopView, What};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{TryRecvError, TrySendError};
use worker::{TapeDone, TapeJob, TapeWorker};

/// A wall holds eight loops, like the eight-track: eight full loops are
/// 295 MB at 48 kHz, the most the wall lets itself take for tape.
pub const MAX_LOOPS: usize = 8;

/// Notes kept for the wall to show; older ones are on the console log.
const MAX_NOTES: usize = 64;

const fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn belongs(note: &str, module: &str) -> bool {
    note.strip_prefix(module)
        .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with("'s "))
}

/// One loop's tape on its way: a job not yet queued, or a tape not yet
/// taken by the loop's feed.
#[derive(Debug)]
struct Waiting {
    ticket: u64,
    job: Option<Box<TapeJob>>,
    install: Option<LoopIn>,
    only_if_blank: bool,
    /// The first eight digits of the reel being fetched, for notes; none
    /// for a blank tape.
    reel: Option<String>,
    full_noted: bool,
}

/// The tape thread: not started until a loop first needs tape, then
/// running, or gone (it could not start, or stopped).
#[derive(Debug)]
enum Thread {
    NotStarted,
    Running(TapeWorker),
    Gone,
}

/// Control-side handles and outstanding tape work.
#[derive(Debug)]
pub struct Loops {
    feeds: HashMap<String, LoopFeed>,
    worker: Thread,
    waiting: HashMap<String, Waiting>,
    notes: VecDeque<String>,
    unseen: RefCell<VecDeque<String>>,
    newest: RefCell<HashMap<String, (u64, String)>>,
    dir: Result<PathBuf, String>,
    tickets: u64,
    /// Each attached loop's `(capacity, tail)` in frames.
    geometry: HashMap<String, (usize, usize)>,
    takes: u64,
    pub recordings: takes::Takes,
    pub checkpoints_paused: bool,
    #[cfg(test)]
    test_directory: Option<tempfile::TempDir>,
}

impl Default for Loops {
    fn default() -> Self {
        Self::new()
    }
}

impl Loops {
    /// The wall's loops, with the reel store at `$KAZOO_REELS_DIR` or
    /// `~/.kazoo/reels`; the tape thread opens it for the first pin or tape job.
    #[must_use]
    pub fn new() -> Self {
        #[cfg(test)]
        {
            let dir = tests::reels_dir();
            let mut loops = Self::with_reels_dir(dir.path().to_path_buf());
            loops.test_directory = Some(dir);
            loops
        }
        #[cfg(not(test))]
        {
            Self::with_directory(kazoo_core::reels::reels_dir().map_err(|error| error.to_string()))
        }
    }

    /// The wall's loops with the reel store at `dir`: for tests, which
    /// cannot set the environment (`set_var` is unsafe, and unsafe code is
    /// forbidden).
    #[must_use]
    pub fn with_reels_dir(dir: PathBuf) -> Self {
        Self::with_directory(Ok(dir))
    }

    fn with_directory(dir: Result<PathBuf, String>) -> Self {
        Self {
            feeds: HashMap::new(),
            worker: Thread::NotStarted,
            waiting: HashMap::new(),
            notes: VecDeque::with_capacity(MAX_NOTES),
            unseen: RefCell::new(VecDeque::with_capacity(MAX_NOTES)),
            newest: RefCell::new(HashMap::new()),
            dir,
            tickets: 0,
            geometry: HashMap::new(),
            takes: 0,
            recordings: takes::Takes::default(),
            checkpoints_paused: false,
            #[cfg(test)]
            test_directory: None,
        }
    }

    /// Keep a control-side failure visible to the console and look.
    pub fn report(&mut self, text: String) {
        self.note(text);
    }

    fn note(&mut self, text: String) {
        eprintln!("kazoo-wall: {text}");
        let unseen = self.unseen.get_mut();
        if unseen.len() == MAX_NOTES {
            unseen.pop_front();
        }
        unseen.push_back(text.clone());
        if self.notes.len() == MAX_NOTES {
            self.notes.pop_front();
        }
        self.notes.push_back(text);
    }

    /// What went wrong with the loops' tape lately, oldest first.
    pub fn notes(&self) -> impl Iterator<Item = &str> {
        self.notes.iter().map(String::as_str)
    }

    /// Module notes travel with their tape view, leaving general failures here.
    pub fn unassigned_notes<'a>(
        &'a self,
        patch: &'a crate::patch::Patch,
    ) -> impl Iterator<Item = &'a str> {
        self.notes().filter(move |note| {
            !patch.modules().iter().any(|module| {
                module.kind == crate::catalogue::Kind::LOOP && belongs(note, &module.id)
            })
        })
    }

    /// Punches finished since the wall started.
    #[must_use]
    pub const fn takes(&self) -> u64 {
        self.takes
    }

    /// What one loop rendered most recently.
    #[must_use]
    pub fn gauges(&self, module: &str) -> Option<&LoopGauges> {
        self.feeds.get(module).map(LoopFeed::gauges)
    }

    /// Recover take numbers from the durable log, including after a restart.
    pub fn observe_takes<'a>(
        &self,
        changes: impl Iterator<Item = &'a Change>,
        patch: &crate::patch::Patch,
    ) {
        let mut newest = self.newest.borrow_mut();
        newest.retain(|id, _| {
            patch
                .module(id)
                .is_some_and(|module| module.kind == crate::catalogue::Kind::LOOP)
        });
        for change in changes {
            if let What::Take {
                module,
                tape: Some(tape),
                ..
            } = &change.what
            {
                if patch
                    .module(module)
                    .is_some_and(|module| module.kind == crate::catalogue::Kind::LOOP)
                {
                    newest.insert(module.clone(), (change.seq, tape.reel.clone()));
                }
            }
        }
    }

    /// Read tape atomics and saved identity on the control side. The loop
    /// keeps its anchor in beats from the wall's origin (the bar the wall
    /// now playing landed on, `origin`); the view gives it on the song, as
    /// the snapshot gives the beat.
    #[must_use]
    pub fn view(&self, id: &str, patch: &crate::patch::Patch, origin: f64) -> Option<LoopView> {
        if patch.module(id)?.kind != crate::catalogue::Kind::LOOP {
            return None;
        }
        let record = patch.loop_record(id);
        let tape = record.and_then(|record| record.tape.as_ref());
        let gauges = self.gauges(id);
        let mut view = LoopView {
            from: record.and_then(|record| record.from.clone()),
            reel: tape.map(|tape| tape.reel.chars().take(8).collect()),
            take: self
                .newest
                .borrow()
                .get(id)
                .and_then(|(seq, reel)| tape.filter(|tape| tape.reel == *reel).map(|_| *seq)),
            ..LoopView::default()
        };
        if let Some(gauges) = gauges {
            view.state = match gauges.state.load(Ordering::Relaxed) {
                value if value == looper::LoopState::Armed as u8 => LoopState::Armed,
                value if value == looper::LoopState::Rolling as u8 => LoopState::Rolling,
                value if value == looper::LoopState::PostRoll as u8 => LoopState::PostRoll,
                value if value == looper::LoopState::Holding as u8 => LoopState::Holding,
                value if value == looper::LoopState::NoTape as u8 => LoopState::NoTape,
                _ => LoopState::Blank,
            };
            view.frames = gauges.frames.load(Ordering::Relaxed);
            view.beats = finite(f64::from_bits(gauges.beats.load(Ordering::Relaxed)));
            view.anchor = finite(f64::from_bits(gauges.anchor.load(Ordering::Relaxed)) + origin);
            view.rate = f32::from_bits(gauges.rate.load(Ordering::Relaxed)) as u32;
            view.pass = gauges.pass.load(Ordering::Relaxed);
            let bars = (view.beats / 4.0).ceil().clamp(0.0, MAX_LOOP_BARS as f64) as usize;
            for gauge in gauges.bar_pass.iter().take(bars) {
                let written = gauge.load(Ordering::Relaxed);
                view.written.push(written != i64::MIN);
                let age = if written == i64::MIN {
                    0
                } else {
                    view.pass.saturating_sub(written).max(0)
                };
                view.ages.push(u32::try_from(age).unwrap_or(u32::MAX));
            }
            view.bins = gauges
                .bins
                .iter()
                .map(|bin| bin.load(Ordering::Relaxed))
                .collect();
            let left = f32::from_bits(gauges.peak_left.swap(0, Ordering::Relaxed));
            let right = f32::from_bits(gauges.peak_right.swap(0, Ordering::Relaxed));
            let peak = finite(f64::from(left)).max(finite(f64::from(right)));
            view.peak_db = if peak > 0.0 {
                (20.0 * peak.log10()).max(-120.0)
            } else {
                -120.0
            };
            view.shortened = gauges.shortened.load(Ordering::Relaxed);
        }
        if self
            .waiting
            .get(id)
            .is_some_and(|waiting| waiting.reel.is_some())
        {
            view.state = LoopState::Loading;
        }
        self.unseen.borrow_mut().retain(|note| {
            if belongs(note, id) {
                view.notes.push(note.clone());
                false
            } else {
                true
            }
        });
        Some(view)
    }

    /// Attach a freshly built loop's feed; the loop was built at `rate`.
    /// Anything still on its way to a loop of this id is dropped.
    pub fn attach(&mut self, module: &str, feed: LoopFeed, rate: f32) {
        self.forget(module);
        self.geometry
            .insert(module.to_string(), looper::geometry(rate));
        self.feeds.insert(module.to_string(), feed);
    }

    fn start(&mut self) {
        #[cfg(test)]
        if let Some(directory) = &self.test_directory {
            assert!(
                directory.path().is_dir(),
                "the wall's test reel directory was removed while it was in use"
            );
        }
        if !matches!(self.worker, Thread::NotStarted) {
            return;
        }
        match TapeWorker::start(self.dir.clone(), self.recordings.budget) {
            Ok(worker) => {
                if let Some(error) = &worker.store_error {
                    let text = format!(
                        "{error}; saved loops come back blank, and loops keep only what they record live"
                    );
                    self.note(text);
                }
                self.worker = Thread::Running(worker);
            }
            Err(error) => {
                self.worker = Thread::Gone;
                self.note(format!(
                    "{error}; saved loops come back blank, and an erase is not heard"
                ));
            }
        }
    }

    /// Put a loop's saved tape back, unless it has recorded since it was
    /// built: then what it recorded stays.
    pub fn put_back(&mut self, module: &str, record: TapeRecord) {
        self.replace(module, Some(record), true);
    }

    /// Give a loop `record`'s reel, or blank tape, over whatever it holds
    /// (`only_if_blank` false: an erase or its undo) or only if it has
    /// recorded nothing since it was built (true: a put-back).
    pub fn replace(&mut self, module: &str, record: Option<TapeRecord>, only_if_blank: bool) {
        // A loop not in the engine yet is given its tape as it goes in.
        let Some(&(capacity_frames, tail_cap)) = self.geometry.get(module) else {
            return;
        };
        self.start();
        let reel = record
            .as_ref()
            .map(|record| record.reel.chars().take(8).collect::<String>());
        if !matches!(self.worker, Thread::Running(_)) {
            self.waiting.remove(module);
            let text = reel.map_or_else(
                || format!("{module} was not erased on the wall: the tape thread is not running; it keeps the tape it had"),
                |reel| format!("{module}'s reel {reel} was not put back: the tape thread is not running; it is blank"),
            );
            self.note(text);
            return;
        }
        self.tickets = self.tickets.wrapping_add(1);
        let job = record.map_or_else(
            || TapeJob::Blank {
                module: module.to_string(),
                capacity_frames,
                tail_cap,
            },
            |record| TapeJob::Fetch {
                module: module.to_string(),
                record,
                capacity_frames,
                tail_cap,
            },
        );
        self.waiting.insert(
            module.to_string(),
            Waiting {
                ticket: self.tickets,
                job: Some(Box::new(job)),
                install: None,
                only_if_blank,
                reel,
                full_noted: false,
            },
        );
        self.poll();
    }

    /// A loop taken out: its feed goes, and tape on its way to it is
    /// dropped when it arrives.
    pub fn forget(&mut self, module: &str) {
        if self.recordings.has_pending(module) {
            self.note(format!("{module}'s newest recording was not kept before its loop was removed; its last take remains in the log"));
        }
        self.feeds.remove(module);
        self.waiting.remove(module);
        self.geometry.remove(module);
        self.recordings.forget(module);
    }

    /// The engine's loops are all replaced: every feed goes, and tape on
    /// its way to them is dropped when it arrives.
    pub fn replace_wall(&mut self) {
        self.feeds.clear();
        self.waiting.clear();
        self.geometry.clear();
        self.recordings.clear_pending();
    }

    fn finish(&mut self, ticket: u64, answer: TapeDone) {
        let (module, result) = match answer {
            answer @ (TapeDone::Spare { .. }
            | TapeDone::Stored { .. }
            | TapeDone::Maintained { .. }) => {
                self.take_answer(ticket, answer);
                return;
            }
            TapeDone::Fetched { module, result } => (
                module,
                result.map(|fetched| {
                    let (samples, splice, bar_pass, bins) = *fetched;
                    LoopTape {
                        samples,
                        splice: Some(splice),
                        bar_pass,
                        bins,
                    }
                }),
            ),
            TapeDone::Blanked { module, result } => (
                module,
                result.map(|samples| LoopTape {
                    samples,
                    splice: None,
                    bar_pass: [i64::MIN; MAX_LOOP_BARS],
                    bins: [0; looper::LOOP_BINS],
                }),
            ),
        };
        // An answer for a loop forgotten or given other tape since is
        // dropped here, on the control side.
        let Some(waiting) = self
            .waiting
            .get_mut(&module)
            .filter(|waiting| waiting.ticket == ticket)
        else {
            return;
        };
        match result {
            Ok(tape) => {
                waiting.install = Some(LoopIn::Install {
                    tape: Box::new(tape),
                    only_if_blank: waiting.only_if_blank,
                });
            }
            Err(why) => {
                let reel = waiting.reel.take();
                self.waiting.remove(&module);
                let text = reel.map_or_else(
                    || format!("{module} {why}; it keeps the tape it had"),
                    |reel| format!("{module}'s reel {reel} {why}; it is blank"),
                );
                self.note(text);
            }
        }
    }

    fn worker_gone(&mut self) {
        self.worker = Thread::Gone;
        let mut modules: Vec<String> = self.waiting.drain().map(|(module, _)| module).collect();
        modules.extend(self.recordings.stopped());
        modules.sort();
        modules.dedup();
        self.note(format!(
            "the tape thread stopped; tape on its way to {} is lost, and loops keep only what they record live",
            if modules.is_empty() {
                "no loop".to_string()
            } else {
                modules.join(", ")
            }
        ));
    }

    /// Take the tape thread's answers, queue jobs a full queue held back,
    /// hand finished tapes to their loops, and free what loops hand back.
    /// Called on every wall tick.
    pub fn poll(&mut self) {
        while let Thread::Running(worker) = &self.worker {
            match worker.done.try_recv() {
                Ok((ticket, answer)) => self.finish(ticket, answer),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.worker_gone();
                    break;
                }
            }
        }
        if !self.waiting.is_empty() {
            let modules: Vec<String> = self.waiting.keys().cloned().collect();
            for module in modules {
                if !self.submit(&module) {
                    break;
                }
                self.install(&module);
            }
        }
        self.collect();
        self.poll_takes();
    }

    /// Queue `module`'s job if it has one waiting. False when the thread
    /// has gone (everything waiting is dropped).
    fn submit(&mut self, module: &str) -> bool {
        let Thread::Running(worker) = &self.worker else {
            return false;
        };
        let Some(waiting) = self.waiting.get_mut(module) else {
            return true;
        };
        let Some(job) = waiting.job.take() else {
            return true;
        };
        match worker.submit(waiting.ticket, job) {
            Ok(()) => true,
            Err(TrySendError::Full((_, job))) => {
                waiting.job = Some(job);
                if !waiting.full_noted {
                    waiting.full_noted = true;
                    self.note(format!(
                        "the tape thread is busy; {module}'s tape is asked for again on the next tick"
                    ));
                }
                true
            }
            Err(TrySendError::Disconnected(_)) => {
                self.worker_gone();
                false
            }
        }
    }

    /// Hand `module`'s finished tape to its feed; a full feed is tried
    /// again on the next tick.
    fn install(&mut self, module: &str) {
        let Some(waiting) = self.waiting.get_mut(module) else {
            return;
        };
        let Some(LoopIn::Install {
            tape,
            only_if_blank,
        }) = waiting.install.take()
        else {
            return;
        };
        let Some(feed) = self.feeds.get_mut(module) else {
            self.waiting.remove(module);
            return;
        };
        let LoopTape {
            samples,
            splice,
            bar_pass,
            bins,
        } = *tape;
        match feed.install(samples, splice, bar_pass, bins, only_if_blank) {
            Ok(()) => {
                self.waiting.remove(module);
            }
            Err(request) => waiting.install = Some(request),
        }
    }

    /// Drain every loop's feed on the control side: tape a loop hands back
    /// is freed here, refusals become notes, and finished punches request
    /// snapshots whose allocations come from the tape thread.
    pub fn collect(&mut self) {
        let mut refused = Vec::new();
        let mut copies_refused = Vec::new();
        let mut recorded = Vec::new();
        let mut snapped = Vec::new();
        for (module, feed) in &mut self.feeds {
            for answer in feed.collect() {
                match answer {
                    LoopOut::Retired(tape) => drop(tape),
                    LoopOut::Snapped(snapshot) => snapped.push((module.clone(), snapshot)),
                    LoopOut::Took(mark) => {
                        self.takes = self.takes.saturating_add(1);
                        recorded.push((module.clone(), mark));
                    }
                    // The tape thread checked the splice and length against
                    // the loop's own checks: a put-back is refused only
                    // because the loop recorded first.
                    LoopOut::Refused(LoopIn::Install {
                        only_if_blank: true,
                        ..
                    }) => refused.push(format!(
                        "{module} was recorded before its saved tape arrived; the saved tape was not put back"
                    )),
                    LoopOut::Refused(LoopIn::Install { .. }) => refused.push(format!(
                        "{module} refused the tape it was given; it keeps what it had"
                    )),
                    // Its take fails, so takes after it can start.
                    LoopOut::Refused(LoopIn::Snapshot { .. }) => {
                        copies_refused.push(module.clone());
                    }
                }
            }
        }
        for (module, mark) in recorded {
            self.request_take(&module, crate::protocol::TakeWhy::Punch, Some(mark));
        }
        for (module, snapshot) in snapped {
            self.snapped(&module, snapshot);
        }
        for module in copies_refused {
            self.copy_refused(&module);
        }
        for note in refused {
            self.note(note);
        }
    }

    /// Wait for the tape thread's next answer and act on it, as a tick
    /// would: for tests, which must not depend on timing.
    #[cfg(test)]
    pub(crate) fn wait_for_test(&mut self) {
        let Thread::Running(worker) = &self.worker else {
            panic!("the tape thread is not running");
        };
        let (ticket, answer) = worker.done.recv().unwrap();
        self.finish(ticket, answer);
        self.poll();
    }
}
