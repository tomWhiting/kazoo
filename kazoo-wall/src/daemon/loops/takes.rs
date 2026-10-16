//! Snapshot requests and completions are owned by the control side.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc::TrySendError;
use std::time::Instant;

use super::worker::{TapeDone, TapeJob};
use super::{Loops, Thread};
use crate::dsp::looper::{LoopSnapshot, LoopState, TakeMark};
use crate::patch::TapeRecord;
use crate::protocol::TakeWhy;

/// Long overdubs preserve their tape at the first seam after this interval.
pub const CHECKPOINT_SECONDS: u64 = 60;

#[derive(Debug)]
pub struct PendingTake {
    module: String,
    why: TakeWhy,
    spare: Option<Box<[f32]>>,
    started: bool,
    snapshot_sent: bool,
    cancelled: bool,
    punch: Option<TakeMark>,
}

#[derive(Debug)]
pub struct FinishedTake {
    pub ticket: u64,
    pub module: String,
    pub tape: TapeRecord,
    pub mark: TakeMark,
    pub written: Vec<u16>,
    pub why: TakeWhy,
}

#[derive(Debug)]
pub struct Takes {
    pending: BTreeMap<u64, PendingTake>,
    jobs: VecDeque<(u64, Box<TapeJob>)>,
    pub finished: VecDeque<FinishedTake>,
    pub logged: BTreeSet<u64>,
    pub last: BTreeMap<String, (Instant, u64, String)>,
    passes: BTreeMap<String, i64>,
    pub(super) interval: u64,
    pub(super) budget: u64,
    budget_pending: bool,
    pub pruned: BTreeMap<kazoo_core::reels::ReelId, String>,
}

impl Default for Takes {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            jobs: VecDeque::new(),
            finished: VecDeque::new(),
            logged: BTreeSet::new(),
            last: BTreeMap::new(),
            passes: BTreeMap::new(),
            interval: CHECKPOINT_SECONDS,
            budget: super::pins::DEFAULT_BUDGET,
            budget_pending: false,
            pruned: BTreeMap::new(),
        }
    }
}

impl Takes {
    #[must_use]
    pub fn has_pending(&self, module: &str) -> bool {
        self.pending.values().any(|take| take.module == module)
            || self.finished.iter().any(|take| take.module == module)
    }
    pub fn forget(&mut self, module: &str) {
        self.pending.retain(|_, take| take.module != module);
        self.finished.retain(|take| take.module != module);
    }
    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.finished.clear();
        self.jobs.retain(|(_, job)| {
            matches!(
                **job,
                TapeJob::Pin { .. } | TapeJob::Sweep | TapeJob::Budget | TapeJob::Uncertain { .. }
            )
        });
    }
    pub fn stopped(&mut self) -> Vec<String> {
        let modules: BTreeSet<_> = self
            .pending
            .values()
            .map(|take| take.module.clone())
            .collect();
        self.clear_pending();
        self.jobs.clear();
        modules.into_iter().collect()
    }
}

