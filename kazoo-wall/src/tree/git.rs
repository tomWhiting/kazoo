//! Canonical, durable commits through git's own importer.

mod restart;
pub(super) use restart::missing_changes;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

use super::TreeError;
use crate::protocol::{Change, ClockStamp};

/// `core.fsync=committed` requires git 2.36 or later.
pub const MIN_GIT: (u32, u32) = (2, 36);

/// Environment variables that would point git at another repository, or
/// set its configuration from outside: the caller's, never the tree's.
const FOREIGN_GIT_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

/// `git [-C <dir>] [-c ...] <name>`: the only way this crate runs git.
///
/// `repack` and `fast-import` get the durability settings they need, and
/// none of the caller's repository variables reach git, so a daemon started
/// from inside some other repository (a hook, a tool) still writes only to
/// its tree.
#[must_use]
pub fn git_command(name: &str, dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    for key in FOREIGN_GIT_ENV {
        command.env_remove(key);
    }
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    if name == "repack" {
        command.args(["-c", "core.fsync=committed"]);
    }
    if name == "fast-import" {
        command.args([
            "-c",
            "core.fsync=committed",
            "-c",
            "core.fsyncMethod=fsync",
            "-c",
            "fastimport.unpackLimit=0",
        ]);
    }
    command.arg(name);
    command
}

fn output(command: &mut Command) -> Result<Output, TreeError> {
    command
        .output()
        .map_err(|err| TreeError::Git(format!("{command:?}: {err}")))
}

fn checked(command: &mut Command) -> Result<String, TreeError> {
    let result = output(command)?;
    if !result.status.success() {
        // What git said, on one line: an error is said in one.
        let said = String::from_utf8_lossy(&result.stderr);
        let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(TreeError::Git(format!(
            "{command:?}: {}: {said}",
            result.status
        )));
    }
    String::from_utf8(result.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|err| TreeError::Git(format!("{command:?}: {err}")))
}

/// Requires `git` on `PATH` at [`MIN_GIT`] or later.
///
/// # Errors
///
/// `TreeError::Git` when git cannot be run, its version does not read, or
/// it is older than [`MIN_GIT`].
pub fn check_version() -> Result<(), TreeError> {
    // On a Mac without the developer tools, `git` is there but only says
    // they are missing.
    let text = checked(&mut git_command("version", None)).map_err(|err| {
        TreeError::Git(format!(
            "the tree needs git {}.{} or later, and none here runs (xcode-select --install puts it in): {err}",
            MIN_GIT.0, MIN_GIT.1
        ))
    })?;
    let version = text
        .strip_prefix("git version ")
        .ok_or_else(|| TreeError::Git(format!("unreadable git version: {text}")))?;
    let mut parts = version.split('.');
    let mut number = || {
        parts
            .next()
            .ok_or_else(|| TreeError::Git(text.clone()))?
            .parse::<u32>()
            .map_err(|err| TreeError::Git(format!("{text}: {err}")))
    };
    let found = (number()?, number()?);
    if found < MIN_GIT {
        return Err(TreeError::Git(format!(
            "{text}; the tree needs git {}.{} or later (xcode-select --install, or brew install git)",
            MIN_GIT.0, MIN_GIT.1
        )));
    }
    Ok(())
}

/// Plant a tree: a bare repository on `trunk` with automatic gc off, made
/// only in a new or empty directory (mode 0700), so no history is ever
/// replaced.
///
/// # Errors
///
/// `TreeError::Git` for a directory that is not empty, or any step that
/// fails, with git's exit status and words.
pub fn plant(dir: &Path) -> Result<(), TreeError> {
    check_version()?;
    if dir.exists() {
        let mut entries = fs::read_dir(dir)
            .map_err(|err| TreeError::Git(format!("read tree directory: {err}")))?;
        if let Some(entry) = entries.next() {
            entry.map_err(|err| TreeError::Git(format!("read tree directory: {err}")))?;
            return Err(TreeError::Git(
                "refusing to plant in a nonempty directory".into(),
            ));
        }
    } else {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|err| TreeError::Git(format!("create tree directory: {err}")))?;
    }
    checked(
        git_command("init", None)
            .args(["--bare", "--quiet", "--initial-branch=trunk"])
            .arg(dir),
    )?;
    checked(git_command("config", Some(dir)).args(["gc.auto", "0"]))?;
    Ok(())
}

