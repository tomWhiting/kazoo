//! Saving the wall: `patch.json` and the change log, in a 0700 directory.
//!
//! The patch is written to a temporary file, synced, renamed over the old
//! one and the directory synced, so a crash leaves the old patch or the new
//! one, never half of either. The log is append-only, one change per line,
//! and rotates to `log.1.jsonl` at [`LOG_ROTATE_LINES`] lines. A patch file
//! that cannot be read is moved aside to `patch.broken-<unix>.json`, never
//! overwritten.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::migrate;
use crate::patch::Patch;
use crate::protocol::Change;

/// The patch file's name.
pub const PATCH_FILE: &str = "patch.json";

/// The change log's name.
pub const LOG_FILE: &str = "log.jsonl";

/// The previous change log's name.
pub const OLD_LOG_FILE: &str = "log.1.jsonl";

/// Lines in the log before it rotates.
pub const LOG_ROTATE_LINES: usize = 10_000;

/// What loading the patch found.
#[derive(Debug)]
pub enum Loaded {
    /// There is no patch yet.
    Missing,
    /// The patch, brought up to date, with what that took.
    Patch {
        /// The patch.
        patch: Box<Patch>,
        /// The file's version before it was brought up to date.
        from_version: u32,
        /// What was migrated or left out, one sentence each; empty when
        /// the file was current and whole.
        notes: Vec<String>,
        /// Where the original file was kept, when anything changed.
        backup: Option<PathBuf>,
    },
    /// The file could not be used; it was moved aside.
    SetAside {
        /// Why it could not be used.
        reason: String,
        /// Where it went.
        moved_to: PathBuf,
    },
}

/// What loading the log found.
#[derive(Debug, Default)]
pub struct LoadedLog {
    /// The changes, oldest first.
    pub changes: Vec<Change>,
    /// Lines that could not be read (skipped).
    pub skipped: usize,
}

/// The wall's saved state.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    log: File,
    log_lines: usize,
}

