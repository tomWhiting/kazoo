//! Saved reels return through one tape worker and bounded feed rings.
//!
//! Until recording checkpoints are implemented, live takes survive only
//! while the daemon runs; a save names the last fetched reel.
#[cfg(test)]
mod tests;
mod worker;

use crate::dsp::looper::{
    LoopFeed, LoopGauges, LoopIn, LoopOut, MAX_LOOP_BARS, MAX_LOOP_FRAMES, MAX_LOOP_SECONDS,
    SEAM_SECONDS,
};
use crate::patch::TapeRecord;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{TryRecvError, TrySendError};
use worker::{TapeDone, TapeJob, TapeWorker};

/// Eight tracks bound the wall's tape memory.
pub const MAX_LOOPS: usize = 8;
#[derive(Debug)]
struct Waiting {
    ticket: u64,
    job: Option<Box<TapeJob>>,
    install: Option<LoopIn>,
    only_if_blank: bool,
    full_noted: bool,
    reel: Option<String>,
}
/// Control-side handles and outstanding tape work.
#[derive(Debug)]
pub struct Loops {
    feeds: HashMap<String, LoopFeed>,
    worker: Option<TapeWorker>,
    waiting: HashMap<String, Waiting>,
    notes: Vec<String>,
    dir: Result<PathBuf, String>,
    started: bool,
    tickets: u64,
    geometry: HashMap<String, (usize, usize)>,
    takes: u64,
}
impl Default for Loops {
    fn default() -> Self {
        Self::new()
    }
}
impl Loops {
    /// Resolve the reel directory without touching the store yet.
    #[must_use]
    pub fn new() -> Self {
        Self::with_directory(kazoo_core::reels::reels_dir().map_err(|error| error.to_string()))
    }
    /// Use an explicit store directory, including for isolated tests.
    #[must_use]
    pub fn with_reels_dir(dir: PathBuf) -> Self {
        Self::with_directory(Ok(dir))
    }
    fn with_directory(dir: Result<PathBuf, String>) -> Self {
        Self {
            feeds: HashMap::new(),
            worker: None,
            waiting: HashMap::new(),
            notes: Vec::new(),
            dir,
            started: false,
            tickets: 0,
            geometry: HashMap::new(),
            takes: 0,
        }
    }
    fn note(&mut self, text: String) {
        eprintln!("kazoo-wall: {text}");
        if self.notes.len() == 64 {
            self.notes.remove(0);
        }
        self.notes.push(text);
    }
    /// Retained warnings for the wall's observations.
    #[must_use]
    pub fn notes(&self) -> &[String] {
        &self.notes
    }
    /// Most recently rendered observations of one loop.
    #[must_use]
    pub fn gauges(&self, module: &str) -> Option<&LoopGauges> {
        self.feeds.get(module).map(LoopFeed::gauges)
    }
    /// Attach a freshly built loop's control handle.
    pub fn attach(&mut self, module: &str, feed: LoopFeed, rate: f32) {
        self.forget(module);
        let capacity = (MAX_LOOP_SECONDS * f64::from(rate))
            .round()
            .min(MAX_LOOP_FRAMES as f64) as usize;
        self.geometry.insert(
            module.into(),
            (capacity, (SEAM_SECONDS * rate).round() as usize),
        );
        self.feeds.insert(module.into(), feed);
    }
    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let result = self.dir.clone().and_then(TapeWorker::start);
        match result {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => self.note(format!(
                "the reel store cannot open ({error}); loops keep only what they record live"
            )),
        }
    }
    /// Restore saved tape without overwriting a recording begun meanwhile.
    pub fn put_back(&mut self, module: &str, record: TapeRecord) {
        self.replace(module, Some(record), true);
    }
    /// Erase or undo an erase, overriding current live tape.
    pub fn replace(&mut self, module: &str, record: Option<TapeRecord>, only_if_blank: bool) {
        let Some(&(capacity_frames, tail_cap)) = self.geometry.get(module) else {
            return;
        };
        self.start();
        if self.worker.is_none() {
            return;
        }
        self.tickets = self.tickets.wrapping_add(1);
        let reel = record
            .as_ref()
            .map(|record| record.reel.chars().take(8).collect::<String>());
        let job = record.map_or_else(
            || TapeJob::Blank {
                module: module.into(),
                capacity_frames,
                tail_cap,
            },
            |record| TapeJob::Fetch {
                module: module.into(),
                record,
                capacity_frames,
                tail_cap,
            },
        );
        self.waiting.insert(
            module.into(),
            Waiting {
                ticket: self.tickets,
                job: Some(Box::new(job)),
                install: None,
                only_if_blank,
                full_noted: false,
                reel,
            },
        );
        self.poll();
    }
    /// Remove a feed and invalidate every outstanding answer for it.
    pub fn forget(&mut self, module: &str) {
        self.feeds.remove(module);
        self.waiting.remove(module);
        self.geometry.remove(module);
    }
    /// A rebuilt engine has new feeds; old worker answers are discarded.
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
                    crate::dsp::looper::LoopTape {
                        samples,
                        splice: Some(splice),
                        bar_pass,
                        bins,
                    }
                }),
            ),
            TapeDone::Blanked { module, result } => (
                module,
                result.map(|samples| crate::dsp::looper::LoopTape {
                    samples,
                    splice: None,
                    bar_pass: [i64::MIN; MAX_LOOP_BARS],
                    bins: [0; 64],
                }),
            ),
        };
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
            Err(error) => {
                let reel = waiting.reel.clone().unwrap_or_default();
                self.waiting.remove(&module);
                self.note(format!("{module}'s reel {reel} is missing or unreadable in the reel store ({error}); it is blank"));
            }
        }
    }
    /// Receive completed buffers, retry full queues and free retired tape.
    pub fn poll(&mut self) {
        while let Some(worker) = &self.worker {
            match worker.done.try_recv() {
                Ok((ticket, answer)) => self.finish(ticket, answer),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.waiting.clear();
                    self.note(
                        "the tape worker stopped; loops keep only what they record live".into(),
                    );
                    break;
                }
            }
        }
        let modules: Vec<String> = self.waiting.keys().cloned().collect();
        for module in modules {
            let Some(waiting) = self.waiting.get_mut(&module) else {
                continue;
            };
            if let Some(job) = waiting.job.take() {
                let Some(worker) = &self.worker else {
                    waiting.job = Some(job);
                    continue;
                };
                match worker.submit(waiting.ticket, job) {
                    Ok(()) => {}
                    Err(TrySendError::Full((_, job))) => {
                        waiting.job = Some(job);
                        if !waiting.full_noted {
                            waiting.full_noted = true;
                            self.note(
                                "the tape worker queue is full; it will retry on the next tick"
                                    .into(),
                            );
                        }
                        continue;
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        self.worker = None;
                        self.waiting.clear();
                        self.note(
                            "the tape worker stopped; loops keep only what they record live".into(),
                        );
                        break;
                    }
                }
            }
            let Some(waiting) = self.waiting.get_mut(&module) else {
                continue;
            };
            if let Some(LoopIn::Install {
                tape,
                only_if_blank,
            }) = waiting.install.take()
            {
                if let Some(feed) = self.feeds.get_mut(&module) {
                    match feed.install(
                        tape.samples,
                        tape.splice,
                        tape.bar_pass,
                        tape.bins,
                        only_if_blank,
                    ) {
                        Ok(()) => {
                            self.waiting.remove(&module);
                        }
                        Err(request) => waiting.install = Some(request),
                    }
                }
            }
        }
        self.collect();
    }
    /// Drain ownership on the control side. Checkpoints will consume take marks.
    pub fn collect(&mut self) {
        let mut refused = Vec::new();
        for (module, feed) in &mut self.feeds {
            for answer in feed.collect() {
                match answer {
                    LoopOut::Retired(tape) => drop(tape),
                    LoopOut::Snapped(snapshot) => drop(snapshot),
                    LoopOut::Took(_) => self.takes = self.takes.saturating_add(1),
                    LoopOut::Refused(request) => {
                        let saved = matches!(
                            request,
                            LoopIn::Install {
                                only_if_blank: true,
                                ..
                            }
                        );
                        drop(request);
                        refused.push(if saved { format!("{module} was recorded before its saved tape arrived; the saved tape was not put back") } else { format!("{module} refused a tape request") });
                    }
                }
            }
        }
        for note in refused {
            self.note(note);
        }
    }
    #[cfg(test)]
    pub(crate) fn wait_for_test(&mut self) {
        let (ticket, answer) = self.worker.as_ref().unwrap().done.recv().unwrap();
        self.finish(ticket, answer);
        self.poll();
    }
}
