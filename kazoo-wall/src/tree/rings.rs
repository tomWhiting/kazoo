use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::Output;
use std::time::Instant;

use super::git::{self, Commit, Importer, Planted};
use super::{Files, TreeError};
use crate::protocol::{Change, What};
use crate::store::{LOG_FILE, OLD_LOG_FILE, Store};

mod model;
#[cfg(test)]
mod tests;

use model::{Model, Proofs, describe, differences};

#[derive(Debug)]
pub enum RingsError {
    WallRunning,
    NotATree(String),
    HasTrunk(String),
    Diverged(String),
    Tree(TreeError),
    Read(io::Error),
}

impl fmt::Display for RingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WallRunning => f.write_str("the wall is playing: stop it first (kazoo wall stop), then import; the first rings grow only from a log nobody is writing"),
            Self::NotATree(s) | Self::HasTrunk(s) | Self::Diverged(s) => f.write_str(s),
            Self::Tree(e) => e.fmt(f),
            Self::Read(e) => write!(f,"the wall's history could not be read: {e}"),
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

#[derive(Debug, Default)]
pub struct Rings {
    pub report: String,
    pub trunk: String,
    pub changes: usize,
    pub gaps: usize,
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
    match (end, trunk) {
        (Some(end), Some(trunk)) => {
            let ancestor = git_output(tree, "merge-base", &["--is-ancestor", &end, &trunk])?;
            if ancestor.status.code() == Some(1) {
                return Err(RingsError::Diverged(format!(
                    "trunk does not grow from the import's last ring (rings/end at {}); nothing is changed",
                    &end[..12]
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
                    &end,
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
            let since = git_text(tree, "log", &["--format=%H", &format!("{end}..{trunk}")])?
                .lines()
                .count();
            Ok(Some(Rings {
                report: format!(
                    "the first rings are in already: trunk holds changes {first} to {last} from the import (rings/end at {}), and {since} commits since\n",
                    &end[..12]
                ),
                trunk,
                ..Rings::default()
            }))
        }
        (Some(end), None) => {
            if !dry_run {
                let mut importer = Importer::new(
                    tree,
                    &Planted {
                        branch: "refs/heads/trunk".into(),
                        tip: None,
                        through: None,
                    },
                )?;
                importer.tag("refs/heads/trunk", &end)?;
                let tip = importer.checkpoint(0)?;
                importer.finish()?;
                if tip != end {
                    return Err(TreeError::Git(format!(
                        "trunk is at {tip}, not rings/end at {end}"
                    ))
                    .into());
                }
            }
            Ok(Some(Rings {
                report: "finished an import that stopped before naming trunk\n".into(),
                trunk: end,
                ..Rings::default()
            }))
        }
        (None, Some(trunk)) => {
            let subjects = git_text(tree, "log", &["--reverse", "--format=%s", &trunk])?;
            let subject = subjects.lines().next().unwrap_or("an unnamed commit");
            Err(RingsError::HasTrunk(format!(
                "the tree already has a trunk the import did not grow (its first commit is {subject}); the first rings grow only beneath everything, and nothing in the tree is ever rewritten, so nothing is changed"
            )))
        }
        (None, None) => Ok(None),
    }
}

fn raw_lines(state: &Path) -> Result<(BTreeMap<u64, Vec<u8>>, u64), io::Error> {
    let mut lines = BTreeMap::new();
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
            let Ok(change) = serde_json::from_slice::<Change>(&line) else {
                continue;
            };
            lines.entry(change.seq).or_insert(line);
        }
    }
    Ok((lines, bytes))
}

fn own_commit(
    kind: &str,
    seq: u64,
    at: &str,
    subject: &str,
    detail: &str,
    files: Files,
    gap: Option<(u64, u64)>,
) -> Result<Commit, TreeError> {
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
        files: files.into_iter().map(|(p, b)| (p, Some(b))).collect(),
    })
}

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