/// Whether `dir` is itself a bare repository: not a directory inside one,
/// and not a plain directory inside some other repository.
///
/// # Errors
///
/// `TreeError::Git` when git cannot be run or `dir` cannot be resolved.
pub fn is_bare(dir: &Path) -> Result<bool, TreeError> {
    if !dir.exists() {
        return Ok(false);
    }
    let result = output(
        git_command("rev-parse", Some(dir)).args(["--is-bare-repository", "--absolute-git-dir"]),
    )?;
    if !result.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&result.stdout);
    let mut lines = text.lines();
    if lines.next() != Some("true") {
        return Ok(false);
    }
    let Some(git_dir) = lines.next() else {
        return Ok(false);
    };
    let resolve = |path: &Path| {
        fs::canonicalize(path)
            .map_err(|err| TreeError::Git(format!("resolve {}: {err}", path.display())))
    };
    Ok(resolve(Path::new(git_dir))? == resolve(dir)?)
}

/// The configured tree, or the directory beside the wall's state.
#[must_use]
pub fn tree_dir(state: &Path) -> Option<PathBuf> {
    std::env::var_os("KAZOO_TREE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| state.parent().map(|dir| dir.join("tree")))
}

/// Where a planted tree stands.
#[derive(Debug, Clone)]
pub struct Planted {
    /// The wall's branch, the one `HEAD` names (`refs/heads/trunk`).
    pub branch: String,
    /// The branch's tip, `None` while it has no commit.
    pub tip: Option<String>,
    /// The last change the tip holds: its `Kazoo-Seq`, or `Kazoo-After`
    /// for KZ-T03's commits; `None` for a tip with neither.
    pub through: Option<u64>,
}

/// Read where the tree in `dir` stands.
///
/// # Errors
///
/// `TreeError::Git` when `HEAD` is not a branch, git fails, or a trailer
/// does not read as a number.
pub fn read(dir: &Path) -> Result<Planted, TreeError> {
    let branch = checked(git_command("symbolic-ref", Some(dir)).args(["-q", "HEAD"]))?;
    let result = output(git_command("rev-parse", Some(dir)).args([
        "-q",
        "--verify",
        &format!("{branch}^{{commit}}"),
    ]))?;
    let tip = if result.status.success() {
        Some(
            String::from_utf8(result.stdout)
                .map_err(|err| TreeError::Git(err.to_string()))?
                .trim()
                .to_string(),
        )
    } else if result.status.code() == Some(1) && result.stdout.is_empty() {
        None
    } else {
        return Err(TreeError::Git(format!(
            "rev-parse {branch}: {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        )));
    };
    let through = if tip.is_some() {
        through_at(dir, &branch)?
    } else {
        None
    };
    Ok(Planted {
        branch,
        tip,
        through,
    })
}

/// Geometric repack: keeps the number of packs logarithmic, never touches
/// a ref.
///
/// # Errors
///
/// `TreeError::Git` with git's exit status and words.
pub fn repack(dir: &Path) -> Result<(), TreeError> {
    checked(git_command("repack", Some(dir)).args(["-d", "-q", "--geometric=2"]))?;
    Ok(())
}

/// One commit for the importer to write.
#[derive(Debug)]
pub struct Commit {
    /// The author: the seat that made the change, or `kazoo-wall`.
    pub seat: String,
    /// Author and committer date, Unix seconds UTC.
    pub unix: i64,
    /// Subject, a blank line and the trailers.
    pub message: String,
    /// Whether `files` is the whole wall (`deleteall` first) rather than
    /// what differs from the commit before.
    pub whole: bool,
    /// Each path with its bytes, or `None` for a path that is gone.
    pub files: Vec<(String, Option<Vec<u8>>)>,
}

fn trailers(kind: &str, seq: u64, clock: Option<&ClockStamp>, gap: Option<(u64, u64)>) -> String {
    let clock = clock.map_or_else(String::new, |clock| {
        format!(
            "Kazoo-Beat: {}\nKazoo-Tempo: {}\nKazoo-Clock: {}\n",
            clock.beat, clock.tempo, clock.line
        )
    });
    let gap = gap.map_or_else(String::new, |(first, last)| {
        format!("Kazoo-Gap: {first}-{last}\n")
    });
    format!("Kazoo-Kind: {kind}\nKazoo-Seq: {seq}\n{clock}{gap}")
}

/// The `Kazoo-Comment` trailer for a sender's comment, if there is one: on
/// one line whatever the log held.
fn comment_trailer(comment: Option<&str>) -> String {
    comment.map_or_else(String::new, |comment| {
        format!("Kazoo-Comment: {}\n", one_line(comment))
    })
}

/// `text` as one line of a commit message, whatever the log held: every
/// character that would break or turn a line (see
/// [`crate::protocol::batch::breaks_line`]) a space.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if crate::protocol::batch::breaks_line(c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Step `index`'s comment in a batch's `comments` (none, or one per step).
fn step_comment(comments: &[Option<String>], index: usize) -> Option<&str> {
    comments.get(index).and_then(Option::as_deref)
}

/// The most parts a batch's moments can hold before its own: a request
/// line holds fewer steps than this.
pub(crate) const MOST_PARTS: usize = 4096;

/// The last logged change the tree holds at `rev`.
///
/// That is its `Kazoo-Seq` (or a gap's `Kazoo-After`). A batch's parts
/// (KZ-T10c) hold no change of their own, so the tree at a part holds
/// through the moment before its batch: the walk back passes them. `None`
/// for a moment with neither.
///
/// # Errors
///
/// `TreeError::Git` when git fails or a number does not read.
pub fn through_at(dir: &Path, rev: &str) -> Result<Option<u64>, TreeError> {
    let text = checked(git_command("log", Some(dir)).args([
        &format!("-n{}", MOST_PARTS + 1),
        "--format=%(trailers:key=Kazoo-Kind,valueonly,separator=%x2C)%x1f%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C)%x1f%(trailers:key=Kazoo-After,valueonly,separator=%x2C)%x1e",
        rev,
        "--",
    ]))?;
    for record in text
        .split('\u{1e}')
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        let mut fields = record.split('\u{1f}').map(str::trim);
        let kind = fields.next().unwrap_or_default();
        if kind == "part" {
            continue;
        }
        let (seq, after) = (
            fields.next().unwrap_or_default(),
            fields.next().unwrap_or_default(),
        );
        let (key, number) = if seq.is_empty() {
            ("Kazoo-After", after)
        } else {
            ("Kazoo-Seq", seq)
        };
        if number.is_empty() {
            return Ok(None);
        }
        return number
            .parse()
            .map(Some)
            .map_err(|err| TreeError::Git(format!("{key}: {err}")));
    }
    Ok(None)
}

impl Commit {
    /// The commit for one made change: the seat as author, dated the
    /// change's `at`, the summary as subject, and the change as JSON.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when `at` does not read or the change does not
    /// serialise.
    pub fn change(
        change: &Change,
        files: Vec<(String, Option<Vec<u8>>)>,
    ) -> Result<Self, TreeError> {
        let unix = chrono::NaiveDateTime::parse_from_str(&change.at, "%Y-%m-%dT%H:%M:%SZ")
            .map_err(|err| TreeError::Git(format!("change date: {err}")))?
            .and_utc()
            .timestamp();
        let json = serde_json::to_string(change)
            .map_err(|err| TreeError::Git(format!("change JSON: {err}")))?;
        // A batch sent is one knot with its note; an undo of one is said as
        // the undo it is, like any other change, never as the batch again.
        let message = if let (
            crate::protocol::What::Batch {
                note,
                changes,
                comments,
            },
            None,
        ) = (&change.what, change.undoes)
        {
            // On one line each, whatever a log edited by hand holds: a
            // line break would end the trailers.
            let note = one_line(note);
            let mut message = format!(
                "{}: {note}\n\n{}{}Kazoo-Note: {note}\n",
                change.seat,
                super::notes::body(change, &files)?,
                trailers("batch", change.seq, change.clock.as_ref(), None)
            );
            for step in changes {
                message.push_str("Kazoo-Step: ");
                message.push_str(&one_line(&crate::change::describe(step)));
                message.push('\n');
            }
            // The batch's last step: the moments before it are its parts
            // (KZ-T10c), and this commit is the batch's own.
            if changes.len() > 1 {
                let count = changes.len().to_string();
                message.push_str("Kazoo-Part: ");
                message.push_str(&count);
                message.push('/');
                message.push_str(&count);
                message.push('\n');
            }
            // Every step's comment, by its number from 1, so none is lost
            // when the batch's parts are not grown (each part carries its
            // own as well).
            for (index, comment) in comments.iter().enumerate() {
                if let Some(comment) = comment {
                    message.push_str("Kazoo-Step-Comment: ");
                    message.push_str(&(index + 1).to_string());
                    message.push_str(": ");
                    message.push_str(&one_line(comment));
                    message.push('\n');
                }
            }
            for note in &change.notes {
                message.push_str("Kazoo-Attachment: ");
                message.push_str(&one_line(&note.name));
                message.push_str(" (");
                message.push_str(&note.bytes.to_string());
                message.push_str(")\n");
            }
            message.push_str("Kazoo-Change: ");
            message.push_str(&json);
            message.push('\n');
            message
        } else {
            format!(
                "{}\n\n{}{}Kazoo-Change: {json}\n",
                one_line(&change.summary),
                trailers("change", change.seq, change.clock.as_ref(), None),
                comment_trailer(change.comment.as_deref())
            )
        };
        Ok(Self {
            seat: change.seat.clone(),
            unix,
            message,
            whole: false,
            files,
        })
    }

    /// The commit for step `index` (from 0) of batch `change`, one of its
    /// steps before the last (KZ-T10c): the wall after that step, under
    /// the batch's note. It is a `part`, not a change: it carries no
    /// `Kazoo-Seq`, so a tree that holds a batch's parts and not its own
    /// commit is still missing the change, and is healed as any other.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when `change` is not a batch, `index` is not one of
    /// its steps before the last, or `at` does not read.
    pub fn part(
        change: &Change,
        index: usize,
        files: Vec<(String, Option<Vec<u8>>)>,
    ) -> Result<Self, TreeError> {
        let crate::protocol::What::Batch {
            note,
            changes,
            comments,
        } = &change.what
        else {
            return Err(TreeError::Git(format!(
                "change {} is not a batch",
                change.seq
            )));
        };
        let step = changes
            .get(index)
            .filter(|_| index + 1 < changes.len())
            .ok_or_else(|| {
                TreeError::Git(format!(
                    "step {} is not one of change {}'s parts",
                    index + 1,
                    change.seq
                ))
            })?;
        let unix = chrono::NaiveDateTime::parse_from_str(&change.at, "%Y-%m-%dT%H:%M:%SZ")
            .map_err(|err| TreeError::Git(format!("change date: {err}")))?
            .and_utc()
            .timestamp();
        let (note, step) = (one_line(note), one_line(&crate::change::describe(step)));
        let clock = change.clock.as_ref().map_or_else(String::new, |clock| {
            format!(
                "Kazoo-Beat: {}\nKazoo-Tempo: {}\nKazoo-Clock: {}\n",
                clock.beat, clock.tempo, clock.line
            )
        });
        let message = format!(
            "{seat}: {note} ({part}/{count}) {step}\n\nKazoo-Kind: part\nKazoo-Batch: {seq}\n{clock}Kazoo-Note: {note}\nKazoo-Part: {part}/{count}\nKazoo-Step: {step}\n{comment}",
            seat = change.seat,
            comment = comment_trailer(step_comment(comments, index)),
            part = index + 1,
            count = changes.len(),
            seq = change.seq,
        );
        Ok(Self {
            seat: change.seat.clone(),
            unix,
            message,
            whole: false,
            files,
        })
    }

    /// A whole-wall commit naming only logged changes missing from the tree.
    #[must_use]
    pub fn whole(
        kind: &str,
        seq: u64,
        clock: Option<&ClockStamp>,
        files: super::Files,
        missing: &[u64],
    ) -> Self {
        let (names, gaps) = restart::describe(missing);
        let summary = if missing.is_empty() {
            if kind == "start" {
                format!("the wall started again after change {seq}")
            } else {
                format!("the tree holds the wall after change {seq}")
            }
        } else if let [one] = missing {
            format!("the tree holds change {one} as one commit")
        } else {
            format!("the tree holds changes {names} as one commit")
        };
        Self {
            seat: "kazoo-wall".into(),
            unix: chrono::Utc::now().timestamp(),
            message: format!("{summary}\n\n{}{gaps}", trailers(kind, seq, clock, None)),
            whole: true,
            files: files.into_iter().map(|(p, b)| (p, Some(b))).collect(),
        }
    }
}

/// A ref name the tree's streams may write.
fn valid_ref(name: &str) -> bool {
    name.starts_with("refs/")
        && !name.contains("..")
        && !name.ends_with('/')
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'+' | b'=')
        })
}

