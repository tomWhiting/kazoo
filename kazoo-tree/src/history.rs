//! The tree read from git: every moment the wall has been, the limbs they
//! grew on, the marks and saves hung on them, and where the wall is now.
//!
//! The tree only branches and never merges (KZ-T02b, KZ-T04b), so a moment
//! has at most one parent, and every limb is a run of moments grown from a
//! moment on an older limb.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Command;

use kazoo_wall::tree::git::git_command;
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
    pub id: String,
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
    /// Every moment, each after the one it grew from.
    pub moments: Vec<Moment>,
    /// The trunk first, then buds in the order they grew.
    pub limbs: Vec<Limb>,
    /// For every moment on a limb, that limb and the moment's place on it.
    pub place: Vec<Option<(usize, usize)>>,
    /// Every mark and save.
    pub marks: Vec<Mark>,
    /// The moment the wall is at.
    pub here: Option<usize>,
}

const RECORD: char = '\u{1e}';
const BODY_END: char = '\u{1f}';

fn run(command: &mut Command) -> Result<String, HistoryError> {
    let output = command
        .output()
        .map_err(|err| HistoryError::Git(format!("git could not run: {err}")))?;
    if !output.status.success() {
        return Err(HistoryError::Git(format!(
            "git failed ({:?}): {}",
            command.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|err| {
        HistoryError::Broken(format!("git answered in bytes that are not text: {err}"))
    })
}

impl History {
    /// Read the tree at `dir` (a bare repository).
    ///
    /// # Errors
    ///
    /// When git cannot run or the tree holds a merge or an unknown parent.
    pub fn read(dir: &Path) -> Result<Self, HistoryError> {
        let log = run(git_command("log", Some(dir)).args([
            "--all",
            "--topo-order",
            "--reverse",
            "--no-renames",
            "--name-status",
            "--format=%x1e%H%x00%P%x00%ct%x00%an%x00%B%x1f",
        ]))?;
        let refs = run(git_command("for-each-ref", Some(dir))
            .args(["--format=%(refname)%00%(objectname)%00%(*objectname)"]))?;
        let here = head(dir)?;
        Self::from_git(&log, &refs, here.as_deref())
    }

    /// Build the tree from git's own words: `log` as `read` asks for it,
    /// `refs` from `for-each-ref`, and HEAD's commit.
    ///
    /// # Errors
    ///
    /// When a moment has two parents, names a parent not seen before it, or
    /// a field does not read.
    pub fn from_git(log: &str, refs: &str, here: Option<&str>) -> Result<Self, HistoryError> {
        let mut history = Self::default();
        let mut index: HashMap<String, usize> = HashMap::new();
        for record in log.split(RECORD).filter(|record| !record.trim().is_empty()) {
            let moment = moment(record, &index, &history.moments)?;
            index.insert(moment.id.clone(), history.moments.len());
            history.moments.push(moment);
        }
        history.limbs = limbs(refs, &index, &history.moments)?;
        history.place = vec![None; history.moments.len()];
        for (l, limb) in history.limbs.iter().enumerate() {
            for (at, &m) in limb.moments.iter().enumerate() {
                history.place[m] = Some((l, at));
            }
        }
        history.marks = marks(refs, &index);
        history.here = here.and_then(|id| index.get(id).copied());
        Ok(history)
    }

    /// The limb a moment is on, and its place there.
    #[must_use]
    pub fn place_of(&self, moment: usize) -> Option<(usize, usize)> {
        self.place.get(moment).copied().flatten()
    }
}

/// What changes whenever the tree grows or the wall moves: every ref and
/// HEAD. Cheap to read every few seconds; the log is read only when it
/// changes.
///
/// # Errors
///
/// When git cannot run.
pub fn stamp(dir: &Path) -> Result<String, HistoryError> {
    let refs =
        run(git_command("for-each-ref", Some(dir)).args(["--format=%(refname) %(objectname)"]))?;
    let here = head(dir)?.unwrap_or_default();
    Ok(format!("{refs}HEAD {here}"))
}

/// HEAD's commit; `None` for a tree with no moment yet, which `rev-parse
/// --verify -q` answers with exit 1 and nothing on either stream.
fn head(dir: &Path) -> Result<Option<String>, HistoryError> {
    let mut command = git_command("rev-parse", Some(dir));
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

fn moment(
    record: &str,
    index: &HashMap<String, usize>,
    moments: &[Moment],
) -> Result<Moment, HistoryError> {
    let (head, files) = record.split_once(BODY_END).ok_or_else(|| {
        HistoryError::Broken(format!("a log record without its end: {record:.80}"))
    })?;
    let mut fields = head.splitn(5, '\0');
    let mut field = |what: &str| {
        fields.next().ok_or_else(|| {
            HistoryError::Broken(format!("a log record without its {what}: {head:.80}"))
        })
    };
    let id = field("commit")?.trim().to_owned();
    let parents = field("parents")?;
    let time = field("time")?;
    let seat = field("author")?.to_owned();
    let body = field("message")?;
    let mut parents = parents.split_whitespace();
    let parent = match (parents.next(), parents.next()) {
        (None, _) => None,
        (Some(one), None) => Some(*index.get(one).ok_or_else(|| {
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
    let summary = body.lines().next().unwrap_or_default().to_owned();
    let mut kind = String::new();
    let mut seq = None;
    let mut beat = None;
    let mut tempo = None;
    let mut sound = None;
    for line in body.lines() {
        if let Some(value) = line.strip_prefix("Kazoo-Kind: ") {
            value.trim().clone_into(&mut kind);
        } else if let Some(value) = line.strip_prefix("Kazoo-Seq: ") {
            seq = Some(number(&id, "Kazoo-Seq", value)?);
        } else if let Some(value) = line.strip_prefix("Kazoo-Beat: ") {
            beat = Some(number(&id, "Kazoo-Beat", value)?);
        } else if let Some(value) = line.strip_prefix("Kazoo-Tempo: ") {
            tempo = Some(number(&id, "Kazoo-Tempo", value)?);
        } else if let Some(value) = line.strip_prefix("Kazoo-Sound: ") {
            sound = Some(sound_of(&id, value)?);
        }
    }
    let before = parent.map_or(0, |p| moments[p].modules);
    let mut modules = i64::from(before);
    for line in files.lines() {
        let mut parts = line.splitn(2, '\t');
        let (Some(status), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !path.starts_with("modules/") {
            continue;
        }
        match status.trim() {
            "A" => modules += 1,
            "D" => modules -= 1,
            _ => {}
        }
    }
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
        modules: u32::try_from(modules.max(0)).unwrap_or(u32::MAX),
    })
}

fn number<T: std::str::FromStr>(id: &str, key: &str, value: &str) -> Result<T, HistoryError>
where
    T::Err: fmt::Display,
{
    value.trim().parse().map_err(|err| {
        HistoryError::Broken(format!("{id:.10}'s {key} does not read ({value}): {err}"))
    })
}

/// Read `rms=-27.2 peak=-9.9 centroid=599 low=0.284 mid=0.712 high=0.004 onsets=3.0`.
fn sound_of(id: &str, value: &str) -> Result<Sound, HistoryError> {
    let mut found: HashMap<&str, f32> = HashMap::new();
    for pair in value.split_whitespace() {
        let (key, number_text) = pair.split_once('=').ok_or_else(|| {
            HistoryError::Broken(format!("{id:.10}'s Kazoo-Sound has {pair}, not key=value"))
        })?;
        found.insert(key, number(id, "Kazoo-Sound", number_text)?);
    }
    let get = |key: &str| {
        found
            .get(key)
            .copied()
            .ok_or_else(|| HistoryError::Broken(format!("{id:.10}'s Kazoo-Sound has no {key}")))
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

/// The order limbs are claimed in: the trunk, then buds by the time in
/// their names (a bud only grows from a moment that already existed, so a
/// parent bud is always claimed before its children), then any branch made
/// by hand, by its tip's time.
fn limbs(
    refs: &str,
    index: &HashMap<String, usize>,
    moments: &[Moment],
) -> Result<Vec<Limb>, HistoryError> {
    let mut heads: Vec<(String, usize)> = Vec::new();
    for r in read_refs(refs) {
        if let Some(name) = r.name.strip_prefix("refs/heads/") {
            let tip = *index.get(r.commit).ok_or_else(|| {
                HistoryError::Broken(format!(
                    "the branch {name} names {:.10}, which the log did not hold",
                    r.commit
                ))
            })?;
            heads.push((name.to_owned(), tip));
        }
    }
    heads.sort_by(|(a, a_tip), (b, b_tip)| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| bud_time(a).cmp(bud_time(b)))
            .then_with(|| moments[*a_tip].time.cmp(&moments[*b_tip].time))
            .then_with(|| a.cmp(b))
    });
    let mut claimed = vec![false; moments.len()];
    let mut limbs = Vec::with_capacity(heads.len());
    for (name, tip) in heads {
        let mut own = Vec::new();
        let mut at = Some(tip);
        let mut from = None;
        while let Some(m) = at {
            if claimed[m] {
                from = Some(m);
                break;
            }
            claimed[m] = true;
            own.push(m);
            at = moments[m].parent;
        }
        own.reverse();
        if own.is_empty() && from.is_none() {
            continue;
        }
        limbs.push(Limb {
            name,
            moments: own,
            from,
        });
    }
    Ok(limbs)
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

/// `bud/<from>/<time>[-n]`: the time part, which sorts as text.
fn bud_time(name: &str) -> &str {
    name.strip_prefix("bud/")
        .and_then(|rest| rest.rsplit_once('/'))
        .map_or("", |(_, time)| time)
}

fn marks(refs: &str, index: &HashMap<String, usize>) -> Vec<Mark> {
    let mut marks: Vec<Mark> = read_refs(refs)
        .filter_map(|r| {
            let (part, save) = match r.name.strip_prefix("refs/tags/mark/") {
                Some(part) => (part, false),
                None => (r.name.strip_prefix("refs/tags/save/")?, true),
            };
            let moment = *index.get(r.commit)?;
            Some(Mark {
                name: name_of(part),
                save,
                moment,
            })
        })
        .collect();
    marks.sort_by(|a, b| a.moment.cmp(&b.moment).then_with(|| a.name.cmp(&b.name)));
    marks
}

#[cfg(test)]
mod tests;