impl Loops {
    /// Limits are per manager, so tests cannot change another wall's policy.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_limits(dir: std::path::PathBuf, seconds: u64, budget: u64) -> Self {
        let mut loops = Self::with_reels_dir(dir);
        loops.recordings.interval = seconds;
        loops.recordings.budget = budget;
        loops
    }

    #[cfg(test)]
    pub(crate) fn snapshot_for_test(&mut self, module: &str, into: Box<[f32]>) {
        self.feeds.get_mut(module).unwrap().snapshot(into).unwrap();
    }

    #[cfg(test)]
    pub(crate) fn wait_install_for_test(&mut self) {
        self.poll();
        while !self.waiting.is_empty() {
            self.wait_for_test();
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_snapshot_for_test(&mut self, module: &str) -> Box<LoopSnapshot> {
        let feed = self.feeds.get_mut(module).unwrap();
        for answer in feed.collect() {
            match answer {
                crate::dsp::looper::LoopOut::Snapped(snapshot) => return snapshot,
                crate::dsp::looper::LoopOut::Retired(tape) => drop(tape),
                other => panic!("unexpected snapshot answer: {other:?}"),
            }
        }
        panic!("the rendered snapshot has not arrived");
    }

    #[cfg(test)]
    pub(crate) fn store_snapshot_for_test(&mut self, module: &str, snapshot: Box<LoopSnapshot>) {
        self.snapped(module, snapshot);
        self.poll_takes();
    }

    #[cfg(test)]
    pub(crate) fn wait_snapshot_for_test(&mut self, ticket: u64) {
        self.poll();
        while self
            .recordings
            .pending
            .get(&ticket)
            .is_some_and(|take| !take.snapshot_sent)
        {
            self.wait_for_test();
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_take_for_test(&mut self, ticket: u64) {
        self.poll();
        while self.recordings.pending.contains_key(&ticket) {
            self.wait_for_test();
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_for_test(&self, module: &str) -> Vec<u64> {
        self.recordings
            .pending
            .iter()
            .filter(|(_, take)| take.module == module)
            .map(|(ticket, _)| *ticket)
            .collect()
    }

    pub fn request_take(
        &mut self,
        module: &str,
        why: TakeWhy,
        punch: Option<TakeMark>,
    ) -> Option<u64> {
        if !self.geometry.contains_key(module) {
            return None;
        }
        self.start();
        if !matches!(self.worker, Thread::Running(_)) {
            self.note(format!("{module}'s take could not be kept: the tape thread is not running; it is still playing, and is only in memory"));
            return None;
        }
        self.tickets = self.tickets.wrapping_add(1);
        let ticket = self.tickets;
        self.recordings.pending.insert(
            ticket,
            PendingTake {
                module: module.to_string(),
                why,
                spare: None,
                started: false,
                snapshot_sent: false,
                cancelled: false,
                punch,
            },
        );
        Some(ticket)
    }

    pub fn cancel_take(&mut self, ticket: u64) {
        if let Some(take) = self.recordings.pending.get_mut(&ticket) {
            take.cancelled = true;
        }
    }

    #[must_use]
    pub fn recording_modules(&self) -> Vec<String> {
        self.feeds
            .iter()
            .filter(|(module, feed)| {
                let state = feed.gauges().state.load(Ordering::Relaxed);
                state == LoopState::Rolling as u8
                    || state == LoopState::PostRoll as u8
                    || self
                        .recordings
                        .pending
                        .values()
                        .any(|take| &take.module == *module)
            })
            .map(|(module, _)| module.clone())
            .collect()
    }

    #[must_use]
    pub fn lost_notes(&self, frame: u64, rate: u32, action: &str) -> Vec<String> {
        self.recording_modules().iter().map(|module| {
            let taken = self.recordings.last.get(module).map_or(0, |(_, frame, _)| *frame);
            let elapsed = frame.saturating_sub(taken).div_ceil(u64::from(rate.max(1)));
            format!("{module} was recording: up to {elapsed} s since its last take are not in {action}")
        }).collect()
    }

    pub fn uncertain_refs(&mut self, reason: String) {
        self.start();
        self.recordings
            .jobs
            .push_back((0, Box::new(TapeJob::Uncertain { reason })));
    }

    pub fn queue_pin(&mut self, refs: super::pins::ReelRefs, sweep: bool) {
        self.start();
        self.recordings.jobs.push_back((
            0,
            Box::new(TapeJob::Pin {
                refs: Box::new(refs),
            }),
        ));
        if sweep {
            self.recordings
                .jobs
                .push_back((0, Box::new(TapeJob::Sweep)));
        }
    }

    pub(super) fn take_answer(&mut self, ticket: u64, answer: TapeDone) {
        match answer {
            TapeDone::Spare { module, result } => match result {
                Ok(spare) => {
                    if let Some(take) = self.recordings.pending.get_mut(&ticket) {
                        take.spare = Some(spare);
                    }
                }
                Err(error) => {
                    self.recordings.pending.remove(&ticket);
                    self.take_failed(&module, &error);
                }
            },
            TapeDone::Stored {
                module,
                snapshot,
                why,
                result,
            } => {
                let pending = self.recordings.pending.remove(&ticket);
                match result {
                    Ok(tape) => {
                        if let Some(pending) = pending.filter(|take| !take.cancelled) {
                            let bars = pending.punch.map_or(snapshot.at.bars, |mark| mark.bars);
                            let written = bars
                                .iter()
                                .enumerate()
                                .filter_map(|(bar, written)| written.then_some(bar as u16))
                                .collect();
                            self.recordings.finished.push_back(FinishedTake {
                                ticket,
                                module,
                                tape,
                                mark: snapshot.at,
                                written,
                                why,
                            });
                        }
                    }
                    Err(error) => self.take_failed(&module, &error),
                }
                // The worker returns its entire allocation; it is freed here.
                drop(snapshot);
            }
            TapeDone::Maintained {
                result,
                paused,
                pruned,
            } => {
                if paused && !self.checkpoints_paused && result.is_ok() {
                    self.note("the reel store is full (4.0 GiB); checkpoints are paused; takes and saves still write".into());
                }
                self.recordings.budget_pending = false;
                self.checkpoints_paused = paused;
                self.recordings.pruned = pruned;
                if let Err(error) = result {
                    self.note(format!("reel retention failed: {error}; checkpoints may be paused, and no sweep is accepted until every reference is readable"));
                }
            }
            answer @ (TapeDone::Fetched { .. } | TapeDone::Blanked { .. }) => {
                self.finish(ticket, answer);
            }
        }
    }

    fn take_failed(&mut self, module: &str, error: &str) {
        self.note(format!(
            "{module}'s take could not be kept: {error}; it is still playing, and is only in memory"
        ));
    }

    pub(super) fn snapped(&mut self, module: &str, snapshot: Box<LoopSnapshot>) {
        let ticket = self
            .recordings
            .pending
            .iter()
            .find(|(_, take)| take.module == module && take.started)
            .map(|(ticket, _)| *ticket);
        if let Some(ticket) = ticket {
            if self.recordings.pending[&ticket].cancelled {
                self.recordings.pending.remove(&ticket);
                drop(snapshot);
                return;
            }
            let why = self.recordings.pending[&ticket].why;
            self.recordings.jobs.push_back((
                ticket,
                Box::new(TapeJob::Store {
                    module: module.to_string(),
                    snapshot,
                    why,
                }),
            ));
        } else {
            self.take_failed(module, "an unrequested snapshot was returned");
            drop(snapshot);
        }
    }

    pub(super) fn poll_takes(&mut self) {
        if self.checkpoints_paused && !self.recordings.budget_pending {
            self.recordings.budget_pending = true;
            self.recordings
                .jobs
                .push_back((0, Box::new(TapeJob::Budget)));
        }
        let mut busy = BTreeSet::new();
        let mut notes = Vec::new();
        for (ticket, take) in &mut self.recordings.pending {
            if !busy.insert(take.module.clone()) {
                continue;
            }
            if !take.started {
                if !self.geometry.contains_key(&take.module) {
                    continue;
                }
                // The loop may hold a take made at a higher rate than its
                // own (KZ-D21): its spare has room for one at any rate.
                let (capacity, tail) = crate::dsp::looper::tape_geometry();
                take.started = true;
                self.recordings.jobs.push_back((
                    *ticket,
                    Box::new(TapeJob::Spare {
                        module: take.module.clone(),
                        len: 2 * (capacity + tail + 1),
                    }),
                ));
            }
            if let Some(spare) = take.spare.take() {
                if let Some(feed) = self.feeds.get_mut(&take.module) {
                    match feed.snapshot(spare) {
                        Ok(()) => take.snapshot_sent = true,
                        Err(spare) => take.spare = Some(spare),
                    }
                } else {
                    notes.push(format!(
                        "{}'s take was lost because its loop was removed",
                        take.module
                    ));
                }
            }
        }
        for note in notes {
            self.note(note);
        }
        while let Some((ticket, job)) = self.recordings.jobs.pop_front() {
            let Thread::Running(worker) = &self.worker else {
                self.note(
                    "the tape thread stopped; queued recordings and pin updates were lost".into(),
                );
                self.recordings.jobs.clear();
                self.recordings.pending.clear();
                break;
            };
            match worker.submit(ticket, job) {
                Ok(()) => {}
                Err(TrySendError::Full(job)) => {
                    self.recordings.jobs.push_front(job);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.worker_gone();
                    break;
                }
            }
        }
    }

    pub fn checkpoint(&mut self, now: Instant) {
        let mut due = Vec::new();
        for (module, feed) in &self.feeds {
            let gauges = feed.gauges();
            let pass = gauges.pass.load(Ordering::Relaxed);
            let prior = self.recordings.passes.insert(module.clone(), pass);
            let last = self
                .recordings
                .last
                .entry(module.clone())
                .or_insert_with(|| (now, 0, "no take".into()));
            if !self.checkpoints_paused
                && gauges.state.load(Ordering::Relaxed) == LoopState::Rolling as u8
                && prior.is_some_and(|prior| prior != pass)
                && now.saturating_duration_since(last.0).as_secs() >= self.recordings.interval
                && !self
                    .recordings
                    .pending
                    .values()
                    .any(|take| &take.module == module)
            {
                due.push(module.clone());
                last.0 = now;
            }
        }
        for module in due {
            self.request_take(&module, TakeWhy::Checkpoint, None);
        }
    }
}