/// A mark `:N` (N from 1) or a full commit id.
fn valid_source(from: &str) -> bool {
    from.strip_prefix(':').is_some_and(|mark| {
        !mark.is_empty() && mark.bytes().all(|b| b.is_ascii_digit()) && mark != "0"
    }) || (from.len() == 40 && from.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One `git fast-import` writing commits on the wall's branch.
#[derive(Debug)]
pub struct Importer {
    dir: PathBuf,
    branch: String,
    tip: Option<String>,
    mark: u64,
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Importer {
    /// Start `fast-import` on `planted.branch`. It is never forced, so git
    /// itself refuses any update that would move the branch backwards.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when git cannot be started or takes no input.
    pub fn new(dir: &Path, planted: &Planted) -> Result<Self, TreeError> {
        let mut command = git_command("fast-import", Some(dir));
        command
            .args(["--quiet", "--done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command
            .spawn()
            .map_err(|err| TreeError::Git(format!("{command:?}: {err}")))?;
        let input = child.stdin.take();
        let Some(output) = child.stdout.take() else {
            if let Err(err) = child.kill() {
                eprintln!("kazoo-wall: stop fast-import: {err}");
            }
            let status = child
                .wait()
                .map_err(|err| TreeError::Git(format!("wait fast-import: {err}")))?;
            return Err(TreeError::Git(format!(
                "fast-import has no stdout: {status}"
            )));
        };
        let mut importer = Self {
            dir: dir.to_path_buf(),
            branch: planted.branch.clone(),
            tip: planted.tip.clone(),
            mark: 0,
            child,
            input,
            output: BufReader::new(output),
        };
        importer.write(b"feature get-mark\n")?;
        Ok(importer)
    }

    fn failed(&mut self, asked: &str, error: impl std::fmt::Display) -> TreeError {
        self.input.take();
        match self.child.wait() {
            Ok(status) => TreeError::Git(format!("fast-import {asked}: {error}; {status}")),
            Err(wait) => TreeError::Git(format!("fast-import {asked}: {error}; wait: {wait}")),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), TreeError> {
        let result = match self.input.as_mut() {
            Some(input) => input.write_all(bytes),
            None => return Err(TreeError::Git("fast-import stdin is closed".into())),
        };
        result.map_err(|err| self.failed("write", err))
    }

    /// Write one commit command; returns its mark (from 1 within this
    /// importer). The first names the branch's tip as its parent.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` for an author or path the tree cannot hold, or a
    /// write git did not take (with git's exit status).
    pub fn commit(&mut self, c: &Commit) -> Result<u64, TreeError> {
        if !crate::protocol::valid_name(&c.seat) {
            return Err(TreeError::Git("invalid commit author".into()));
        }
        self.mark += 1;
        let slug = c.seat.to_ascii_lowercase().replace(' ', "-");
        self.write(format!("commit {}\nmark :{}\nauthor {} <{slug}@kazoo.invalid> {} +0000\ncommitter kazoo-wall <kazoo-wall@kazoo.invalid> {} +0000\ndata {}\n", self.branch, self.mark, c.seat, c.unix, c.unix, c.message.len()).as_bytes())?;
        self.write(c.message.as_bytes())?;
        self.write(b"\n")?;
        // The first commit on a branch names its parent: the planted tip,
        // or where `switch` grew the branch from.
        if let Some(tip) = self.tip.take() {
            self.write(format!("from {tip}\n").as_bytes())?;
        }
        if c.whole {
            self.write(b"D modules\nD wall.toml\n")?;
        }
        for (path, bytes) in &c.files {
            if !super::notes::valid_path(path)
                && path != "wall.toml"
                && !path
                    .strip_prefix("modules/")
                    .and_then(|p| p.strip_suffix(".toml"))
                    .is_some_and(crate::patch::valid_id)
            {
                return Err(TreeError::Git(format!("invalid tree path {path}")));
            }
            if let Some(bytes) = bytes {
                self.write(format!("M 100644 inline {path}\ndata {}\n", bytes.len()).as_bytes())?;
                self.write(bytes)?;
                self.write(b"\n")?;
            } else {
                self.write(format!("D {path}\n").as_bytes())?;
            }
        }
        self.write(b"\n")?;
        Ok(self.mark)
    }

    /// Name a new ref in this batch; an existing ref is never moved.
    ///
    /// # Errors
    ///
    /// Refuses an existing name, an invalid stream value, or a git failure.
    pub fn tag(&mut self, name: &str, from: &str) -> Result<(), TreeError> {
        if !valid_ref(name) {
            return Err(TreeError::Git("invalid tree ref name".into()));
        }
        if !valid_source(from) {
            return Err(TreeError::Git("invalid tree ref source".into()));
        }
        let result =
            output(git_command("rev-parse", Some(&self.dir)).args(["-q", "--verify", name]))?;
        if result.status.success() {
            return Err(TreeError::Git(format!(
                "{name} exists; a tag is never moved"
            )));
        }
        if result.status.code() != Some(1) {
            return Err(TreeError::Git(format!(
                "read {name}: {}: {}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            )));
        }
        self.write(format!("reset {name}\nfrom {from}\n\n").as_bytes())
    }

    /// Write the next commits on the new branch `branch` in this same
    /// stream, the first of them a child of `from` (a mark `:N` or a commit
    /// id). Like every commit, it is durable only at the next checkpoint
    /// or the finish, so one stream may grow several branches and name
    /// them all at once.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` for a branch name or a source the stream cannot
    /// take.
    pub fn switch(&mut self, branch: &str, from: &str) -> Result<(), TreeError> {
        if !branch.starts_with("refs/heads/") || !valid_ref(branch) {
            return Err(TreeError::Git(format!("invalid tree branch {branch}")));
        }
        if !valid_source(from) {
            return Err(TreeError::Git("invalid tree branch source".into()));
        }
        branch.clone_into(&mut self.branch);
        self.tip = Some(from.into());
        Ok(())
    }

    /// The commit id of mark `mark`, as the stream wrote it. Nothing is
    /// made durable.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when git does not answer with a commit id.
    pub fn id(&mut self, mark: u64) -> Result<String, TreeError> {
        if mark == 0 || mark > self.mark {
            return Err(TreeError::Git(format!(
                "mark {mark} is not a commit this stream wrote"
            )));
        }
        self.write(format!("get-mark :{mark}\n").as_bytes())?;
        let mut line = String::new();
        match self.output.read_line(&mut line) {
            Ok(0) => Err(self.failed("get-mark", "stdout ended")),
            Ok(_) => {
                let id = line.trim();
                if id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                    Ok(id.to_string())
                } else {
                    Err(self.failed("get-mark", format!("answered {id:?}")))
                }
            }
            Err(err) => Err(self.failed("get-mark read", err)),
        }
    }

    /// Make everything written durable, and require the branch to be at
    /// the commit `mark` names: git refused the update, or someone else
    /// moved the branch, when it is not.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when git fails or the branch is elsewhere.
    pub fn checkpoint(&mut self, mark: u64) -> Result<String, TreeError> {
        if mark == 0 {
            self.write(b"checkpoint\n\nprogress kazoo 0\n")?;
            loop {
                let mut line = String::new();
                match self.output.read_line(&mut line) {
                    Ok(0) => return Err(self.failed("checkpoint", "stdout ended")),
                    Ok(_) => {
                        if line.trim() == "progress kazoo 0" {
                            break;
                        }
                    }
                    Err(err) => return Err(self.failed("checkpoint read", err)),
                }
            }
            return checked(
                git_command("rev-parse", Some(&self.dir)).args(["--verify", &self.branch]),
            );
        }
        self.write(
            format!("checkpoint\n\nprogress kazoo {mark}\n\nget-mark :{mark}\n").as_bytes(),
        )?;
        let mut progress = false;
        let sha = loop {
            let mut line = String::new();
            match self.output.read_line(&mut line) {
                Ok(0) => return Err(self.failed("checkpoint", "stdout ended")),
                Ok(_) => {
                    let line = line.trim();
                    if line == format!("progress kazoo {mark}") {
                        progress = true;
                    } else if progress
                        && line.len() == 40
                        && line.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        break line.to_string();
                    }
                }
                Err(err) => return Err(self.failed("checkpoint read", err)),
            }
        };
        let tip =
            checked(git_command("rev-parse", Some(&self.dir)).args(["--verify", &self.branch]))?;
        if tip != sha {
            return Err(TreeError::Git(format!(
                "{} is at {tip} but the tree wrote {sha}",
                self.branch
            )));
        }
        Ok(sha)
    }

    /// End the stream with `done` and require git to exit 0.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` with git's exit status.
    pub fn finish(mut self) -> Result<(), TreeError> {
        self.write(b"done\n")?;
        self.input.take();
        let status = self
            .child
            .wait()
            .map_err(|err| TreeError::Git(format!("fast-import done wait: {err}")))?;
        if !status.success() {
            return Err(TreeError::Git(format!("fast-import done: {status}")));
        }
        Ok(())
    }
}

impl Drop for Importer {
    fn drop(&mut self) {
        self.input.take();
        match self.child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(err) => eprintln!("kazoo-wall: fast-import status: {err}"),
        }
        if let Err(err) = self.child.kill() {
            eprintln!("kazoo-wall: stop fast-import: {err}");
        }
        if let Err(err) = self.child.wait() {
            eprintln!("kazoo-wall: wait fast-import: {err}");
        }
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod batch_tests;
