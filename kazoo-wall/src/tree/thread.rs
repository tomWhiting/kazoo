//! A bounded control-thread feed to the sole tree writer.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::git::{self, Commit, Importer, Planted};
use super::{Files, TreeError};
use crate::protocol::{Change, ClockStamp, MarkResult, TreeHealth, TreePlace, TreeResult, What};

#[cfg(test)]
mod finish;
pub mod here;
pub use here::HereError;

/// A fast-import mark or commit id; absent on an unborn line.
pub type Tip = Option<String>;

/// The wall is on a line, or at a detached immutable moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    On {
        branch: String,
        tip: Tip,
    },
    At {
        commit: String,
        mark: Option<String>,
    },
}

/// The result of a control-side tree job.
#[derive(Debug)]
pub enum Answered {
    Marked(MarkResult),
    Resolved {
        to: TreePlace,
        here: TreePlace,
        wall: Box<crate::patch::PatchFile>,
    },
    Here(TreePlace),
    Listed(Box<TreeResult>),
}

/// An ordered answer, including the latest change taken by the tree.
pub type TreeAnswer = (u64, u64, Result<Answered, HereError>);

/// Items the queue holds.
///
/// The log's busiest second held 30 changes and its
/// busiest minute 165 (26 Sep to 13 Oct); the flood guard lets eight seats
/// make 240 at once. 1,024 is four times that, and over six of the busiest
/// minutes.
pub const TREE_QUEUE: usize = 1_024;
/// Bytes the queue holds: eight of the largest walls (`MAX_WALL_FILE`,
/// 8 MiB) or about sixteen thousand ordinary changes of about 4 KB, so
/// memory stays bounded whatever happens.
pub const TREE_QUEUE_BYTES: usize = 64 * 1024 * 1024;
/// A durable checkpoint costs about 17 ms (measured), so at most one a
/// second costs under 2% of the thread, and a crash loses at most a
/// second of commits (never of the log; the next start covers them).
pub const CHECKPOINT_AFTER: Duration = Duration::from_secs(1);
/// Checkpoint after this many commits even while a flood keeps the queue
/// busy, so a crash never loses more than this many.
pub const CHECKPOINT_COMMITS: u64 = 256;
/// Checkpoints between repacks.
///
/// Each checkpoint writes one pack; a geometric repack after every 64
/// keeps their number logarithmic without rewriting everything. The
/// importer is finished and started again around it, since a running
/// `fast-import` keeps its packs from any repack.
pub const REPACK_EVERY: u64 = 64;
/// How long a stopped tree waits before starting git again: often enough
/// to recover soon, seldom enough not to spawn git over and over while a
/// disk is full.
pub const RETRY_AFTER: Duration = Duration::from_secs(5);
/// The longest a stopping daemon waits for the tree to take its last items
/// and finish: the only time the control thread waits on the tree.
pub const FINISH_WAIT: Duration = Duration::from_secs(5);
/// How long the control thread waits for news between tries while it
/// finishes, so a full queue has time to drain.
const FINISH_POLL: Duration = Duration::from_millis(20);

/// What the control thread sends the tree thread.
#[derive(Debug)]
pub enum TreeItem {
    /// A made change: what differs from the last item.
    Change {
        change: Box<Change>,
        files: Vec<(String, Option<Vec<u8>>)>,
        generation: u64,
    },
    /// The whole wall, after change `seq`.
    Whole {
        kind: WholeKind,
        seq: u64,
        clock: Option<ClockStamp>,
        files: Files,
        generation: u64,
    },
    /// Give the current moment an immutable name.
    Mark {
        ticket: u64,
        name: String,
        console: bool,
    },
    /// Give a saved wall an immutable name.
    SaveMark { name: String, at: String },
    /// Read a named moment's wall.
    Resolve { ticket: u64, moment: String },
    /// Read the wall's current place.
    Here { ticket: u64 },
    /// List the tree's bounded view.
    List { ticket: u64, recent: Option<u32> },
    /// A go missed by the change queue, before its whole-wall gap.
    Moved { to: TreePlace },
    /// Finish: checkpoint, `done`, and say so.
    Finish,
}

