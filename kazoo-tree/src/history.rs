//! The tree read from git: every moment the wall has been, the limbs they
//! grew on, the marks and saves hung on them, and where the wall is now.
//!
//! The tree only branches and never merges (KZ-T02b, KZ-T04b), so a moment
//! has at most one parent, and every limb is a run of moments grown from a
//! moment on an older limb.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use kazoo_wall::tree::git::git_command;

use crate::colour::{self, Kinds};
use kazoo_wall::tree::thread::here::name_of;

/// What went wrong reading the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// Git could not run, or answered with a failure.
    Git(String),
    /// The tree holds something it never should.
    Broken(String),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(text) | Self::Broken(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for HistoryError {}

/// What the wall sounded like at a moment (KZ-T08c's `Kazoo-Sound`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sound {
    /// RMS level, dBFS.
    pub rms_db: f32,
    /// Peak level, dBFS.
    pub peak_db: f32,
    /// Spectral centroid, Hz.
    pub centroid_hz: f32,
    /// Share of energy below 250 Hz.
    pub low: f32,
    /// Share from 250 Hz to 4 kHz.
    pub mid: f32,
    /// Share above 4 kHz.
    pub high: f32,
    /// Onsets per second.
    pub onsets: f32,
}

/// One moment: one commit in the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Moment {
    /// The commit, 40 hex.
    pub id: Arc<str>,
    /// The moment it grew from.
    pub parent: Option<usize>,
    /// Committer time, unix seconds.
    pub time: i64,
    /// Who made it: a seat, or `kazoo-wall` for the wall's own commits.
    pub seat: String,
    /// The change in words.
    pub summary: String,
    /// `change`, `start`, `gap`, `settle`, … (`Kazoo-Kind`).
    pub kind: String,
    /// The change's number in the wall's log.
    pub seq: Option<u64>,
    /// Beats since the wall's clock started, when the moment was made.
    pub beat: Option<f64>,
    /// Tempo, BPM, when it was made.
    pub tempo: Option<f64>,
    /// What it sounded like, when the commit says.
    pub sound: Option<Sound>,
    /// Modules alive at this moment.
    pub modules: u32,
    /// Modules alive at this moment, by family (its colour).
    pub alive: Kinds,
    /// Modules this moment turned, repatched, added or took away, by
    /// family: what was being played with.
    pub touched: Kinds,
    /// Several changes sent at once with a note, when the moment is one.
    pub batch: Option<Box<Batch>>,
    /// What did not read in its commit: said in the details, never a
    /// reason not to open the tree.
    pub problems: Vec<String>,
}

/// Several changes a seat or Tom sent at once, with a note like a commit
/// message (`Kazoo-Kind: batch`, KZ-T10a): one moment on the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The sender's note (`Kazoo-Note`).
    pub note: String,
    /// Each change in it, in words, in the order applied (`Kazoo-Step`).
    pub steps: Vec<String>,
}

/// A run of moments grown from one moment of an older limb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limb {
    /// The branch's name: `trunk`, `bud/…`.
    pub name: String,
    /// Its own moments, oldest first.
    pub moments: Vec<usize>,
    /// The moment it grew from, on an older limb.
    pub from: Option<usize>,
}

/// A bookmark hung on a moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// Its name, as people wrote it.
    pub name: String,
    /// A save's mark (`save/`) rather than one made by hand (`mark/`).
    pub save: bool,
    /// The moment it hangs on.
    pub moment: usize,
}

/// The whole tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct History {
    /// Every moment, each after the one it grew from. Shared, so a tree
    /// handed to the screen costs a count each, never a copy.
    pub moments: Vec<Arc<Moment>>,
    /// The trunk first, then buds in the order they grew.
    pub limbs: Vec<Limb>,
    /// For every moment on a limb, that limb and the moment's place on it.
    pub place: Vec<Option<(usize, usize)>>,
    /// Every mark and save.
    pub marks: Vec<Mark>,
    /// Every moment that is a batch, oldest first: the knots, found
    /// without looking at every moment.
    pub batches: Vec<usize>,
    /// The moment the wall is at.
    pub here: Option<usize>,
    /// What did not read in the tree's refs.
    pub problems: Vec<String>,
    /// The branches made by hand, in the order they claimed their moments:
    /// what the ledger keeps, so a new branch never takes an older one's.
    pub by_hand: Vec<String>,
    /// Each commit's moment.
    index: HashMap<Arc<str>, usize>,
    /// The tips read so far: everything behind them is held.
    tips: Vec<String>,
}

