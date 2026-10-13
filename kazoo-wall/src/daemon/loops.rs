//! The wall's loops on the control side: each loop's feed, and the tape
//! thread that moves reels from the store onto the loops.
//!
//! Every disk read and every whole-tape allocation happens on the tape
//! thread (`worker`), never on the control thread or the render thread.
//! A loop's tape is allocated there or in `LoopModule::new` (built on the
//! control side), and freed here in [`Loops::collect`] (what an install
//! hands back) or in the engine's retire drain (a module taken out).
//!
//! Until recording checkpoints land (KZ-L04), a loop's live recording is
//! kept only while the daemon runs, and a save names the last reel
//! fetched for it.
#[cfg(test)]
pub(crate) mod tests;
mod worker;

use crate::dsp::looper::{self, LoopFeed, LoopGauges, LoopIn, LoopOut, LoopTape, MAX_LOOP_BARS};
use crate::patch::TapeRecord;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{TryRecvError, TrySendError};
use worker::{TapeDone, TapeJob, TapeWorker};

/// A wall holds eight loops, like the eight-track: eight full loops are
/// 295 MB at 48 kHz, the most the wall lets itself take for tape.
pub const MAX_LOOPS: usize = 8;

/// Notes kept for the wall to show; older ones are on the console log.
const MAX_NOTES: usize = 64;

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
    dir: Result<PathBuf, String>,
    tickets: u64,
    /// Each attached loop's `(capacity, tail)` in frames.
    geometry: HashMap<String, (usize, usize)>,
    takes: u64,
}

impl Default for Loops {
    fn default() -> Self {
        Self::new()
    }
}

impl Loops {
    /// The wall's loops, with the reel store at `$KAZOO_REELS_DIR` or
    /// `~/.kazoo/reels`; nothing is opened until a loop needs tape.
    #[must_use]
    pub fn new() -> Self {
        Self::with_directory(kazoo_core::reels::reels_dir().map_err(|error| error.to_string()))
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
            dir,
            tickets: 0,
            geometry: HashMap::new(),
            takes: 0,
        }
    }

    fn note(&mut self, text: String) {
        eprintln!("kazoo-wall: {text}");
        if self.notes.len() == MAX_NOTES {
            self.notes.pop_front();
        }
        self.notes.push_back(text);
    }

    /// What went wrong with the loops' tape lately, oldest first.
    pub fn notes(&self) -> impl Iterator<Item = &str> {
        self.notes.iter().map(String::as_str)
    }

    /// Punches finished since the wall started (KZ-L04 turns them into
    /// takes).
    #[must_use]
    pub const fn takes(&self) -> u64 {
        self.takes
    }

    /// What one loop rendered most recently.
    #[must_use]
    pub fn gauges(&self, module: &str) -> Option<&LoopGauges> {
        self.feeds.get(module).map(LoopFeed::gauges)
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
        if !matches!(self.worker, Thread::NotStarted) {
            return;
        }
        match TapeWorker::start(self.dir.clone()) {
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
        self.feeds.remove(module);
        self.waiting.remove(module);
        self.geometry.remove(module);
    }

    /// The engine's loops are all replaced: every feed goes, and tape on
    /// its way to them is dropped when it arrives.
    pub fn replace_wall(&mut self) {
        self.feeds.clear();
        self.waiting.clear();
        self.geometry.clear();
    }

    fn finish(&mut self, ticket: u64, answer: TapeDone) {
        let (module, result) = match answer {
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
        let modules: Vec<String> = self.waiting.drain().map(|(module, _)| module).collect();
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
    /// is freed here, refusals become notes, and finished punches are
    /// counted (KZ-L04 makes them takes; until then a snapshot is freed).
    pub fn collect(&mut self) {
        let mut refused = Vec::new();
        for (module, feed) in &mut self.feeds {
            for answer in feed.collect() {
                match answer {
                    LoopOut::Retired(tape) => drop(tape),
                    LoopOut::Snapped(snapshot) => drop(snapshot),
                    LoopOut::Took(_) => self.takes = self.takes.saturating_add(1),
                    LoopOut::Refused(request) => {
                        // The tape thread checked the splice and length
                        // against the loop's own checks: a put-back is
                        // refused only because the loop recorded first.
                        refused.push(match request {
                            LoopIn::Install {
                                only_if_blank: true,
                                ..
                            } => format!(
                                "{module} was recorded before its saved tape arrived; the saved tape was not put back"
                            ),
                            LoopIn::Install { .. } => {
                                format!("{module} refused the tape it was given; it keeps what it had")
                            }
                            LoopIn::Snapshot { .. } => {
                                format!("{module} refused a copy of its tape")
                            }
                        });
                    }
                }
            }
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