struct Walk<'a> {
    state: &'a Path,
    changes: &'a [Change],
    raw: &'a BTreeMap<u64, Vec<u8>>,
    proofs: &'a Proofs,
    abandoned: &'a BTreeSet<u64>,
    model: Model,
    importer: Importer,
    previous: Files,
    mark: u64,
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
        seq: u64,
        at: &str,
        subject: &str,
        detail: &str,
        gap: Option<(u64, u64)>,
    ) -> Result<(), TreeError> {
        let files = self.model.files(None)?;
        self.mark = self.importer.commit(&own_commit(
            kind,
            seq,
            at,
            subject,
            detail,
            files.clone(),
            gap,
        )?)?;
        self.previous = files;
        match kind {
            "gap" => self.report.gaps += 1,
            "settle" => self.report.settles += 1,
            _ => self.starts += 1,
        }
        Ok(())
    }

    fn evidence(&mut self, seq: u64, at: &str) -> Result<(), TreeError> {
        let Some(evidence) = self.proofs.at(seq) else {
            return Ok(());
        };
        let label = evidence.path.file_name().map_or_else(
            || evidence.path.display().to_string(),
            |p| p.to_string_lossy().into_owned(),
        );
        if !self.model.known {
            let first = self.unknown_from;
            let mut seats = BTreeMap::<String, usize>::new();
            for change in self
                .changes
                .iter()
                .filter(|c| c.seq >= first && c.seq <= seq && !self.abandoned.contains(&c.seq))
            {
                *seats.entry(change.seat.clone()).or_default() += 1;
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
            self.model.adopt(&evidence.file, seq, false)?;
            self.own(
                "gap",
                seq,
                at,
                &subject,
                &evidence
                    .path
                    .strip_prefix(self.state)
                    .unwrap_or(&evidence.path)
                    .display()
                    .to_string(),
                Some((first, seq)),
            )?;
            self.gap_lines
                .push(format!("gap after change {seq}: {subject}"));
            if self.start.is_empty() {
                self.start = subject;
            }
            return Ok(());
        }
        let differences = differences(&self.model.file, &evidence.file, evidence.seeded);
        if differences.is_empty() {
            let current = self.model.files(None)?;
            self.model.adopt(&evidence.file, seq, false)?;
            let adopted = self.model.files(None)?;
            if current != adopted {
                let subject = format!(
                    "the rack's rows, groups, fingerprints and words as {label} kept them after change {seq}; they are not logged"
                );
                self.own(
                    "settle",
                    seq,
                    at,
                    &subject,
                    &evidence
                        .path
                        .strip_prefix(self.state)
                        .unwrap_or(&evidence.path)
                        .display()
                        .to_string(),
                    None,
                )?;
            }
        } else {
            let subject = format!(
                "after change {seq} the wall was as {label} has it, not as the log makes it: {}; something the log does not have changed it (the desk's tempo is not logged)",
                describe(&differences)
            );
            self.model.adopt(&evidence.file, seq, false)?;
            self.own(
                "gap",
                seq,
                at,
                &subject,
                &evidence
                    .path
                    .strip_prefix(self.state)
                    .unwrap_or(&evidence.path)
                    .display()
                    .to_string(),
                None,
            )?;
            self.gap_lines
                .push(format!("gap after change {seq}: {subject}"));
        }
        Ok(())
    }

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
            .proofs
            .at(change.seq.saturating_sub(1))
            .filter(|e| e.file.version == *from_version);
        let mut file = if let Some(e) = before {
            crate::migrate::read(&e.bytes).map_err(TreeError::Git)?.0
        } else {
            self.model.file.clone()
        };
        crate::migrate::migrate(&mut file).map_err(TreeError::Git)?;
        let normalized = crate::patch::Patch::load(&file)
            .map_err(TreeError::Git)?
            .0
            .to_file();
        for module in &mut file.modules {
            if let Some(loaded) = normalized.modules.iter().find(|m| m.id == module.id) {
                for (knob, value) in &loaded.knobs {
                    module.knobs.entry(knob.clone()).or_insert(*value);
                }
            }
        }
        if !notes.iter().any(|n| n.starts_with("the patch had no seed")) {
            file.seed = self.model.file.seed;
        }
        self.model.adopt(&file, change.seq, false)?;
        Ok(if before.is_none() {
            "patch.json before this migration was not kept, so anything the migration took from the file's own bytes may differ".into()
        } else {
            String::new()
        })
    }

    fn change(&mut self, change: &Change) -> Result<(), TreeError> {
        let mut whole = false;
        let mut note = String::new();
        if self.model.known {
            match &change.what {
                What::Load { save, .. } => {
                    if let Some(file) = self.proofs.incoming(self.state, change, self.changes) {
                        self.model.adopt(&file, change.seq, true)?;
                        whole = true;
                    } else {
                        self.model.known = false;
                        self.unknown_from = change.seq;
                        self.unknown_reason = format!(
                            "the wall change {} put up (the save \"{}\") was not kept exactly",
                            change.seq,
                            save.as_deref().unwrap_or("unnamed")
                        );
                    }
                }
                What::Migrate { .. } => {
                    note = self.migration(change)?;
                    whole = true;
                }
                _ => {
                    let before = self.model.file.clone();
                    let applied = self.model.apply(change);
                    if let Some(reason) = applied.unknown {
                        self.unknown_from = change.seq;
                        self.unknown_reason = reason;
                    } else if let Some(reason) = applied.doubt {
                        let after = self.model.file.clone();
                        if !self.model.undo(&change.what) {
                            self.model.file = before;
                        }
                        self.own(
                            "gap",
                            change.seq.saturating_sub(1),
                            &change.at,
                            &reason,
                            &reason,
                            None,
                        )?;
                        self.model.file = after;
                        self.gap_lines
                            .push(format!("gap before change {}: {reason}", change.seq));
                    }
                }
            }
            if self.model.known {
                let line = self.raw.get(&change.seq).ok_or_else(|| {
                    TreeError::Git(format!("change {} has no logged line", change.seq))
                })?;
                self.model.stamp(change, line)?;
                let files = self.model.files(Some(change))?;
                let mut commit = Commit::change(
                    change,
                    if whole {
                        files
                            .iter()
                            .map(|(p, b)| (p.clone(), Some(b.clone())))
                            .collect()
                    } else {
                        delta(&self.previous, &files)
                    },
                )?;
                commit.whole = whole;
                if !note.is_empty() {
                    if let Some((subject, body)) = commit.message.split_once("\n\n") {
                        commit.message = format!("{subject}\n\n{note}\n\n{body}");
                    }
                }
                self.mark = self.importer.commit(&commit)?;
                self.previous = files;
                self.report.changes += 1;
            }
        }
        self.evidence(change.seq, &change.at)
    }
}