/// Why a whole item is sent.
#[derive(Debug, Clone, Copy)]
pub enum WholeKind {
    /// The daemon's start, on a fresh clock line.
    Start,
    /// After items the tree did not take one by one.
    Gap,
}

/// What the tree thread tells the control thread.
#[derive(Debug)]
pub enum News {
    /// The tree durably holds every change through `through`.
    Committed { through: u64 },
    /// A gap commit holds changes `first` to `last` as one.
    Gap { first: u64, last: u64 },
    /// Git failed: the tree is stopped; it holds changes through `through`.
    Down { error: String, through: Option<u64> },
    /// Git works again: send the whole wall.
    NeedWhole,
    /// An answer after every change ahead of its job.
    Answer {
        ticket: u64,
        through: u64,
        answer: Result<Answered, HereError>,
    },
    /// The answer to [`TreeItem::Finish`].
    Finished(Result<(), String>),
}

impl TreeItem {
    fn seq(&self) -> Option<u64> {
        match self {
            Self::Change { change, .. } => Some(change.seq),
            Self::Whole { seq, .. } => Some(*seq),
            Self::Mark { .. }
            | Self::SaveMark { .. }
            | Self::Resolve { .. }
            | Self::Here { .. }
            | Self::List { .. }
            | Self::Moved { .. }
            | Self::Finish => None,
        }
    }

    const fn ticket(&self) -> Option<u64> {
        match self {
            Self::Mark { ticket, .. }
            | Self::Resolve { ticket, .. }
            | Self::Here { ticket }
            | Self::List { ticket, .. } => Some(*ticket),
            Self::Change { .. }
            | Self::Whole { .. }
            | Self::SaveMark { .. }
            | Self::Moved { .. }
            | Self::Finish => None,
        }
    }

    fn bytes(&self) -> Result<usize, TreeError> {
        let files = |files: &[(String, Option<Vec<u8>>)]| {
            files
                .iter()
                .map(|(p, b)| {
                    p.len()
                        + b.as_ref().map_or(0, Vec::len)
                        + std::mem::size_of::<(String, Option<Vec<u8>>)>()
                })
                .sum::<usize>()
        };
        Ok(std::mem::size_of::<Self>()
            + match self {
                Self::Change {
                    change,
                    files: diff,
                    ..
                } => {
                    serde_json::to_vec(change)
                        .map_err(|err| TreeError::Git(format!("queued change: {err}")))?
                        .len()
                        + files(diff)
                }
                Self::Whole {
                    files: whole,
                    clock,
                    ..
                } => {
                    whole
                        .iter()
                        .map(|(p, b)| p.len() + b.len() + 64)
                        .sum::<usize>()
                        + clock.as_ref().map_or(0, |c| c.line.len() + 32)
                }
                Self::Mark { name, .. } => name.len(),
                Self::SaveMark { name, at } => name.len() + at.len(),
                Self::Resolve { moment, .. } => moment.len(),
                Self::Moved { to } => {
                    to.commit.len()
                        + to.branch.as_ref().map_or(0, String::len)
                        + to.mark.as_ref().map_or(0, String::len)
                }
                Self::Here { .. } | Self::List { .. } | Self::Finish => 0,
            })
    }
}

/// The control thread's end of the tree: it offers items, never blocking,
/// and hears news.
#[derive(Debug)]
pub struct TreeHandle {
    sender: SyncSender<TreeItem>,
    news: Receiver<News>,
    bytes: Arc<AtomicUsize>,
    waiting: Arc<AtomicUsize>,
    join: Option<JoinHandle<()>>,
    pub generation: u64,
    pub resync: bool,
    pub behind_from: Option<u64>,
    pub through: Option<u64>,
    pub error: Option<String>,
    pub answered: Vec<TreeAnswer>,
}

impl TreeHandle {
    /// Check git, read where the tree in `dir` stands, and start the tree
    /// thread `kazoo-wall-tree`.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when git is missing or too old, the tree does not
    /// read, or the thread cannot start.
    pub fn attach(dir: &Path) -> Result<Self, TreeError> {
        Self::spawn(dir, None)
    }

    #[cfg(test)]
    pub fn attach_paused(dir: &Path, latch: Receiver<()>) -> Result<Self, TreeError> {
        Self::spawn(dir, Some(latch))
    }

