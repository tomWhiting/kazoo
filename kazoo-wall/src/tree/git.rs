//! Canonical, durable commits through git's own importer.

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
        return Err(TreeError::Git(format!(
            "{command:?}: {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
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
    let text = checked(&mut git_command("version", None))?;
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
            "{text}; the tree needs git 2.36 or later"
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
    let mut through = None;
    if tip.is_some() {
        for key in ["Kazoo-Seq", "Kazoo-After"] {
            let text = checked(git_command("log", Some(dir)).args([
                "-1",
                &format!("--format=%(trailers:key={key},valueonly,separator=%x2C)"),
                &branch,
            ]))?;
            if !text.is_empty() {
                through = Some(
                    text.parse()
                        .map_err(|err| TreeError::Git(format!("{key}: {err}")))?,
                );
                break;
            }
        }
    }
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
        Ok(Self {
            seat: change.seat.clone(),
            unix,
            message: format!(
                "{}\n\n{}Kazoo-Change: {json}\n",
                change.summary.replace(['\r', '\n'], " "),
                trailers("change", change.seq, change.clock.as_ref(), None)
            ),
            whole: false,
            files,
        })
    }

    /// A start or gap commit holding the whole wall after change `seq`,
    /// naming the changes after `through` it holds without their own
    /// commits.
    #[must_use]
    pub fn whole(
        kind: &str,
        seq: u64,
        clock: Option<&ClockStamp>,
        files: super::Files,
        through: Option<u64>,
    ) -> Self {
        let previous = through.unwrap_or(0);
        let gap = (seq > previous).then(|| (previous + 1, seq));
        let summary = match (kind, gap, previous > seq) {
            ("start", Some((first, last)), _) => format!(
                "the wall started after change {seq}; the tree did not hold changes {first} to {last} one by one"
            ),
            ("start", None, true) => format!(
                "the tree held changes up to {previous}; the wall started from change {seq}"
            ),
            ("start", None, false) => format!("the wall started after change {seq}"),
            (_, Some((first, last)), _) => {
                format!("the tree holds changes {first} to {last} as one commit")
            }
            _ => format!("the tree holds the wall after change {seq}"),
        };
        Self {
            seat: "kazoo-wall".into(),
            unix: chrono::Utc::now().timestamp(),
            message: format!("{summary}\n\n{}", trailers(kind, seq, clock, gap)),
            whole: true,
            files: files.into_iter().map(|(p, b)| (p, Some(b))).collect(),
        }
    }
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
    /// Start `fast-import` on `planted.branch`, never with `--force`.
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
        if self.mark == 1 {
            if let Some(tip) = self.tip.clone() {
                self.write(format!("from {tip}\n").as_bytes())?;
            }
        }
        if c.whole {
            self.write(b"deleteall\n")?;
        }
        for (path, bytes) in &c.files {
            if path != "wall.toml"
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

    /// Make everything written durable, and require the branch to be at
    /// the commit `mark` names: git refused the update, or someone else
    /// moved the branch, when it is not.
    ///
    /// # Errors
    ///
    /// `TreeError::Git` when git fails or the branch is elsewhere.
    pub fn checkpoint(&mut self, mark: u64) -> Result<String, TreeError> {
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