const RECORD: u8 = 0x1e;
const BODY_END: char = '\u{1f}';

/// The log's format: commit, parents, committer time, author, subject, and
/// only the wall's own trailers (never the whole message: a `Kazoo-Change`
/// trailer holds a page of JSON, and a million of them would be most of
/// the read).
const LOG_FORMAT: &str = "--format=%x1e%H%x00%P%x00%ct%x00%an%x00%s%x00%(trailers:key=Kazoo-Kind,key=Kazoo-Seq,key=Kazoo-Beat,key=Kazoo-Tempo,key=Kazoo-Sound,key=Kazoo-Note,key=Kazoo-Step,unfold)%x1f";

/// Git as the tree reads it: the wall's own way to run git, with the
/// person's own settings that would change what `log` says held fixed.
/// `log.showRoot=false` would drop a root commit's files, and with them
/// every module count; a signature or colour setting would mix other lines
/// into the answer; a quoted path would hide a module's file; another
/// trailer separator would hide every trailer. The person's own and the
/// system's settings are not read at all, and git's replacements are
/// never followed: the tree is read as the wall wrote it.
fn git(dir: &Path, name: &str) -> Command {
    let mut command = git_command(name, Some(dir));
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    let settings = [
        ("trailer.separators", ":"),
        ("log.showRoot", "true"),
        ("log.showSignature", "false"),
        ("color.ui", "never"),
        ("core.quotePath", "false"),
        ("i18n.logOutputEncoding", "UTF-8"),
    ];
    command.env("GIT_CONFIG_COUNT", settings.len().to_string());
    for (i, (key, value)) in settings.iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{i}"), key);
        command.env(format!("GIT_CONFIG_VALUE_{i}"), value);
    }
    command
}

fn run(command: &mut Command) -> Result<String, HistoryError> {
    let output = command
        .output()
        .map_err(|err| HistoryError::Git(format!("git could not run: {err}")))?;
    answer(command, output)
}

fn answer(command: &Command, output: std::process::Output) -> Result<String, HistoryError> {
    let bytes = answer_bytes(command, output, None)?;
    String::from_utf8(bytes).map_err(|err| {
        HistoryError::Broken(format!("git answered in bytes that are not text: {err}"))
    })
}

/// Git's answer as bytes, or why it failed. Git's own failure is said
/// first; a failure to write its input (`written`) is said after it, since
/// git stopping early is what breaks the pipe.
fn answer_bytes(
    command: &Command,
    output: std::process::Output,
    written: Option<io::Error>,
) -> Result<Vec<u8>, HistoryError> {
    if !output.status.success() {
        let also = written.map_or_else(String::new, |err| {
            format!("; and its input could not all be written: {err}")
        });
        return Err(HistoryError::Git(format!(
            "git failed ({:?}): {}{also}",
            command.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if let Some(err) = written {
        return Err(HistoryError::Git(format!(
            "git's input could not be written: {err}"
        )));
    }
    Ok(output.stdout)
}

/// Run `command` with `input` on its standard input. The input is written
/// from its own thread, so a long list of tips can never wait on git
/// while git waits on its answer being read.
fn run_with(command: &mut Command, input: &str) -> Result<Vec<u8>, HistoryError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| HistoryError::Git(format!("git could not run: {err}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| HistoryError::Git("git took no input".to_owned()))?;
    let (output, written) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let mut stdin = stdin;
            stdin.write_all(input.as_bytes())
        });
        let output = child.wait_with_output();
        let written = writer
            .join()
            .map_err(|_| HistoryError::Git("the thread writing to git panicked".to_owned()))?;
        let output = output
            .map_err(|err| HistoryError::Git(format!("git could not be waited for: {err}")))?;
        Ok::<_, HistoryError>((output, written.err()))
    })?;
    answer_bytes(command, output, written)
}

/// The refs the tree is made of: the wall's branches, marks and saves.
/// Nothing else (notes, a stash, a remote's copy) is a moment of the wall.
const REF_BRANCHES: &str = "refs/heads/";
const REF_MARKS: &str = "refs/tags/mark/";
const REF_SAVES: &str = "refs/tags/save/";

/// Every ref and HEAD, read together before the log.
///
/// The log is then read for exactly these tips and always holds every commit they name. Also
/// what is compared every few seconds to see whether the tree has grown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// `for-each-ref`'s answer: name, object, and a tag's commit.
    refs: String,
    /// HEAD's commit.
    head: Option<String>,
}

