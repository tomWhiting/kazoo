//! The order the tree grew in, kept with the tree so it never changes.
//!
//! A pure rule over commit times cannot promise that growing never moves
//! the past: two limbs can grow in the same second, the wall dates a gap
//! after changes it covers, and a branch made by hand can appear at any
//! time with moments older than everything grown since. So the order the
//! tree first grew in is written down, and every later growing follows it:
//! what was grown is grown again exactly as before, and only what is new is
//! grown after it.
//!
//! The ledger is a small text file inside the tree's own folder (git leaves
//! files it does not know alone), so it goes wherever the tree goes and is
//! gone with it. It is only ever appended to, a line at a time:
//!
//! ```text
//! limb\t<branch name>
//! step\t<commit of the limb's first moment>\t<step number on the limb>
//! ```
//!
//! A `limb` line is the order the branches made by hand claimed their
//! moments; a `step` line is the order the steps of growth grew. A line seen
//! twice counts once (two `kazoo tree`s may both write what they grew), and
//! an unfinished last line (a write cut off) waits until it is finished.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// The ledger's file in the tree's folder. It carries no version: what it
/// holds is where the tree was drawn, and that never changes, whatever the
/// rules for new growth become.
pub const LEDGER: &str = "kazoo-tree-grown";

/// A step of growth, named so it is the same however the tree is read: the
/// commit of its limb's first moment, and its number on that limb.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StepKey {
    /// The commit of the limb's first moment of its own.
    pub limb: String,
    /// The step's number on its limb, from 0.
    pub index: usize,
}

/// Where a step was drawn: kept, so it is drawn there for good, whatever
/// the rules for new growth become.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// Where it starts, world units.
    pub start: (f64, f64),
    /// Where a whole step ends.
    pub end: (f64, f64),
    /// Its heading, radians.
    pub heading: f64,
}

/// A step as the ledger keeps it: its name, and where it was drawn (a line
/// written without it grows by the rules).
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// The step.
    pub key: StepKey,
    /// Where it was drawn.
    pub shape: Option<Shape>,
}

/// The order the tree grew in, as far as it has been read and written.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// The file; `None` keeps the order in memory only (a test, or a tree
    /// read where nothing may be written).
    path: Option<PathBuf>,
    /// How far into the file has been read.
    read_to: u64,
    /// The branches made by hand, in the order they claimed their moments.
    pub limbs: Vec<String>,
    /// The steps, in the order they grew, each where it was drawn.
    pub steps: Vec<Saved>,
    seen_limbs: HashSet<String>,
    seen_steps: HashSet<StepKey>,
    /// Lines that did not read, counted.
    pub unreadable: usize,
    /// Why the ledger could not be read, when it could not.
    read_problem: Option<String>,
    /// Why the ledger could not be written, when it could not: the tree is
    /// still drawn, but may move between runs until this is put right.
    write_problem: Option<String>,
}

impl Ledger {
    /// The ledger kept in the tree at `dir`.
    #[must_use]
    pub fn of_tree(dir: &Path) -> Self {
        Self {
            path: Some(dir.join(LEDGER)),
            ..Self::default()
        }
    }

