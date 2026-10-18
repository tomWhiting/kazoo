//! Git reads and HEAD movement for the wall's current line.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use super::{Answered, Line, News, Tip, TreeItem, tree_error};
use crate::patch::PatchFile;
use crate::protocol::MarkResult;
use crate::protocol::tree::{DEFAULT_RECENT, MAX_LISTED, MAX_MARKS, MAX_MOMENT, MAX_RECENT};
use crate::protocol::{BranchView, MarkKind, MarkView, MomentView, TreePlace, TreeResult};
use crate::tree::TreeError;
use crate::tree::git::{Importer, Planted};
use crate::tree::{Files, git};

mod ancestry;

/// A whole sentence explaining why a tree job could not be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HereError {
    UnknownMoment(String),
    MarkExists(String),
    Full(String),
    Broken(String),
    Down(String),
}

impl fmt::Display for HereError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMoment(s)
            | Self::MarkExists(s)
            | Self::Full(s)
            | Self::Broken(s)
            | Self::Down(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for HereError {}

/// Encode a valid save name reversibly as one Git ref component.
#[must_use]
pub fn ref_part(name: &str) -> String {
    let mut part = String::with_capacity(name.len());
    let mut previous = None;
    for ch in name.chars() {
        part.push(match ch {
            ' ' => '+',
            '.' if previous == Some('.') => '=',
            other => other,
        });
        previous = Some(ch);
    }
    if part.as_bytes().ends_with(b".lock") {
        let dot = part.len() - 5;
        part.replace_range(dot..=dot, "=");
    }
    part
}

/// Read the original save name from its ref component.
#[must_use]
pub fn name_of(part: &str) -> String {
    part.replace('+', " ").replace('=', ".")
}

fn output(command: &mut Command) -> Result<Output, HereError> {
    command
        .output()
        .map_err(|err| HereError::Broken(format!("the tree could not run {command:?}: {err}")))
}

fn checked(command: &mut Command) -> Result<String, HereError> {
    let result = output(command)?;
    if !result.status.success() {
        return Err(HereError::Broken(format!(
            "the tree could not run {command:?}: {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    String::from_utf8(result.stdout)
        .map_err(|err| HereError::Broken(format!("the tree's git output is not text: {err}")))
}

fn optional(command: &mut Command) -> Result<Option<String>, HereError> {
    let result = output(command)?;
    if result.status.success() {
        let value = String::from_utf8(result.stdout).map_err(|err| {
            HereError::Broken(format!("the tree's git output is not text: {err}"))
        })?;
        Ok(Some(value.trim().into()))
    } else if result.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(HereError::Broken(format!(
            "the tree could not read {command:?}: {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )))
    }
}

fn number(text: &str) -> Result<Option<u64>, HereError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some).map_err(|err| {
        HereError::Broken(format!(
            "the tree's change number {text:?} cannot be read: {err}"
        ))
    })
}

fn through(tree: &Path, commit: &str) -> Result<Option<u64>, HereError> {
    git::through_at(tree, commit).map_err(|err| HereError::Broken(err.to_string()))
}

/// Read HEAD, including an unborn branch and a detached moment.
pub fn read_line(tree: &Path) -> Result<(Line, Option<u64>), HereError> {
    if let Some(branch) =
        optional(git::git_command("symbolic-ref", Some(tree)).args(["-q", "HEAD"]))?
    {
        let name = branch.strip_prefix("refs/heads/").ok_or_else(|| {
            HereError::Broken(format!(
                "the tree's HEAD names {branch}, which is not a line"
            ))
        })?;
        let tip = branch_tip(tree, name)?;
        let seq = tip
            .as_deref()
            .map(|tip| through(tree, tip))
            .transpose()?
            .flatten();
        Ok((
            Line::On {
                branch: name.into(),
                tip,
            },
            seq,
        ))
    } else {
        let commit =
            optional(git::git_command("rev-parse", Some(tree)).args(["-q", "--verify", "HEAD"]))?
                .ok_or_else(no_moment)?;
        let seq = through(tree, &commit)?;
        Ok((Line::At { commit, mark: None }, seq))
    }
}

fn no_moment() -> HereError {
    HereError::UnknownMoment("the tree has no moment yet: nothing has been committed".into())
}

pub(super) fn branch_tip(tree: &Path, branch: &str) -> Result<Tip, HereError> {
    let reference = format!("refs/heads/{branch}");
    checked(git::git_command("check-ref-format", Some(tree)).arg(&reference))?;
    optional(git::git_command("rev-parse", Some(tree)).args([
        "-q",
        "--verify",
        &format!("{reference}^{{commit}}"),
    ]))
}

/// Move only HEAD; branch and tag refs are never updated here.
pub(super) fn move_head(tree: &Path, line: &Line, summary: &str) -> Result<(), HereError> {
    let message = format!("kazoo: {}", summary.replace(['\r', '\n'], " "));
    match line {
        Line::On { branch, .. } => {
            let reference = format!("refs/heads/{branch}");
            let current =
                optional(git::git_command("symbolic-ref", Some(tree)).args(["-q", "HEAD"]))?;
            if current.as_deref() != Some(reference.as_str()) {
                checked(
                    git::git_command("symbolic-ref", Some(tree))
                        .args(["-m", &message, "HEAD", &reference]),
                )?;
            }
        }
        Line::At { commit, .. } => {
            checked(git::git_command("update-ref", Some(tree)).args([
                "--no-deref",
                "-m",
                &message,
                "HEAD",
                commit,
            ]))?;
        }
    }
    Ok(())
}

pub(in crate::tree) fn compact_time(at: &str) -> Result<String, HereError> {
    let date = chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%SZ").map_err(|err| {
        HereError::Broken(format!(
            "the tree cannot read the moment's UTC time {at:?}: {err}"
        ))
    })?;
    Ok(date.format("%Y%m%dT%H%M%SZ").to_string())
}

pub(super) fn unused(tree: &Path, prefix: &str) -> Result<String, HereError> {
    for suffix in 1..=99 {
        let reference = if suffix == 1 {
            prefix.into()
        } else {
            format!("{prefix}-{suffix}")
        };
        if optional(git::git_command("rev-parse", Some(tree)).args(["-q", "--verify", &reference]))?
            .is_none()
        {
            checked(git::git_command("check-ref-format", Some(tree)).arg(&reference))?;
            return Ok(reference);
        }
    }
    Err(HereError::Full(format!(
        "the tree already holds 99 names at {prefix}; this moment needs another second"
    )))
}

const MOMENT_FORMAT: &str = "--format=%H%x00%an%x00%aI%x00%s%x00%(trailers:key=Kazoo-Kind,valueonly,separator=%x2C)%x00%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C)";

fn moments(text: &str) -> Result<Vec<MomentView>, HereError> {
    let mut fields: Vec<_> = text.split('\0').collect();
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    if fields.len() % 6 != 0 {
        return Err(HereError::Broken(
            "the tree's commit metadata is incomplete".into(),
        ));
    }
    fields
        .chunks_exact(6)
        .map(|row| {
            let commit = row[0].trim_start_matches('\n');
            if !full_id(commit) {
                return Err(HereError::Broken(
                    "the tree returned an invalid commit id".into(),
                ));
            }
            Ok(MomentView {
                commit: commit.into(),
                short: commit[..12].into(),
                seat: row[1].into(),
                at: row[2].into(),
                summary: row[3].into(),
                change: if is_change(row[4]) {
                    number(row[5])?
                } else {
                    None
                },
            })
        })
        .collect()
}

/// A commit of one logged change: a change's own, or a batch's (one seq,
/// one commit, its kind `batch`, KZ-T10a).
pub(in crate::tree) fn is_change(kind: &str) -> bool {
    matches!(kind.trim(), "change" | "batch")
}

fn full_id(value: &str) -> bool {
    value.len() == 40 && lower_hex(value)
}

fn lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn moment(tree: &Path, commit: &str) -> Result<MomentView, HereError> {
    let text = checked(git::git_command("log", Some(tree)).args([
        "-z",
        "--no-walk=unsorted",
        MOMENT_FORMAT,
        commit,
        "--",
    ]))?;
    moments(&text)?.into_iter().next().ok_or_else(no_moment)
}

pub(super) fn place(tree: &Path, line: &Line) -> Result<TreePlace, HereError> {
    let (commit, branch, mark) = match line {
        Line::On { branch, tip } => (
            tip.as_ref().ok_or_else(no_moment)?,
            Some(branch.clone()),
            None,
        ),
        Line::At { commit, mark } => (commit, None, mark.clone()),
    };
    Ok(TreePlace {
        commit: commit.clone(),
        branch,
        mark,
        change: moment(tree, commit)?.change,
    })
}

#[derive(Debug)]
struct Reference {
    name: String,
    commit: String,
}

fn refs(tree: &Path) -> Result<Vec<Reference>, HereError> {
    let text = checked(git::git_command("for-each-ref", Some(tree)).args([
        "--format=%(refname)%00%(objectname)",
        "--sort=refname",
        "--sort=-committerdate",
        "refs/heads/",
        "refs/tags/mark/",
        "refs/tags/save/",
    ]))?;
    text.lines()
        .map(|row| {
            let (name, commit) = row
                .split_once('\0')
                .ok_or_else(|| HereError::Broken("the tree's ref metadata is incomplete".into()))?;
            if !full_id(commit) {
                return Err(HereError::Broken(format!(
                    "the tree's ref {name} does not name a commit id"
                )));
            }
            Ok(Reference {
                name: name.into(),
                commit: commit.into(),
            })
        })
        .collect()
}

pub(super) fn mark_ref(tree: &Path, name: &str, console: bool) -> Result<String, HereError> {
    if !crate::protocol::valid_save_name(name) {
        return Err(HereError::Broken(
            "a mark's name must be a valid save name".into(),
        ));
    }
    let refs = refs(tree)?;
    let marks: Vec<_> = refs
        .iter()
        .filter_map(|r| {
            r.name
                .strip_prefix("refs/tags/mark/")
                .map(|part| (name_of(part), &r.commit))
        })
        .collect();
    if let Some((existing, commit)) = marks
        .iter()
        .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
    {
        return Err(HereError::MarkExists(format!(
            "a mark \"{existing}\" names {} already; marks never move: choose another name",
            &commit[..12]
        )));
    }
    if !console && marks.len() >= MAX_MARKS {
        return Err(HereError::Full(
            "the tree holds 1,000 marks; only Tom's console marks more".into(),
        ));
    }
    Ok(format!("refs/tags/mark/{}", ref_part(name)))
}

/// A lazy change index, extended when later change commits become durable.
#[derive(Debug, Default)]
pub(super) struct Resolver {
    seq: Option<BTreeMap<u64, String>>,
}

const SEQ_FORMAT: &str = "--format=%H%x00%(trailers:key=Kazoo-Kind,valueonly,separator=%x2C)%x00%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C)";

fn changes(tree: &Path, branch: Option<(&str, u64)>) -> Result<BTreeMap<u64, String>, HereError> {
    let mut command = git::git_command("log", Some(tree));
    command.args(["-z", SEQ_FORMAT]);
    if let Some((branch, count)) = branch {
        command
            .arg(format!("-n{count}"))
            .arg(format!("refs/heads/{branch}"));
    } else {
        command.arg("--all");
    }
    command.arg("--");
    let text = checked(&mut command)?;
    let mut fields: Vec<_> = text.split('\0').collect();
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    if fields.len() % 3 != 0 {
        return Err(HereError::Broken(
            "the tree's change index is incomplete".into(),
        ));
    }
    let mut map = BTreeMap::new();
    for row in fields.chunks_exact(3) {
        if is_change(row[1]) {
            let seq = number(row[2])?
                .ok_or_else(|| HereError::Broken("a change commit has no change number".into()))?;
            map.insert(seq, row[0].trim_start_matches('\n').into());
        }
    }
    Ok(map)
}

impl Resolver {
    pub(super) fn extend(
        &mut self,
        tree: &Path,
        branch: &str,
        count: u64,
    ) -> Result<(), HereError> {
        if let Some(seq) = &mut self.seq {
            seq.extend(changes(tree, Some((branch, count)))?);
        }
        Ok(())
    }

    pub(super) fn resolve(
        &mut self,
        tree: &Path,
        line: &Line,
        input: &str,
    ) -> Result<TreePlace, HereError> {
        if matches!(line, Line::On { tip: Some(tip), .. } if tip.starts_with(':')) {
            return Err(HereError::Broken(
                "the tree must checkpoint its line before resolving a moment".into(),
            ));
        }
        if input.len() > MAX_MOMENT {
            return Err(unknown(input));
        }
        if (input == "trunk" || input.starts_with("bud/"))
            && let Some(commit) = branch_tip(tree, input)?
        {
            return found(tree, commit, Some(input.into()), None);
        }
        if full_id(input) {
            if let Some(commit) = commit_id(tree, input)? {
                return found(tree, commit, None, None);
            }
            return Err(unknown(input));
        }
        let refs = refs(tree)?;
        for r in &refs {
            if let Some(part) = r.name.strip_prefix("refs/tags/mark/") {
                let name = name_of(part);
                if name.eq_ignore_ascii_case(input) {
                    return found(tree, r.commit.clone(), None, Some(name));
                }
            }
        }
        if input.starts_with("save/")
            && let Some(r) = refs
                .iter()
                .find(|r| r.name.strip_prefix("refs/tags/") == Some(input))
        {
            return found(tree, r.commit.clone(), None, None);
        }
        if let Some(digits) = input.strip_prefix('#')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            let n = digits.parse::<u64>().map_err(|_| unknown(input))?;
            if self.seq.is_none() {
                self.seq = Some(changes(tree, None)?);
            }
            if let Some(commit) = self.seq.as_ref().and_then(|seq| seq.get(&n)) {
                return found(tree, commit.clone(), None, None);
            }
            return Err(HereError::UnknownMoment(format!(
                "change {n} has no moment of its own: a go, a change that did not happen, or one the tree has not taken yet"
            )));
        }
        if (7..40).contains(&input.len())
            && lower_hex(input)
            && let Some(commit) = commit_id(tree, input)?
        {
            return found(tree, commit, None, None);
        }
        Err(unknown(input))
    }
}

fn unknown(input: &str) -> HereError {
    HereError::UnknownMoment(format!(
        "no moment \"{input}\": give a branch (trunk, bud/…), a mark, a change as #<number>, or a commit (7 to 40 hex digits)"
    ))
}

fn commit_id(tree: &Path, input: &str) -> Result<Option<String>, HereError> {
    let mut command = git::git_command("rev-parse", Some(tree));
    command.args([
        "-q",
        "--verify",
        "--end-of-options",
        &format!("{input}^{{commit}}"),
    ]);
    let result = output(&mut command)?;
    if result.status.success() {
        return Ok(Some(
            String::from_utf8(result.stdout)
                .map_err(|err| {
                    HereError::Broken(format!("the tree's commit id is not text: {err}"))
                })?
                .trim()
                .into(),
        ));
    }
    let why = String::from_utf8_lossy(&result.stderr);
    if why.contains("ambiguous") {
        return Err(HereError::UnknownMoment(format!(
            "{input} names more than one moment; give more of it"
        )));
    }
    if input.len() < 40 {
        let matches = checked(
            git::git_command("rev-parse", Some(tree)).arg(format!("--disambiguate={input}")),
        )?;
        if matches.lines().nth(1).is_some() {
            return Err(HereError::UnknownMoment(format!(
                "{input} names more than one moment; give more of it"
            )));
        }
    }
    if result.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(HereError::Broken(format!(
            "the tree could not resolve {input}: {}: {}",
            result.status,
            why.trim()
        )))
    }
}

fn found(
    tree: &Path,
    commit: String,
    branch: Option<String>,
    mark: Option<String>,
) -> Result<TreePlace, HereError> {
    Ok(TreePlace {
        change: moment(tree, &commit)?.change,
        commit,
        branch,
        mark,
    })
}

/// Resolve a branch, whole id, mark, save tag, change number or short id.
pub fn resolve(tree: &Path, line: &Line, moment: &str) -> Result<TreePlace, HereError> {
    Resolver::default().resolve(tree, line, moment)
}

fn blob_entries(listing: &[u8]) -> Result<Vec<(String, String)>, HereError> {
    let mut entries = Vec::new();
    for row in listing.split(|b| *b == 0).filter(|row| !row.is_empty()) {
        let text = std::str::from_utf8(row)
            .map_err(|err| HereError::Broken(format!("the tree's path is not text: {err}")))?;
        let (header, path) = text
            .split_once('\t')
            .ok_or_else(|| HereError::Broken("the tree's blob listing is incomplete".into()))?;
        let parts: Vec<_> = header.split(' ').collect();
        if parts.len() != 3 || parts[0] != "100644" || parts[1] != "blob" || !full_id(parts[2]) {
            return Err(HereError::Broken(format!(
                "the tree's file {path} is not an ordinary blob"
            )));
        }
        if !crate::tree::notes::valid_path(path) {
            entries.push((path.to_string(), parts[2].to_string()));
        }
    }
    Ok(entries)
}

/// Read all blobs through one batch child, waiting for it on every path.
pub(super) fn files(tree: &Path, commit: &str) -> Result<Files, HereError> {
    let listing =
        output(git::git_command("ls-tree", Some(tree)).args(["-r", "-z", "--full-tree", commit]))?;
    if !listing.status.success() {
        return Err(HereError::Broken(format!(
            "the tree cannot list moment {}: {}: {}",
            &commit[..12],
            listing.status,
            String::from_utf8_lossy(&listing.stderr).trim()
        )));
    }
    let entries = blob_entries(&listing.stdout)?;
    let mut command = git::git_command("cat-file", Some(tree));
    command
        .arg("--batch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|err| HereError::Broken(format!("the tree cannot read its blobs: {err}")))?;
    let result =
        (|| {
            let mut input = child
                .stdin
                .take()
                .ok_or_else(|| HereError::Broken("the tree's blob reader has no input".into()))?;
            let mut reader =
                BufReader::new(child.stdout.take().ok_or_else(|| {
                    HereError::Broken("the tree's blob reader has no output".into())
                })?);
            let mut files = Files::new();
            let mut total = 0usize;
            for (path, oid) in entries {
                writeln!(input, "{oid}")
                    .and_then(|()| input.flush())
                    .map_err(|err| {
                        HereError::Broken(format!("the tree could not request blob {oid}: {err}"))
                    })?;
                let mut header = String::new();
                reader.read_line(&mut header).map_err(|err| {
                    HereError::Broken(format!(
                        "the tree could not read blob {oid}'s header: {err}"
                    ))
                })?;
                let fields: Vec<_> = header.split_whitespace().collect();
                if fields.len() != 3 || fields[0] != oid || fields[1] != "blob" {
                    return Err(HereError::Broken(format!(
                        "the tree cannot read blob {oid}: {}",
                        header.trim()
                    )));
                }
                let size: usize = fields[2].parse().map_err(|err| {
                    HereError::Broken(format!("the tree's blob size cannot be read: {err}"))
                })?;
                total = total
                    .checked_add(size)
                    .ok_or_else(|| HereError::Broken("the tree's wall is too large".into()))?;
                if u64::try_from(total).unwrap_or(u64::MAX) > crate::store::MAX_WALL_FILE {
                    return Err(HereError::Broken(
                        "the tree's wall exceeds the wall file limit".into(),
                    ));
                }
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes).map_err(|err| {
                    HereError::Broken(format!("the tree could not read blob {oid}: {err}"))
                })?;
                let mut end = [0];
                reader.read_exact(&mut end).map_err(|err| {
                    HereError::Broken(format!("the tree could not finish blob {oid}: {err}"))
                })?;
                if end != *b"\n" {
                    return Err(HereError::Broken(
                        "the tree's blob has no terminating newline".into(),
                    ));
                }
                files.insert(path, bytes);
            }
            Ok(files)
        })();
    child.stdin.take();
    child.stdout.take();
    let status = child.wait().map_err(|err| {
        HereError::Broken(format!(
            "the tree could not wait for its blob reader: {err}"
        ))
    })?;
    if !status.success() {
        return Err(HereError::Broken(format!(
            "the tree's blob reader failed: {status}"
        )));
    }
    result
}

pub(in crate::tree) fn wall(tree: &Path, commit: &str) -> Result<Box<PatchFile>, HereError> {
    let result = files(tree, commit).and_then(|files| {
        crate::tree::from_files(&files).map_err(|err| HereError::Broken(err.to_string()))
    });
    result.map(Box::new).map_err(|err| {
        HereError::Broken(format!(
            "the moment {}'s files cannot be read: {err}",
            &commit[..12]
        ))
    })
}

fn listed_refs(all: Vec<Reference>) -> (Vec<Reference>, usize, usize) {
    let branch_count = all
        .iter()
        .filter(|r| r.name.starts_with("refs/heads/"))
        .count();
    let mark_count = all.len() - branch_count;
    let mut branches_left = MAX_LISTED;
    let mut marks_left = MAX_LISTED;
    let refs: Vec<_> = all
        .into_iter()
        .filter(|r| {
            let left = if r.name.starts_with("refs/heads/") {
                &mut branches_left
            } else {
                &mut marks_left
            };
            if *left == 0 {
                false
            } else {
                *left -= 1;
                true
            }
        })
        .collect();
    (refs, branch_count, mark_count)
}

fn ref_views(
    refs: Vec<Reference>,
    by_id: &BTreeMap<String, MomentView>,
) -> Result<(Vec<BranchView>, Vec<MarkView>), HereError> {
    let mut branches = Vec::new();
    let mut marks = Vec::new();
    for r in refs {
        let m = by_id
            .get(&r.commit)
            .ok_or_else(|| {
                HereError::Broken(format!("the tree's ref {} has no readable commit", r.name))
            })?
            .clone();
        if let Some(name) = r.name.strip_prefix("refs/heads/") {
            let grew_from = name
                .strip_prefix("bud/")
                .and_then(|s| s.split_once('/'))
                .map(|(from, _)| name_of(from));
            branches.push(BranchView {
                name: name.into(),
                tip: m,
                grew_from,
            });
        } else if let Some(part) = r.name.strip_prefix("refs/tags/mark/") {
            marks.push(MarkView {
                name: name_of(part),
                kind: MarkKind::Mark,
                tag: r.name.trim_start_matches("refs/tags/").into(),
                moment: m,
            });
        } else if let Some(part) = r.name.strip_prefix("refs/tags/save/") {
            let (name, _) = part.rsplit_once('/').ok_or_else(|| {
                HereError::Broken(format!("the tree's save mark {} has no time", r.name))
            })?;
            marks.push(MarkView {
                name: name_of(name),
                kind: MarkKind::Save,
                tag: r.name.trim_start_matches("refs/tags/").into(),
                moment: m,
            });
        }
    }
    Ok((branches, marks))
}

pub(super) fn list(
    tree: &Path,
    line: &Line,
    through: u64,
    recent: Option<u32>,
) -> Result<TreeResult, HereError> {
    let here = match line {
        Line::On { tip: None, .. } => None,
        _ => Some(place(tree, line)?),
    };
    let all = refs(tree)?;
    let (refs, branch_count, mark_count) = listed_refs(all);
    let mut command = git::git_command("log", Some(tree));
    command.args(["-z", "--no-walk=unsorted", MOMENT_FORMAT]);
    let mut ids: Vec<_> = refs.iter().map(|r| r.commit.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let by_id: BTreeMap<_, _> = if ids.is_empty() {
        BTreeMap::new()
    } else {
        command.args(ids).arg("--");
        moments(&checked(&mut command)?)?
            .into_iter()
            .map(|m| (m.commit.clone(), m))
            .collect()
    };
    let (branches, marks) = ref_views(refs, &by_id)?;
    let count = recent.unwrap_or(DEFAULT_RECENT).min(MAX_RECENT);
    let recent = if let Some(here) = &here {
        if count == 0 {
            Vec::new()
        } else {
            moments(&checked(git::git_command("log", Some(tree)).args([
                "-z",
                "--first-parent",
                &format!("-n{count}"),
                MOMENT_FORMAT,
                &here.commit,
                "--",
            ]))?)?
        }
    } else {
        Vec::new()
    };
    let parents = ancestry::read(tree, &by_id, here.as_ref())?;
    Ok(TreeResult {
        parents,
        here,
        through,
        revision: 0,
        branches,
        branch_count,
        marks,
        mark_count,
        recent,
    })
}

pub(super) fn released(item: &TreeItem, bytes: &AtomicUsize, waiting: &AtomicUsize) -> bool {
    match item.bytes() {
        Ok(size) => {
            bytes.fetch_sub(size, Ordering::AcqRel);
        }
        Err(err) => eprintln!("kazoo-wall: tree item accounting: {err}"),
    }
    waiting.fetch_sub(1, Ordering::AcqRel);
    matches!(item, TreeItem::Finish)
}

impl super::Writer {
    pub(super) fn grow(&mut self, dir: &Path, at: &str) -> Result<(), TreeError> {
        if let Line::At { commit, mark } = &self.line {
            let from = mark
                .as_deref()
                .map_or_else(|| commit[..12].into(), ref_part);
            let time = compact_time(at).map_err(tree_error)?;
            let reference =
                unused(dir, &format!("refs/heads/bud/{from}/{time}")).map_err(tree_error)?;
            let branch = reference.trim_start_matches("refs/heads/").to_string();
            let tip = Some(commit.clone());
            self.importer = Some(Importer::new(
                dir,
                &Planted {
                    branch: reference,
                    tip: tip.clone(),
                    through: self.through,
                },
            )?);
            self.line = Line::On { branch, tip };
        } else if self.importer.is_none()
            && let Line::On { branch, tip } = &self.line
        {
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

    pub(super) fn answer(
        &mut self,
        item: TreeItem,
        ticket: u64,
        dir: &Path,
        news: &mpsc::Sender<News>,
    ) -> Result<(), TreeError> {
        let answer = match self.checkpoint(dir, news) {
            Ok(()) => self.job(item, dir),
            Err(err) => Err(HereError::Down(err.to_string())),
        };
        let failure = match &answer {
            Err(HereError::Down(why)) => Some(TreeError::Git(why.clone())),
            _ => None,
        };
        super::tell(
            news,
            News::Answer {
                ticket,
                through: self.through.unwrap_or(0),
                answer,
            },
        );
        failure.map_or(Ok(()), Err)
    }

    pub(super) fn save_mark(
        &mut self,
        dir: &Path,
        news: &mpsc::Sender<News>,
        name: &str,
        at: &str,
    ) -> Result<(), TreeError> {
        let result = (|| {
            self.checkpoint(dir, news)?;
            let place = place(dir, &self.line).map_err(tree_error)?;
            if !crate::protocol::valid_save_name(name) {
                return Err(TreeError::Git(
                    "a save mark's name must be a valid save name".into(),
                ));
            }
            let time = compact_time(at).map_err(tree_error)?;
            let reference = unused(
                dir,
                &format!(
                    "refs/tags/save/{}/{time}",
                    ref_part(&name.to_ascii_lowercase())
                ),
            )
            .map_err(tree_error)?;
            self.tag(dir, &reference, &place.commit)
        })();
        result.map_err(|err| {
            TreeError::Git(format!(
                "the save \"{name}\" could not be marked in the tree: {err}"
            ))
        })
    }

    pub(super) fn tag(
        &mut self,
        dir: &Path,
        reference: &str,
        commit: &str,
    ) -> Result<(), TreeError> {
        if self.importer.is_some() {
            self.importer()?.tag(reference, commit)?;
            let mark = self.last.map_or(0, |(mark, _)| mark);
            let actual = self.importer()?.checkpoint(mark)?;
            if let Line::On { tip: Some(tip), .. } = &self.line
                && actual != *tip
            {
                return Err(TreeError::Git(format!(
                    "the tree's line is at {actual}, but the wall is at {tip}"
                )));
            }
        } else {
            let mut importer = Importer::new(
                dir,
                &Planted {
                    branch: "refs/heads/trunk".into(),
                    tip: None,
                    through: self.through,
                },
            )?;
            importer.tag(reference, commit)?;
            importer.finish()?;
        }
        Ok(())
    }

    pub(super) fn job(&mut self, item: TreeItem, dir: &Path) -> Result<Answered, HereError> {
        match item {
            TreeItem::Mark { name, console, .. } => {
                let at = place(dir, &self.line)?;
                let reference = mark_ref(dir, &name, console)?;
                self.tag(dir, &reference, &at.commit)
                    .map_err(|err| HereError::Down(err.to_string()))?;
                Ok(Answered::Marked(MarkResult {
                    name,
                    tag: reference.trim_start_matches("refs/tags/").into(),
                    at,
                }))
            }
            TreeItem::Resolve { moment, .. } => {
                let to = self.resolver.resolve(dir, &self.line, &moment)?;
                let wall = wall(dir, &to.commit)?;
                Ok(Answered::Resolved {
                    to,
                    here: place(dir, &self.line)?,
                    wall,
                })
            }
            TreeItem::Here { .. } => Ok(Answered::Here(place(dir, &self.line)?)),
            TreeItem::List { recent, .. } => Ok(Answered::Listed(Box::new(list(
                dir,
                &self.line,
                self.through.unwrap_or(0),
                recent,
            )?))),
            _ => Err(HereError::Broken(
                "the tree received an item that is not a question".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests;