impl Snapshot {
    /// Read the refs and HEAD of the tree at `dir`.
    ///
    /// # Errors
    ///
    /// When git cannot run.
    pub fn read(dir: &Path) -> Result<Self, HistoryError> {
        // HEAD first: the wall may commit between the two reads, and then
        // HEAD is behind its branch's tip, on the branch, never past it.
        let head = head(dir)?;
        let refs = run(git(dir, "for-each-ref").args([
            "--format=%(refname)%00%(objectname)%00%(*objectname)",
            REF_BRANCHES,
            REF_MARKS,
            REF_SAVES,
        ]))?;
        Ok(Self { refs, head })
    }

    /// Every commit a ref or HEAD names, once each.
    fn tips(&self) -> Vec<String> {
        let mut tips: Vec<String> = read_refs(&self.refs)
            .map(|r| r.commit.to_owned())
            .chain(self.head.clone())
            .collect();
        tips.sort_unstable();
        tips.dedup();
        tips
    }
}

impl History {
    /// Read the tree at `dir` (a bare repository), whole.
    ///
    /// # Errors
    ///
    /// When git cannot run, or the tree holds a merge, a moment whose parent
    /// it does not hold, or a branch naming a commit it does not hold.
    pub fn read(dir: &Path) -> Result<Self, HistoryError> {
        let snapshot = Snapshot::read(dir)?;
        let mut history = Self::default();
        history.grow(dir, &snapshot, &[])?;
        Ok(history)
    }

    /// Bring the tree up to `snapshot`: read only the commits not already
    /// held (everything behind a tip read before is held already, since a
    /// branch only grows), then lay the limbs, marks and HEAD out again.
    /// Answers how many moments were added.
    ///
    /// # Errors
    ///
    /// As [`History::read`]. A failure leaves every moment already held
    /// valid, and the next grow reads again from the last good tips.
    pub fn grow(
        &mut self,
        dir: &Path,
        snapshot: &Snapshot,
        known: &[String],
    ) -> Result<usize, HistoryError> {
        let tips = snapshot.tips();
        let added = if tips.is_empty() {
            0
        } else {
            let mut input = tips.join("\n");
            if !self.tips.is_empty() {
                input.push_str("\n--not\n");
                input.push_str(&self.tips.join("\n"));
            }
            input.push('\n');
            let log = run_with(
                git(dir, "log").args([
                    "--stdin",
                    "--topo-order",
                    "--reverse",
                    "--root",
                    "--no-renames",
                    "--no-show-signature",
                    "--no-color",
                    "--encoding=UTF-8",
                    "--name-status",
                    LOG_FORMAT,
                ]),
                &input,
            )?;
            self.append(&log)?
        };
        self.lay_out(&snapshot.refs, snapshot.head.as_deref(), known)?;
        self.tips = tips;
        Ok(added)
    }

    /// Build the tree from git's own words: `log` as `grow` asks for it,
    /// `refs` from `for-each-ref`, and HEAD's commit.
    ///
    /// # Errors
    ///
    /// When a moment has two parents or names a parent not seen before it,
    /// or a branch names a commit the log did not hold.
    pub fn from_git(log: &str, refs: &str, here: Option<&str>) -> Result<Self, HistoryError> {
        let mut history = Self::default();
        history.append(log.as_bytes())?;
        history.lay_out(refs, here, &[])?;
        Ok(history)
    }

    /// Lay the limbs out again from `refs`, the branches made by hand that
    /// claimed before (`known`) claiming in that order.
    ///
    /// # Errors
    ///
    /// As [`History::from_git`].
    #[cfg(test)]
    pub(crate) fn lay_out_with(
        &mut self,
        refs: &str,
        here: Option<&str>,
        known: &[String],
    ) -> Result<(), HistoryError> {
        self.lay_out(refs, here, known)
    }

