//! Replay the wall's change log into the tree's lines.
//!
//! Changes commit and goes move HEAD, exact wherever the log and the wall's own files
//! allow, with every gap a commit that says what is not known.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Instant;

use super::git::{self, Commit, Importer, Planted};
use super::{Files, TreeError};
use crate::protocol::{Change, What};
use crate::store::{LOG_FILE, LoadedLog, OLD_LOG_FILE, Store};

mod births;
mod evidence;
#[cfg(test)]
mod fixtures;
mod go;
#[cfg(test)]
mod go_tests;
mod model;
#[cfg(test)]
mod regressions;
#[cfg(test)]
mod tests;

use evidence::{Evidence, Proofs};
use model::{Found, Model, describe, differences};

/// Why the first rings were not grown. Nothing in the tree is changed.
#[derive(Debug)]
pub enum RingsError {
    /// The wall holds its state directory's lock: it is playing.
    WallRunning,
    /// The tree's directory is something other than a tree, or cannot be.
    NotATree(String),
    /// The bare tree's HEAD names something other than trunk.
    NotTrunk(String),
    /// The tree has a trunk the import did not grow.
    HasTrunk(String),
    /// No line in the tree grows from the import's last ring.
    Diverged(String),
    /// The log and the wall's files give no wall to grow from.
    NoWall(String),
    /// Git failed, with KZ-T02b's sentence.
    Tree(TreeError),
    /// The state directory or its log could not be read.
    Read(io::Error),
}

impl fmt::Display for RingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WallRunning => f.write_str(
                "the wall is playing: stop it first (kazoo wall stop), then import; the first rings grow only from a log nobody is writing",
            ),
            Self::NotATree(s) | Self::NotTrunk(s) | Self::HasTrunk(s) | Self::Diverged(s) | Self::NoWall(s) => {
                f.write_str(s)
            }
            Self::Tree(e) => e.fmt(f),
            Self::Read(e) => write!(f, "the wall's history could not be read: {e}"),
        }
    }
}

impl std::error::Error for RingsError {}

impl From<TreeError> for RingsError {
    fn from(e: TreeError) -> Self {
        Self::Tree(e)
    }
}

impl From<io::Error> for RingsError {
    fn from(e: io::Error) -> Self {
        Self::Read(e)
    }
}

/// What an import did (or, dry, would do).
#[derive(Debug, Default)]
pub struct Rings {
    /// The report, line by line, as the command prints it.
    pub report: String,
    /// `trunk`'s commit id.
    pub trunk: String,
    /// Change commits written.
    pub changes: usize,
    /// Gap commits written.
    pub gaps: usize,
    /// Settle commits written.
    pub settles: usize,
}

impl fmt::Display for Rings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.report)
    }
}

fn git_output(tree: &Path, command: &str, args: &[&str]) -> Result<Output, TreeError> {
    git::git_command(command, Some(tree))
        .args(args)
        .output()
        .map_err(|e| TreeError::Git(format!("{command}: {e}")))
}

