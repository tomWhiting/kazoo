//! A bounded control-thread feed to the sole tree writer.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::git::{self, Commit, Importer, Planted};
use super::{Files, TreeError};
use crate::protocol::{Change, ClockStamp, TreeHealth};

/// Items the queue holds. The log's busiest second held 30 changes and its
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
/// Each checkpoint writes one pack; a geometric repack after every 64
/// keeps their number logarithmic without rewriting everything.
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
    /// The answer to [`TreeItem::Finish`].
    Finished(Result<(), String>),
}

impl TreeItem {
    fn seq(&self) -> Option<u64> {
        match self {
            Self::Change { change, .. } => Some(change.seq),
            Self::Whole { seq, .. } => Some(*seq),
            Self::Finish => None,
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
                Self::Finish => 0,
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
        let planted = git::read(dir)?;
        let through = planted.through;
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
                    &planted,
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
        })
    }

    /// Queue `item` without ever blocking. When the queue is full (by
    /// count or bytes) or the thread has gone, the item is dropped, the
    /// tree is marked behind from its change, and a whole item must follow.
    pub fn offer(&mut self, item: TreeItem) -> bool {
        let seq = item.seq();
        let bytes = match item.bytes() {
            Ok(bytes) => bytes,
            Err(err) => {
                self.error = Some(err.to_string());
                self.missed(seq);
                return false;
            }
        };
        let previous = self.bytes.fetch_add(bytes, Ordering::AcqRel);
        if previous.saturating_add(bytes) > TREE_QUEUE_BYTES {
            self.bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.missed(seq);
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
                self.missed(seq);
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

/// The tree thread's state while git works.
struct Writer {
    importer: Option<Importer>,
    accepted: u64,
    through: Option<u64>,
    last: Option<(u64, u64)>,
    pending: u64,
    checkpointed: Instant,
    checkpoints: u64,
    gaps: Vec<(u64, u64)>,
    base: Option<Files>,
}

impl Writer {
    fn new(dir: &Path, planted: &Planted) -> Result<Self, TreeError> {
        Ok(Self {
            importer: Some(Importer::new(dir, planted)?),
            accepted: 0,
            through: planted.through,
            last: None,
            pending: 0,
            checkpointed: Instant::now(),
            checkpoints: 0,
            gaps: Vec::new(),
            base: None,
        })
    }

    fn importer(&mut self) -> Result<&mut Importer, TreeError> {
        self.importer
            .as_mut()
            .ok_or_else(|| TreeError::Git("no fast-import is running".into()))
    }

    fn checkpoint(&mut self, dir: &Path, news: &mpsc::Sender<News>) -> Result<(), TreeError> {
        if self.pending == 0 {
            return Ok(());
        }
        if let Some((mark, seq)) = self.last {
            self.importer()?.checkpoint(mark)?;
            self.through = Some(seq);
            self.pending = 0;
            self.checkpointed = Instant::now();
            tell(news, News::Committed { through: seq });
            for (first, last) in self.gaps.drain(..) {
                tell(news, News::Gap { first, last });
            }
            self.checkpoints += 1;
            if self.checkpoints % REPACK_EVERY == 0 {
                git::repack(dir)?;
            }
        }
        Ok(())
    }

    fn finish(mut self, dir: &Path, news: &mpsc::Sender<News>) -> Result<(), String> {
        self.checkpoint(dir, news)
            .and_then(|()| self.importer.take().map_or(Ok(()), git::Importer::finish))
            .map_err(|err| err.to_string())
    }

    fn item(
        &mut self,
        item: TreeItem,
        dir: &Path,
        news: &mpsc::Sender<News>,
    ) -> Result<(), TreeError> {
        let (commit, seq) = match item {
            TreeItem::Change {
                change,
                files,
                generation,
            } => {
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
                (commit, change.seq)
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
                // A gap item that covers nothing the tree lacks is the base
                // of the next change commit, not a commit; the tree already
                // holds every change through `through`, so say so.
                if let (WholeKind::Gap, Some(through)) = (kind, self.through) {
                    if seq <= through {
                        self.base = Some(files);
                        tell(news, News::Committed { through });
                        return Ok(());
                    }
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
                (commit, seq)
            }
            TreeItem::Finish => return Ok(()),
        };
        let mark = self.importer()?.commit(&commit)?;
        self.last = Some((mark, seq));
        self.pending += 1;
        Ok(())
    }
}

fn worker(
    dir: &Path,
    planted: &Planted,
    receiver: &Receiver<TreeItem>,
    news: &mpsc::Sender<News>,
    bytes: &AtomicUsize,
    waiting: &AtomicUsize,
) {
    let mut through = planted.through;
    let mut writer = match Writer::new(dir, planted) {
        Ok(writer) => Some(writer),
        Err(err) => {
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
            Ok(item) => {
                match item.bytes() {
                    Ok(size) => {
                        bytes.fetch_sub(size, Ordering::AcqRel);
                    }
                    Err(err) => eprintln!("kazoo-wall: tree item accounting: {err}"),
                }
                waiting.fetch_sub(1, Ordering::AcqRel);
                matches!(item, TreeItem::Finish)
            }
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
        if let Some(w) = writer.as_mut() {
            // `outcome` is an item or a timeout here: a closed queue
            // finished above.
            let result = outcome
                .map_or(Ok(()), |item| w.item(item, dir, news))
                .and_then(|()| {
                    if w.pending >= CHECKPOINT_COMMITS
                        || (waiting.load(Ordering::Acquire) == 0
                            && w.checkpointed.elapsed() >= CHECKPOINT_AFTER)
                    {
                        w.checkpoint(dir, news)
                    } else {
                        Ok(())
                    }
                });
            through = w.through;
            if let Err(err) = result {
                tell(
                    news,
                    News::Down {
                        error: err.to_string(),
                        through,
                    },
                );
                writer = None;
                retry = Instant::now() + RETRY_AFTER;
            } else if w.pending == 0 && w.checkpointed.elapsed() >= CHECKPOINT_AFTER {
                w.checkpointed = Instant::now();
            }
        } else if Instant::now() >= retry {
            match git::read(dir).and_then(|mut planted| {
                if planted.through.is_none() {
                    planted.through = through;
                }
                Writer::new(dir, &planted)
            }) {
                Ok(w) => {
                    writer = Some(w);
                    tell(news, News::NeedWhole);
                }
                Err(err) => {
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

/// The wall's files (T01's `to_files`) compared with `mirror`: each new or
/// changed file with its bytes, each gone one with `None`; then `mirror`
/// becomes the new files. Rack moves and groups ride in `wall.toml`.
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