    /// Add the moments in `log`, all or none. A moment already held is
    /// skipped, so reading a commit twice never doubles it.
    fn append(&mut self, log: &[u8]) -> Result<usize, HistoryError> {
        let mut fresh: Vec<Moment> = Vec::new();
        let mut fresh_index: HashMap<Arc<str>, usize> = HashMap::new();
        for bytes in log.split(|&byte| byte == RECORD) {
            // Each record is read on its own, so one commit's bad bytes
            // are that commit's problem, never the whole tree's.
            let record = String::from_utf8_lossy(bytes);
            if record.trim().is_empty() {
                continue;
            }
            let mut moment = moment(
                &record,
                |id| {
                    self.index
                        .get(id)
                        .copied()
                        .or_else(|| fresh_index.get(id).map(|&i| self.moments.len() + i))
                },
                |m| {
                    let at = if m < self.moments.len() {
                        &self.moments[m]
                    } else {
                        &fresh[m - self.moments.len()]
                    };
                    (at.modules, at.alive)
                },
            )?;
            if self.index.contains_key(&moment.id) || fresh_index.contains_key(&moment.id) {
                continue;
            }
            if matches!(record, std::borrow::Cow::Owned(_)) {
                moment.problems.push(
                    "its words are not all UTF-8; what could not be read shows as \u{fffd}"
                        .to_owned(),
                );
            }
            fresh_index.insert(Arc::clone(&moment.id), fresh.len());
            fresh.push(moment);
        }
        let added = fresh.len();
        let base = self.moments.len();
        self.batches.extend(
            fresh
                .iter()
                .enumerate()
                .filter(|(_, moment)| moment.batch.is_some())
                .map(|(i, _)| base + i),
        );
        for (id, i) in fresh_index {
            self.index.insert(id, base + i);
        }
        self.moments.extend(fresh.into_iter().map(Arc::new));
        Ok(added)
    }

    /// Lay the limbs, marks, problems and the wall's place out again from
    /// the refs and HEAD.
    fn lay_out(
        &mut self,
        refs: &str,
        here: Option<&str>,
        known: &[String],
    ) -> Result<(), HistoryError> {
        let mut problems = Vec::new();
        let (limbs, by_hand) = limbs(refs, &self.index, &self.moments, known, &mut problems)?;
        let mut place = vec![None; self.moments.len()];
        for (l, limb) in limbs.iter().enumerate() {
            for (at, &m) in limb.moments.iter().enumerate() {
                place[m] = Some((l, at));
            }
        }
        let marks = marks(refs, &self.index, &mut problems);
        let here = match here {
            None => None,
            Some(id) => {
                let found = self.index.get(id).copied();
                if found.is_none() {
                    problems.push(format!(
                        "the wall's place, {id:.10}, is a commit the tree does not hold"
                    ));
                }
                found
            }
        };
        self.limbs = limbs;
        self.by_hand = by_hand;
        self.place = place;
        self.marks = marks;
        self.here = here;
        self.problems = problems;
        Ok(())
    }

    /// Add a moment made by hand (the tests' made-up trees), on no limb
    /// until one claims it.
    #[cfg(test)]
    pub(crate) fn push(&mut self, moment: Moment) -> usize {
        let m = self.moments.len();
        if moment.batch.is_some() {
            self.batches.push(m);
        }
        self.index.insert(Arc::clone(&moment.id), m);
        self.moments.push(Arc::new(moment));
        self.place.push(None);
        m
    }

    /// The moment that is `commit`.
    #[must_use]
    pub fn moment_of(&self, commit: &str) -> Option<usize> {
        self.index.get(commit).copied()
    }

    /// The limb a moment is on, and its place there.
    #[must_use]
    pub fn place_of(&self, moment: usize) -> Option<(usize, usize)> {
        self.place.get(moment).copied().flatten()
    }

    /// Every problem found reading the tree: the tree's own, and each
    /// moment's.
    #[must_use]
    pub fn problem_count(&self) -> usize {
        self.problems.len() + self.moments.iter().map(|m| m.problems.len()).sum::<usize>()
    }
}

