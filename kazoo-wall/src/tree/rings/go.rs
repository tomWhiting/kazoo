use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::model::{Found, same};
use super::{Rings, RingsError, Walk, git_output, git_text, in_already, reference, short};
use crate::patch::PatchFile;
use crate::protocol::{Change, TreePlace, What};
use crate::tree::git::{Importer, Planted};
use crate::tree::{Files, TreeError, from_files};

pub(super) struct Lineage {
    tree: PathBuf,
    branch: String,
    track: bool,
    pub(super) tip: String,
    at: Option<(String, Option<String>)>,
    whole: bool,
    seq_commit: HashMap<u64, String>,
    branch_of: HashMap<String, String>,
}

impl Lineage {
    pub(super) fn new(tree: &Path, changes: &[Change]) -> Self {
        Self {
            tree: tree.into(),
            branch: "refs/heads/trunk".into(),
            // Logs without movement do not need durable commit lookups.
            track: changes.iter().any(|c| matches!(c.what, What::Go { .. })),
            tip: String::new(),
            at: None,
            whole: false,
            seq_commit: HashMap::new(),
            branch_of: HashMap::new(),
        }
    }
}

pub(super) fn already(tree: &Path, end: &str, trunk: String) -> Result<Rings, RingsError> {
    let output = git_output(tree, "symbolic-ref", &["-q", "HEAD"])?;
    let (line, tip) = if output.status.success() {
        let branch = String::from_utf8(output.stdout).map_err(|e| TreeError::Git(e.to_string()))?;
        let branch = branch.trim();
        if branch == "refs/heads/trunk" && reference(tree, branch)?.as_deref() == Some(end) {
            return in_already(tree, end, trunk);
        }
        let tip = reference(tree, branch)?
            .ok_or_else(|| TreeError::Git(format!("the imported line {branch} has no commit")))?;
        (branch.trim_start_matches("refs/heads/").to_string(), tip)
    } else if output.status.code() == Some(1) {
        ("HEAD".into(), git_text(tree, "rev-parse", &["HEAD"])?)
    } else {
        return Err(TreeError::Git(format!(
            "read imported HEAD: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    };
    let mut report = in_already(tree, end, tip)?;
    report.report = report.report.replace("trunk", &line);
    report.trunk = trunk;
    Ok(report)
}

fn wall(tree: &Path, commit: &str) -> Result<PatchFile, TreeError> {
    let paths = git_text(tree, "ls-tree", &["-r", "--name-only", commit])?;
    let mut files = Files::new();
    for path in paths.lines() {
        let output = git_output(tree, "show", &[&format!("{commit}:{path}")])?;
        if !output.status.success() {
            return Err(TreeError::Git(format!(
                "read moment {commit}'s {path}: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        files.insert(path.into(), output.stdout);
    }
    from_files(&files)
}

fn differences(file: &PatchFile, modules: u32, cables: u32, tempo: f64) -> Vec<String> {
    let mut differences = Vec::new();
    if u32::try_from(file.modules.len()) != Ok(modules) {
        differences.push(format!("{} modules, go says {modules}", file.modules.len()));
    }
    if u32::try_from(file.cables.len()) != Ok(cables) {
        differences.push(format!("{} cables, go says {cables}", file.cables.len()));
    }
    if !same(file.tempo, tempo) {
        differences.push(format!("{} BPM, go says {tempo}", file.tempo));
    }
    differences
}

fn destination(to: &TreePlace) -> String {
    to.change.map_or_else(
        || to.mark.as_deref().unwrap_or(&to.commit).into(),
        |seq| seq.to_string(),
    )
}

fn bud(tree: &Path, from: &str, at: &str) -> Result<String, TreeError> {
    let time = chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%SZ")
        .map_err(|e| TreeError::Git(format!("go date: {e}")))?
        .format("%Y%m%dT%H%M%SZ")
        .to_string();
    let prefix = format!("refs/heads/bud/{from}/{time}");
    for suffix in 1..=99 {
        let name = if suffix == 1 {
            prefix.clone()
        } else {
            format!("{prefix}-{suffix}")
        };
        if reference(tree, &name)?.is_none() {
            git_text(tree, "check-ref-format", &[&name])?;
            return Ok(name);
        }
    }
    Err(TreeError::Git(format!(
        "the tree already holds 99 names at {prefix}; this moment needs another second"
    )))
}

impl Walk<'_> {
    pub(super) fn finish_line(&mut self) -> Result<(), TreeError> {
        let commit = self.importer.checkpoint(self.mark)?;
        if self.line.at.is_none() {
            self.line.tip = commit;
        }
        self.report.trunk = reference(&self.line.tree, "refs/heads/trunk")?
            .ok_or_else(|| TreeError::Git("the imported trunk has no commit".into()))?;
        Ok(())
    }

    pub(super) fn line_summary(&self, seq: u64, at: &str) -> String {
        let line = if self.line.at.is_some() {
            "HEAD"
        } else {
            self.line.branch.trim_start_matches("refs/heads/")
        };
        format!("{line}: {} (change {seq}, {at})", self.line.tip)
    }

    pub(super) fn remember(&mut self, change: &Change) -> Result<(), TreeError> {
        if self.line.track {
            let commit = self.importer.checkpoint(self.mark)?;
            self.line
                .branch_of
                .insert(commit.clone(), self.line.branch.clone());
            self.line.seq_commit.insert(change.seq, commit.clone());
            self.line.tip = commit;
        }
        Ok(())
    }

    pub(super) fn remember_evidence(&mut self, seq: u64) -> Result<(), TreeError> {
        if self.line.seq_commit.contains_key(&seq) {
            let commit = self.importer.checkpoint(self.mark)?;
            self.line
                .branch_of
                .insert(commit.clone(), self.line.branch.clone());
            self.line.seq_commit.insert(seq, commit.clone());
            self.line.tip = commit;
        }
        Ok(())
    }

    fn switch(&mut self, branch: String, parent: String) -> Result<(), TreeError> {
        self.importer.checkpoint(self.mark)?;
        let importer = Importer::new(
            &self.line.tree,
            &Planted {
                branch: branch.clone(),
                tip: Some(parent.clone()),
                through: Some(self.tip.0),
            },
        )?;
        std::mem::replace(&mut self.importer, importer).finish()?;
        self.mark = 0;
        git_text(&self.line.tree, "symbolic-ref", &["HEAD", &branch])?;
        self.line.branch = branch;
        self.line.tip = parent;
        self.line.at = None;
        Ok(())
    }

    pub(super) fn grow_line(&mut self, at: &str) -> Result<bool, TreeError> {
        if let Some((parent, mark)) = self.line.at.clone() {
            let from = mark.as_deref().map_or_else(
                || short(&parent).into(),
                crate::tree::thread::here::ref_part,
            );
            let branch = bud(&self.line.tree, &from, at)?;
            self.switch(branch, parent)?;
        }
        Ok(std::mem::take(&mut self.line.whole))
    }

    fn detach(&mut self, parent: String, mark: Option<String>) -> Result<(), TreeError> {
        self.importer.checkpoint(self.mark)?;
        git_text(
            &self.line.tree,
            "update-ref",
            &["--no-deref", "HEAD", &parent],
        )?;
        self.line.tip.clone_from(&parent);
        self.line.at = Some((parent, mark));
        Ok(())
    }

    fn return_place(&self, undone: u64) -> Option<&TreePlace> {
        let original = self.log.changes.iter().find(|c| c.seq == undone)?;
        let What::Go { from, .. } = &original.what else {
            return None;
        };
        Some(from)
    }

    fn move_to(&mut self, place: &TreePlace, parent: String) -> Result<(), TreeError> {
        if place.branch.is_some() {
            if let Some(branch) = self.line.branch_of.get(&parent).cloned() {
                if reference(&self.line.tree, &branch)?.as_deref() == Some(parent.as_str()) {
                    return self.switch(branch, parent);
                }
            }
        }
        self.detach(parent, place.mark.clone())
    }

    fn incoming_go(
        &self,
        change: &Change,
        held: Option<&str>,
    ) -> Result<(PatchFile, Option<String>), TreeError> {
        if change.undoes.is_none() {
            if let Some(commit) = held {
                return Ok((wall(&self.line.tree, commit)?, None));
            }
        }
        self.log
            .proofs
            .incoming(self.log.state, change, self.log.changes)
            .map_err(TreeError::Git)
    }

    pub(super) fn go(&mut self, change: &Change) -> Result<(), TreeError> {
        let What::Go {
            to,
            modules,
            cables,
            tempo,
            ..
        } = &change.what
        else {
            return Err(TreeError::Git(format!("change {} is not a go", change.seq)));
        };
        let held = to
            .change
            .and_then(|seq| self.line.seq_commit.get(&seq))
            .cloned();
        let (file, note) = match self.incoming_go(change, held.as_deref()) {
            Ok(incoming) => incoming,
            Err(why) => {
                self.model.known = false;
                self.unknown_from = change.seq;
                self.unknown_reason = format!(
                    "the wall change {} went to a moment the import does not hold ({})",
                    change.seq,
                    destination(to)
                );
                self.gap_lines
                    .push(format!("{}: {why}", self.unknown_reason));
                return Ok(());
            }
        };
        let differed = differences(&file, *modules, *cables, *tempo);
        if !differed.is_empty() {
            self.model.known = false;
            self.unknown_from = change.seq;
            self.unknown_reason = format!(
                "the wall change {} went to change {} but the moment's wall was not what the go put up ({})",
                change.seq,
                destination(to),
                differed.join(", ")
            );
            self.gap_lines.push(self.unknown_reason.clone());
            return Ok(());
        }
        self.model.adopt(&file, change.seq, Found::Load)?;
        if let Some(note) = note {
            self.gap_lines
                .push(format!("births at go {}: {note}", change.seq));
        }
        let returning = change
            .undoes
            .and_then(|seq| self.return_place(seq))
            .cloned();
        let parent = returning
            .as_ref()
            .and_then(|place| place.change)
            .and_then(|seq| self.line.seq_commit.get(&seq))
            .cloned();
        if let (Some(place), Some(parent)) = (returning, parent) {
            self.move_to(&place, parent)?;
        } else if let Some(parent) = held {
            if change.undoes.is_some() {
                self.gap_lines.push(format!(
                    "gap at go {}: the wall is proved by a kept file, but its line is not held; a new bud grows from {}",
                    change.seq, short(&parent)
                ));
                self.detach(parent, to.mark.clone())?;
            } else {
                self.move_to(to, parent)?;
            }
        } else {
            let why = format!(
                "gap at go {}: the wall is proved by a kept file, but its line is not held; a new bud grows from the proved wall",
                change.seq
            );
            self.gap_lines.push(why.clone());
            if self.starts + self.report.changes + self.report.gaps + self.report.settles > 0 {
                let parent = if self.line.at.is_some() {
                    self.line.tip.clone()
                } else {
                    self.importer.checkpoint(self.mark)?
                };
                self.detach(parent, to.mark.clone())?;
            }
            self.own(
                "gap",
                (change.seq, &change.at),
                (
                    &why,
                    "The kept file proves the wall; the log does not hold its original parent.",
                ),
                None,
            )?;
            self.line.tip = self.importer.checkpoint(self.mark)?;
        }
        self.line
            .seq_commit
            .insert(change.seq, self.line.tip.clone());
        self.line.whole = true;
        self.previous = self.model.files(None)?;
        self.tip = (change.seq, change.at.clone());
        Ok(())
    }
}