impl Store {
    /// Open (creating it, mode 0700) the state directory `dir`.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or made private, or the
    /// log cannot be opened.
    pub fn open(dir: &Path) -> io::Result<Self> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let log_path = dir.join(LOG_FILE);
        let log_lines = count_lines(&log_path)?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            log,
            log_lines,
        })
    }

    /// The state directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Load `patch.json`: bring it up to date (see [`crate::migrate`]) and
    /// keep everything in it that can load. When anything had to change,
    /// the original is kept as `patch.before-<unix>.json`. Only a file that
    /// is not a patch at all (not JSON, no version, a version this wall
    /// never wrote) is moved aside to `patch.broken-<unix>.json`.
    ///
    /// # Errors
    ///
    /// Fails if the original cannot be kept, or an unusable file cannot be
    /// moved aside (it is then left alone, and the caller must not
    /// overwrite it).
    pub fn load_patch(&self) -> io::Result<Loaded> {
        let path = self.dir.join(PATCH_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
            Err(err) => {
                let moved_to = self.set_aside(&path, "broken")?;
                return Ok(Loaded::SetAside {
                    reason: format!("it could not be read: {err}"),
                    moved_to,
                });
            }
        };
        let loaded = migrate::read(&bytes).and_then(|(mut file, mut notes)| {
            let from_version = file.version;
            notes.extend(migrate::migrate(&mut file)?);
            let (current, left_out) = Patch::load(&file)?;
            notes.extend(left_out);
            Ok((current, from_version, notes))
        });
        match loaded {
            Ok((current, from_version, notes)) => {
                let backup = if notes.is_empty() {
                    None
                } else {
                    Some(self.keep(&bytes)?)
                };
                Ok(Loaded::Patch {
                    patch: Box::new(current),
                    from_version,
                    notes,
                    backup,
                })
            }
            Err(reason) => {
                let moved_to = self.set_aside(&path, "broken")?;
                Ok(Loaded::SetAside { reason, moved_to })
            }
        }
    }

    /// Keep a copy of `bytes` (the original patch file) as a free
    /// `patch.before-<unix>[-n].json`, synced.
    fn keep(&self, bytes: &[u8]) -> io::Result<PathBuf> {
        let target = self.free_name("before")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(target)
    }

    /// Move `path` to a free `patch.<label>-<unix>[-n].json` beside it.
    fn set_aside(&self, path: &Path, label: &str) -> io::Result<PathBuf> {
        let target = self.free_name(label)?;
        fs::rename(path, &target)?;
        Ok(target)
    }

    /// A name `patch.<label>-<unix>[-n].json` nothing has yet.
    fn free_name(&self, label: &str) -> io::Result<PathBuf> {
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        for attempt in 0..1_000_u32 {
            let name = if attempt == 0 {
                format!("patch.{label}-{unix}.json")
            } else {
                format!("patch.{label}-{unix}-{attempt}.json")
            };
            let target = self.dir.join(name);
            if !target.exists() {
                return Ok(target);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("no free name for patch.{label}-{unix}.json"),
        ))
    }

    /// Write `patch` to `patch.json` atomically.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written, synced or renamed.
    pub fn save_patch(&self, patch: &Patch) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(&patch.to_file()).map_err(io::Error::other)?;
        let temporary = self.dir.join(format!("{PATCH_FILE}.tmp"));
        {
            let mut file = File::create(&temporary)?;
            file.write_all(&text)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&temporary, self.dir.join(PATCH_FILE))?;
        File::open(&self.dir)?.sync_all()
    }

    /// Append `change` to the log, rotating it first if it is full.
    ///
    /// # Errors
    ///
    /// Fails if the log cannot be written or rotated.
    pub fn append(&mut self, change: &Change) -> io::Result<()> {
        if self.log_lines >= LOG_ROTATE_LINES {
            self.rotate()?;
        }
        let mut line = serde_json::to_vec(change).map_err(io::Error::other)?;
        line.push(b'\n');
        self.log.write_all(&line)?;
        self.log.flush()?;
        self.log_lines += 1;
        Ok(())
    }

    /// Sync the log to disk.
    ///
    /// # Errors
    ///
    /// Fails if the sync fails.
    pub fn sync_log(&self) -> io::Result<()> {
        self.log.sync_data()
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.log.sync_data()?;
        fs::rename(self.dir.join(LOG_FILE), self.dir.join(OLD_LOG_FILE))?;
        self.log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(LOG_FILE))?;
        self.log_lines = 0;
        Ok(())
    }

    /// Read back the logged changes, oldest first: the previous log, then
    /// the current one. Lines that cannot be read are skipped and counted.
    ///
    /// # Errors
    ///
    /// Fails if a log exists but cannot be read.
    pub fn load_log(&self) -> io::Result<LoadedLog> {
        let mut loaded = LoadedLog::default();
        for name in [OLD_LOG_FILE, LOG_FILE] {
            let file = match File::open(self.dir.join(name)) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            for line in BufReader::new(file).split(b'\n') {
                let line = line?;
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                match serde_json::from_slice::<Change>(&line) {
                    Ok(change) => loaded.changes.push(change),
                    Err(_) => loaded.skipped += 1,
                }
            }
        }
        Ok(loaded)
    }
}

