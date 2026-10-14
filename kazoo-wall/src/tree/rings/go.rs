//! How the import follows a go, exactly as the live tree writer does
//! (`tree/thread.rs`, `moved` and `grow`): a go writes no commit of its own;
//! it moves the line to the moment, onto the moment's branch when the
//! moment is that branch's tip, and the next commit after it grows a bud
//! from the moment otherwise.
//!
//! Every line grows in the import's one `fast-import` stream, and nothing
//! is named until the walk is over: one durable write, as a load of the
//! wall's log is one operation.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::model::{Found, same};
use super::{Rings, RingsError, Walk, git_output, git_text, in_already, reference, short};
use crate::patch::PatchFile;
use crate::protocol::{Change, TreePlace, What};
use crate::tree::{Files, TreeError, from_files};

/// Where the import's line is, and where each change went.
pub(super) struct Lineage {
    tree: PathBuf,
    /// The branch the stream writes on.
    branch: String,
    /// The mark of the moment the line is at: the last commit, or the
    /// moment a go went to.
    pub(super) tip: u64,
    /// The moment a go left the line at, and the mark it went by, while
    /// the line is detached there: the next commit grows a bud from it.
    at: Option<(u64, Option<String>)>,
    /// The next commit holds the whole wall: the first after a go.
    whole: bool,
    /// The log has goes: where each change went is kept.
    track: bool,
    /// Each change's seq to the mark of the commit the import wrote for it.
    seq_commit: HashMap<u64, u64>,
    /// Each commit's mark to the branch it was written on.
    branch_of: HashMap<u64, String>,
    /// Each branch's tip in this stream.
    tips: HashMap<String, u64>,
    /// The changes a go goes to, whose walls are kept as they are written.
    wanted: HashSet<u64>,
    /// The files of each wanted change's commit.
    walls: HashMap<u64, Files>,
    /// The commit id the line ends at, once the walk is over.
    end: String,
}

impl Lineage {
    pub(super) fn new(tree: &Path, changes: &[Change]) -> Self {
        let wanted: HashSet<u64> = changes
            .iter()
            .flat_map(|c| match &c.what {
                What::Go {
                    to, from, backup, ..
                } => [
                    to.change,
                    from.change.filter(|_| backup.starts_with("tree:")),
                ],
                _ => [None, None],
            })
            .flatten()
            .collect();
        Self {
            tree: tree.into(),
            branch: "refs/heads/trunk".into(),
            tip: 0,
            at: None,
            whole: false,
            // Logs without movement keep nothing per change.
            track: changes.iter().any(|c| matches!(c.what, What::Go { .. })),
            seq_commit: HashMap::new(),
            branch_of: HashMap::new(),
            tips: HashMap::new(),
            wanted,
            walls: HashMap::new(),
            end: String::new(),
        }
    }
}