    /// Why the ledger could not be read or written, when it could not.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        match (&self.read_problem, &self.write_problem) {
            (None, None) => None,
            (Some(one), None) | (None, Some(one)) => Some(one.clone()),
            (Some(read), Some(write)) => Some(format!("{read}; and {write}")),
        }
    }

    /// Hold the tree's ledger for this view alone until the answer is
    /// dropped: what is read, grown and written while it is held is one
    /// step, so two views never grow the same tree in two orders. `None`
    /// for a ledger in memory.
    ///
    /// # Errors
    ///
    /// When the lock file cannot be opened or locked.
    pub fn hold(&self) -> io::Result<Option<File>> {
        let Some(path) = &self.path else {
            return Ok(None);
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)?;
        Ok(Some(file))
    }

    /// A ledger kept in memory only.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Read whatever has been written to the file since the last read: by
    /// this tree view, or another one open on the same tree.
    pub fn refresh(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        match self.read_more(&path) {
            Ok(()) => self.read_problem = None,
            Err(err) => {
                self.read_problem = Some(format!(
                    "the order the tree grew in could not be read from {}: {err}",
                    path.display()
                ));
            }
        }
    }

    fn read_more(&mut self, path: &Path) -> io::Result<()> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            // No ledger yet: nothing has been grown in this tree before.
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        };
        let length = file.metadata()?.len();
        if length < self.read_to {
            // Shorter than what was read: the file was replaced. Write it
            // again from what is held, so the order is not lost, then read
            // on from its end.
            drop(file);
            return self.rewrite(path);
        }
        file.seek(SeekFrom::Start(self.read_to))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // Only whole lines: a line still being written is read next time.
        let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') else {
            return Ok(());
        };
        let whole = &bytes[..=end];
        self.read_to += u64::try_from(whole.len()).unwrap_or(u64::MAX);
        for line in String::from_utf8_lossy(whole).lines() {
            self.take(line);
        }
        Ok(())
    }

    fn take(&mut self, line: &str) {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["limb", name] if !name.is_empty() => {
                self.hold_limb((*name).to_owned());
            }
            ["step", limb, index, rest @ ..] if !limb.is_empty() => {
                let Ok(index) = index.parse() else {
                    self.unreadable += 1;
                    return;
                };
                let mut numbers = Vec::with_capacity(rest.len());
                for n in rest {
                    match n.parse::<f64>() {
                        Ok(value) if value.is_finite() => numbers.push(value),
                        Ok(_) | Err(_) => {
                            self.unreadable += 1;
                            return;
                        }
                    }
                }
                let shape = match numbers.as_slice() {
                    [] => None,
                    &[sx, sy, ex, ey, heading] => Some(Shape {
                        start: (sx, sy),
                        end: (ex, ey),
                        heading,
                    }),
                    _ => {
                        self.unreadable += 1;
                        return;
                    }
                };
                self.hold_step(Saved {
                    key: StepKey {
                        limb: (*limb).to_owned(),
                        index,
                    },
                    shape,
                });
            }
            _ => self.unreadable += 1,
        }
    }

    fn hold_limb(&mut self, name: String) -> bool {
        if self.seen_limbs.insert(name.clone()) {
            self.limbs.push(name);
            true
        } else {
            false
        }
    }

    fn hold_step(&mut self, step: Saved) -> bool {
        if self.seen_steps.insert(step.key.clone()) {
            self.steps.push(step);
            true
        } else {
            false
        }
    }

    /// Whether `name` has claimed its moments already.
    #[must_use]
    pub fn knows_limb(&self, name: &str) -> bool {
        self.seen_limbs.contains(name)
    }

    /// Add what was grown for the first time, in the order it grew, and
    /// write it down in one append.
    pub fn record(&mut self, limbs: &[String], steps: &[Saved]) {
        // Read to the file's end first: what another view wrote comes
        // before what this one writes, here as in the file.
        self.refresh();
        let mut text = String::new();
        for name in limbs {
            if self.hold_limb(name.clone()) {
                limb_line(&mut text, name);
            }
        }
        for step in steps {
            if self.hold_step(step.clone()) {
                step_line(&mut text, step);
            }
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        match append(&path, text.as_bytes()) {
            Ok(()) => {
                self.write_problem = None;
                // Read on from where this ledger had read to: another tree
                // view may have written between, and its lines count too.
                // Our own lines come back as well, and count once.
                self.refresh();
            }
            Err(err) => {
                self.write_problem = Some(format!(
                    "the order the tree grew in could not be written to {}: {err}; the tree may be drawn differently next time",
                    path.display()
                ));
            }
        }
    }
}

impl Ledger {
    /// Write the whole file again from what is held, in order, through a
    /// new file renamed over it, and read on from its end.
    fn rewrite(&mut self, path: &Path) -> io::Result<()> {
        let mut text = String::new();
        for name in &self.limbs {
            limb_line(&mut text, name);
        }
        for step in &self.steps {
            step_line(&mut text, step);
        }
        let fresh = path.with_extension("new");
        {
            let mut file = File::create(&fresh)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&fresh, path)?;
        sync_folder(path)?;
        self.read_to = u64::try_from(text.len()).unwrap_or(u64::MAX);
        Ok(())
    }
}

/// A branch made by hand, as a ledger line. A ref name never holds a tab
/// or a newline (git refuses control characters in one), so it reads back.
fn limb_line(text: &mut String, name: &str) {
    text.push_str("limb\t");
    text.push_str(name);
    text.push('\n');
}

/// A step, as a ledger line, with where it was drawn. Rust writes the
/// shortest digits that read back to the very same f64, so a drawn step
/// comes back exact.
fn step_line(text: &mut String, step: &Saved) {
    text.push_str("step\t");
    text.push_str(&step.key.limb);
    text.push('\t');
    text.push_str(&step.key.index.to_string());
    if let Some(shape) = step.shape {
        for n in [
            shape.start.0,
            shape.start.1,
            shape.end.0,
            shape.end.1,
            shape.heading,
        ] {
            text.push('\t');
            text.push_str(&n.to_string());
        }
    }
    text.push('\n');
}

/// Append `bytes` to the file at `path` in one write, made durable. A last
/// line left unfinished (a write cut off) is ended first, so it never
/// joins the next one; a file made new has its folder made durable too.
fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let new = !path.exists();
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    let length = file.metadata()?.len();
    if length > 0 {
        let mut last = [0_u8; 1];
        file.seek(SeekFrom::Start(length - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    file.write_all(bytes)?;
    file.sync_data()?;
    if new {
        sync_folder(path)?;
    }
    Ok(())
}

/// Make the folder holding `path` durable, so a file made in it stays.
fn sync_folder(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(folder) => File::open(folder)?.sync_all(),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