/// HEAD's commit; `None` for a tree with no moment yet, which `rev-parse
/// --verify -q` answers with exit 1 and nothing on either stream.
fn head(dir: &Path) -> Result<Option<String>, HistoryError> {
    let mut command = git(dir, "rev-parse");
    command.args(["-q", "--verify", "HEAD^{commit}"]);
    let output = command
        .output()
        .map_err(|err| HistoryError::Git(format!("git could not run: {err}")))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        return Ok(Some(text));
    }
    if output.status.code() == Some(1) && text.is_empty() && output.stderr.is_empty() {
        return Ok(None);
    }
    Err(HistoryError::Git(format!(
        "git could not read the tree's HEAD: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Text from a commit as it may be shown: a control character (an escape
/// that would drive the terminal, a tab, a newline) becomes `�`.
#[must_use]
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// One log record as a moment. `index` finds a parent's place, `alive_at`
/// the modules alive at a place, all of them and by family.
fn moment(
    record: &str,
    index: impl Fn(&str) -> Option<usize>,
    alive_at: impl Fn(usize) -> (u32, Kinds),
) -> Result<Moment, HistoryError> {
    let (head, files) = record.split_once(BODY_END).ok_or_else(|| {
        HistoryError::Broken(format!("a log record without its end: {record:.80}"))
    })?;
    let mut fields = head.splitn(6, '\0');
    let mut field = |what: &str| {
        fields.next().ok_or_else(|| {
            HistoryError::Broken(format!("a log record without its {what}: {head:.80}"))
        })
    };
    let id: Arc<str> = Arc::from(field("commit")?.trim());
    let parents = field("parents")?;
    let time = field("time")?;
    let seat = clean(field("author")?);
    let summary = clean(field("subject")?);
    let trailers = field("trailers")?;
    let mut parents = parents.split_whitespace();
    let parent = match (parents.next(), parents.next()) {
        (None, _) => None,
        (Some(one), None) => Some(index(one).ok_or_else(|| {
            HistoryError::Broken(format!(
                "{id:.10} grew from {one:.10}, which the tree does not hold before it"
            ))
        })?),
        (Some(_), Some(_)) => {
            return Err(HistoryError::Broken(format!(
                "{id:.10} has two parents; the tree never merges"
            )));
        }
    };
    let time = time
        .trim()
        .parse::<i64>()
        .map_err(|err| HistoryError::Broken(format!("{id:.10}'s time does not read: {err}")))?;
    let mut problems = Vec::new();
    let mut kind = String::new();
    let mut seq = None;
    let mut beat = None;
    let mut tempo = None;
    let mut sound = None;
    let mut note = None;
    let mut steps = Vec::new();
    for line in trailers.lines() {
        if let Some(value) = line.strip_prefix("Kazoo-Kind: ") {
            kind = clean(value.trim());
        } else if let Some(value) = line.strip_prefix("Kazoo-Seq: ") {
            seq = keep(&mut problems, number("Kazoo-Seq", value));
        } else if let Some(value) = line.strip_prefix("Kazoo-Beat: ") {
            beat = keep(&mut problems, number("Kazoo-Beat", value));
        } else if let Some(value) = line.strip_prefix("Kazoo-Tempo: ") {
            tempo = keep(&mut problems, number("Kazoo-Tempo", value));
        } else if let Some(value) = line.strip_prefix("Kazoo-Sound: ") {
            sound = keep(&mut problems, sound_of(value));
        } else if let Some(value) = line.strip_prefix("Kazoo-Note: ") {
            note = Some(clean(value.trim()));
        } else if let Some(value) = line.strip_prefix("Kazoo-Step: ") {
            steps.push(clean(value.trim()));
        }
    }
    let batch = batch_of(&kind, &summary, note, steps, &mut problems);
    let (before, alive) = parent.map_or_else(|| (0, Kinds::default()), &alive_at);
    let (modules, alive, touched) = files_of(files, before, alive, &mut problems);
    Ok(Moment {
        id,
        parent,
        time,
        seat,
        summary,
        kind,
        seq,
        beat,
        tempo,
        sound,
        modules,
        alive,
        touched,
        batch,
        problems,
    })
}

/// A moment's batch, from its kind and its `Kazoo-Note` and `Kazoo-Step`
/// trailers. A batch with no note is shown by its subject (the wall
/// writes it `{seat}: {note}`), and with no steps as one with none; each
/// is said in `problems`. A note or steps on a moment that is not a batch
/// are kept: the words are the sender's, and nothing is thrown away.
fn batch_of(
    kind: &str,
    summary: &str,
    note: Option<String>,
    steps: Vec<String>,
    problems: &mut Vec<String>,
) -> Option<Box<Batch>> {
    let batch = kind == "batch";
    if !batch && note.is_none() && steps.is_empty() {
        return None;
    }
    if batch && steps.is_empty() {
        problems.push("a batch whose changes are not listed (no Kazoo-Step)".to_owned());
    }
    let note = note.unwrap_or_else(|| {
        if batch {
            problems.push("a batch without its note (no Kazoo-Note)".to_owned());
        }
        summary.to_owned()
    });
    Some(Box::new(Batch { note, steps }))
}

/// What a record's files did to the modules: how many are alive after it
/// (from `before`), by family too (from `alive`), and which families it
/// touched. Anything that does not add up is said in `problems`.
fn files_of(
    files: &str,
    before: u32,
    mut alive: Kinds,
    problems: &mut Vec<String>,
) -> (u32, Kinds, Kinds) {
    let mut modules = i64::from(before);
    let mut touched = Kinds::default();
    let mut missing = false;
    for line in files.lines() {
        let mut parts = line.splitn(2, '\t');
        let (Some(status), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(file) = path.strip_prefix("modules/") else {
            continue;
        };
        let kind = colour::kind_of(file.trim_end().trim_end_matches(".toml"));
        match status.trim() {
            "A" => {
                modules += 1;
                alive.add(kind);
            }
            "D" => {
                modules -= 1;
                missing |= !alive.remove(kind);
            }
            _ => {}
        }
        touched.add(kind);
    }
    if missing {
        problems.push(
            "takes away a module whose kind was not alive before it; its colour leaves it out"
                .to_owned(),
        );
    }
    if modules < 0 {
        problems.push(format!(
            "removes more modules than were alive ({before} alive, {modules} after); counted as none"
        ));
    }
    (
        u32::try_from(modules.max(0)).unwrap_or(u32::MAX),
        alive,
        touched,
    )
}

/// A trailer's value, or `None` with what was wrong said in `problems`.
fn keep<T>(problems: &mut Vec<String>, result: Result<T, String>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(problem) => {
            problems.push(problem);
            None
        }
    }
}

fn number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String>
where
    T::Err: fmt::Display,
{
    value
        .trim()
        .parse()
        .map_err(|err| format!("its {key} does not read ({}): {err}", clean(value.trim())))
}

/// Read `rms=-27.2 peak=-9.9 centroid=599 low=0.284 mid=0.712 high=0.004 onsets=3.0`.
fn sound_of(value: &str) -> Result<Sound, String> {
    let mut found: HashMap<&str, f32> = HashMap::new();
    for pair in value.split_whitespace() {
        let (key, number_text) = pair
            .split_once('=')
            .ok_or_else(|| format!("its Kazoo-Sound has {}, not key=value", clean(pair)))?;
        found.insert(key, number("Kazoo-Sound", number_text)?);
    }
    let get = |key: &str| {
        found
            .get(key)
            .copied()
            .ok_or_else(|| format!("its Kazoo-Sound has no {key}"))
    };
    Ok(Sound {
        rms_db: get("rms")?,
        peak_db: get("peak")?,
        centroid_hz: get("centroid")?,
        low: get("low")?,
        mid: get("mid")?,
        high: get("high")?,
        onsets: get("onsets")?,
    })
}

struct Ref<'a> {
    name: &'a str,
    commit: &'a str,
}

fn read_refs(refs: &str) -> impl Iterator<Item = Ref<'_>> {
    refs.lines().filter_map(|line| {
        let mut parts = line.split('\0');
        let name = parts.next()?;
        let object = parts.next()?;
        // An annotated tag names the tag object; the commit is its peel.
        let peeled = parts.next().filter(|peel| !peel.is_empty());
        Some(Ref {
            name,
            commit: peeled.unwrap_or(object),
        })
    })
}

/// The limbs, each claiming its own moments, and the branches made by hand
/// in the order they claimed theirs.
///
/// The trunk first. Then the buds, in the order the wall grew them (the
/// time in their names, then the number after a taken name): a bud's name
/// also says the moment it grew from (`bud/<from>/<time>`, `<from>` its
/// commit's first 12 hex or the mark it grew from), so its walk stops
/// exactly there and a bud never takes another limb's moments, whatever
/// order they are claimed in. Then the branches made by hand: those that
/// have claimed before (`known`, from the ledger) in the order they first
/// did, then any new one, the one whose first moment of its own is oldest
/// first (then by name). A new branch never takes an older one's moments.
/// Last, every moment no branch holds (a deleted branch's, or HEAD's when
/// it is on no branch) is its own limb, said among the problems: nothing
/// read is ever left out.
fn limbs(
    refs: &str,
    index: &HashMap<Arc<str>, usize>,
    moments: &[Arc<Moment>],
    known: &[String],
    problems: &mut Vec<String>,
) -> Result<(Vec<Limb>, Vec<String>), HistoryError> {
    let mut grown: Vec<(String, usize)> = Vec::new();
    let mut by_hand: Vec<(String, usize)> = Vec::new();
    let mut mark_commits: HashMap<&str, usize> = HashMap::new();
    for r in read_refs(refs) {
        if let Some(part) = r.name.strip_prefix("refs/tags/mark/")
            && let Some(&m) = index.get(r.commit)
        {
            mark_commits.insert(part, m);
        }
        if let Some(name) = r.name.strip_prefix("refs/heads/") {
            let tip = *index.get(r.commit).ok_or_else(|| {
                HistoryError::Broken(format!(
                    "the branch {} names {:.10}, which the log did not hold",
                    clean(name),
                    r.commit
                ))
            })?;
            if rank(name) < 2 {
                grown.push((name.to_owned(), tip));
            } else {
                by_hand.push((name.to_owned(), tip));
            }
        }
    }
    grown.sort_by(|(a, _), (b, _)| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| bud_order(a).cmp(&bud_order(b)))
            .then_with(|| a.cmp(b))
    });
    let mut claimed = vec![false; moments.len()];
    let mut limbs = Vec::with_capacity(grown.len() + by_hand.len());
    for (name, tip) in grown {
        let from = bud_from(&name);
        let stop = from.and_then(|from| {
            mark_commits.get(from).copied().or_else(|| {
                // The commit's 12 hex: the first moment on the walk whose
                // commit starts with them.
                let mut at = Some(tip);
                while let Some(m) = at {
                    if from.len() >= 7 && moments[m].id.starts_with(from) && m != tip {
                        return Some(m);
                    }
                    at = moments[m].parent;
                }
                None
            })
        });
        let limb = claim(&mut claimed, moments, &name, tip, stop);
        if let Some(from) = from
            && (stop.is_none() || limb.from != stop)
        {
            problems.push(format!(
                "{} says it grew from {}, which is not on its line (a mark moved or gone?); it is shown down to where its line meets another",
                clean(&name),
                clean(from)
            ));
        }
        limbs.push(limb);
    }
    let mut order = Vec::with_capacity(by_hand.len());
    for name in known {
        if let Some(i) = by_hand.iter().position(|(n, _)| n == name) {
            let (name, tip) = by_hand.swap_remove(i);
            limbs.push(claim(&mut claimed, moments, &name, tip, None));
            order.push(name);
        }
    }
    // Each new hand-made branch's oldest unclaimed moment, kept and looked
    // at again only once claimed: a walk that meets the last limb claimed
    // follows it down, so its oldest moment is then claimed too.
    let mut oldest: Vec<Option<usize>> = by_hand
        .iter()
        .map(|&(_, tip)| first_unclaimed(tip, &claimed, moments))
        .collect();
    while !by_hand.is_empty() {
        for (i, &(_, tip)) in by_hand.iter().enumerate() {
            if oldest[i].is_some_and(|m| claimed[m]) {
                oldest[i] = first_unclaimed(tip, &claimed, moments);
            }
        }
        let time = |m: Option<usize>| m.map_or(i64::MAX, |m| moments[m].time);
        let first = (0..by_hand.len())
            .min_by(|&a, &b| {
                time(oldest[a])
                    .cmp(&time(oldest[b]))
                    .then_with(|| by_hand[a].0.cmp(&by_hand[b].0))
            })
            .unwrap_or(0);
        let (name, tip) = by_hand.swap_remove(first);
        oldest.swap_remove(first);
        limbs.push(claim(&mut claimed, moments, &name, tip, None));
        order.push(name);
    }
    unbranched(&mut limbs, &mut claimed, moments, problems);
    Ok((limbs, order))
}