fn git_text(tree: &Path, command: &str, args: &[&str]) -> Result<String, TreeError> {
    let output = git_output(tree, command, args)?;
    if !output.status.success() {
        return Err(TreeError::Git(format!(
            "{command}: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().into())
        .map_err(|e| TreeError::Git(e.to_string()))
}

/// The commit `name` names, or `None` when there is no such ref.
fn reference(tree: &Path, name: &str) -> Result<Option<String>, TreeError> {
    let output = git_output(
        tree,
        "rev-parse",
        &["-q", "--verify", &format!("{name}^{{commit}}")],
    )?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8(output.stdout)
                .map_err(|e| TreeError::Git(e.to_string()))?
                .trim()
                .into(),
        ))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(TreeError::Git(format!(
            "read {name}: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// Steps 1 to 5 of the tree's order: a report when the import is in
/// already (or is finished here), `None` to grow.
fn existing(tree: &Path, dry_run: bool) -> Result<Option<Rings>, RingsError> {
    if !tree.exists() || fs::read_dir(tree)?.next().transpose()?.is_none() {
        return Ok(None);
    }
    if !git::is_bare(tree)? {
        return Err(RingsError::NotATree(format!(
            "{} is not empty and is not a tree; nothing is changed",
            tree.display()
        )));
    }
    let end = reference(tree, "refs/tags/rings/end")?;
    let trunk = reference(tree, "refs/heads/trunk")?;
    if trunk.is_none() {
        let head = fs::read_to_string(tree.join("HEAD"))?;
        let head = head.trim();
        if head != "ref: refs/heads/trunk" {
            return Err(RingsError::NotTrunk(format!(
                "the tree's HEAD is {}, not trunk; nothing is changed",
                head.strip_prefix("ref: ").unwrap_or(head)
            )));
        }
    }
    match (end, trunk) {
        (Some(end), Some(trunk)) => go::already(tree, &end, trunk).map(Some),
        (Some(end), None) => {
            let report = if dry_run {
                format!(
                    "an import stopped before naming trunk; the import names trunk at rings/end ({}) and writes nothing else\n",
                    short(&end)
                )
            } else {
                let mut importer = Importer::new(
                    tree,
                    &Planted {
                        branch: "refs/heads/trunk".into(),
                        tip: None,
                        through: None,
                    },
                )?;
                importer.tag("refs/heads/trunk", &end)?;
                importer.finish()?;
                let named = reference(tree, "refs/heads/trunk")?;
                if named.as_deref() != Some(end.as_str()) {
                    return Err(TreeError::Git(format!(
                        "trunk is at {}, not at rings/end ({end})",
                        named.as_deref().unwrap_or("nothing")
                    ))
                    .into());
                }
                format!(
                    "finished an import that stopped before naming trunk: trunk is at rings/end ({})\n",
                    short(&end)
                )
            };
            Ok(Some(Rings {
                report,
                trunk: end,
                ..Rings::default()
            }))
        }
        (None, Some(trunk)) => {
            let subjects = git_text(tree, "log", &["--reverse", "--format=%s", &trunk])?;
            let subject = subjects
                .lines()
                .next()
                .unwrap_or("a commit with no subject");
            Err(RingsError::HasTrunk(format!(
                "the tree already has a trunk the import did not grow (its first commit is {subject}); the first rings grow only beneath everything, and nothing in the tree is ever rewritten, so nothing is changed"
            )))
        }
        (None, None) => Ok(None),
    }
}

/// The import is in already on `line` (at `trunk`), which grows from the
/// import's last ring `end`.
fn in_already(tree: &Path, end: &str, trunk: String, line: &str) -> Result<Rings, RingsError> {
    let ancestor = git_output(tree, "merge-base", &["--is-ancestor", end, &trunk])?;
    if ancestor.status.code() == Some(1) {
        return Err(RingsError::Diverged(format!(
            "{line} does not grow from the import's last ring (rings/end at {}); nothing is changed",
            short(end)
        )));
    }
    if !ancestor.status.success() {
        return Err(TreeError::Git(format!(
            "merge-base: {}: {}",
            ancestor.status,
            String::from_utf8_lossy(&ancestor.stderr)
        ))
        .into());
    }
    let seqs = git_text(
        tree,
        "log",
        &[
            "--reverse",
            "--format=%(trailers:key=Kazoo-Seq,valueonly)",
            end,
        ],
    )?;
    let numbers = seqs
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<u64>()
                .map_err(|e| TreeError::Git(format!("Kazoo-Seq: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let first = numbers.first().copied().unwrap_or(0).saturating_add(1);
    let last = numbers.last().copied().unwrap_or(0);
    let since = git_text(tree, "rev-list", &["--count", &format!("{end}..{trunk}")])?;
    Ok(Rings {
        report: format!(
            "the first rings are in already: {line} holds changes {first} to {last} from the import (rings/end at {}), and {since} commits since\n",
            short(end)
        ),
        trunk,
        ..Rings::default()
    })
}

/// Each change's salt: `fnv1a64` of its line exactly as the log has it
/// (the first line for a seq written twice, as `Store::load_log` keeps),
/// and the log's size in bytes. Only the hashes are held, never the lines.
fn salts(state: &Path) -> Result<(BTreeMap<u64, u64>, u64), io::Error> {
    let mut salts = BTreeMap::new();
    let mut bytes = 0;
    for name in [OLD_LOG_FILE, LOG_FILE] {
        let file = match fs::File::open(state.join(name)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        bytes += file.metadata()?.len();
        for line in BufReader::new(file).split(b'\n') {
            let line = line?;
            // Unreadable lines are `load_log`'s to count; they have no seq.
            let Ok(change) = serde_json::from_slice::<Change>(&line) else {
                continue;
            };
            salts
                .entry(change.seq)
                .or_insert_with(|| crate::chance::fnv1a64(&line));
        }
    }
    Ok((salts, bytes))
}

/// A start, gap or settle commit: the wall's own, dated `at`, holding the
/// whole wall.
fn own_commit(
    kind: &str,
    seq: u64,
    at: &str,
    lines: (&str, &str),
    files: &Files,
    gap: Option<(u64, u64)>,
) -> Result<Commit, TreeError> {
    let (subject, detail) = lines;
    let unix = chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%SZ")
        .map_err(|e| TreeError::Git(format!("change date: {e}")))?
        .and_utc()
        .timestamp();
    let gap = gap.map_or_else(String::new, |(a, b)| format!("Kazoo-Gap: {a}-{b}\n"));
    Ok(Commit {
        seat: "kazoo-wall".into(),
        unix,
        message: format!("{subject}\n\n{detail}\n\nKazoo-Kind: {kind}\nKazoo-Seq: {seq}\n{gap}"),
        whole: true,
        files: files
            .iter()
            .map(|(p, b)| (p.clone(), Some(b.clone())))
            .collect(),
    })
}

/// What differs from `previous` to `next`: `M` for new or changed paths,
/// `D` for gone ones.
fn delta(previous: &Files, next: &Files) -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = Vec::new();
    for (path, bytes) in next {
        if previous.get(path) != Some(bytes) {
            files.push((path.clone(), Some(bytes.clone())));
        }
    }
    for path in previous.keys() {
        if !next.contains_key(path) {
            files.push((path.clone(), None));
        }
    }
    files
}

/// The log as read, and what is known of it before the walk.
struct Log<'a> {
    state: &'a Path,
    changes: &'a [Change],
    salts: &'a BTreeMap<u64, u64>,
    proofs: &'a Proofs,
    abandoned: &'a BTreeSet<u64>,
}

struct Walk<'a> {
    log: Log<'a>,
    line: go::Lineage,
    model: Model,
    importer: Importer,
    /// The files of the commit last written.
    previous: Files,
    mark: u64,
    /// The last commit's `Kazoo-Seq` and date.
    tip: (u64, String),
    report: Rings,
    gap_lines: Vec<String>,
    start: String,
    starts: usize,
    unknown_from: u64,
    unknown_reason: String,
}

impl Walk<'_> {
    fn own(
        &mut self,
        kind: &str,
        (seq, at): (u64, &str),
        lines: (&str, &str),
        gap: Option<(u64, u64)>,
    ) -> Result<(), TreeError> {
        self.grow_line(at)?;
        let files = self.model.files(None)?;
        self.mark = self
            .importer
            .commit(&own_commit(kind, seq, at, lines, &files, gap)?)?;
        self.remember(seq, &files, false);
        self.previous = files;
        self.tip = (seq, at.into());
        match kind {
            "gap" => self.report.gaps += 1,
            "settle" => self.report.settles += 1,
            _ => self.starts += 1,
        }
        Ok(())
    }

    fn relative(&self, evidence: &Evidence) -> String {
        evidence
            .path
            .strip_prefix(self.log.state)
            .unwrap_or(&evidence.path)
            .display()
            .to_string()
    }

    /// Step 6: the evidence after change `seq`, if any, checked.
    fn evidence(&mut self, seq: u64, at: &str) -> Result<(), TreeError> {
        let Some(evidence) = self.log.proofs.at(seq) else {
            return Ok(());
        };
        let file = evidence.file()?;
        let label = evidence.label();
        let path = self.relative(evidence);
        if !self.model.known {
            let first = self.unknown_from;
            let mut seats = BTreeMap::<&str, usize>::new();
            for change in
                self.log.changes.iter().filter(|c| {
                    (first..=seq).contains(&c.seq) && !self.log.abandoned.contains(&c.seq)
                })
            {
                *seats.entry(&change.seat).or_default() += 1;
            }
            let count: usize = seats.values().sum();
            let seats = seats
                .into_iter()
                .map(|(seat, n)| format!("{seat} {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            let subject = format!(
                "the wall from here is {label} as it stood after change {seq}; changes {first} to {seq} ({count} changes: {seats}) are not in the tree one by one: {}",
                self.unknown_reason
            );
            let detail = format!(
                "The wall is taken whole from {path}, which holds it after change {seq}; changes {first} to {seq} were made by {seats}."
            );
            self.model.adopt(&file, seq, Found::File)?;
            self.own("gap", (seq, at), (&subject, &detail), Some((first, seq)))?;
            self.gap_lines
                .push(format!("gap after change {seq}: {subject}"));
            if self.start.is_empty() {
                self.start = format!(
                    "{subject} ({} modules, {} cables, {} BPM)",
                    file.modules.len(),
                    file.cables.len(),
                    file.tempo
                );
            }
            return Ok(());
        }
        let differences = differences(&self.model.file, &file, evidence.seeded);
        if differences.is_empty() {
            let current = self.model.files(None)?;
            let mut file = file;
            // A reused load backup can still name the wall it was first saved from.
            if evidence.is_backup() {
                file.wall = self.model.file.wall;
            }
            self.model.adopt(&file, seq, Found::File)?;
            if current != self.model.files(None)? {
                let subject = format!(
                    "the rack's rows, groups, fingerprints and words as {label} kept them after change {seq}; they are not logged"
                );
                let detail = format!(
                    "The wall's sound is as the log makes it; {path} holds how it hung after change {seq}."
                );
                self.own("settle", (seq, at), (&subject, &detail), None)?;
            }
        } else {
            let subject = format!(
                "after change {seq} the wall was as {label} has it, not as the log makes it: {}; something the log does not have changed it (the desk's tempo is not logged)",
                describe(&differences)
            );
            let detail =
                format!("The wall is taken whole from {path}, which holds it after change {seq}.");
            self.model.adopt(&file, seq, Found::File)?;
            self.own("gap", (seq, at), (&subject, &detail), None)?;
            self.gap_lines
                .push(format!("gap after change {seq}: {subject}"));
        }
        Ok(())
    }

    /// Step 5: the wall a `migrate` made; a sentence for its commit when
    /// the file it migrated was not kept.
    fn migration(&mut self, change: &Change) -> Result<String, TreeError> {
        let What::Migrate {
            from_version,
            notes,
            ..
        } = &change.what
        else {
            return Ok(String::new());
        };
        let before = self
            .log
            .proofs
            .at(change.seq.saturating_sub(1))
            .filter(|e| e.version == *from_version);
        let mut file = match before {
            Some(e) => e.as_read()?,
            None => self.model.file.clone(),
        };
        crate::migrate::migrate(&mut file).map_err(TreeError::Git)?;
        if !notes.iter().any(|n| n.starts_with("the patch had no seed")) {
            file.seed = self.model.file.seed;
        }
        self.model.adopt(&file, change.seq, Found::File)?;
        Ok(if before.is_none() {
            "patch.json before this migration was not kept, so anything the migration took from the file's own bytes may differ".into()
        } else {
            String::new()
        })
    }

    /// Steps 3 to 5 for change `change`, then step 6 after it.
    fn change(&mut self, change: &Change) -> Result<(), TreeError> {
        if matches!(change.what, What::Go { .. }) {
            self.go(change)?;
            return self.evidence(change.seq, &change.at);
        }
        let mut whole = false;
        let mut note = String::new();
        if self.model.known {
            match &change.what {
                What::Load { save, .. } => {
                    match self
                        .log
                        .proofs
                        .incoming(self.log.state, change, self.log.changes)
                    {
                        Ok((file, note)) => {
                            self.model.adopt(&file, change.seq, Found::Load)?;
                            if let Some(note) = note {
                                self.gap_lines
                                    .push(format!("births at load {}: {note}", change.seq));
                            }
                            whole = true;
                        }
                        Err(why) => {
                            self.model.known = false;
                            self.unknown_from = change.seq;
                            self.unknown_reason = format!(
                                "the wall change {} put up (the save \"{}\") was not kept exactly ({why})",
                                change.seq,
                                save.as_deref().unwrap_or("none: an undone load")
                            );
                        }
                    }
                }
                What::Migrate { .. } => {
                    note = self.migration(change)?;
                    whole = true;
                }
                _ => self.applied(change)?,
            }
            if self.model.known {
                self.commit(change, whole, &note)?;
            }
        }
        self.evidence(change.seq, &change.at)
    }

    /// A change the model applies, with a gap commit just before it when
    /// the log's before-values disagree with the wall it makes.
    fn applied(&mut self, change: &Change) -> Result<(), TreeError> {
        let before = self.model.file.clone();
        let applied = self.model.apply(change);
        if let Some(reason) = applied.unknown {
            self.unknown_from = change.seq;
            self.unknown_reason = reason;
        } else if let Some(reason) = applied.doubt {
            // The gap commit holds the wall just before the change as its
            // own before-values have it, where they can be put back.
            let after = self.model.file.clone();
            if self.model.undo(&change.what) {
                self.model.file.last_seq = change.seq.saturating_sub(1);
            } else {
                self.model.file = before;
            }
            let detail = format!(
                "Change {} by {} does not agree with the wall the log makes before it; this commit holds the wall as that change's own before-values have it.",
                change.seq, change.seat
            );
            self.own(
                "gap",
                (change.seq.saturating_sub(1), &change.at),
                (&reason, &detail),
                None,
            )?;
            self.model.file = after;
            self.gap_lines
                .push(format!("gap before change {}: {reason}", change.seq));
        }
        Ok(())
    }

    /// Change `change`'s own commit, as KZ-T02b writes a live one.
    fn commit(&mut self, change: &Change, whole: bool, note: &str) -> Result<(), TreeError> {
        let whole = self.grow_line(&change.at)? || whole;
        let salt =
            self.log.salts.get(&change.seq).copied().ok_or_else(|| {
                TreeError::Git(format!("change {} has no logged line", change.seq))
            })?;
        self.model.stamp(change, salt)?;
        let files = self.model.files(Some(change))?;
        let paths = if whole {
            files
                .iter()
                .map(|(p, b)| (p.clone(), Some(b.clone())))
                .collect()
        } else {
            delta(&self.previous, &files)
        };
        let mut commit = Commit::change(change, paths)?;
        commit.whole = whole;
        if !note.is_empty() {
            if let Some((subject, body)) = commit.message.split_once("\n\n") {
                commit.message = format!("{subject}\n\n{note}\n\n{body}");
            }
        }
        self.mark = self.importer.commit(&commit)?;
        self.remember(change.seq, &files, true);
        self.previous = files;
        self.tip = (change.seq, change.at.clone());
        self.report.changes += 1;
        Ok(())
    }

    /// Step 1's start commit.
    fn starting(&mut self, evidence: &Evidence, first: &Change) -> Result<(), TreeError> {
        let how = if evidence.after < first.seq {
            "as"
        } else {
            "worked back from"
        };
        let subject = format!(
            "the wall before change {}, {how} {}",
            first.seq,
            evidence.label()
        );
        let detail = if evidence.after < first.seq {
            format!(
                "{} holds the wall after change {}.",
                self.relative(evidence),
                evidence.after
            )
        } else {
            format!(
                "{} holds the wall after change {}; changes {} back to {} were taken off it, every before-value agreeing.",
                self.relative(evidence),
                evidence.after,
                evidence.after,
                first.seq
            )
        };
        self.start = format!(
            "{subject} ({} modules, {} cables, {} BPM)",
            self.model.file.modules.len(),
            self.model.file.cables.len(),
            self.model.file.tempo
        );
        self.own(
            "start",
            (first.seq.saturating_sub(1), &first.at),
            (&subject, &detail),
            None,
        )
    }
}

/// Step 1: the wall before the first change, from a file at that point or
/// worked back from the earliest file after it.
fn beginning<'a>(
    changes: &[Change],
    proofs: &'a Proofs,
    abandoned: &BTreeSet<u64>,
) -> Result<(Model, bool, &'a Evidence), RingsError> {
    let first = changes.first().ok_or_else(|| {
        RingsError::NoWall(
            "the wall's log has no readable changes; no first rings can be grown".into(),
        )
    })?;
    let evidence = proofs
        .at(first.seq.saturating_sub(1))
        .or_else(|| proofs.files.iter().find(|e| e.after >= first.seq))
        .ok_or_else(|| {
            RingsError::NoWall(
                "no kept wall file holds the wall after any change in the log, so no known wall can be grown; nothing is planted".into(),
            )
        })?;
    let file = evidence.file()?;
    let mut model = Model::new(file.clone(), true)?;
    let mut known = evidence.after == first.seq.saturating_sub(1);
    if !known {
        let mut expected = evidence.after;
        known = true;
        for change in changes
            .iter()
            .filter(|c| (first.seq..=evidence.after).contains(&c.seq))
            .rev()
        {
            if change.seq != expected
                || (!abandoned.contains(&change.seq) && !model.undo(&change.what))
            {
                known = false;
                break;
            }
            expected = expected.saturating_sub(1);
        }
        known &= expected == first.seq.saturating_sub(1);
    }
    let model = if known {
        model.file.last_seq = first.seq.saturating_sub(1);
        if evidence.after >= first.seq {
            // The fingerprints cannot be worked back: the changes left
            // them; the wall before them carries none, as a seed does.
            model.file.dye.clear();
        }
        Model::new(model.file, true)?
    } else {
        Model::new(file, false)?
    };
    Ok((model, known, evidence))
}

/// The walk, into the tree at `tree` (planted here when it is not yet).
fn run(tree: &Path, log: Log<'_>) -> Result<(Rings, String), RingsError> {
    let changes = log.changes;
    let (Some(first), Some(last)) = (changes.first(), changes.last()) else {
        return Err(RingsError::NoWall(
            "the wall's log has no readable changes; no first rings can be grown".into(),
        ));
    };
    let (model, known, evidence) = beginning(changes, log.proofs, log.abandoned)?;
    if !git::is_bare(tree)? {
        git::plant(tree)?;
    }
    let importer = Importer::new(
        tree,
        &Planted {
            branch: "refs/heads/trunk".into(),
            tip: None,
            through: None,
        },
    )?;
    let unknown_reason = format!(
        "the wall before change {} could not be worked back exactly from {}",
        first.seq,
        evidence.label()
    );
    let abandoned = log.abandoned;
    let mut walk = Walk {
        log,
        line: go::Lineage::new(tree, changes),
        model,
        importer,
        previous: Files::new(),
        mark: 0,
        tip: (0, String::new()),
        report: Rings::default(),
        gap_lines: Vec::new(),
        start: String::new(),
        starts: 0,
        unknown_from: first.seq,
        unknown_reason,
    };
    if known {
        walk.starting(evidence, first)?;
    }
    let mut expected = first.seq;
    for change in changes {
        if change.seq > expected {
            if walk.model.known {
                walk.unknown_from = expected;
                walk.unknown_reason = format!("change {expected} is not in the log");
            }
            walk.model.known = false;
        }
        expected = change.seq.saturating_add(1);
        if abandoned.contains(&change.seq) {
            // It did not happen: no commit, but a file after it still
            // holds the wall.
            walk.evidence(change.seq, &change.at)?;
        } else {
            walk.change(change)?;
        }
    }
    if walk.starts + walk.report.changes + walk.report.gaps + walk.report.settles == 0 {
        return Err(RingsError::NoWall(format!(
            "no known wall could be grown from the log and its kept files: {}",
            walk.unknown_reason
        )));
    }
    walk.ending(last)?;
    walk.finish_line()?;
    let summary = walk.summary(last);
    walk.importer.finish()?;
    Ok((walk.report, summary))
}

impl Walk<'_> {
    fn summary(&self, last: &Change) -> String {
        let model = &self.model;
        let patch = self.log.proofs.patch.as_ref();
        let (tip_seq, tip_at) = &self.tip;
        let mut trunk = self.line_summary(*tip_seq, tip_at);
        if *tip_seq < last.seq {
            trunk = format!(
                "{trunk}; changes {} to {} are not in it: {}",
                tip_seq + 1,
                last.seq,
                self.unknown_reason
            );
        }
        match patch.map(|e| e.after) {
            Some(after) if after == last.seq && model.known => {
                trunk.push_str(", as patch.json has it");
            }
            Some(after) => {
                trunk = format!(
                    "{trunk}; patch.json is after change {after}: the wall's next start says so"
                );
            }
            None => {
                trunk.push_str("; there is no readable patch.json: the wall's next start says so");
            }
        }
        let knobs = model.omitted.iter().filter(|s| s.contains('.')).count();
        let modules = model.omitted.len() - knobs;
        let omitted = model.omitted.iter().cloned().collect::<Vec<_>>();
        let found_file = if model.found_file > 0 {
            format!(", {} found in files after gaps", model.found_file)
        } else {
            String::new()
        };
        format!(
            "start: {}\ncommits: {} ({} start, {} changes, {} gaps, {} settles)\n{}births: {} found at the start, {} at adds, {} kept across loads, {} found at loads{found_file}\nthis build left out: {knobs} knobs, {modules} modules ({})\n{trunk}\n",
            self.start,
            self.starts + self.report.changes + self.report.gaps + self.report.settles,
            self.starts,
            self.report.changes,
            self.report.gaps,
            self.report.settles,
            joined_lines(&self.gap_lines),
            model.found_start,
            model.born,
            model.kept_load,
            model.found_load,
            describe(&omitted),
        )
    }
}

/// Plant the tree at `tree` and grow its trunk from the wall's log in
/// `state`; with `dry_run`, grow it beside the tree, report, and remove it.
///
/// # Errors
///
/// A [`RingsError`] saying why nothing was grown; the tree is unchanged.
pub fn grow(state: &Path, tree: &Path, dry_run: bool) -> Result<Rings, RingsError> {
    let started = Instant::now();
    if !state.is_dir() {
        return Err(RingsError::Read(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is not the wall's state directory: it does not exist",
                state.display()
            ),
        )));
    }
    let store = Store::open(state).map_err(|e| {
        if e.kind() == io::ErrorKind::WouldBlock {
            RingsError::WallRunning
        } else {
            RingsError::Read(e)
        }
    })?;
    if let Some(report) = existing(tree, dry_run)? {
        return Ok(report);
    }
    let loaded = store.load_log()?;
    let (salts, bytes) = salts(state)?;
    let proofs = Proofs::read(state, &loaded.changes)?;
    let abandoned = loaded
        .changes
        .iter()
        .filter_map(|c| match c.what {
            What::Abandoned { change } => Some(change),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let dry = if dry_run {
        let name = tree.file_name().ok_or_else(|| {
            RingsError::NotATree("the tree directory has no name to grow a dry run beside".into())
        })?;
        Some(tree.with_file_name(format!(
            "{}.dry-run-{}-{}",
            name.to_string_lossy(),
            std::process::id(),
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        )))
    } else {
        None
    };
    let destination = dry.as_deref().unwrap_or(tree);
    if dry.is_some() && destination.exists() {
        return Err(RingsError::NotATree(format!(
            "{} already exists; nothing is changed",
            destination.display()
        )));
    }
    let log = Log {
        state,
        changes: &loaded.changes,
        salts: &salts,
        proofs: &proofs,
        abandoned: &abandoned,
    };
    let result = run(destination, log);
    let cleanup = match (&dry, destination.exists()) {
        (Some(_), true) => match fs::remove_dir_all(destination) {
            Ok(()) => None,
            Err(e) => Some(format!(
                "the dry run could not remove {}: {e}\n",
                destination.display()
            )),
        },
        _ => None,
    };
    let (mut report, summary) = match (result, cleanup.as_ref()) {
        (Ok(result), _) => result,
        (Err(e), Some(cleanup)) => {
            return Err(TreeError::Git(format!("{e}; {}", cleanup.trim_end())).into());
        }
        (Err(e), None) => return Err(e),
    };
    report.report = report_text(
        (state, tree, dry),
        &loaded,
        &proofs,
        (bytes, abandoned.len()),
        (&summary, cleanup, started),
    );
    Ok(report)
}

fn report_text(
    (state, tree, dry): (&Path, &Path, Option<PathBuf>),
    loaded: &LoadedLog,
    proofs: &Proofs,
    (bytes, abandoned): (u64, usize),
    (summary, cleanup, started): (&str, Option<String>, Instant),
) -> String {
    let first = loaded.changes.first().map_or(0, |c| c.seq);
    let last = loaded.changes.last().map_or(0, |c| c.seq);
    let mode = dry.map_or_else(
        || "planted and grown".into(),
        |dir| format!("dry run: grown in {}, then removed", dir.display()),
    );

    format!(
        "the first rings: {} → {} ({mode})\nlog: changes {first} to {last} ({} lines, {}.{:03} MB); {} unreadable, {} repeated, {abandoned} abandoned\nevidence: {} wall files; {} load backups hold their fingerprints, {} do not ({}); {} unreadable ({})\n{}{summary}{}took {:.3} s\n",
        state.display(),
        tree.display(),
        loaded.changes.len() + loaded.skipped + loaded.repeated,
        bytes / 1_000_000,
        (bytes % 1_000_000) / 1000,
        loaded.skipped,
        loaded.repeated,
        proofs.count,
        proofs.backups,
        proofs.bad_backups.len(),
        proofs.bad_backups.join(", "),
        proofs.unreadable.len(),
        proofs.unreadable.join(", "),
        joined_lines(&proofs.disagreements),
        cleanup.unwrap_or_default(),
        started.elapsed().as_secs_f64()
    )
}

fn joined_lines(items: &[String]) -> String {
    items.iter().fold(String::new(), |mut text, item| {
        text.push_str(item);
        text.push('\n');
        text
    })
}
