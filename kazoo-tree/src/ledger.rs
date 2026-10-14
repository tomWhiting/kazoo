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

use crate::growth::GROWTH_VERSION;

/// A step of growth, named so it is the same however the tree is read: the
/// commit of its limb's first moment, and its number on that limb.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StepKey {
    /// The commit of the limb's first moment of its own.
    pub limb: String,
    /// The step's number on its limb, from 0.
    pub index: usize,
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
    /// The steps, in the order they grew.
    pub steps: Vec<StepKey>,
    seen_limbs: HashSet<String>,
    seen_steps: HashSet<StepKey>,
    /// Lines that did not read, counted.
    pub unreadable: usize,
    /// Why the ledger could not be read or written, when it could not: the
    /// tree is still drawn, but may move between runs until this is put
    /// right.
    pub problem: Option<String>,
}

impl Ledger {
    /// The ledger kept in the tree at `dir`.
    #[must_use]
    pub fn of_tree(dir: &Path) -> Self {
        Self {
            path: Some(dir.join(format!("kazoo-tree-grown-v{GROWTH_VERSION}"))),
            ..Self::default()
        }
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
            Ok(()) => {}
            Err(err) => {
                self.problem = Some(format!(
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
            // Shorter than what was read: the file was replaced. Read it
            // again whole; lines already held still count once.
            self.read_to = 0;
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
        let mut parts = line.split('\t');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("limb"), Some(name), None, None) if !name.is_empty() => {
                self.hold_limb(name.to_owned());
            }
            (Some("step"), Some(limb), Some(index), None) if !limb.is_empty() => {
                match index.parse() {
                    Ok(index) => {
                        self.hold_step(StepKey {
                            limb: limb.to_owned(),
                            index,
                        });
                    }
                    Err(_) => self.unreadable += 1,
                }
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

    fn hold_step(&mut self, key: StepKey) -> bool {
        if self.seen_steps.insert(key.clone()) {
            self.steps.push(key);
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
    pub fn record(&mut self, limbs: &[String], steps: &[StepKey]) {
        let mut text = String::new();
        for name in limbs {
            // A ref name never holds a tab or a newline (git refuses
            // control characters in one), so the line always reads back.
            if self.hold_limb(name.clone()) {
                text.push_str("limb\t");
                text.push_str(name);
                text.push('\n');
            }
        }
        for key in steps {
            if self.hold_step(key.clone()) {
                text.push_str("step\t");
                text.push_str(&key.limb);
                text.push('\t');
                text.push_str(&key.index.to_string());
                text.push('\n');
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
                self.problem = None;
                // Read on from where this ledger had read to: another tree
                // view may have written between, and its lines count too.
                // Our own lines come back as well, and count once.
                self.refresh();
            }
            Err(err) => {
                self.problem = Some(format!(
                    "the order the tree grew in could not be written to {}: {err}; the tree may be drawn differently next time",
                    path.display()
                ));
            }
        }
    }
}

/// Append `bytes` to the file at `path` in one write, made durable.
fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_data()
}

#[cfg(test)]
mod tests;