/// What a limb on no branch is called.
pub const NO_BRANCH: &str = "(no branch)";

/// Every moment no branch claimed, as limbs of their own: one from each
/// such moment with no unclaimed moment after it, oldest tip first.
fn unbranched(
    limbs: &mut Vec<Limb>,
    claimed: &mut [bool],
    moments: &[Arc<Moment>],
    problems: &mut Vec<String>,
) {
    let mut has_child = vec![false; moments.len()];
    for (m, moment) in moments.iter().enumerate() {
        if !claimed[m]
            && let Some(parent) = moment.parent
            && !claimed[parent]
        {
            has_child[parent] = true;
        }
    }
    let mut count = 0;
    for tip in 0..moments.len() {
        if claimed[tip] || has_child[tip] {
            continue;
        }
        let limb = claim(claimed, moments, NO_BRANCH, tip, None);
        count += limb.moments.len();
        limbs.push(limb);
    }
    if count > 0 {
        problems.push(format!(
            "{count} moments are on no branch (a branch was deleted, or HEAD is on none); they are drawn as {NO_BRANCH}"
        ));
    }
}

/// The oldest moment a branch would claim: its first-parent walk from
/// `tip` down to the first claimed moment, the last step before it.
fn first_unclaimed(tip: usize, claimed: &[bool], moments: &[Arc<Moment>]) -> Option<usize> {
    let mut at = Some(tip);
    let mut oldest = None;
    while let Some(m) = at {
        if claimed[m] {
            break;
        }
        oldest = Some(m);
        at = moments[m].parent;
    }
    oldest
}