fn beginning<'a>(
    changes: &[Change],
    proofs: &'a Proofs,
    abandoned: &BTreeSet<u64>,
) -> Result<(Model, bool, &'a model::Evidence), RingsError> {
    let first = changes
        .first()
        .ok_or_else(|| RingsError::NotATree("the wall's log has no readable changes".into()))?;
    let evidence = proofs
        .at(first.seq.saturating_sub(1))
        .or_else(|| proofs.files.iter().find(|e| e.after >= first.seq))
        .ok_or_else(|| {
            RingsError::NotATree(
                "no kept wall file can give the import a known wall; nothing is grown".into(),
            )
        })?;
    let mut model = Model::new(evidence.file.clone(), true)?;
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
        Model::new(model.file, true)?
    } else {
        Model::new(evidence.file.clone(), false)?
    };
    Ok((model, known, evidence))
}

fn run(
    state: &Path,
    tree: &Path,
    changes: &[Change],
    raw: &BTreeMap<u64, Vec<u8>>,
    proofs: &Proofs,
    abandoned: &BTreeSet<u64>,
) -> Result<(Rings, String), RingsError> {
    let first = changes.first().ok_or_else(|| {
        RingsError::NotATree(
            "the wall's log has no readable changes; no first rings can be grown".into(),
        )
    })?;
    let last = changes
        .last()
        .ok_or_else(|| RingsError::NotATree("the wall's log has no readable changes".into()))?;
    let (model, known, evidence) = beginning(changes, proofs, abandoned)?;
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
    let mut walk = Walk {
        state,
        changes,
        raw,
        proofs,
        abandoned,
        model,
        importer,
        previous: Files::new(),
        mark: 0,
        report: Rings::default(),
        gap_lines: Vec::new(),
        start: String::new(),
        starts: 0,
        unknown_from: first.seq,
        unknown_reason: format!(
            "the wall before change {} could not be worked back exactly from {}",
            first.seq,
            evidence
                .path
                .strip_prefix(state)
                .unwrap_or(&evidence.path)
                .display()
        ),
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
            continue;
        }
        walk.change(change)?;
    }
    if walk.mark == 0 {
        return Err(RingsError::NotATree(
            "no known wall could be grown from the log and its kept files".into(),
        ));
    }
    walk.importer
        .tag("refs/tags/rings/end", &format!(":{}", walk.mark))?;
    walk.report.trunk = walk.importer.checkpoint(walk.mark)?;
    let summary = walk.summary(last);
    walk.importer.finish()?;
    Ok((walk.report, summary))
}