/// Lines in `path`, 0 if it does not exist.
fn count_lines(path: &Path) -> io::Result<usize> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut count = 0;
    for line in BufReader::new(file).split(b'\n') {
        line?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::What;

    fn change(seq: u64) -> Change {
        Change {
            seq,
            at: "2026-09-26T00:00:00Z".to_string(),
            seat: "Tom".to_string(),
            what: What::Tempo {
                from: 90.0,
                to: 100.0,
                desk: false,
            },
            summary: format!("change {seq}"),
            undoes: None,
        }
    }

    #[test]
    fn the_directory_is_private() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a").join("wall");
        Store::open(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn patches_save_and_load() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        assert!(matches!(store.load_patch().unwrap(), Loaded::Missing));
        let mut patch = Patch::empty(97.0);
        patch.add("vco", Some("drone"), None).unwrap();
        store.save_patch(&patch).unwrap();
        store.save_patch(&patch).unwrap();
        match store.load_patch().unwrap() {
            Loaded::Patch {
                patch: loaded,
                notes,
                backup,
                ..
            } => {
                assert_eq!(*loaded, patch);
                assert!(notes.is_empty() && backup.is_none());
            }
            other => panic!("{other:?}"),
        }
        assert!(!root.path().join("patch.json.tmp").exists());
    }

    #[test]
    fn a_corrupt_patch_is_set_aside_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        fs::write(root.path().join(PATCH_FILE), b"{ not json").unwrap();
        let Loaded::SetAside { reason, moved_to } = store.load_patch().unwrap() else {
            panic!("not set aside");
        };
        assert!(reason.contains("not JSON"), "{reason}");
        assert_eq!(fs::read(&moved_to).unwrap(), b"{ not json");
        assert!(!root.path().join(PATCH_FILE).exists());
        // A second broken file in the same second gets its own name.
        fs::write(root.path().join(PATCH_FILE), b"[]").unwrap();
        let Loaded::SetAside {
            moved_to: second, ..
        } = store.load_patch().unwrap()
        else {
            panic!("not set aside");
        };
        assert_ne!(second, moved_to);
        // A well-formed file that breaks the rules is set aside too.
        let mut file = Patch::empty(120.0).to_file();
        file.version = 99;
        fs::write(
            root.path().join(PATCH_FILE),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load_patch().unwrap(),
            Loaded::SetAside { .. }
        ));
    }

    #[test]
    fn a_patch_with_bad_parts_loads_the_rest_and_keeps_the_original() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let mut file = serde_json::to_value(Patch::empty(100.0).to_file()).unwrap();
        file["modules"] = serde_json::json!([
            {"id": "vco1", "kind": "vco", "knobs": {"tune": 3.0}},
            {"id": "gone1", "kind": "gone", "knobs": {}}
        ]);
        file["cables"] = serde_json::json!([
            {"id": 1, "from": "gone1.out", "to": "vco1.pitch", "amount": 1.0}
        ]);
        let bytes = serde_json::to_vec(&file).unwrap();
        fs::write(root.path().join(PATCH_FILE), &bytes).unwrap();
        let Loaded::Patch {
            patch,
            from_version,
            notes,
            backup,
        } = store.load_patch().unwrap()
        else {
            panic!("not loaded");
        };
        assert_eq!(from_version, crate::patch::FILE_VERSION);
        assert_eq!(patch.modules().len(), 1);
        assert!(patch.cables().is_empty());
        assert_eq!(notes.len(), 2, "{notes:?}");
        let backup = backup.unwrap();
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("patch.before-")
        );
        assert_eq!(fs::read(backup).unwrap(), bytes);
        // The file itself is untouched until the daemon saves the new one.
        assert_eq!(fs::read(root.path().join(PATCH_FILE)).unwrap(), bytes);
    }

    #[test]
    fn the_log_appends_rotates_and_reads_back() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        for seq in 1..=(LOG_ROTATE_LINES as u64 + 5) {
            store.append(&change(seq)).unwrap();
        }
        store.sync_log().unwrap();
        assert_eq!(
            count_lines(&root.path().join(OLD_LOG_FILE)).unwrap(),
            LOG_ROTATE_LINES
        );
        assert_eq!(count_lines(&root.path().join(LOG_FILE)).unwrap(), 5);
        // A torn line is skipped, not fatal.
        let mut log = OpenOptions::new()
            .append(true)
            .open(root.path().join(LOG_FILE))
            .unwrap();
        log.write_all(b"{\"seq\":\n").unwrap();
        let loaded = store.load_log().unwrap();
        assert_eq!(loaded.changes.len(), LOG_ROTATE_LINES + 5);
        assert_eq!(loaded.skipped, 1);
        assert_eq!(
            loaded.changes.last().unwrap().seq,
            LOG_ROTATE_LINES as u64 + 5
        );
        // Reopening counts the lines already there.
        drop(store);
        let store = Store::open(root.path()).unwrap();
        assert_eq!(store.log_lines, 6);
    }
}