/// Walk a branch from its tip, claiming each moment not yet claimed, down
/// to the first claimed one or to `stop`: that is where it grew from.
fn claim(
    claimed: &mut [bool],
    moments: &[Arc<Moment>],
    name: &str,
    tip: usize,
    stop: Option<usize>,
) -> Limb {
    let mut own = Vec::new();
    let mut at = Some(tip);
    let mut from = None;
    while let Some(m) = at {
        if claimed[m] || stop == Some(m) {
            from = Some(m);
            break;
        }
        claimed[m] = true;
        own.push(m);
        at = moments[m].parent;
    }
    own.reverse();
    Limb {
        name: clean(name),
        moments: own,
        from,
    }
}

fn rank(name: &str) -> u8 {
    if name == "trunk" {
        0
    } else if name.starts_with("bud/") {
        1
    } else {
        2
    }
}

/// `bud/<from>/<time>[-n]`: the order the wall grew it in: the time (a
/// compact UTC time, which sorts as text), then `n`, the number after a
/// name already taken, as a number (so `-10` comes after `-2`), none first.
fn bud_order(name: &str) -> (&str, u64) {
    let time = name
        .strip_prefix("bud/")
        .and_then(|rest| rest.rsplit_once('/'))
        .map_or("", |(_, time)| time);
    match time.rsplit_once('-') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (base, n.parse().unwrap_or(u64::MAX).saturating_add(1))
        }
        _ => (time, 0),
    }
}