impl Walk<'_> {
    fn starting(&mut self, evidence: &model::Evidence, first: &Change) -> Result<(), TreeError> {
        let subject = if evidence.after < first.seq {
            format!(
                "the wall before change {}, as {} kept it",
                first.seq,
                evidence
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        } else {
            format!(
                "the wall before change {}, worked back from {}",
                first.seq,
                evidence
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
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
            first.seq.saturating_sub(1),
            &first.at,
            &subject,
            &evidence
                .path
                .strip_prefix(self.state)
                .unwrap_or(&evidence.path)
                .display()
                .to_string(),
            None,
        )?;
        Ok(())
    }
}

impl Walk<'_> {
    fn summary(&self, last: &Change) -> String {
        let model = &self.model;
        let report = &self.report;
        let start = &self.start;
        let starts = self.starts;
        let mark = self.mark;
        let gap_lines = &self.gap_lines;
        let state = self.state;
        let proofs = self.proofs;
        let patch_after = proofs
            .files
            .iter()
            .find(|e| e.path == state.join("patch.json"))
            .map(|e| e.after);
        let end = if patch_after == Some(last.seq) && model.known {
            ", as patch.json has it".into()
        } else {
            format!(
                "; patch.json is after change {}: the wall's next start says so",
                patch_after.unwrap_or(0)
            )
        };
        let omitted = model.omitted.iter().cloned().collect::<Vec<_>>();
        let knobs = omitted.iter().filter(|s| s.contains("knob")).count();
        let modules = omitted.iter().filter(|s| s.starts_with("module '")).count();
        let summary = format!(
            "start: {}\ncommits: {} ({} start, {} changes, {} gaps, {} settles)\n{}births: {} found at the start, {} at adds, {} kept across loads, {} found at loads\nthis build left out: {knobs} knobs, {modules} modules ({})\ntrunk: {} (change {}, {}){end}\n",
            start,
            mark,
            starts,
            report.changes,
            report.gaps,
            report.settles,
            if gap_lines.is_empty() {
                String::new()
            } else {
                gap_lines.join("\n") + "\n"
            },
            model.found_start,
            model.born,
            model.kept_load,
            model.found_load,
            describe(&omitted),
            report.trunk,
            last.seq,
            last.at
        );
        summary
    }
}

pub fn grow(state: &Path, tree: &Path, dry_run: bool) -> Result<Rings, RingsError> {
    let started = Instant::now();
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
    let (raw, bytes) = raw_lines(state)?;
    let proofs = Proofs::read(state, &loaded.changes)?;
    let abandoned = loaded
        .changes
        .iter()
        .filter_map(|c| {
            if let What::Abandoned { change } = c.what {
                Some(change)
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();
    let dry = if dry_run {
        let name = tree
            .file_name()
            .ok_or_else(|| {
                RingsError::NotATree(
                    "the tree directory has no name to grow a dry run beside".into(),
                )
            })?
            .to_string_lossy();
        Some(tree.with_file_name(format!(
            "{name}.dry-run-{}-{}",
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
    let result = run(
        state,
        destination,
        &loaded.changes,
        &raw,
        &proofs,
        &abandoned,
    );
    let cleanup = if dry.is_some() && destination.exists() {
        fs::remove_dir_all(destination)
            .err()
            .map(|e| format!("dry run could not remove {}: {e}\n", destination.display()))
    } else {
        None
    };
    let (mut report, summary) = match result {
        Ok(result) => result,
        Err(e) => {
            if let Some(cleanup) = cleanup {
                return Err(TreeError::Git(format!("{e}; {cleanup}")).into());
            }
            return Err(e);
        }
    };
    report.report = report_text(
        (state, tree),
        dry,
        &loaded,
        &proofs,
        (bytes, abandoned.len()),
        (&summary, cleanup, started),
    );
    Ok(report)
}

fn report_text(
    paths: (&Path, &Path),
    dry: Option<std::path::PathBuf>,
    loaded: &crate::store::LoadedLog,
    proofs: &Proofs,
    counts: (u64, usize),
    details: (&str, Option<String>, Instant),
) -> String {
    let (state, tree) = paths;
    let (bytes, abandoned) = counts;
    let (summary, cleanup, started) = details;
    let first = loaded.changes.first().map_or(0, |c| c.seq);
    let last = loaded.changes.last().map_or(0, |c| c.seq);
    let mode = dry.map_or_else(
        || "planted and grown".into(),
        |dir| format!("dry run: grown in {}, then removed", dir.display()),
    );
    let mb = bytes / 1_000_000;
    let fraction = (bytes % 1_000_000) / 1000;
    format!(
        "the first rings: {} → {} ({mode})\nlog: changes {first} to {last} ({} lines, {mb}.{fraction:03} MB); {} unreadable, {} repeated, {} abandoned\nevidence: {} wall files; {} load backups hold their fingerprints, {} do not ({}); {} unreadable ({})\n{summary}{}{}took {:.3} s\n",
        state.display(),
        tree.display(),
        loaded.changes.len() + loaded.skipped + loaded.repeated,
        loaded.skipped,
        loaded.repeated,
        abandoned,
        proofs.count,
        proofs.backups,
        proofs.bad_backups.len(),
        proofs.bad_backups.join(", "),
        proofs.unreadable.len(),
        proofs.unreadable.join(", "),
        if proofs.disagreements.is_empty() {
            String::new()
        } else {
            proofs.disagreements.join("\n") + "\n"
        },
        cleanup.unwrap_or_default(),
        started.elapsed().as_secs_f64()
    )
}