    fn spawn(dir: &Path, latch: Option<Receiver<()>>) -> Result<Self, TreeError> {
        git::check_version()?;
        let (line, through) = here::read_line(dir).map_err(tree_error)?;
        let (sender, receiver) = mpsc::sync_channel(TREE_QUEUE);
        let (news_tx, news) = mpsc::channel();
        let bytes = Arc::new(AtomicUsize::new(0));
        let waiting = Arc::new(AtomicUsize::new(0));
        let dir = dir.to_path_buf();
        let worker_bytes = Arc::clone(&bytes);
        let worker_waiting = Arc::clone(&waiting);
        let join = thread::Builder::new()
            .name("kazoo-wall-tree".into())
            .spawn(move || {
                if let Some(latch) = latch {
                    if let Err(err) = latch.recv() {
                        eprintln!("kazoo-wall: tree test latch: {err}");
                        return;
                    }
                }
                worker(
                    &dir,
                    line,
                    through,
                    &receiver,
                    &news_tx,
                    &worker_bytes,
                    &worker_waiting,
                );
            })
            .map_err(|err| TreeError::Git(format!("start tree thread: {err}")))?;
        Ok(Self {
            sender,
            news,
            bytes,
            waiting,
            join: Some(join),
            generation: 1,
            resync: false,
            behind_from: None,
            through,
            error: None,
            answered: Vec::new(),
        })
    }

    /// Queue `item` without ever blocking. When the queue is full (by
    /// count or bytes) or the thread has gone, the item is dropped, the
    /// tree is marked behind from its change, and a whole item must follow.
    pub fn offer(&mut self, item: TreeItem) -> bool {
        let seq = item.seq();
        let sent = self.ask(item);
        if !sent {
            self.missed(seq);
        }
        sent
    }

    /// Queue a job without changing the resync state when it is refused.
    pub fn ask(&mut self, item: TreeItem) -> bool {
        let bytes = match item.bytes() {
            Ok(bytes) => bytes,
            Err(err) => {
                self.error = Some(err.to_string());
                return false;
            }
        };
        let previous = self.bytes.fetch_add(bytes, Ordering::AcqRel);
        if previous.saturating_add(bytes) > TREE_QUEUE_BYTES {
            self.bytes.fetch_sub(bytes, Ordering::AcqRel);
            return false;
        }
        self.waiting.fetch_add(1, Ordering::AcqRel);
        match self.sender.try_send(item) {
            Ok(()) => true,
            Err(err) => {
                self.bytes.fetch_sub(bytes, Ordering::AcqRel);
                self.waiting.fetch_sub(1, Ordering::AcqRel);
                if matches!(err, TrySendError::Disconnected(_)) {
                    self.error = Some("the tree thread has stopped".into());
                }
                false
            }
        }
    }

    fn missed(&mut self, seq: Option<u64>) {
        self.resync = true;
        if self.behind_from.is_none() {
            self.behind_from = seq;
            if let Some(seq) = seq {
                eprintln!(
                    "kazoo-wall: the tree's queue is full (1,024 changes or 64 MiB waiting): change {seq} and those after it are not committed one by one; the tree will hold them in one commit as soon as it has room"
                );
            }
        }
    }

    /// The tree's health for the snapshot.
    #[must_use]
    pub fn health(&self) -> TreeHealth {
        TreeHealth {
            planted: true,
            through: self.through,
            waiting: u32::try_from(self.waiting.load(Ordering::Acquire)).unwrap_or(u32::MAX),
            behind_from: self.behind_from,
            error: self.error.clone(),
        }
    }

    /// Hear all the news waiting, never blocking.
    pub fn poll(&mut self) {
        while let Ok(news) = self.news.try_recv() {
            self.hear(news);
        }
    }