/// The import is in already: a line in the tree grows from its last ring.
/// The line HEAD is on is asked first, then every branch: once the wall
/// has gone elsewhere and grown on, HEAD may be on a line the import never
/// wrote, and the import is in all the same.
pub(super) fn already(tree: &Path, end: &str, trunk: String) -> Result<Rings, RingsError> {
    let output = git_output(tree, "symbolic-ref", &["-q", "HEAD"])?;
    let head = if output.status.success() {
        let branch = String::from_utf8(output.stdout).map_err(|e| TreeError::Git(e.to_string()))?;
        Some(branch.trim().to_string())
    } else if output.status.code() == Some(1) {
        None
    } else {
        return Err(TreeError::Git(format!(
            "read imported HEAD: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    };
    let holding = git_text(
        tree,
        "for-each-ref",
        &["--format=%(refname)", "--contains", end, "refs/heads/"],
    )?;
    let holding: Vec<&str> = holding.lines().collect();
    let line = head
        .as_deref()
        .filter(|branch| holding.contains(branch))
        .or_else(|| holding.iter().copied().find(|b| *b == "refs/heads/trunk"))
        .or_else(|| holding.first().copied());
    let Some(line) = line else {
        let detached = head.is_none()
            && git_output(tree, "merge-base", &["--is-ancestor", end, "HEAD"])?
                .status
                .success();
        if detached {
            let at = git_text(tree, "rev-parse", &["HEAD"])?;
            let mut report = in_already(tree, end, at, "HEAD")?;
            report.trunk = trunk;
            return Ok(report);
        }
        return Err(RingsError::Diverged(format!(
            "no line in the tree grows from the import's last ring (rings/end at {}); nothing is changed",
            short(end)
        )));
    };
    let tip = reference(tree, line)?
        .ok_or_else(|| TreeError::Git(format!("the imported line {line} has no commit")))?;
    let mut report = in_already(tree, end, tip, line.trim_start_matches("refs/heads/"))?;
    report.trunk = trunk;
    Ok(report)
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

impl Walk<'_> {
    /// The ref name a bud from `parent` gets, as the live writer names one
    /// (`Writer::grow`): `bud/<mark or the parent's first 12 hex>/<time>`,
    /// with `-2` and on for a name taken in the tree or in this stream.
    fn bud(&mut self, parent: u64, mark: Option<&str>, at: &str) -> Result<String, TreeError> {
        let from = match mark {
            Some(mark) => crate::tree::thread::here::ref_part(mark),
            None => short(&self.importer.id(parent)?).to_string(),
        };
        let time = crate::tree::thread::here::compact_time(at)
            .map_err(|err| TreeError::Git(err.to_string()))?;
        let prefix = format!("refs/heads/bud/{from}/{time}");
        for suffix in 1..=99 {
            let name = if suffix == 1 {
                prefix.clone()
            } else {
                format!("{prefix}-{suffix}")
            };
            if !self.line.tips.contains_key(&name) && reference(&self.line.tree, &name)?.is_none() {
                git_text(&self.line.tree, "check-ref-format", &[&name])?;
                return Ok(name);
            }
        }
        Err(TreeError::Git(format!(
            "the tree already holds 99 names at {prefix}; this moment needs another second"
        )))
    }

    /// Before a commit: a line left at a moment by a go grows a bud from
    /// it. True when the commit must hold the whole wall.
    pub(super) fn grow_line(&mut self, at: &str) -> Result<bool, TreeError> {
        if let Some((parent, mark)) = self.line.at.take() {
            let branch = self.bud(parent, mark.as_deref(), at)?;
            self.switch(branch, parent)?;
        }
        Ok(std::mem::take(&mut self.line.whole))
    }

    /// After commit `self.mark` for `seq` (`files` its wall): the line is
    /// at it, and where it went is kept when the log has goes. A change's
    /// own commit (`change`) holds its seq, as a live change commit is the
    /// only kind a place names by number; a gap or settle after a change
    /// the import holds stands for that change from then on, as the newest
    /// wall proved after it. A start or gap with no change of its own is
    /// never a numbered moment.
    pub(super) fn remember(&mut self, seq: u64, files: &Files, change: bool) {
        self.line.tip = self.mark;
        if !self.line.track {
            return;
        }
        self.line.tips.insert(self.line.branch.clone(), self.mark);
        self.line
            .branch_of
            .insert(self.mark, self.line.branch.clone());
        if !change && !self.line.seq_commit.contains_key(&seq) {
            return;
        }
        self.line.seq_commit.insert(seq, self.mark);
        if self.line.wanted.contains(&seq) {
            self.line.walls.insert(seq, files.clone());
        }
    }

    fn switch(&mut self, branch: String, parent: u64) -> Result<(), TreeError> {
        self.importer.switch(&branch, &format!(":{parent}"))?;
        self.line.branch = branch;
        self.line.tip = parent;
        self.line.at = None;
        Ok(())
    }

    fn detach(&mut self, parent: u64, mark: Option<String>) {
        self.line.tip = parent;
        self.line.at = Some((parent, mark));
    }

    /// The live writer's move (`moved`): onto the branch `place` names
    /// when the moment is that branch's tip, else at the moment.
    fn move_to(&mut self, place: &TreePlace, parent: u64) -> Result<(), TreeError> {
        if place.branch.is_some()
            && let Some(branch) = self.line.branch_of.get(&parent).cloned()
            && self.line.tips.get(&branch) == Some(&parent)
        {
            return self.switch(branch, parent);
        }
        self.detach(parent, place.mark.clone());
        Ok(())
    }

    /// The go `undone` left from.
    fn return_place(&self, undone: u64) -> Option<&TreePlace> {
        let original = self.log.changes.iter().find(|c| c.seq == undone)?;
        let What::Go { from, .. } = &original.what else {
            return None;
        };
        Some(from)
    }

    /// The wall a go put up: the moment's own when the import holds it,
    /// else what the kept files prove (an undone go's backup, or the next
    /// kept file).
    fn incoming_go(
        &self,
        change: &Change,
        held: Option<u64>,
    ) -> Result<(PatchFile, Option<String>), TreeError> {
        let moment = change.undoes.map_or(held, |undone| {
            self.log
                .changes
                .iter()
                .find(|c| c.seq == undone)
                .and_then(|original| {
                    let What::Go { from, backup, .. } = &original.what else {
                        return None;
                    };
                    from.change.filter(|seq| {
                        backup.starts_with("tree:") && self.line.walls.contains_key(seq)
                    })
                })
        });
        if let Some(seq) = moment {
            let files = self.line.walls.get(&seq).ok_or_else(|| {
                TreeError::Git(format!(
                    "the import did not keep the wall of change {seq}, which a go went to"
                ))
            })?;
            return Ok((from_files(files)?, None));
        }
        self.log
            .proofs
            .incoming(self.log.state, change, self.log.changes)
            .map_err(TreeError::Git)
    }

    /// A go: the model takes the moment's wall and the line moves to it,
    /// writing no commit, as the live writer does. While the model is not
    /// known, the go is one more change the gap it is in holds, as a load
    /// is: nothing is moved and the gap still begins where it began.
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
        if !self.model.known {
            return Ok(());
        }
        let held = to
            .change
            .filter(|seq| self.line.seq_commit.contains_key(seq));
        let (file, note) = match self.incoming_go(change, held) {
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
        let back = returning
            .as_ref()
            .and_then(|place| place.change)
            .and_then(|seq| self.line.seq_commit.get(&seq))
            .copied();
        let parent = held.and_then(|seq| self.line.seq_commit.get(&seq)).copied();
        if let (Some(place), Some(back)) = (returning, back) {
            self.move_to(&place, back)?;
        } else if let Some(parent) = parent {
            if change.undoes.is_some() {
                self.gap_lines.push(format!(
                    "gap at go {}: the wall is proved by a kept file, but its line is not held; a new bud grows from {}",
                    change.seq,
                    short(&self.importer.id(parent)?)
                ));
                self.detach(parent, to.mark.clone());
            } else {
                self.move_to(to, parent)?;
            }
        } else {
            let why = format!(
                "gap at go {}: the wall is proved by a kept file, but its line is not held; a new bud grows from the proved wall",
                change.seq
            );
            self.gap_lines.push(why.clone());
            if self.line.tip > 0 {
                self.detach(self.line.tip, to.mark.clone());
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
        }
        if self.line.track {
            self.line.seq_commit.insert(change.seq, self.line.tip);
        }
        self.line.whole = true;
        self.previous = self.model.files(None)?;
        self.tip = (change.seq, change.at.clone());
        Ok(())
    }

    /// The end of the walk: the import's last ring is tagged where the
    /// line is, every line and the tag are made durable together, and HEAD
    /// is put where the live writer would leave it.
    pub(super) fn finish_line(&mut self) -> Result<(), TreeError> {
        self.importer
            .tag("refs/tags/rings/end", &format!(":{}", self.line.tip))?;
        if self.line.track {
            // Several lines grew: each must be where the stream left it.
            self.importer.checkpoint(0)?;
            let tips: Vec<(String, u64)> = self
                .line
                .tips
                .iter()
                .map(|(branch, mark)| (branch.clone(), *mark))
                .collect();
            for (branch, mark) in tips {
                let wrote = self.importer.id(mark)?;
                let named = reference(&self.line.tree, &branch)?;
                if named.as_deref() != Some(wrote.as_str()) {
                    return Err(TreeError::Git(format!(
                        "{branch} is at {} but the import wrote {wrote}",
                        named.as_deref().unwrap_or("nothing")
                    )));
                }
            }
        } else {
            self.importer.checkpoint(self.mark)?;
        }
        self.line.end = self.importer.id(self.line.tip)?;
        if self.line.at.is_some() {
            git_text(
                &self.line.tree,
                "update-ref",
                &["--no-deref", "HEAD", &self.line.end],
            )?;
        } else {
            git_text(
                &self.line.tree,
                "symbolic-ref",
                &["HEAD", &self.line.branch],
            )?;
        }
        self.report.trunk = reference(&self.line.tree, "refs/heads/trunk")?
            .ok_or_else(|| TreeError::Git("the imported trunk has no commit".into()))?;
        Ok(())
    }

    /// The line the import ends on and its tip, for the summary.
    pub(super) fn line_summary(&self, seq: u64, at: &str) -> String {
        let line = if self.line.at.is_some() {
            "HEAD"
        } else {
            self.line.branch.trim_start_matches("refs/heads/")
        };
        format!("{line}: {} (change {seq}, {at})", self.line.end)
    }
}