/// `bud/<from>/<time>[-n]`: the part saying where it grew from.
fn bud_from(name: &str) -> Option<&str> {
    name.strip_prefix("bud/")
        .and_then(|rest| rest.rsplit_once('/'))
        .map(|(from, _)| from)
}

/// Every mark and save. A save's tag is `save/<name>/<time>` (the wall's
/// own reading drops the time the same way); one the tree cannot place is
/// said among its problems.
fn marks(refs: &str, index: &HashMap<Arc<str>, usize>, problems: &mut Vec<String>) -> Vec<Mark> {
    let mut marks = Vec::new();
    for r in read_refs(refs) {
        let (part, save) = if let Some(part) = r.name.strip_prefix("refs/tags/mark/") {
            (part, false)
        } else if let Some(part) = r.name.strip_prefix("refs/tags/save/") {
            (part, true)
        } else {
            continue;
        };
        let name = if save {
            if let Some((name, _time)) = part.rsplit_once('/') {
                name
            } else {
                problems.push(format!(
                    "the save mark {} has no time; shown by its whole name",
                    clean(r.name)
                ));
                part
            }
        } else {
            part
        };
        let Some(&moment) = index.get(r.commit) else {
            problems.push(format!(
                "the mark {} names {:.10}, which is not a moment",
                clean(r.name),
                r.commit
            ));
            continue;
        };
        marks.push(Mark {
            name: clean(&name_of(name)),
            save,
            moment,
        });
    }
    marks.sort_by(|a, b| a.moment.cmp(&b.moment).then_with(|| a.name.cmp(&b.name)));
    marks
}

#[cfg(test)]
mod tests;