    fn hear(&mut self, news: News) {
        match news {
            News::Committed { through } => {
                self.through = Some(through);
                self.error = None;
                if self.behind_from.is_some_and(|first| through >= first) {
                    self.behind_from = None;
                }
            }
            News::Gap { first, last } => {
                self.through = Some(last);
                self.behind_from = None;
                self.error = None;
                eprintln!("kazoo-wall: the tree holds changes {first} to {last} as one commit");
            }
            News::Down { error, through } => {
                eprintln!(
                    "kazoo-wall: the tree stopped: {error}; changes from {} are not committed one by one",
                    through.map_or(1, |n| n.saturating_add(1))
                );
                self.error = Some(error);
                self.through = through;
                self.resync = true;
            }
            News::NeedWhole => self.resync = true,
            News::Answer {
                ticket,
                through,
                answer,
            } => self.answered.push((ticket, through, answer)),
            News::Finished(result) => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn wait_event(&mut self) {
        match self.news.recv() {
            Ok(news) => self.hear(news),
            Err(err) => panic!("tree news: {err}"),
        }
    }

    /// Hear the next news, waiting at most `wait`; whether any came.
    pub fn wait_news(&mut self, wait: Duration) -> bool {
        match self.news.recv_timeout(wait) {
            Ok(news) => {
                self.hear(news);
                true
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => false,
        }
    }

    /// Finish the tree by [`FINISH_WAIT`] from now (see
    /// [`Self::finish_by`]).
    ///
    /// # Errors
    ///
    /// As [`Self::finish_by`].
    pub fn finish(&mut self) -> Result<(), String> {
        self.finish_by(Instant::now() + FINISH_WAIT)
    }

    /// Send `Finish` (trying again while the queue is full), wait for the
    /// thread to checkpoint and close git, and join it; all by `deadline`.
    ///
    /// # Errors
    ///
    /// The tree's own failure, or that it did not finish by `deadline`.
    pub fn finish_by(&mut self, deadline: Instant) -> Result<(), String> {
        let bytes = TreeItem::Finish.bytes().map_err(|err| err.to_string())?;
        loop {
            self.bytes.fetch_add(bytes, Ordering::AcqRel);
            self.waiting.fetch_add(1, Ordering::AcqRel);
            match self.sender.try_send(TreeItem::Finish) {
                Ok(()) => break,
                Err(err) => {
                    self.bytes.fetch_sub(bytes, Ordering::AcqRel);
                    self.waiting.fetch_sub(1, Ordering::AcqRel);
                    if matches!(err, TrySendError::Disconnected(_)) {
                        return Err("the tree thread has stopped".into());
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the tree's queue was still full after {} s",
                    FINISH_WAIT.as_secs()
                ));
            }
            self.wait_news(FINISH_POLL);
        }
        loop {
            match self
                .news
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(News::Finished(result)) => {
                    // The thread returns as soon as it has said so.
                    if let Some(join) = self.join.take() {
                        join.join()
                            .map_err(|_| "the tree thread panicked".to_string())?;
                    }
                    if let Err(error) = &result {
                        self.error = Some(error.clone());
                    }
                    return result;
                }
                Ok(news) => self.hear(news),
                Err(err) => return Err(format!("waiting for the tree to finish: {err}")),
            }
        }
    }
}

fn tell(sender: &mpsc::Sender<News>, news: News) {
    if let Err(err) = sender.send(news) {
        eprintln!("kazoo-wall: tree news receiver closed: {err}");
    }
}

fn tree_error(error: HereError) -> TreeError {
    match error {
        HereError::UnknownMoment(message)
        | HereError::MarkExists(message)
        | HereError::Full(message)
        | HereError::Broken(message)
        | HereError::Down(message) => TreeError::Git(message),
    }
}

/// The tree thread's state while git works.
struct Writer {
    importer: Option<Importer>,
    line: Line,
    resolver: here::Resolver,
    accepted: u64,
    through: Option<u64>,
    last: Option<(u64, u64)>,
    pending: u64,
    checkpointed: Instant,
    checkpoints: u64,
    gaps: Vec<(u64, u64)>,
    base: Option<Files>,
    summary: String,
}

impl Writer {
    fn new(dir: &Path, line: Line, through: Option<u64>) -> Result<Self, TreeError> {
        let importer = match &line {
            Line::On { branch, tip } => Some(Importer::new(
                dir,
                &Planted {
                    branch: format!("refs/heads/{branch}"),
                    tip: tip.clone(),
                    through,
                },
            )?),
            Line::At { .. } => None,
        };
        Ok(Self {
            importer,
            line,
            resolver: here::Resolver::default(),
            accepted: 0,
            through,
            last: None,
            pending: 0,
            checkpointed: Instant::now(),
            checkpoints: 0,
            gaps: Vec::new(),
            base: None,
            summary: String::new(),
        })
    }

    fn importer(&mut self) -> Result<&mut Importer, TreeError> {
        self.importer
            .as_mut()
            .ok_or_else(|| TreeError::Git("no fast-import is running".into()))
    }

    fn close(&mut self) -> Result<(), TreeError> {
        if let Some(importer) = self.importer.take() {
            importer.finish()?;
        }
        self.last = None;
        Ok(())
    }

    fn renew(&mut self, dir: &Path) -> Result<(), TreeError> {
        self.close()?;
        git::repack(dir)?;
        if let Line::On { branch, tip } = &self.line {
            self.importer = Some(Importer::new(
                dir,
                &Planted {
                    branch: format!("refs/heads/{branch}"),
                    tip: tip.clone(),
                    through: self.through,
                },
            )?);
        }
        Ok(())
    }

    fn checkpoint(&mut self, dir: &Path, news: &mpsc::Sender<News>) -> Result<(), TreeError> {
        if self.pending == 0 {
            return Ok(());
        }
        if let Some((mark, seq)) = self.last {
            let sha = self.importer()?.checkpoint(mark)?;
            if let Line::On { branch, tip } = &mut self.line {
                *tip = Some(sha);
                self.resolver
                    .extend(dir, branch, self.pending)
                    .map_err(tree_error)?;
            }
            here::move_head(dir, &self.line, &self.summary).map_err(tree_error)?;
            self.through = Some(seq);
            self.pending = 0;
            self.checkpointed = Instant::now();
            tell(news, News::Committed { through: seq });
            for (first, last) in self.gaps.drain(..) {
                tell(news, News::Gap { first, last });
            }
            self.checkpoints += 1;
            if self.checkpoints % REPACK_EVERY == 0 {
                self.renew(dir)?;
            }
        }
        Ok(())
    }

    fn finish(mut self, dir: &Path, news: &mpsc::Sender<News>) -> Result<(), String> {
        self.checkpoint(dir, news)
            .and_then(|()| self.close())
            .map_err(|err| err.to_string())
    }

    fn moved(
        &mut self,
        dir: &Path,
        news: &mpsc::Sender<News>,
        to: TreePlace,
        seq: Option<u64>,
        summary: &str,
    ) -> Result<(), TreeError> {
        self.checkpoint(dir, news)?;
        self.close()?;
        let commit = here::resolve(dir, &self.line, &to.commit)
            .map_err(tree_error)?
            .commit;
        self.line = if let Some(branch) = to.branch {
            if here::branch_tip(dir, &branch)
                .map_err(tree_error)?
                .as_deref()
                == Some(commit.as_str())
            {
                Line::On {
                    branch,
                    tip: Some(commit),
                }
            } else {
                Line::At {
                    commit,
                    mark: to.mark,
                }
            }
        } else {
            Line::At {
                commit,
                mark: to.mark,
            }
        };
        here::move_head(dir, &self.line, summary).map_err(tree_error)?;
        self.base = None;
        if let Line::On { branch, tip } = &self.line {
            self.importer = Some(Importer::new(
                dir,
                &Planted {
                    branch: format!("refs/heads/{branch}"),
                    tip: tip.clone(),
                    through: self.through,
                },
            )?);
        }
        if let Some(seq) = seq {
            self.through = Some(seq);
            tell(news, News::Committed { through: seq });
        }
        Ok(())
    }

    fn item(
        &mut self,
        item: TreeItem,
        dir: &Path,
        news: &mpsc::Sender<News>,
    ) -> Result<(), TreeError> {
        if let Some(ticket) = item.ticket() {
            return self.answer(item, ticket, dir, news);
        }
        let (commit, seq, at) = match item {
            TreeItem::Moved { to } => {
                return self.moved(dir, news, to, None, "the wall moved to a moment");
            }
            TreeItem::SaveMark { name, at } => {
                return self.save_mark(dir, news, &name, &at);
            }
            TreeItem::Change {
                change,
                files,
                generation,
            } => {
                if let What::Go { to, .. } = &change.what {
                    return self.moved(dir, news, to.clone(), Some(change.seq), &change.summary);
                }
                if generation != self.accepted {
                    return Ok(());
                }
                let mut commit = Commit::change(&change, files)?;
                if let Some(mut base) = self.base.take() {
                    for (path, bytes) in commit.files {
                        if let Some(bytes) = bytes {
                            base.insert(path, bytes);
                        } else {
                            base.remove(&path);
                        }
                    }
                    commit.files = base.into_iter().map(|(p, b)| (p, Some(b))).collect();
                    commit.whole = true;
                }
                (commit, change.seq, change.at.clone())
            }
            TreeItem::Whole {
                kind,
                seq,
                clock,
                files,
                generation,
            } => {
                self.checkpoint(dir, news)?;
                self.accepted = generation;
                if matches!(kind, WholeKind::Start) {
                    if let Line::At { commit, .. } = &self.line {
                        if here::files(dir, commit).map_err(tree_error)? == files {
                            self.base = Some(files);
                            self.through = Some(seq);
                            tell(news, News::Committed { through: seq });
                            return Ok(());
                        }
                    }
                }
                if matches!(kind, WholeKind::Gap)
                    && self.through.is_some_and(|through| seq <= through)
                {
                    self.base = Some(files);
                    tell(
                        news,
                        News::Committed {
                            through: self.through.unwrap_or(0),
                        },
                    );
                    return Ok(());
                }
                self.base = None;
                let kind = match kind {
                    WholeKind::Start => "start",
                    WholeKind::Gap => "gap",
                };
                let commit = Commit::whole(kind, seq, clock.as_ref(), files, self.through);
                if seq > self.through.unwrap_or(0) {
                    self.gaps.push((self.through.unwrap_or(0) + 1, seq));
                }
                (commit, seq, crate::change::utc_now())
            }
            TreeItem::Finish => return Ok(()),
            TreeItem::Mark { .. }
            | TreeItem::Resolve { .. }
            | TreeItem::Here { .. }
            | TreeItem::List { .. } => {
                return Err(TreeError::Git("a tree question lost its ticket".into()));
            }
        };
        self.grow(dir, &at)?;
        self.summary = commit.message.lines().next().unwrap_or("").into();
        let mark = self.importer()?.commit(&commit)?;
        if let Line::On { tip, .. } = &mut self.line {
            *tip = Some(format!(":{mark}"));
        }
        self.last = Some((mark, seq));
        self.pending += 1;
        Ok(())
    }
    fn take(
        &mut self,
        item: Result<TreeItem, RecvTimeoutError>,
        dir: &Path,
        news: &mpsc::Sender<News>,
        waiting: &AtomicUsize,
    ) -> Result<(), TreeError> {
        item.map_or(Ok(()), |item| self.item(item, dir, news))?;
        if self.pending >= CHECKPOINT_COMMITS
            || (waiting.load(Ordering::Acquire) == 0
                && self.checkpointed.elapsed() >= CHECKPOINT_AFTER)
        {
            self.checkpoint(dir, news)?;
        }
        Ok(())
    }
}

type Movement = (TreePlace, Option<u64>, String);

fn remember(
    item: &TreeItem,
    moved: &mut Option<Movement>,
    down: Option<(&str, Option<u64>)>,
    news: &mpsc::Sender<News>,
) {
    match item {
        TreeItem::Moved { to } => {
            *moved = Some((to.clone(), None, "the wall moved to a moment".into()));
        }
        TreeItem::Change { change, .. } => {
            if let What::Go { to, .. } = &change.what {
                *moved = Some((to.clone(), Some(change.seq), change.summary.clone()));
            }
        }
        TreeItem::SaveMark { name, .. } if down.is_some() => {
            if let Some((why, through)) = down {
                tell(
                    news,
                    News::Down {
                        error: format!(
                            "the save \"{name}\" could not be marked in the tree: {why}"
                        ),
                        through,
                    },
                );
            }
        }
        _ => {}
    }
    if let Some((down, through)) = down {
        if let Some(ticket) = item.ticket() {
            tell(
                news,
                News::Answer {
                    ticket,
                    through: through.unwrap_or(0),
                    answer: Err(HereError::Down(down.into())),
                },
            );
        }
    }
}

fn restart(
    dir: &Path,
    through: Option<u64>,
    moved: Option<&Movement>,
    news: &mpsc::Sender<News>,
) -> Result<Writer, TreeError> {
    let (line, read_through) = here::read_line(dir).map_err(tree_error)?;
    let mut writer = Writer::new(dir, line, read_through.or(through))?;
    if let Some((to, seq, summary)) = moved {
        writer.moved(dir, news, to.clone(), *seq, summary)?;
    }
    Ok(writer)
}

fn worker(
    dir: &Path,
    line: Line,
    initial_through: Option<u64>,
    receiver: &Receiver<TreeItem>,
    news: &mpsc::Sender<News>,
    bytes: &AtomicUsize,
    waiting: &AtomicUsize,
) {
    let mut through = initial_through;
    let mut down = String::from("the tree is stopped");
    let mut moved = None;
    let mut writer = match Writer::new(dir, line, through) {
        Ok(writer) => Some(writer),
        Err(err) => {
            down = err.to_string();
            tell(
                news,
                News::Down {
                    error: err.to_string(),
                    through,
                },
            );
            None
        }
    };
    let mut retry = Instant::now() + RETRY_AFTER;
    loop {
        let timeout = writer.as_ref().map_or_else(
            || retry.saturating_duration_since(Instant::now()),
            |w| CHECKPOINT_AFTER.saturating_sub(w.checkpointed.elapsed()),
        );
        let outcome = receiver.recv_timeout(timeout);
        let finished = match &outcome {
            Ok(item) => here::released(item, bytes, waiting),
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        };
        if finished {
            let result = writer.take().map_or_else(
                || Err("the tree is stopped".into()),
                |w| w.finish(dir, news),
            );
            tell(news, News::Finished(result));
            return;
        }
        let arrived = match &outcome {
            Ok(item) => Some(item),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if let Some(item) = arrived {
            remember(
                item,
                &mut moved,
                writer.is_none().then_some((down.as_str(), through)),
                news,
            );
        }
        if let Some(w) = writer.as_mut() {
            // `outcome` is an item or a timeout here: a closed queue
            // finished above.
            let result = w.take(outcome, dir, news, waiting);
            through = w.through;
            if let Err(err) = result {
                down = err.to_string();
                tell(
                    news,
                    News::Down {
                        error: err.to_string(),
                        through,
                    },
                );
                writer = None;
                retry = Instant::now() + RETRY_AFTER;
            } else {
                moved = None;
                if w.pending == 0 && w.checkpointed.elapsed() >= CHECKPOINT_AFTER {
                    w.checkpointed = Instant::now();
                }
            }
        } else if Instant::now() >= retry {
            match restart(dir, through, moved.as_ref(), news) {
                Ok(w) => {
                    through = w.through;
                    moved = None;
                    writer = Some(w);
                    tell(news, News::NeedWhole);
                }
                Err(err) => {
                    down = err.to_string();
                    tell(
                        news,
                        News::Down {
                            error: err.to_string(),
                            through,
                        },
                    );
                    retry = Instant::now() + RETRY_AFTER;
                }
            }
        }
    }
}

/// Changed paths and their optional replacement bytes.
pub type FileDiff = Vec<(String, Option<Vec<u8>>)>;

/// The wall's files (T01's `to_files`) compared with `mirror`.
///
/// Each new or changed file comes with its bytes, each gone one with
/// `None`; then `mirror` becomes the new files. Rack moves and groups ride in `wall.toml`.
///
/// # Errors
///
/// When the wall does not turn into files.
pub fn files_diff(
    file: &crate::patch::PatchFile,
    mirror: &mut Files,
) -> Result<FileDiff, TreeError> {
    let files = super::to_files(file)?;
    let mut diff: Vec<_> = files
        .iter()
        .filter(|(path, bytes)| mirror.get(*path) != Some(*bytes))
        .map(|(p, b)| (p.clone(), Some(b.clone())))
        .collect();
    diff.extend(
        mirror
            .keys()
            .filter(|path| !files.contains_key(*path))
            .map(|p| (p.clone(), None)),
    );
    *mirror = files;
    Ok(diff)
}

#[cfg(test)]
mod tests;
