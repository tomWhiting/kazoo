//! Saving the wall: `patch.json`, the change log and the named saves, in a
//! 0700 directory.
//!
//! The patch is written to a temporary file, synced, renamed over the old
//! one and the directory synced, so a crash leaves the old patch or the new
//! one, never half of either. The log is append-only, one change per line,
//! and rotates to `log.1.jsonl` at [`LOG_ROTATE_LINES`] lines. A patch file
//! that cannot be read is moved aside to `patch.broken-<unix>.json`, never
//! overwritten.
//!
//! Named saves live in `saves/<name>.json`, in `patch.json`'s format and
//! written the same durable way. They are read through the same migration,
//! so a save from an older wall loads (its file is left as it was). The
//! first time a wall with saves starts, the copies Tom made by hand beside
//! `patch.json` (`patch.<label>.json`) are copied in as saves, once.

mod chains;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::migrate;

use crate::MAX_KNOBS;
use crate::fingerprints::MAX_DYE_SEATS;
use crate::patch::{MAX_RETIRED_WORDS, Patch, hash_bytes, printable};
use crate::protocol::{
    AUTO_SAVE_PREFIX, Change, MAX_NAME, MAX_SAVE_NAME, MAX_SERVER_LINE, is_auto_save,
    valid_save_name,
};

/// The patch file's name.
pub const PATCH_FILE: &str = "patch.json";

/// The change log's name.
pub const LOG_FILE: &str = "log.jsonl";

/// The previous change log's name.
pub const OLD_LOG_FILE: &str = "log.1.jsonl";

/// The file a store holds an exclusive lock on, so one wall alone uses
/// a state directory (see [`Store::open`]).
pub const LOCK_FILE: &str = "lock";

/// How many times its file's size a wall's `look` may be, at most: a look
/// shows every knob with its range, unit and value in words where the file
/// keeps the value alone. A test holds every kind to it.
pub const LOOK_PER_FILE: usize = 7;

/// The largest wall file the wall reads or writes.
///
/// An eighth of the longest line the daemon sends ([`MAX_SERVER_LINE`]), so
/// a wall that fits a file fits a `look` too ([`LOOK_PER_FILE`] times
/// larger at most) with room to spare for what a look carries besides
/// (the scopes' readings, the log's health).
///
/// The wall has no fixed number of modules or cables, so this is the one
/// bound on its size: a change that would take the file past it is
/// refused (`full`, see [`WORST_MODULE_BYTES`]), a save or `patch.json`
/// larger than it is refused, never written, and a save or copy found
/// larger is listed as broken, never read whole.
pub const MAX_WALL_FILE: u64 = (MAX_SERVER_LINE / 8) as u64;

const _: () = assert!(
    MAX_WALL_FILE as usize * LOOK_PER_FILE + 4 * 1024 * 1024 <= MAX_SERVER_LINE,
    "a wall that fits its file may not fit a look"
);

/// The most kinds of module the catalogue may hold.
///
/// For [`MAX_NUMBERING_BYTES`] (each kind may have its numbering in the
/// file): a test holds the catalogue to it, and every kind's, knob's and
/// port's name to [`MAX_CATALOGUE_NAME`] bytes.
pub const MAX_KINDS: usize = 256;

/// The longest name of a kind, knob or port in the catalogue, in bytes.
pub const MAX_CATALOGUE_NAME: usize = 32;

/// The most one module adds to a wall's file (pretty-printed).
///
/// At its worst: its id (at most 40 bytes), kind, name (24 characters,
/// each escaped to 6 bytes at worst), and every knob's name and value; its
/// place in the rack's rows; and its dye, the [`MAX_DYE_SEATS`] seats it
/// keeps (each name 24 characters, escaped at worst) and their amounts;
/// each with its JSON punctuation, and twice over for the indentation.
///
/// The wall checks a change against what it adds at worst (this, and
/// [`WORST_CABLE_BYTES`] and [`WORST_WORDS_BYTES`]) before making it, so
/// a wall never grows past [`MAX_WALL_FILE`] (see
/// `daemon::wall::Wall::file_room`). It is a bound given the catalogue
/// facts a test checks (see [`MAX_KINDS`]).
pub const WORST_MODULE_BYTES: usize = 2
    * ((64 + MAX_CATALOGUE_NAME + MAX_NAME * 6 + MAX_KNOBS * (MAX_CATALOGUE_NAME + 32) + 64)
        + 64
        + (64 + MAX_DYE_SEATS * (MAX_NAME * 6 + 32)));

/// The most one cable adds to a wall's file: its number, both ends (a
/// module id and a port), its amount, with punctuation, twice over for
/// the indentation (see [`WORST_MODULE_BYTES`]).
pub const WORST_CABLE_BYTES: usize = 2 * (24 + 2 * (64 + MAX_CATALOGUE_NAME) + 32 + 64);

/// The most one speaker's words add to a wall's file: the longest text and
/// voice, each character escaped at worst, with 160 bytes for the entry
/// around them (see [`WORST_MODULE_BYTES`]).
pub const WORST_WORDS_BYTES: usize =
    160 + (kazoo_speech::tts::MAX_TEXT_CHARS + kazoo_speech::tts::MAX_VOICE_CHARS) * 6;

/// The most one loop record adds to a pretty-printed file, including escaped
/// labels, geometry and one pass per bar (see [`WORST_MODULE_BYTES`]).
pub const WORST_LOOP_BYTES: usize =
    2 * (64 + 64 + 6 * 32 + crate::dsp::looper::MAX_LOOP_BARS * 24 + MAX_NAME * 6 + 96);

/// The most the numbering of every kind, the retired speakers' words and
/// the retired loops' records add to a wall's file, whatever its size.
pub const MAX_NUMBERING_BYTES: usize = 2 * MAX_KINDS * 64
    + MAX_RETIRED_WORDS * WORST_WORDS_BYTES
    + crate::patch::MAX_RETIRED_LOOPS * WORST_LOOP_BYTES;

// A wall of the 96 modules and 320 cables it once held at most, every
// module a speaker, with eight loops, fits its file twice over. (Four times
// over until the 64 retired loops' records joined the numbering: 2.37 MB
// against the file's 8 MB.)
const _: () = assert!(
    (96 * (WORST_MODULE_BYTES + WORST_WORDS_BYTES)
        + 320 * WORST_CABLE_BYTES
        + 8 * WORST_LOOP_BYTES
        + MAX_NUMBERING_BYTES) as u64
        <= MAX_WALL_FILE / 2,
    "a wall the size it once held at most may not fit MAX_WALL_FILE"
);

/// Save files one listing reads at most (the rest are read by later
/// listings, and a few each tick), on the wall's control thread...
pub const HEADER_READS_PER_LISTING: usize = 8;

/// ...and the bytes it reads at most, but for its first file (which may
/// be as large as [`MAX_WALL_FILE`]): a few hundred KB in practice.
pub const HEADER_BYTES_PER_LISTING: u64 = 16 * 1024 * 1024;

/// Lines in the log before it rotates.
pub const LOG_ROTATE_LINES: usize = 10_000;

/// The directory holding the named saves.
pub const SAVES_DIR: &str = "saves";

/// The mark left among the saves once the hand-made copies beside
/// `patch.json` have been brought in, so they are brought in only once (a
/// save deleted afterwards stays deleted).
pub const IMPORTED_MARK: &str = ".imported";

/// Most automatic saves made in the same second before naming gives up.
const MAX_SAME_SECOND: u32 = 1_000;

/// The wall's own saves (`before-…`) always kept, newest first.
///
/// Twenty is an evening of trying saves one after another with every wall
/// before each still there to go back to, at a few tens of kilobytes each.
pub const AUTO_SAVES_KEPT: usize = 20;

/// The most of the wall's own saves kept at all.
///
/// Older ones beyond [`AUTO_SAVES_KEPT`] are kept only while a load in the
/// change log still needs one to be undone, and never more than this many
/// in all, so a seat loading in a loop cannot fill the disk (a wall of a
/// hundred modules is about 100 KB: about 10 MB of them). A load whose
/// wall was let go says so when asked to be undone.
pub const AUTO_SAVES_MOST: usize = 100;

/// A save read back.
#[derive(Debug)]
pub struct ReadSave {
    /// Its name, as stored.
    pub name: String,
    /// The wall in it, brought up to date.
    pub patch: Box<Patch>,
    /// The file's version before it was brought up to date.
    pub from_version: u32,
    /// What was migrated or left out, one sentence each; empty when the
    /// file was current and whole.
    pub notes: Vec<String>,
    /// [`hash_bytes`] of the file's bytes as they were read, before any
    /// migration.
    pub hash: String,
}

/// What a save's file holds, as a load would find it: the file read whole
/// through the migration (see `header_of`).
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// The wall's own tempo.
    pub tempo: f64,
    /// Modules a load puts up (parts it cannot load left out).
    pub modules: u32,
    /// Cables a load puts up.
    pub cables: u32,
    /// The file's format version.
    pub version: u32,
}

/// One file among the saves.
#[derive(Debug, Clone)]
pub struct Listed {
    /// Its name: the file's name without `.json`.
    pub name: String,
    /// When it was last written.
    pub saved_at: SystemTime,
    /// What a load would find in it, or why it cannot be loaded; `None`
    /// when it has not been read yet (see [`Store::list_saves`]).
    pub read: Option<Result<Header, String>>,
}

/// What says a save's file is still the one whose header was read: its
/// modification time, length, inode, and change time (to the nanosecond).
/// A file written in place, renamed over, or touched back to an old time
/// moves at least one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
    inode: u64,
    changed: i64,
    changed_nanos: i64,
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            modified: meta.modified()?,
            len: meta.len(),
            inode: meta.ino(),
            changed: meta.ctime(),
            changed_nanos: meta.ctime_nsec(),
        })
    }
}

/// A save's header as last read, with its file's [`Stamp`] then: the
/// header is read again only when the stamp moves.
type CachedHeader = (Stamp, Result<Header, String>);

/// Why a save could not be read, written or deleted.
#[derive(Debug)]
pub enum SaveError {
    /// There is no save by that name.
    Unknown(String),
    /// A save by that name (as stored) is there, and replacing it was not
    /// asked for.
    Exists(String),
    /// The save's file is not a wall this one can load.
    Broken {
        /// The save, as stored.
        name: String,
        /// Why.
        reason: String,
    },
    /// The disk refused: full, gone, or not ours.
    Io(io::Error),
    /// The wall is larger than a wall's file may be ([`MAX_WALL_FILE`]):
    /// nothing was written, as it could not be read back. How large it is.
    TooLarge(usize),
}

/// Why a wall of `bytes` is not written (see [`MAX_WALL_FILE`]).
fn too_large(bytes: usize) -> String {
    format!(
        "the wall is {} KB, more than a wall's file may hold ({} KB), so it was not written: \
         it could not be read back",
        bytes / 1024,
        MAX_WALL_FILE / 1024
    )
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(bytes) => f.write_str(&too_large(*bytes)),
            Self::Unknown(name) => write!(f, "there is no save named \"{}\"", printable(name)),
            Self::Exists(name) => write!(f, "a save named \"{name}\" is there already"),
            Self::Broken { name, reason } => {
                write!(f, "the save \"{name}\" cannot be loaded: {reason}")
            }
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

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
    /// Lines repeating the number of a change read already (skipped: the
    /// first is kept).
    pub repeated: usize,
}

/// The wall's saved state.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    log: File,
    log_lines: usize,
    /// Whether the log may end part-way through a line (a write that
    /// failed, or a file found so): the next line starts on its own.
    torn: bool,
    /// Whether a write or sync of the log failed: the handle is not trusted
    /// again, and the log is opened afresh before the next line (see
    /// [`Self::reopen_log`]).
    log_failed: bool,
    /// The lines appended since the log was last synced, whole: after a
    /// sync fails they are written again before anything else, as the
    /// system may have dropped them already (on Linux a failed fsync can
    /// mark the pages clean without writing them).
    unsynced: Vec<Vec<u8>>,
    /// Each save's header as last read, by name.
    headers: HashMap<String, CachedHeader>,
    /// Steps told to fail (in tests only; see [`Self::fail_once`]).
    faults: Faults,
    /// The saves the start-up pass has yet to look at (see
    /// [`Self::warm_saves`]): `None` before it begins.
    warm: Option<VecDeque<(String, PathBuf)>>,
    /// The state directory's lock, held for as long as the store lives:
    /// never read, only kept open (see [`Self::open`]).
    _lock: Option<File>,
}

/// Steps told to fail, in tests.
#[cfg(test)]
type Faults = Vec<FailAt>;

/// Nothing: the daemon never fails a step on purpose.
#[cfg(not(test))]
#[derive(Debug)]
struct Faults;

#[cfg(test)]
thread_local! {
    /// Steps the next store opened on this thread is to fail (see
    /// [`fail_on_open`]).
    static ON_OPEN: std::cell::RefCell<Vec<FailAt>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// Make the next store opened on this thread fail step `at`, once: for a
/// test going through [`crate::daemon::Daemon::start`], which opens its own.
#[cfg(test)]
pub(crate) fn fail_on_open(at: FailAt) {
    ON_OPEN.with(|on_open| on_open.borrow_mut().push(at));
}

/// The steps a store opened now is to fail.
#[cfg(test)]
fn faults_on_open() -> Faults {
    ON_OPEN.with(|on_open| std::mem::take(&mut *on_open.borrow_mut()))
}

/// The steps a store opened now is to fail: none.
#[cfg(not(test))]
const fn faults_on_open() -> Faults {
    Faults
}

/// Steps of writing that may be made to fail.
trait Inject {
    /// Fail at `at` if a test said so (see [`Store::fail_once`]).
    fn inject(&mut self, at: FailAt) -> io::Result<()>;
}

#[cfg(test)]
impl Inject for Vec<FailAt> {
    fn inject(&mut self, at: FailAt) -> io::Result<()> {
        if let Some(index) = self.iter().position(|failing| *failing == at) {
            self.remove(index);
            return Err(io::Error::other(format!(
                "a failure made for a test at {at:?}"
            )));
        }
        Ok(())
    }
}

/// Outside tests nothing fails on purpose.
#[cfg(not(test))]
impl Inject for Faults {
    #[inline]
    fn inject(&mut self, _at: FailAt) -> io::Result<()> {
        Ok(())
    }
}

/// Whether one of the wall's own saves is still needed to undo a load in
/// the log, and by whose load (see [`Store::prune_auto_saves`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// No change in the log needs it.
    Not,
    /// A load by a seat other than the console needs it.
    Seat,
    /// A load from the console needs it.
    Console,
}

/// A step of writing the wall's state, which a test can make fail (see
/// [`Store::fail_once`]): each with its own failure, deterministically,
/// whoever runs the tests. Outside tests no step is ever failed on
/// purpose, and none of this is part of the crate's interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailAt {
    /// Writing a save made by hand.
    WriteSave,
    /// Reading a hand-made copy for the import.
    ReadCopy,
    /// Writing `patch.json`.
    WritePatch,
    /// Writing a load's temporary files.
    WriteTemp,
    /// Renaming a load's backup into place.
    RenameBackup,
    /// Syncing the saves directory after it.
    SyncSaves,
    /// Writing a line to the log.
    AppendLog,
    /// Syncing the log.
    SyncLog,
    /// Renaming a load's new wall over `patch.json`.
    RenamePatch,
    /// Syncing the directories at the end of a load.
    SyncDirs,
    /// Flushing the state directory after the log rotates.
    SyncRotation,
    /// Flushing a directory after a save, `patch.json` or a deletion.
    SyncDir,
    /// Reading the saves directory for the start-up pass.
    ListSaves,
}

impl Store {
    /// Make step `at` fail, once, the next time it is reached.
    #[cfg(test)]
    pub(crate) fn fail_once(&mut self, at: FailAt) {
        self.faults.push(at);
    }

    /// Fail here if a test said so (see [`Self::fail_once`]).
    fn inject(&mut self, at: FailAt) -> io::Result<()> {
        self.faults.inject(at)
    }

    /// Open (creating it, mode 0700) the state directory `dir`, as its one
    /// user: an exclusive lock on `<dir>/lock` is taken and held for as
    /// long as the store lives, so no second wall (whatever socket it
    /// serves on) ever reads or writes the same state. Temporary files a
    /// crash left are then removed (see [`Self::prune_temporaries`]).
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::WouldBlock`] ("another wall is using
    /// <dir>") when another store holds the directory; fails if the
    /// directory cannot be created or made private, or the lock or the log
    /// cannot be opened.
    pub fn open(dir: &Path) -> io::Result<Self> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(dir.join(LOCK_FILE))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |errno| {
                if errno == rustix::io::Errno::WOULDBLOCK {
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!("another wall is using {}", dir.display()),
                    )
                } else if errno == rustix::io::Errno::NOTSUP || errno == rustix::io::Errno::NOLCK {
                    // A network disk that will not lock: one wall alone
                    // could not be made sure of there.
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!(
                            "the disk {} is on cannot lock files, so the wall cannot be sure \
                             it is the only one using it; use a local disk",
                            dir.display()
                        ),
                    )
                } else {
                    io::Error::from(errno)
                }
            },
        )?;
        let store = Self::unlocked(dir, Some(lock))?;
        for removed in store.prune_temporaries()? {
            eprintln!("kazoo-wall: removed {removed}, left by a write a crash cut short");
        }
        Ok(store)
    }

    /// The store on `dir` without its lock, to read what a running wall
    /// wrote (in tests: the lock is the running wall's).
    #[cfg(test)]
    pub(crate) fn peek(dir: &Path) -> io::Result<Self> {
        Self::unlocked(dir, None)
    }

    /// The store on `dir`, holding `lock` (see [`Self::open`]).
    fn unlocked(dir: &Path, lock: Option<File>) -> io::Result<Self> {
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
            torn: ends_torn(&log_path)?,
            log_failed: false,
            unsynced: Vec::new(),
            headers: HashMap::new(),
            faults: faults_on_open(),
            warm: None,
            _lock: lock,
        })
    }

    /// Whether a write or sync of the log failed and it has not been made
    /// whole since (see [`Self::reopen_log`]).
    #[must_use]
    pub const fn log_failed(&self) -> bool {
        self.log_failed
    }

    /// Remove the temporary files a write cut short by a crash left
    /// behind (`patch.json.tmp`; `*.tmp` among the saves): nothing names
    /// them, and nothing can reach them. Returns what was removed. (This
    /// store holds the state directory's lock: no other writes there.)
    ///
    /// # Errors
    ///
    /// Fails if a directory cannot be read or a file cannot be removed.
    fn prune_temporaries(&self) -> io::Result<Vec<String>> {
        let mut found = vec![self.dir.join(format!("{PATCH_FILE}.tmp"))];
        // No saves directory yet: no saves, and nothing left of one.
        if self.saves_dir().is_dir() {
            for entry in fs::read_dir(self.saves_dir())? {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "tmp") {
                    found.push(path);
                }
            }
        }
        let mut removed = Vec::new();
        for path in found {
            if let Err(err) = fs::remove_file(&path) {
                // Not there is nothing to remove; anything else fails.
                if err.kind() != io::ErrorKind::NotFound {
                    return Err(err);
                }
            } else {
                removed.push(path.display().to_string());
            }
        }
        Ok(removed)
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
        // Read whole whatever its size: the daemon wrote it (never larger
        // than a wall's file may be), and it is never set aside for size.
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
        match decode(&bytes) {
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

    /// Write `patch` to `patch.json` after the log, which it is never
    /// ahead of, with two full flushes in all: the new file is written and
    /// passed to the disk, the log is flushed in full (the first: every
    /// line `patch` holds, and the new file's bytes, are on the disk), the
    /// file renamed over `patch.json`, and the state directory flushed in
    /// full (the second: so a crash cannot take the rename back). The
    /// rename must wait for the first, or `patch.json` could be on the
    /// disk ahead of the log.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::FileTooLarge`] when the wall is larger
    /// than a wall's file may be (nothing is written: it could not be read
    /// back); fails if the file cannot be written or renamed, the log
    /// cannot be flushed (`patch.json` is then untouched), or the
    /// directory cannot be flushed (the new file may then be in place, but
    /// the caller saves it again: it is not durable yet).
    pub fn save_patch(&mut self, patch: &Patch) -> io::Result<()> {
        let temporary = self.dir.join(format!("{PATCH_FILE}.tmp"));
        let bytes = encode(patch)?;
        if bytes.len() as u64 > MAX_WALL_FILE {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                too_large(bytes.len()),
            ));
        }
        let written = self
            .inject(FailAt::WritePatch)
            .and_then(|()| write_passed(&temporary, &bytes, None))
            .and_then(|()| self.sync_log())
            .and_then(|()| fs::rename(&temporary, self.dir.join(PATCH_FILE)));
        if let Err(err) = written {
            tidy_away(&temporary);
            return Err(err);
        }
        self.inject(FailAt::SyncDir)
            .and_then(|()| full_flush(&self.dir))
    }

    // -----------------------------------------------------------------
    // Named saves
    // -----------------------------------------------------------------

    /// The directory holding the saves.
    #[must_use]
    pub fn saves_dir(&self) -> PathBuf {
        self.dir.join(SAVES_DIR)
    }

    /// The saves directory, made (mode 0700) if it is not there.
    fn open_saves(&self) -> io::Result<PathBuf> {
        let dir = self.saves_dir();
        if !dir.is_dir() {
            DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
            // Its name on the disk before anything in it is relied on (on
            // Linux, a file's flushes do not take its directory's entry
            // in its own parent with them).
            full_flush(&self.dir)?;
        }
        Ok(dir)
    }

    /// The files `patch.json` was moved aside to, as the wall could not
    /// use it (`patch.broken-*.json`, in the state directory), with when
    /// each was last written, newest first: listed among the broken saves
    /// so what was set aside is never out of sight.
    ///
    /// # Errors
    ///
    /// Fails if the state directory cannot be read.
    pub fn set_aside_files(&self) -> io::Result<Vec<(String, SystemTime)>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // The wall's own name for them, exactly as it makes them.
            let ours = name
                .strip_prefix("patch.broken-")
                .and_then(|rest| rest.strip_suffix(".json"))
                .is_some();
            if !ours {
                continue;
            }
            let modified = match entry.metadata().and_then(|meta| meta.modified()) {
                Ok(at) => at,
                // Gone since the directory was read: nothing to list.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            files.push((name, modified));
        }
        files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(files)
    }

    /// Every file among the saves that could be one: a plain file named
    /// `<something>.json`, not hidden. Names that are not save names are
    /// included (and listed as broken), so nothing there goes unseen.
    fn save_files(&self) -> io::Result<Vec<(String, PathBuf)>> {
        let entries = match fs::read_dir(self.saves_dir()) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(name) = file_name.strip_suffix(".json") else {
                continue;
            };
            if name.is_empty() || name.starts_with('.') {
                continue;
            }
            files.push((name.to_string(), entry.path()));
        }
        files.sort();
        Ok(files)
    }

    /// The save named `name` (ignoring case, as macOS's disks do): its
    /// name as stored, and its file.
    ///
    /// # Errors
    ///
    /// Fails if the saves directory cannot be read.
    pub fn find_save(&self, name: &str) -> io::Result<Option<(String, PathBuf)>> {
        Ok(self
            .save_files()?
            .into_iter()
            .find(|(stored, _)| valid_save_name(stored) && stored.eq_ignore_ascii_case(name)))
    }

    /// Every file among the saves, newest first (then by name), each with
    /// what a load would find in it (tempo, counts, version), or why it
    /// cannot be loaded, or `None` when it has not been read yet.
    ///
    /// What was found is kept between listings and found again only for a
    /// file whose stamp has moved, and one listing reads at most
    /// [`HEADER_READS_PER_LISTING`] files and [`HEADER_BYTES_PER_LISTING`]
    /// (but for its first file, at most [`MAX_WALL_FILE`]), newest first:
    /// so a listing's work on the wall's
    /// control thread is bounded however many saves there are, cold or
    /// warm. The rest are read by [`Self::warm_saves`], a file a tick, and
    /// by the listings after.
    ///
    /// # Errors
    ///
    /// Fails if the saves directory cannot be read.
    pub fn list_saves(&mut self) -> io::Result<Vec<Listed>> {
        self.scan_saves(HEADER_READS_PER_LISTING)
    }

    /// Read what the saves hold, at most `budget` files of those not read
    /// yet (see [`Self::list_saves`]): one pass, once, after the store
    /// opens, over the files there then, a few a tick, so the first
    /// listing finds them read. The directory is read once, at the start
    /// of the pass; each file is looked at once (and read only if nothing
    /// is kept of it as it is), so the pass costs one look per file in
    /// all, and ends whatever changes meanwhile (listings read what
    /// changes). Returns whether any are left to look at.
    ///
    /// # Errors
    ///
    /// Fails if the saves directory or a file's stamp cannot be read: the
    /// pass ends there, once (listings read the rest), so a directory
    /// that cannot be read is said once, not every tick.
    pub fn warm_saves(&mut self, budget: usize) -> io::Result<bool> {
        if self.warm.is_none() {
            let files = self
                .inject(FailAt::ListSaves)
                .and_then(|()| self.save_files());
            match files {
                Ok(files) => self.warm = Some(files.into_iter().collect()),
                Err(err) => {
                    self.warm = Some(VecDeque::new());
                    return Err(err);
                }
            }
        }
        let mut reads = budget;
        while reads > 0 {
            let Some((name, path)) = self.warm.as_mut().and_then(VecDeque::pop_front) else {
                break;
            };
            let stamp = match fs::metadata(&path).and_then(|meta| Stamp::of(&meta)) {
                Ok(stamp) => stamp,
                // Gone since the directory was read: nothing to read.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    self.warm = Some(VecDeque::new());
                    return Err(err);
                }
            };
            if self
                .headers
                .get(&name)
                .is_some_and(|(was, _)| *was == stamp)
            {
                continue;
            }
            reads -= 1;
            let read = header_of(&name, &path);
            self.headers.insert(name, (stamp, read));
        }
        Ok(self.warm.as_ref().is_some_and(|queue| !queue.is_empty()))
    }

    /// Every file among the saves, newest first, reading at most `budget`
    /// of those whose stamp is new (see [`Self::list_saves`]).
    fn scan_saves(&mut self, budget: usize) -> io::Result<Vec<Listed>> {
        let mut found = Vec::new();
        for (name, path) in self.save_files()? {
            let meta = match fs::metadata(&path) {
                Ok(meta) => meta,
                // Gone since the directory was read: nothing to list.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            found.push((Stamp::of(&meta)?, name, path));
        }
        found.sort_by(|a, b| b.0.modified.cmp(&a.0.modified).then_with(|| a.1.cmp(&b.1)));
        let mut reads = budget;
        let mut bytes = 0_u64;
        let mut listed = Vec::with_capacity(found.len());
        let mut kept = HashMap::new();
        for (stamp, name, path) in found {
            // The first file read is read whatever its size, so every
            // listing moves on; the rest while they fit the bytes left.
            let fits =
                reads == budget || bytes.saturating_add(stamp.len) <= HEADER_BYTES_PER_LISTING;
            let read = match self.headers.remove(&name) {
                Some((was, read)) if was == stamp => Some(read),
                Some(_) | None if reads > 0 && fits => {
                    reads -= 1;
                    bytes = bytes.saturating_add(stamp.len);
                    Some(header_of(&name, &path))
                }
                Some(_) | None => None,
            };
            if let Some(read) = &read {
                kept.insert(name.clone(), (stamp, read.clone()));
            }
            listed.push(Listed {
                name,
                saved_at: stamp.modified,
                read,
            });
        }
        // Saves gone from the directory are forgotten.
        self.headers = kept;
        Ok(listed)
    }

    /// Delete the oldest of the wall's own saves (`before-…`), keeping
    /// the newest [`AUTO_SAVES_KEPT`], and older ones that `needed` says a
    /// change in the log still needs to be undone, up to
    /// [`AUTO_SAVES_MOST`] in all: those a load from the console needs
    /// first, then those a seat's load needs, newest first in each, so a
    /// seat loading again and again never pushes out the console's way
    /// back. Returns the names deleted. Saves made by hand are never
    /// touched.
    ///
    /// # Errors
    ///
    /// Fails if the saves cannot be read, or one cannot be deleted (those
    /// already deleted stay deleted).
    pub fn prune_auto_saves(&mut self, needed: &dyn Fn(&str) -> Need) -> io::Result<Vec<String>> {
        let mut autos = Vec::new();
        for (name, path) in self.save_files()? {
            if !valid_save_name(&name) || !is_auto_save(&name) {
                continue;
            }
            let modified = match fs::metadata(&path).and_then(|meta| meta.modified()) {
                Ok(at) => at,
                // Gone since the directory was read: nothing to prune.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            autos.push((modified, name, path));
        }
        // Newest first; their names carry the time too, to the second.
        autos.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        // The newest are kept whatever; the room left goes to those still
        // needed, the console's before a seat's.
        let mut room = AUTO_SAVES_MOST.saturating_sub(AUTO_SAVES_KEPT.min(autos.len()));
        let needs: Vec<Need> = autos.iter().map(|(_, name, _)| needed(name)).collect();
        let mut keep: Vec<bool> = (0..autos.len())
            .map(|index| index < AUTO_SAVES_KEPT)
            .collect();
        for wanted in [Need::Console, Need::Seat] {
            for (index, need) in needs.iter().enumerate() {
                if room > 0 && !keep[index] && *need == wanted {
                    keep[index] = true;
                    room -= 1;
                }
            }
        }
        let mut deleted = Vec::new();
        for ((_, name, path), kept) in autos.into_iter().zip(keep) {
            if kept {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            }
            self.headers.remove(&name);
            deleted.push(name);
        }
        // Passed to the disk, not flushed in full: a deletion a crash takes
        // back only leaves an old automatic save, which the next pruning
        // lets go again.
        if !deleted.is_empty() {
            plain_fsync(&File::open(self.saves_dir())?)?;
        }
        Ok(deleted)
    }

    /// Read the save named `name` (any case), bringing it up to date in
    /// memory as `patch.json` is on start.
    ///
    /// # Errors
    ///
    /// [`SaveError::Unknown`] when there is none, [`SaveError::Broken`]
    /// when it is not a wall this one can load, [`SaveError::Io`] when the
    /// disk refuses.
    pub fn read_save(&self, name: &str) -> Result<ReadSave, SaveError> {
        let Some((stored, path)) = self.find_save(name)? else {
            return Err(SaveError::Unknown(name.to_string()));
        };
        let bytes = match read_wall_file(&path) {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(reason)) => {
                return Err(SaveError::Broken {
                    name: stored,
                    reason,
                });
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(SaveError::Unknown(name.to_string()));
            }
            Err(err) => return Err(SaveError::Io(err)),
        };
        // The file's own bytes, before any migration: what a load logged
        // to recognise this save by, whatever this build makes of them.
        let hash = hash_bytes(&bytes);
        let (wall, from_version, notes) = decode(&bytes).map_err(|reason| SaveError::Broken {
            name: stored.clone(),
            reason,
        })?;
        Ok(ReadSave {
            name: stored,
            patch: Box::new(wall),
            from_version,
            notes,
            hash,
        })
    }

    /// [`hash_bytes`] of the save named `name`'s file as it is on disk;
    /// `None` when there is no such save.
    ///
    /// # Errors
    ///
    /// Fails if the save cannot be read.
    pub fn hash_save(&self, name: &str) -> io::Result<Option<String>> {
        let Some((_, path)) = self.find_save(name)? else {
            return Ok(None);
        };
        match read_wall_file(&path) {
            Ok(Ok(bytes)) => Ok(Some(hash_bytes(&bytes))),
            // No wall: the empty hash, which no save is ever said to have.
            Ok(Err(_)) => Ok(Some(String::new())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Write `patch` as the save `name`, durably (see [`write_durably`]). A
    /// save of that name (any case) is replaced only when `overwrite` is
    /// set, and keeps its name as stored. Returns what was written, and
    /// whether its directory could not be flushed after (the save is in
    /// place then, the old one gone, but a crash may yet take it back).
    ///
    /// # Errors
    ///
    /// [`SaveError::Exists`] when a save of that name is there and
    /// `overwrite` is not set; [`SaveError::TooLarge`] when the wall is
    /// larger than a wall's file may be; [`SaveError::Io`] when the disk
    /// refuses before the new file is in place (it is full, say). The old
    /// save, if any, is then untouched.
    pub fn write_save(
        &mut self,
        name: &str,
        patch: &Patch,
        overwrite: bool,
    ) -> Result<Written, SaveError> {
        let dir = self.open_saves()?;
        let (stored, replaced) = match self.find_save(name)? {
            Some((stored, _)) if !overwrite => return Err(SaveError::Exists(stored)),
            Some((stored, _)) => (stored, true),
            None => (name.to_string(), false),
        };
        // Read afresh next time, whatever the disk's clock says.
        self.headers.remove(&stored);
        let bytes = encode(patch)?;
        if bytes.len() as u64 > MAX_WALL_FILE {
            return Err(SaveError::TooLarge(bytes.len()));
        }
        self.inject(FailAt::WriteSave)?;
        let faults = &mut self.faults;
        let unsynced = write_durably(&dir, &format!("{stored}.json"), &bytes, None, || {
            faults.inject(FailAt::SyncDir)
        })?;
        Ok(Written {
            name: stored,
            replaced,
            hash: hash_bytes(&bytes),
            unsynced,
        })
    }

    /// How many saves made by hand there are (not the wall's own).
    ///
    /// # Errors
    ///
    /// Fails if the saves directory cannot be read.
    pub fn count_saves(&self) -> io::Result<usize> {
        Ok(self
            .save_files()?
            .iter()
            .filter(|(name, _)| valid_save_name(name) && !is_auto_save(name))
            .count())
    }

    /// Make a load one batch on disk (see [`LoadWriteError`] for what a
    /// failure leaves), with as few full flushes as its order needs:
    ///
    /// 1. the wall as it was (`backup`, a name and the file's bytes, when
    ///    it is not kept in a save already) and the new wall (`wall`) are
    ///    written to temporary files beside their own, and passed to the
    ///    disk (a plain fsync: see [`plain_fsync`]);
    /// 2. the backup is renamed into place and the saves directory flushed
    ///    in full (the first full flush, with a backup only), so the
    ///    backup's bytes and name are on the disk before the log line that
    ///    names them is written: the log never names a save that a crash
    ///    could take back;
    /// 3. the load's change is appended to the log and flushed in full (the
    ///    second): from here the load has happened, and the new wall's
    ///    bytes are on the disk too;
    /// 4. the new wall is renamed over `patch.json`, after the log line, so
    ///    the new wall is never on disk without the change that put it up;
    /// 5. the state directory is flushed in full (the third), so the load
    ///    answered as made is the wall a restart comes up on.
    ///
    /// Three full flushes with a backup, two without. The only window left
    /// is a crash between 3 and 5: the log has the load and `patch.json`
    /// may still be the wall before it. The daemon finds that on start, by
    /// the wall `patch.json` names, and leaves the load out (see
    /// [`crate::daemon::wall::resume`]).
    ///
    /// # Errors
    ///
    /// [`LoadWriteError`].
    pub fn commit_load(
        &mut self,
        backup: Option<(&str, &[u8])>,
        wall: &Patch,
        change: &Change,
    ) -> Result<LoadWritten, LoadWriteError> {
        let unlogged = LoadWriteError::NotLogged;
        let wall_bytes = encode(wall).map_err(unlogged)?;
        // A wall that could not be read back is never put up, and is
        // turned away before the log is touched.
        if wall_bytes.len() as u64 > MAX_WALL_FILE {
            return Err(LoadWriteError::TooLarge(wall_bytes.len()));
        }
        // A rotation the load's line would bring is done first, outside
        // the batch's own order (see `rotate_if_due`).
        self.rotate_if_due().map_err(LoadWriteError::LogNotReady)?;
        let saves = self.open_saves().map_err(unlogged)?;
        let wall_temporary = self.dir.join(format!("{PATCH_FILE}.tmp"));
        let backup_files = backup.map(|(name, _)| {
            (
                saves.join(format!("{name}.json.tmp")),
                saves.join(format!("{name}.json")),
            )
        });
        let tidy = |paths: &[&Path]| {
            for path in paths {
                if let Err(err) = fs::remove_file(path) {
                    if err.kind() != io::ErrorKind::NotFound {
                        eprintln!(
                            "kazoo-wall: {} could not be tidied away: {err}",
                            path.display()
                        );
                    }
                }
            }
        };
        // 1. Both written and passed to the disk, or neither left behind.
        let written = self
            .inject(FailAt::WriteTemp)
            .and_then(|()| {
                backup
                    .zip(backup_files.as_ref())
                    .map_or(Ok(()), |((_, bytes), (temporary, _))| {
                        write_passed(temporary, bytes, None)
                    })
            })
            .and_then(|()| write_passed(&wall_temporary, &wall_bytes, None));
        let backup_temporary = backup_files
            .as_ref()
            .map(|(temporary, _)| temporary.as_path());
        if let Err(err) = written {
            tidy(&[backup_temporary.unwrap_or(&wall_temporary), &wall_temporary]);
            return Err(LoadWriteError::NotLogged(err));
        }
        // 2. The backup in place, durably. Should the sync fail, the backup
        // stays: it is the wall as it was, a save like any other, and
        // nothing names it.
        if let Some((temporary, target)) = &backup_files {
            let renamed = self
                .inject(FailAt::RenameBackup)
                .and_then(|()| fs::rename(temporary, target));
            if let Err(err) = renamed {
                tidy(&[temporary, &wall_temporary]);
                return Err(LoadWriteError::NotLogged(err));
            }
            if let Some((name, _)) = backup {
                self.headers.remove(name);
            }
            let synced = self
                .inject(FailAt::SyncSaves)
                .and_then(|()| full_flush(&saves));
            if let Err(err) = synced {
                tidy(&[&wall_temporary]);
                return Err(LoadWriteError::NotLogged(err));
            }
        }
        // 3. The change, durably.
        if let Err(err) = self.append_synced(change) {
            tidy(&[&wall_temporary]);
            return Err(LoadWriteError::MaybeLogged(err));
        }
        // 4 and 5: the load has happened; what fails now is said, and the
        // wall is saved again with the next save.
        let mut unsaved = self
            .inject(FailAt::RenamePatch)
            .and_then(|()| fs::rename(&wall_temporary, self.dir.join(PATCH_FILE)))
            .err();
        if unsaved.is_some() {
            tidy(&[&wall_temporary]);
        }
        let synced = self
            .inject(FailAt::SyncDirs)
            .and_then(|()| full_flush(&self.dir));
        if let Err(err) = synced {
            unsaved.get_or_insert(err);
        }
        Ok(LoadWritten { unsaved })
    }

    /// When the save named `name` (as stored) was written.
    ///
    /// # Errors
    ///
    /// Fails if it cannot be looked at.
    pub fn saved_at(&self, name: &str) -> io::Result<SystemTime> {
        fs::metadata(self.saves_dir().join(format!("{name}.json")))?.modified()
    }

    /// Delete the save named `name` (any case). Returns its name as
    /// stored, and whether the saves directory could not be flushed after
    /// (the save is gone then, but a crash may yet bring it back).
    ///
    /// # Errors
    ///
    /// [`SaveError::Unknown`] when there is none, [`SaveError::Io`] when
    /// the disk refuses to remove it (it is then still there).
    pub fn delete_save(&mut self, name: &str) -> Result<(String, Unsynced), SaveError> {
        let Some((stored, path)) = self.find_save(name)? else {
            return Err(SaveError::Unknown(name.to_string()));
        };
        self.headers.remove(&stored);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(SaveError::Unknown(name.to_string()));
            }
            Err(err) => return Err(SaveError::Io(err)),
        }
        let unsynced = self
            .inject(FailAt::SyncDir)
            .and_then(|()| full_flush(&self.saves_dir()))
            .err();
        Ok((stored, unsynced))
    }

    /// A free name for the save the wall makes before it replaces the wall
    /// by `how` (`load`, `new`, `undo`): `before-load-20261009-143005`, in
    /// local time (as recordings are named), with `-2`, `-3`... when the
    /// second is taken.
    ///
    /// # Errors
    ///
    /// Fails if the saves cannot be read, or a thousand saves were made in
    /// the same second.
    pub fn auto_name(&self, how: &str) -> io::Result<String> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let base = format!("{AUTO_SAVE_PREFIX}{how}-{stamp}");
        for attempt in 1..=MAX_SAME_SECOND {
            let name = if attempt == 1 {
                base.clone()
            } else {
                format!("{base}-{attempt}")
            };
            if self.find_save(&name)?.is_none() {
                return Ok(name);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("no free name for {base}"),
        ))
    }

    /// Bring the copies of the patch Tom made by hand beside `patch.json`
    /// (`patch.<label>.json`; not the wall's own `patch.before-*` and
    /// `patch.broken-*` files) in as saves named `<label>`, once: each is
    /// copied (never moved) through the same migration as `patch.json`, so
    /// an old one is saved up to date, keeping the copy's time. A copy that
    /// is not a wall, whose label is not a save name, or whose name is
    /// taken is left out and said why. A copy the disk will not read is
    /// named and tried again on the next start; every other copy comes in
    /// now, and the import is marked done only once none is left to try.
    /// Returns what was done, one sentence each; nothing once the copies
    /// are in.
    ///
    /// It is one batch with one full flush: every save written to a
    /// temporary file and passed to the disk (a plain fsync), all renamed
    /// in, then the saves directory flushed in full. The copies themselves
    /// are never touched, so a crash part-way loses nothing: what did not
    /// land is brought in again on the next start (a half-written save is
    /// listed as broken, and its copy is still there to bring in by hand).
    /// The mark is written after, passed to the disk only: if a crash takes
    /// it back, the next start finds every save in and writes it again.
    ///
    /// # Errors
    ///
    /// Fails if the disk refuses the saves: the state directory cannot be
    /// read, or a save cannot be written (the disk is full, say). Nothing is
    /// marked done then: the copies are tried again on the next start,
    /// skipping those already in.
    pub fn import_copies(&mut self) -> io::Result<Vec<String>> {
        let saves = self.open_saves()?;
        if saves.join(IMPORTED_MARK).exists() {
            return Ok(Vec::new());
        }
        let mut copies = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file = entry.file_name().to_string_lossy().into_owned();
            if let Some(label) = copy_label(&file) {
                copies.push((file.clone(), label.to_string(), entry.path()));
            }
        }
        copies.sort();
        let mut said = Vec::new();
        let mut coming: Vec<Incoming> = Vec::new();
        let mut unread = 0_usize;
        for (file, label, path) in copies {
            // Two copies whose labels differ only in case are one save.
            let taken = coming
                .iter()
                .find(|other| other.label.eq_ignore_ascii_case(&label))
                .map(|other| other.label.clone());
            match taken {
                Some(other) => said.push(format!(
                    "{file} was left out: the save \"{other}\" comes from another copy"
                )),
                None => match self.import_copy(&file, &label, &path) {
                    Ok(Ok(incoming)) => {
                        said.push(incoming.said.clone());
                        coming.push(incoming);
                    }
                    Ok(Err(left_out)) => said.push(left_out),
                    // The disk will not read this one: the rest come in,
                    // and it is tried again next time.
                    Err(err) => {
                        unread += 1;
                        said.push(format!("{err}; it is tried again on the next start"));
                    }
                },
            }
        }
        // One batch: every save written and passed to the disk, then all
        // renamed in, then the directory flushed in full once, and only
        // then the mark.
        let temporaries: Vec<PathBuf> = coming
            .iter()
            .map(|incoming| saves.join(format!("{}.json.tmp", incoming.label)))
            .collect();
        let tidy = || {
            for temporary in &temporaries {
                if let Err(err) = fs::remove_file(temporary) {
                    if err.kind() != io::ErrorKind::NotFound {
                        eprintln!(
                            "kazoo-wall: {} could not be tidied away: {err}",
                            temporary.display()
                        );
                    }
                }
            }
        };
        for (incoming, temporary) in coming.iter().zip(&temporaries) {
            if let Err(err) = write_passed(temporary, &incoming.bytes, Some(incoming.modified)) {
                tidy();
                return Err(err);
            }
        }
        for (incoming, temporary) in coming.iter().zip(&temporaries) {
            let target = saves.join(format!("{}.json", incoming.label));
            if let Err(err) = fs::rename(temporary, &target) {
                tidy();
                return Err(err);
            }
        }
        if !coming.is_empty() {
            full_flush(&saves)?;
        }
        if unread > 0 {
            return Ok(said);
        }
        let mark = if said.is_empty() {
            "No hand-made copies of the patch were found to bring in.\n".to_string()
        } else {
            format!("{}\n", said.join("\n"))
        };
        let temporary = saves.join(format!("{IMPORTED_MARK}.tmp"));
        let marked = write_passed(&temporary, mark.as_bytes(), None)
            .and_then(|()| fs::rename(&temporary, saves.join(IMPORTED_MARK)))
            .and_then(|()| plain_fsync(&File::open(&saves)?));
        if let Err(err) = marked {
            tidy_away(&temporary);
            return Err(err);
        }
        Ok(said)
    }

    /// Read one hand-made copy (see [`Self::import_copies`]): the save it
    /// becomes, or why it is left out, in a sentence.
    fn import_copy(
        &mut self,
        file: &str,
        label: &str,
        path: &Path,
    ) -> io::Result<Result<Incoming, String>> {
        if !valid_save_name(label) {
            return Ok(Err(format!(
                "{} was left out: \"{}\" cannot be a save's name (1 to {MAX_SAVE_NAME} of A-Z \
                 a-z 0-9 space _ . -, starting with a letter or digit)",
                printable(file),
                printable(label)
            )));
        }
        // The disk refusing is not the copy's fault: it is tried again on
        // the next start. Only what the copy holds can leave it out for
        // good.
        let read = self
            .inject(FailAt::ReadCopy)
            .and_then(|()| read_wall_file(path))
            .map_err(|err| {
                io::Error::new(err.kind(), format!("{file} could not be read: {err}"))
            })?;
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(reason) => return Ok(Err(format!("{file} was left out: {reason}"))),
        };
        let (wall, from_version, notes) = match decode(&bytes) {
            Ok(decoded) => decoded,
            Err(reason) => return Ok(Err(format!("{file} was left out: {reason}"))),
        };
        let wall_bytes = encode(&wall)?;
        if wall_bytes.len() as u64 > MAX_WALL_FILE {
            return Ok(Err(format!(
                "{file} was left out: {}",
                too_large(wall_bytes.len())
            )));
        }
        let modified = fs::metadata(path)
            .and_then(|meta| meta.modified())
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("when {file} was made cannot be read: {err}"),
                )
            })?;
        let version = if from_version == crate::patch::FILE_VERSION {
            String::new()
        } else {
            format!(
                " (version {from_version}, brought up to version {})",
                crate::patch::FILE_VERSION
            )
        };
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(": {}", notes.join("; "))
        };
        if let Some((stored, saved)) = self.find_save(label)? {
            // Brought in by an import that could not finish (another copy
            // would not read): the save is this copy, byte for byte.
            let same = matches!(read_wall_file(&saved), Ok(Ok(held)) if held == wall_bytes);
            return Ok(Err(if same {
                format!("brought {file} in as the save \"{stored}\"{version}{notes}")
            } else {
                format!("{file} was left out: the save \"{stored}\" is there already")
            }));
        }
        Ok(Ok(Incoming {
            label: label.to_string(),
            bytes: wall_bytes,
            modified,
            said: format!("brought {file} in as the save \"{label}\"{version}{notes}"),
        }))
    }

    /// Append `change` to the log, rotating it first if it is full.
    ///
    /// After a write or sync of the log failed, the handle is not used
    /// again: the log is opened afresh first, and the next line starts on
    /// a line of its own if the last one was cut short.
    ///
    /// # Errors
    ///
    /// Fails if the log cannot be reopened, written or rotated.
    pub fn append(&mut self, change: &Change) -> io::Result<()> {
        self.rotate_if_due()?;
        let mut line = serde_json::to_vec(change).map_err(io::Error::other)?;
        line.push(b'\n');
        // After a write that failed part-way, the next line starts on a
        // line of its own: the torn one is skipped on reading, and never
        // takes this one with it.
        let mut bytes = Vec::with_capacity(line.len() + 1);
        if self.torn {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&line);
        let written = self
            .inject(FailAt::AppendLog)
            .and_then(|()| self.log.write_all(&bytes))
            .and_then(|()| self.log.flush());
        if let Err(err) = written {
            self.torn = true;
            self.log_failed = true;
            return Err(err);
        }
        self.torn = false;
        self.log_lines += 1;
        self.unsynced.push(line);
        Ok(())
    }

    /// Append `change` to the log and sync it to disk.
    ///
    /// # Errors
    ///
    /// Fails if the log cannot be written, rotated or synced; the line may
    /// be on disk even so.
    pub fn append_synced(&mut self, change: &Change) -> io::Result<()> {
        self.append(change)?;
        self.sync_log()
    }

    /// Sync the log to disk. After a failure (of this or of a write) the
    /// log is opened afresh and the lines since the last good sync written
    /// again first (see [`Self::reopen_log`]).
    ///
    /// # Errors
    ///
    /// Fails if the log cannot be reopened, or the sync fails.
    pub fn sync_log(&mut self) -> io::Result<()> {
        if self.log_failed {
            return self.reopen_log();
        }
        let synced = self
            .inject(FailAt::SyncLog)
            .and_then(|()| self.log.sync_data());
        if let Err(err) = synced {
            self.log_failed = true;
            return Err(err);
        }
        self.unsynced.clear();
        Ok(())
    }

    /// Open the log afresh after a write or sync of it failed, and make
    /// it whole again: the handle that failed is never used again, and
    /// every line appended since the last good sync is written again and
    /// synced. On Linux a failed fsync may already have dropped those
    /// pages while the file still reads as if they were there, so a sync
    /// that succeeds on a new handle alone would not make them durable. A
    /// line written twice is read once (see [`Self::load_log`]). The log
    /// is trusted again only once all of that is done.
    ///
    /// # Errors
    ///
    /// Fails if the log cannot be opened, written or synced; it stays
    /// untrusted.
    fn reopen_log(&mut self) -> io::Result<()> {
        let log_path = self.dir.join(LOG_FILE);
        self.log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        self.torn = ends_torn(&log_path)?;
        self.log_lines = count_lines(&log_path)?;
        let mut bytes = Vec::new();
        if self.torn && !self.unsynced.is_empty() {
            bytes.push(b'\n');
        }
        for line in &self.unsynced {
            bytes.extend_from_slice(line);
        }
        let rewritten = self
            .inject(FailAt::AppendLog)
            .and_then(|()| self.log.write_all(&bytes))
            .and_then(|()| self.inject(FailAt::SyncLog))
            // The lines passed to the disk, then the directory (its names,
            // after a rotation that could not flush it) and they flushed
            // in full together.
            .and_then(|()| plain_fsync(&self.log))
            .and_then(|()| full_flush(&self.dir));
        if let Err(err) = rewritten {
            self.torn = true;
            return Err(err);
        }
        if !self.unsynced.is_empty() {
            self.torn = false;
            self.log_lines += self.unsynced.len();
            self.unsynced.clear();
        }
        self.log_failed = false;
        Ok(())
    }

    /// Rotate the log, with one full flush: the old log passed to the disk
    /// (a plain fsync), renamed to the previous log, a new one made, and
    /// the state directory flushed in full, which puts the old log's lines
    /// and both names on the disk together before a line goes into the
    /// new log (else a crash could take the rename back, and the new log's
    /// lines with it). Lines appended since the last good flush are kept
    /// to write again until that flush is done (see [`Self::reopen_log`]),
    /// so a rotation cut short loses none of them.
    fn rotate(&mut self) -> io::Result<()> {
        plain_fsync(&self.log)?;
        fs::rename(self.dir.join(LOG_FILE), self.dir.join(OLD_LOG_FILE))?;
        self.log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(LOG_FILE))?;
        self.log_lines = 0;
        // A torn line stays in the previous log; the new one starts clean.
        self.torn = false;
        // (On failure the caller marks the log failed; making it whole
        // again flushes the directory too.)
        self.inject(FailAt::SyncRotation)
            .and_then(|()| full_flush(&self.dir))?;
        // On the disk in the file that is now the previous log.
        self.unsynced.clear();
        Ok(())
    }

    /// Rotate the log now if the next line would, so a batch with an
    /// order of its own (a load) never also pays for a rotation: it is
    /// done, and its flush paid, before the batch begins.
    fn rotate_if_due(&mut self) -> io::Result<()> {
        if self.log_failed {
            self.reopen_log()?;
        }
        if self.log_lines >= LOG_ROTATE_LINES {
            if let Err(err) = self.rotate() {
                self.log_failed = true;
                return Err(err);
            }
        }
        Ok(())
    }

    /// Read back the logged changes in the order of their numbers: the
    /// previous log, then the current one. Lines that cannot be read are
    /// skipped and counted, and so are lines repeating a number read
    /// already (the first is kept).
    ///
    /// # Errors
    ///
    /// Fails if a log exists but cannot be read.
    pub fn load_log(&self) -> io::Result<LoadedLog> {
        let mut loaded = LoadedLog::default();
        let mut seen = std::collections::HashSet::new();
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
                    // A change written again (after a write or sync that
                    // failed, see `reopen_log`): the first is the one.
                    Ok(change) if !seen.insert(change.seq) => loaded.repeated += 1,
                    Ok(change) => loaded.changes.push(change),
                    Err(_) => loaded.skipped += 1,
                }
            }
        }
        // Lines written again after a failed sync may follow lines that
        // came after them: the log is in the order of its numbers. (In
        // order already, as it is unless that happened, this costs one
        // pass.)
        if !loaded.changes.is_sorted_by_key(|change| change.seq) {
            loaded.changes.sort_by_key(|change| change.seq);
        }
        Ok(loaded)
    }
}

/// What the save `name` at `path` says of itself, exactly as a load would
/// find it: the whole file read through the migration, which forgives what
/// it can, so the tempo and counts are those of the wall a load puts up
/// (parts that cannot load left out), and a file a load would refuse is
/// listed as broken, saying why. (Listing keeps what it found by the file's
/// stamp, so each file is read whole once per change.)
fn header_of(name: &str, path: &Path) -> Result<Header, String> {
    if !valid_save_name(name) {
        return Err(format!(
            "its name is not a save's name: rename the file to 1 to {MAX_SAVE_NAME} of A-Z a-z \
             0-9 space _ . -, starting with a letter or digit"
        ));
    }
    let bytes = read_wall_file(path).map_err(|err| format!("it could not be read: {err}"))??;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    decode(&bytes).map(|(patch, version, _)| Header {
        tempo: patch.tempo,
        modules: count(patch.modules().len()),
        cables: count(patch.cables().len()),
        version,
    })
}

/// The bytes of the file at `path`, read as a wall's: at most
/// [`MAX_WALL_FILE`] of them, so no file, however large, is read whole
/// into memory. A larger file is not a wall, and says so in a sentence.
///
/// # Errors
///
/// Fails if the file cannot be opened or read.
fn read_wall_file(path: &Path) -> io::Result<Result<Vec<u8>, String>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_WALL_FILE + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_WALL_FILE {
        return Ok(Err(format!(
            "it is too large to be a wall (more than {} MB; a wall of a hundred modules is \
             about 100 KB)",
            MAX_WALL_FILE / (1024 * 1024)
        )));
    }
    Ok(Ok(bytes))
}

/// A patch from a file's bytes, brought up to date (see [`crate::migrate`])
/// keeping everything that can load: the patch, the file's version before,
/// and what was migrated or left out.
fn decode(bytes: &[u8]) -> Result<(Patch, u32, Vec<String>), String> {
    let (mut file, mut notes) = migrate::read(bytes)?;
    let from_version = file.version;
    notes.extend(migrate::migrate(&mut file)?);
    let (patch, left_out) = Patch::load(&file)?;
    notes.extend(left_out);
    Ok((patch, from_version, notes))
}

/// `patch` as its file's bytes, as every save and `patch.json` are written.
///
/// # Errors
///
/// Never in practice: a patch is plain data.
pub fn encode(patch: &Patch) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&patch.to_file()).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// A file put in place, and why its directory could not then be synced, if
/// it could not: the file is there, but a crash may yet take the rename
/// back.
type Unsynced = Option<io::Error>;

/// Write `bytes` to `dir/file` (`file.tmp` beside it, dated `modified`
/// when given) and make it durable, so a crash leaves the old file or the
/// new one, never half of either: the temporary file flushed to the disk
/// in full, renamed over the old one, then the directory flushed in full.
/// Two full flushes: the file's bytes must be on the disk before the
/// rename that names them can be. A write that fails before the rename (a
/// full disk) takes its temporary file away again; once renamed, the file
/// is in place, and a directory that cannot be flushed is said in the
/// result rather than as a failure.
///
/// # Errors
///
/// Fails if the file cannot be written, flushed or renamed: the old file,
/// if any, is then untouched.
fn write_durably(
    dir: &Path,
    file: &str,
    bytes: &[u8],
    modified: Option<SystemTime>,
    sync_dir: impl FnOnce() -> io::Result<()>,
) -> io::Result<Unsynced> {
    let temporary = dir.join(format!("{file}.tmp"));
    let written = write_temporary(&temporary, bytes, modified)
        .and_then(|file| file.sync_all())
        .and_then(|()| fs::rename(&temporary, dir.join(file)));
    if let Err(err) = written {
        tidy_away(&temporary);
        return Err(err);
    }
    Ok(sync_dir().and_then(|()| full_flush(dir)).err())
}

/// Take a temporary file away again after a write failed, saying so if
/// it cannot be (one never made is fine).
fn tidy_away(temporary: &Path) {
    if let Err(err) = fs::remove_file(temporary) {
        if err.kind() != io::ErrorKind::NotFound {
            eprintln!(
                "kazoo-wall: {} could not be tidied away: {err}",
                temporary.display()
            );
        }
    }
}

/// Write `bytes` to `path` (a temporary file), dated `modified` when
/// given, and hand back the file, not yet flushed.
fn write_temporary(path: &Path, bytes: &[u8], modified: Option<SystemTime>) -> io::Result<File> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    if let Some(at) = modified {
        file.set_modified(at)?;
    }
    Ok(file)
}

/// Write `bytes` to `path` (a temporary file), dated `modified` when
/// given, and pass it to the disk with a plain fsync: durable once a full
/// flush follows it (see [`plain_fsync`]).
fn write_passed(path: &Path, bytes: &[u8], modified: Option<SystemTime>) -> io::Result<()> {
    plain_fsync(&write_temporary(path, bytes, modified)?)
}

/// A plain fsync: the system passes `file` to the disk, without waiting
/// for the disk to commit its own cache. On Linux that is as durable as
/// any sync; on macOS std's `sync_all` and `sync_data` are each a full
/// flush (`F_FULLFSYNC`, which commits everything the disk holds), and
/// this is not. So writes passed with it and then covered by one full
/// flush are durable together, for the cost of one.
fn plain_fsync(file: &File) -> io::Result<()> {
    rustix::fs::fsync(file).map_err(io::Error::from)
}

/// Flush the directory `dir` in full: its entries, and on macOS
/// everything the disk was passed before (see [`plain_fsync`]).
fn full_flush(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// A hand-made copy read for the import: the save it becomes.
#[derive(Debug)]
struct Incoming {
    /// The save's name.
    label: String,
    /// The save's bytes, brought up to date.
    bytes: Vec<u8>,
    /// When the copy was made.
    modified: SystemTime,
    /// What became of it, in a sentence.
    said: String,
}

/// What a load's batch did (see [`Store::commit_load`]).
#[derive(Debug)]
pub struct LoadWritten {
    /// Why the new wall is not on disk yet, when it is not: the load has
    /// happened (its change is in the log), and the wall is saved again
    /// with the next save.
    pub unsaved: Option<io::Error>,
}

/// Why a load's batch stopped (see [`Store::commit_load`]).
#[derive(Debug)]
pub enum LoadWriteError {
    /// Before its change reached the log: nothing happened (a backup
    /// renamed into place is unreferenced, and pruned like any other).
    NotLogged(io::Error),
    /// While its change was going to the log: the line may be on disk, so
    /// its number is used, but the load did not happen; the caller says so
    /// in the log at once ([`crate::protocol::What::Abandoned`]).
    MaybeLogged(io::Error),
    /// Before the load wrote anything: the log could not be made ready
    /// for the load's line (it was due to rotate, or had to be made whole
    /// after a failure, and that failed). The wall and its saves are as
    /// they were; only the log's own housekeeping may have begun.
    LogNotReady(io::Error),
    /// Too large, before anything was written, the log included.
    ///
    /// The new wall is larger than a wall's file may be (see
    /// [`MAX_WALL_FILE`]), so it could not be read back. How large it is.
    /// Nothing happened.
    TooLarge(usize),
}

impl fmt::Display for LoadWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLogged(err) | Self::MaybeLogged(err) | Self::LogNotReady(err) => {
                write!(f, "{err}")
            }
            Self::TooLarge(bytes) => f.write_str(&too_large(*bytes)),
        }
    }
}

impl std::error::Error for LoadWriteError {}

/// What [`Store::write_save`] wrote.
#[derive(Debug)]
pub struct Written {
    /// The save's name as stored.
    pub name: String,
    /// Whether it replaced a save of that name.
    pub replaced: bool,
    /// [`hash_bytes`] of the bytes written.
    pub hash: String,
    /// Why the saves directory could not be flushed after, if it could
    /// not: the save is in place (any old one replaced), but a crash may
    /// yet take it back.
    pub unsynced: Option<io::Error>,
}

/// The label of a hand-made copy of the patch: `label` for
/// `patch.<label>.json`, but not `patch.json` itself, nor the wall's own
/// `patch.before-*` (originals kept by a migration) and `patch.broken-*`
/// (files it could not use).
fn copy_label(file: &str) -> Option<&str> {
    let label = file.strip_prefix("patch.")?.strip_suffix(".json")?;
    let own = ["before-", "broken-"]
        .iter()
        .any(|prefix| label.starts_with(prefix));
    (!label.is_empty() && !own).then_some(label)
}

/// Whether the file at `path` ends part-way through a line: it is not
/// empty, and its last byte is not a newline (a write cut short). False
/// if it does not exist.
fn ends_torn(path: &Path) -> io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
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

    // Direct model fixtures have no numbered daemon change to stamp them.
    fn mark_fixture_births(patch: &mut Patch) {
        let mut notes = Vec::new();
        patch.find_births(&mut notes);
        assert!(notes.is_empty(), "{notes:?}");
    }

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
            clock: None,
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
        let mut store = Store::open(root.path()).unwrap();
        assert!(matches!(store.load_patch().unwrap(), Loaded::Missing));
        let mut patch = Patch::empty(97.0);
        patch.add("vco", Some("drone"), None).unwrap();
        mark_fixture_births(&mut patch);
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

    /// Tom's "Airports" wall as version 1 saved it.
    const AIRPORTS: &str = include_str!("../tests/fixtures/airports-v1.json");

    fn a_wall() -> Patch {
        let mut patch = Patch::empty(97.0);
        patch.add("vco", Some("drone"), None).unwrap();
        patch.add("out", None, None).unwrap();
        mark_fixture_births(&mut patch);
        patch
            .plug("vco1.out", "out1.left", Some(0.5), None)
            .unwrap();
        patch
    }

    fn names(store: &mut Store) -> Vec<String> {
        let mut names: Vec<String> = store
            .list_saves()
            .unwrap()
            .into_iter()
            .map(|listed| listed.name)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn saves_round_trip_durably_and_privately() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        assert!(store.list_saves().unwrap().is_empty(), "none before any");
        let patch = a_wall();
        let written = store.write_save("first jam", &patch, false).unwrap();
        assert_eq!(
            (written.name.as_str(), written.replaced),
            ("first jam", false)
        );
        // patch.json's format, byte for byte, and the hash of those bytes,
        // which reading it back gives too.
        let file = root.path().join(SAVES_DIR).join("first jam.json");
        assert_eq!(written.hash, hash_bytes(&fs::read(&file).unwrap()));
        assert_eq!(store.read_save("first jam").unwrap().hash, written.hash);
        assert_eq!(store.hash_save("first jam").unwrap(), Some(written.hash));
        assert_eq!(store.hash_save("no such").unwrap(), None);
        assert_eq!(fs::read(&file).unwrap(), encode(&patch).unwrap());
        let mode = fs::metadata(root.path().join(SAVES_DIR))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        assert!(
            !root
                .path()
                .join(SAVES_DIR)
                .join("first jam.json.tmp")
                .exists()
        );
        let read = store.read_save("FIRST JAM").unwrap();
        assert_eq!(read.name, "first jam", "found whatever the case");
        assert_eq!(*read.patch, patch);
        assert_eq!(read.from_version, crate::patch::FILE_VERSION);
        assert!(read.notes.is_empty());
        let listed = store.list_saves().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].read.clone().expect("read"),
            Ok(Header {
                tempo: 97.0,
                modules: 2,
                cables: 1,
                version: crate::patch::FILE_VERSION,
            })
        );
        assert_eq!(store.saved_at("first jam").unwrap(), listed[0].saved_at);
        let (deleted, unsynced) = store.delete_save("First Jam").unwrap();
        assert_eq!(deleted, "first jam");
        assert!(unsynced.is_none());
        assert!(!file.exists());
        assert!(matches!(
            store.delete_save("first jam"),
            Err(SaveError::Unknown(_))
        ));
        assert!(matches!(
            store.read_save("first jam"),
            Err(SaveError::Unknown(_))
        ));
    }

    #[test]
    fn a_save_is_replaced_only_when_asked_and_keeps_its_name() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let first = a_wall();
        store.write_save("Jam", &first, false).unwrap();
        let mut second = a_wall();
        second.add("lfo", None, None).unwrap();
        mark_fixture_births(&mut second);
        // Refused, in any case, and the save is untouched.
        for name in ["Jam", "jam", "JAM"] {
            let Err(SaveError::Exists(there)) = store.write_save(name, &second, false) else {
                panic!("{name} replaced a save without being asked");
            };
            assert_eq!(there, "Jam");
        }
        assert_eq!(*store.read_save("Jam").unwrap().patch, first);
        // Asked, it replaces the save under the name it had.
        let written = store.write_save("jam", &second, true).unwrap();
        assert_eq!((written.name.as_str(), written.replaced), ("Jam", true));
        assert_eq!(*store.read_save("jam").unwrap().patch, second);
        assert_eq!(names(&mut store), ["Jam"]);
    }

    #[test]
    fn a_save_that_cannot_be_written_leaves_the_old_one_whole() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let first = a_wall();
        store.write_save("keep", &first, false).unwrap();
        // The disk refusing (it is full, say).
        let saves = root.path().join(SAVES_DIR);
        let mut second = a_wall();
        second.add("lfo", None, None).unwrap();
        mark_fixture_births(&mut second);
        store.fail_once(FailAt::WriteSave);
        let failed = store.write_save("keep", &second, true);
        store.fail_once(FailAt::WriteSave);
        let new = store.write_save("new", &second, false);
        assert!(matches!(failed, Err(SaveError::Io(_))), "{failed:?}");
        assert!(matches!(new, Err(SaveError::Io(_))), "{new:?}");
        assert_eq!(*store.read_save("keep").unwrap().patch, first);
        assert_eq!(names(&mut store), ["keep"], "nothing half-written is left");
        assert_eq!(fs::read_dir(&saves).unwrap().count(), 1);
    }

    #[test]
    fn broken_old_and_misnamed_saves_are_told_apart() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let saves = root.path().join(SAVES_DIR);
        fs::create_dir_all(&saves).unwrap();
        fs::write(saves.join("torn.json"), b"{ \"version\": 2, ").unwrap();
        fs::write(saves.join("airports.json"), AIRPORTS).unwrap();
        fs::write(saves.join("..json"), b"{}").unwrap();
        fs::write(saves.join("bad name!.json"), encode(&a_wall()).unwrap()).unwrap();
        fs::write(saves.join("notes.txt"), b"not a save").unwrap();
        fs::write(saves.join(".imported"), b"").unwrap();
        // A torn file is broken, and left as it is.
        let Err(SaveError::Broken { name, reason }) = store.read_save("torn") else {
            panic!("a torn save loaded");
        };
        assert_eq!(name, "torn");
        assert!(reason.contains("not JSON"), "{reason}");
        assert_eq!(
            fs::read(saves.join("torn.json")).unwrap(),
            b"{ \"version\": 2, "
        );
        // An old one is brought up to date as it is read; its file is not.
        let airports = store.read_save("airports").unwrap();
        assert_eq!(airports.from_version, 1);
        assert_eq!(airports.patch.modules().len(), 47);
        assert!(!airports.notes.is_empty());
        assert_eq!(
            fs::read(saves.join("airports.json")).unwrap(),
            AIRPORTS.as_bytes()
        );
        // A file whose name is not a save's cannot be found by it, but is
        // listed, as broken; hidden files and other files are not.
        assert!(matches!(
            store.read_save("bad name!"),
            Err(SaveError::Unknown(_))
        ));
        let listed = store.list_saves().unwrap();
        let mut seen: Vec<(&str, bool)> = listed
            .iter()
            .map(|listed| (listed.name.as_str(), listed.read.as_ref().unwrap().is_ok()))
            .collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            [("airports", true), ("bad name!", false), ("torn", false)]
        );
    }

    #[test]
    fn automatic_saves_take_a_free_name() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let first = store.auto_name("load").unwrap();
        assert!(first.starts_with("before-load-"), "{first}");
        assert!(crate::protocol::valid_save_name(&first));
        assert!(crate::protocol::is_auto_save(&first));
        store.write_save(&first, &a_wall(), false).unwrap();
        let second = store.auto_name("load").unwrap();
        assert_ne!(second, first);
        // Within the same second it is numbered on.
        if second.starts_with(&first) {
            assert_eq!(second, format!("{first}-2"));
        }
    }

    #[test]
    fn hand_made_copies_come_in_as_saves_once() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let current = encode(&a_wall()).unwrap();
        fs::write(dir.join(PATCH_FILE), &current).unwrap();
        fs::write(dir.join("patch.tom-fiddle-2026-09-29.json"), &current).unwrap();
        fs::write(
            dir.join("patch.blade-runner-hour-2026-09-27.json"),
            AIRPORTS,
        )
        .unwrap();
        fs::write(dir.join("patch.before-1790000000.json"), &current).unwrap();
        fs::write(dir.join("patch.broken-1790000000.json"), b"[]").unwrap();
        fs::write(dir.join("patch.torn.json"), b"{").unwrap();
        fs::write(dir.join("patch.not ok!.json"), &current).unwrap();
        fs::write(dir.join("patch.json.tmp"), &current).unwrap();
        // The copy's time comes with it.
        let made = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_424_001);
        File::options()
            .write(true)
            .open(dir.join("patch.tom-fiddle-2026-09-29.json"))
            .unwrap()
            .set_modified(made)
            .unwrap();
        let said = store.import_copies().unwrap();
        assert_eq!(said.len(), 4, "{said:?}");
        assert!(
            said.iter().any(|line| line
                == "brought patch.tom-fiddle-2026-09-29.json in as the save \
                    \"tom-fiddle-2026-09-29\""),
            "{said:?}"
        );
        assert!(
            said.iter().any(|line| line.starts_with(
                "brought patch.blade-runner-hour-2026-09-27.json in as the save \
                 \"blade-runner-hour-2026-09-27\" (version 1, brought up to version 2): "
            )),
            "{said:?}"
        );
        assert!(
            said.iter()
                .any(|line| line.starts_with("patch.torn.json was left out"))
        );
        assert!(
            said.iter()
                .any(|line| line.starts_with("patch.not ok!.json was left out"))
        );
        assert_eq!(
            names(&mut store),
            ["blade-runner-hour-2026-09-27", "tom-fiddle-2026-09-29"]
        );
        // Copied, not moved; the old one is saved up to date.
        assert!(dir.join("patch.blade-runner-hour-2026-09-27.json").exists());
        let airports = store.read_save("blade-runner-hour-2026-09-27").unwrap();
        assert_eq!(airports.from_version, crate::patch::FILE_VERSION);
        assert!(airports.notes.is_empty(), "{:?}", airports.notes);
        assert_eq!(airports.patch.modules().len(), 47);
        assert_eq!(store.saved_at("tom-fiddle-2026-09-29").unwrap(), made);
        // Once: a deleted save stays deleted.
        store.delete_save("tom-fiddle-2026-09-29").unwrap();
        assert!(store.import_copies().unwrap().is_empty());
        assert_eq!(names(&mut store), ["blade-runner-hour-2026-09-27"]);
    }

    #[test]
    fn an_import_cut_short_carries_on_next_time_without_doubling() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let current = encode(&a_wall()).unwrap();
        fs::write(dir.join("patch.one.json"), &current).unwrap();
        fs::write(dir.join("patch.two.json"), &current).unwrap();
        // One came in before the import was cut short; its save is kept.
        let mut other = a_wall();
        other.add("lfo", None, None).unwrap();
        mark_fixture_births(&mut other);
        store.write_save("one", &other, false).unwrap();
        let said = store.import_copies().unwrap();
        assert!(
            said.contains(
                &"patch.one.json was left out: the save \"one\" is there already".to_string()
            ),
            "{said:?}"
        );
        assert_eq!(*store.read_save("one").unwrap().patch, other);
        assert_eq!(names(&mut store), ["one", "two"]);
    }

    #[test]
    fn only_hand_made_copies_have_labels() {
        assert_eq!(copy_label("patch.jam.json"), Some("jam"));
        assert_eq!(
            copy_label("patch.garage-korgy-2026-09-29.json"),
            Some("garage-korgy-2026-09-29")
        );
        for not in [
            "patch.json",
            "patch..json",
            "patch.json.tmp",
            "patch.before-1790000000.json",
            "patch.broken-1790000000-2.json",
            "log.jsonl",
            "patchy.json",
        ] {
            assert_eq!(copy_label(not), None, "{not}");
        }
    }

    /// Date `path` `seconds` after a fixed time.
    fn date(path: &Path, seconds: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000 + seconds),
            )
            .unwrap();
    }

    #[test]
    fn listing_reads_each_header_once_until_the_file_moves() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let saves = root.path().join(SAVES_DIR);
        store.write_save("jam", &a_wall(), false).unwrap();
        fs::write(saves.join("airports.json"), AIRPORTS).unwrap();
        // A tempo that is not a number: the migration forgives it, as a
        // load would.
        let mut odd = serde_json::to_value(a_wall().to_file()).unwrap();
        odd["tempo"] = serde_json::json!("fast");
        fs::write(saves.join("odd.json"), serde_json::to_vec(&odd).unwrap()).unwrap();
        let header = |store: &mut Store, name: &str| {
            store
                .list_saves()
                .unwrap()
                .into_iter()
                .find(|listed| listed.name == name)
                .unwrap()
                .read
                .expect("read")
        };
        assert_eq!(
            header(&mut store, "airports"),
            Ok(Header {
                tempo: 68.0,
                modules: 47,
                // As a load finds it, brought up to date: the file holds
                // 55, and the migration makes 59 of them.
                cables: 59,
                version: 1,
            })
        );
        assert_eq!(
            store.read_save("airports").unwrap().patch.cables().len(),
            59
        );
        assert_eq!(header(&mut store, "odd").map(|h| h.tempo), Ok(120.0));
        // Untouched, the header kept is the one used.
        let jam = saves.join("jam.json");
        let length = fs::metadata(&jam).unwrap().len();
        date(&jam, 7);
        assert_eq!(header(&mut store, "jam").map(|h| h.modules), Ok(2));
        let stamp = store.headers["jam"].0;
        assert_eq!(header(&mut store, "jam").map(|h| h.modules), Ok(2));
        assert_eq!(store.headers["jam"].0, stamp);
        // Garbled behind the store's back at the same length, and dated
        // back to the same time: its change time moved, so it is read
        // again, and now it cannot be loaded.
        fs::write(&jam, vec![b' '; usize::try_from(length).unwrap()]).unwrap();
        date(&jam, 7);
        assert!(header(&mut store, "jam").is_err());
        // Another file renamed over it, at the same length and time: its
        // inode moved.
        let other = saves.join("other.json.tmp");
        fs::write(&other, encode(&a_wall()).unwrap()).unwrap();
        let wanted = fs::metadata(&jam).unwrap().len();
        let mut bytes = fs::read(&other).unwrap();
        bytes.resize(usize::try_from(wanted).unwrap(), b' ');
        fs::write(&other, &bytes).unwrap();
        date(&other, 7);
        fs::rename(&other, &jam).unwrap();
        assert_eq!(header(&mut store, "jam").map(|h| h.modules), Ok(2));
        date(&jam, 8);
        // Written through the store: read again whatever the clock says.
        store.write_save("jam", &Patch::empty(100.0), true).unwrap();
        date(&jam, 8);
        assert_eq!(header(&mut store, "jam").map(|h| h.modules), Ok(0));
        // A save gone is forgotten.
        store.delete_save("jam").unwrap();
        store.list_saves().unwrap();
        assert!(!store.headers.contains_key("jam"));
    }

    #[test]
    fn the_walls_own_saves_are_pruned_beyond_the_newest_unless_needed() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let saves = root.path().join(SAVES_DIR);
        let wall = a_wall();
        store.write_save("tom fiddle", &wall, false).unwrap();
        date(&saves.join("tom fiddle.json"), 0);
        let auto = |n: usize| format!("before-load-20260927-{n:06}");
        for n in 0..AUTO_SAVES_KEPT + 5 {
            store.write_save(&auto(n), &wall, false).unwrap();
            date(&saves.join(format!("{}.json", auto(n))), 10 + n as u64);
        }
        // The oldest is still needed by a load in the log.
        let needed = auto(0);
        let need = |name: &str| {
            if name == needed {
                Need::Seat
            } else {
                Need::Not
            }
        };
        let deleted = store.prune_auto_saves(&need).unwrap();
        assert_eq!(deleted, (1..5).rev().map(auto).collect::<Vec<_>>());
        assert!(saves.join(format!("{}.json", auto(0))).exists());
        assert!(saves.join("tom fiddle.json").exists(), "never Tom's");
        assert_eq!(names(&mut store).len(), AUTO_SAVES_KEPT + 2);
        // Pruning again finds nothing more to do.
        assert!(store.prune_auto_saves(&need).unwrap().is_empty());
        // However many are needed, no more than the most are kept.
        for n in 100..100 + AUTO_SAVES_MOST {
            store.write_save(&auto(n), &wall, false).unwrap();
            date(&saves.join(format!("{}.json", auto(n))), 10 + n as u64);
        }
        store.prune_auto_saves(&|_| Need::Seat).unwrap();
        let autos = names(&mut store)
            .into_iter()
            .filter(|name| is_auto_save(name))
            .count();
        assert_eq!(autos, AUTO_SAVES_MOST);
        assert!(saves.join("tom fiddle.json").exists());
    }

    #[test]
    fn the_consoles_way_back_is_kept_before_a_seats() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let saves = root.path().join(SAVES_DIR);
        let wall = a_wall();
        let auto = |n: usize| format!("before-load-20261009-{n:06}");
        // The console's loads, oldest; then a seat's loads by the hundred,
        // every one still needed by the log.
        let console = 5;
        let total = AUTO_SAVES_MOST + 50;
        for n in 0..total {
            store.write_save(&auto(n), &wall, false).unwrap();
            date(&saves.join(format!("{}.json", auto(n))), 10 + n as u64);
        }
        let need = |name: &str| {
            let n: usize = name.rsplit('-').next().unwrap().parse().unwrap();
            if n < console {
                Need::Console
            } else {
                Need::Seat
            }
        };
        let deleted = store.prune_auto_saves(&need).unwrap();
        assert_eq!(deleted.len(), total - AUTO_SAVES_MOST);
        for n in 0..console {
            assert!(
                saves.join(format!("{}.json", auto(n))).exists(),
                "the console's {} was let go for a seat's",
                auto(n)
            );
        }
        // The newest are kept whatever; the seat's oldest went.
        for n in total - AUTO_SAVES_KEPT..total {
            assert!(saves.join(format!("{}.json", auto(n))).exists());
        }
        assert!(!saves.join(format!("{}.json", auto(console))).exists());
    }

    #[test]
    fn a_copy_the_disk_will_not_read_is_tried_again_next_start() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let copy = dir.join("patch.jam.json");
        fs::write(&copy, encode(&a_wall()).unwrap()).unwrap();
        fs::write(dir.join("patch.riff.json"), encode(&a_wall()).unwrap()).unwrap();
        // The first copy (in order) will not read: the other comes in.
        store.fail_once(FailAt::ReadCopy);
        let said = store.import_copies().unwrap();
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(
            said[0].starts_with("patch.jam.json could not be read")
                && said[0].ends_with("it is tried again on the next start"),
            "{said:?}"
        );
        assert_eq!(said[1], "brought patch.riff.json in as the save \"riff\"");
        assert_eq!(names(&mut store), ["riff"]);
        assert!(
            !dir.join(SAVES_DIR).join(IMPORTED_MARK).exists(),
            "not marked done while one is left"
        );
        // Next start: it comes in, and the import is done.
        let said = store.import_copies().unwrap();
        assert_eq!(
            said,
            [
                "brought patch.jam.json in as the save \"jam\"",
                // Brought in last time, byte for byte: said so again.
                "brought patch.riff.json in as the save \"riff\"",
            ]
        );
        let mut both = names(&mut store);
        both.sort();
        assert_eq!(both, ["jam", "riff"]);
        assert!(dir.join(SAVES_DIR).join(IMPORTED_MARK).exists());
        assert!(store.import_copies().unwrap().is_empty());
    }

    #[test]
    fn copies_named_alike_but_for_case_come_in_once() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let bytes = encode(&a_wall()).unwrap();
        fs::write(dir.join("patch.Jam.json"), &bytes).unwrap();
        fs::write(dir.join("patch.jam.json"), &bytes).unwrap();
        // On a disk that ignores case (macOS's, as a rule) the second is the
        // first written again; on one that does not, there are two copies.
        let copies = fs::read_dir(dir)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("patch.jam.json")
            })
            .count();
        let said = store.import_copies().unwrap();
        if copies == 2 {
            assert_eq!(
                said,
                [
                    "brought patch.Jam.json in as the save \"Jam\"",
                    "patch.jam.json was left out: the save \"Jam\" comes from another copy",
                ]
            );
        } else {
            assert_eq!(said, ["brought patch.Jam.json in as the save \"Jam\""]);
        }
        assert_eq!(names(&mut store), ["Jam"]);
        let leftovers = fs::read_dir(dir.join(SAVES_DIR))
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    fn a_load(seq: u64) -> Change {
        Change {
            seq,
            at: "2026-10-09T03:30:05Z".to_string(),
            seat: "Tom".to_string(),
            what: What::Load {
                save: Some("jam".to_string()),
                backup: "before-load-20261009-143005".to_string(),
                fingerprint: "0".to_string(),
                modules: 2,
                cables: 1,
                tempo: 97.0,
                by_console: true,
            },
            summary: "Tom loaded the save \"jam\"".to_string(),
            undoes: None,
            clock: None,
        }
    }

    #[test]
    fn a_load_lands_in_one_batch_or_not_at_all() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let old = a_wall();
        store.save_patch(&old).unwrap();
        let backup_bytes = encode(&old).unwrap();
        let mut new = Patch::empty(120.0);
        new.last_seq = 1;
        let written = store
            .commit_load(
                Some(("before-load-20261009-143005", &backup_bytes)),
                &new,
                &a_load(1),
            )
            .unwrap();
        assert!(written.unsaved.is_none());
        // The backup, the line, the wall: all there, nothing half-written.
        let kept = dir.join(SAVES_DIR).join("before-load-20261009-143005.json");
        assert_eq!(fs::read(&kept).unwrap(), backup_bytes);
        assert_eq!(store.load_log().unwrap().changes, [a_load(1)]);
        let Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
            panic!("no patch");
        };
        assert_eq!(*patch, new);
        assert!(!dir.join("patch.json.tmp").exists());
        assert!(
            !dir.join(SAVES_DIR)
                .join("before-load-20261009-143005.json.tmp")
                .exists()
        );
        // A wall that cannot be written: nothing logged, nothing left.
        store.fail_once(FailAt::WriteTemp);
        let failed = store.commit_load(
            Some(("before-load-20261009-150000", &backup_bytes)),
            &new,
            &a_load(2),
        );
        assert!(
            matches!(failed, Err(LoadWriteError::NotLogged(_))),
            "{failed:?}"
        );
        assert_eq!(store.load_log().unwrap().changes, [a_load(1)]);
        assert!(
            !dir.join(SAVES_DIR)
                .join("before-load-20261009-150000.json")
                .exists()
        );
        assert!(
            !dir.join(SAVES_DIR)
                .join("before-load-20261009-150000.json.tmp")
                .exists()
        );
        // No backup to write (the wall was kept in a save already): only
        // the line and the wall.
        let written = store.commit_load(None, &old, &a_load(3)).unwrap();
        assert!(written.unsaved.is_none());
        assert_eq!(store.load_log().unwrap().changes.len(), 2);
    }

    /// What a load whose batch failed at `at` leaves: the result, the
    /// changes in the log, whether the backup is among the saves, and
    /// what `patch.json` holds.
    fn a_load_failing_at(
        at: FailAt,
    ) -> (Result<LoadWritten, LoadWriteError>, Vec<u64>, bool, Patch) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let old = a_wall();
        store.save_patch(&old).unwrap();
        let backup_bytes = encode(&old).unwrap();
        let mut new = Patch::empty(120.0);
        new.last_seq = 1;
        new.wall = 1;
        store.fail_once(at);
        let result = store.commit_load(
            Some(("before-load-20261009-143005", &backup_bytes)),
            &new,
            &a_load(1),
        );
        let logged = store
            .load_log()
            .unwrap()
            .changes
            .iter()
            .map(|change| change.seq)
            .collect();
        let saves = dir.join(SAVES_DIR);
        let kept = saves.join("before-load-20261009-143005.json").exists();
        // Nothing temporary is ever left behind.
        assert!(!dir.join("patch.json.tmp").exists(), "{at:?}");
        assert!(
            !saves.join("before-load-20261009-143005.json.tmp").exists(),
            "{at:?}"
        );
        let Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
            panic!("no patch after {at:?}");
        };
        (result, logged, kept, *patch)
    }

    #[test]
    fn a_load_failing_at_each_step_leaves_what_it_says() {
        let old = a_wall();
        // Before the log: nothing logged, patch.json as it was.
        for at in [FailAt::WriteTemp, FailAt::RenameBackup, FailAt::SyncSaves] {
            let (result, logged, kept, patch) = a_load_failing_at(at);
            assert!(
                matches!(result, Err(LoadWriteError::NotLogged(_))),
                "{at:?}: {result:?}"
            );
            assert!(logged.is_empty(), "{at:?}");
            assert_eq!(patch, old, "{at:?}");
            // Renamed in but not synced: the wall as it was, a save like
            // any other that nothing names.
            assert_eq!(kept, at == FailAt::SyncSaves, "{at:?}");
        }
        // At the log: maybe logged, patch.json as it was, the backup kept.
        for at in [FailAt::AppendLog, FailAt::SyncLog] {
            let (result, logged, kept, patch) = a_load_failing_at(at);
            assert!(
                matches!(result, Err(LoadWriteError::MaybeLogged(_))),
                "{at:?}: {result:?}"
            );
            assert_eq!(
                logged,
                if at == FailAt::SyncLog {
                    vec![1]
                } else {
                    vec![]
                },
                "{at:?}"
            );
            assert_eq!(patch, old, "{at:?}");
            assert!(kept, "{at:?}");
        }
        // After the log: the load happened, and what was not written is said.
        let (result, logged, kept, patch) = a_load_failing_at(FailAt::RenamePatch);
        assert!(result.unwrap().unsaved.is_some());
        assert_eq!((logged, kept), (vec![1], true));
        assert_eq!(patch, old, "patch.json is written again by the next save");
        let (result, logged, kept, patch) = a_load_failing_at(FailAt::SyncDirs);
        assert!(result.unwrap().unsaved.is_some());
        assert_eq!((logged, kept), (vec![1], true));
        assert_eq!(patch.wall, 1);
    }

    #[test]
    fn the_log_is_opened_afresh_after_it_fails() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        store.append(&change(1)).unwrap();
        store.fail_once(FailAt::AppendLog);
        assert!(store.append(&change(2)).is_err());
        assert!(store.log_failed && store.torn);
        // The next line goes through a new handle, on a line of its own.
        store.append(&change(2)).unwrap();
        assert!(!store.log_failed && !store.torn);
        store.fail_once(FailAt::SyncLog);
        assert!(store.sync_log().is_err());
        assert!(store.log_failed);
        // A sync after a failed one is on a new handle too.
        store.sync_log().unwrap();
        assert!(!store.log_failed);
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 2]);
        assert_eq!(loaded.skipped, 0);
    }

    #[test]
    fn a_log_ending_part_way_through_a_line_is_found_on_opening() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        assert!(!store.torn, "an empty log is not torn");
        store.append(&change(1)).unwrap();
        drop(store);
        assert!(!Store::open(root.path()).unwrap().torn);
        // The daemon stopped part-way through writing a line.
        let mut log = OpenOptions::new()
            .append(true)
            .open(root.path().join(LOG_FILE))
            .unwrap();
        log.write_all(b"{\"seq\":2,\"at\"").unwrap();
        drop(log);
        let mut store = Store::open(root.path()).unwrap();
        assert!(store.torn);
        store.append(&change(3)).unwrap();
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 3]);
        assert_eq!(loaded.skipped, 1);
    }

    #[test]
    fn a_change_written_twice_is_read_once() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        // Written whole, though its sync said it failed; then again.
        for seq in [1, 2, 2, 3] {
            store.append(&change(seq)).unwrap();
        }
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 2, 3]);
        assert_eq!((loaded.skipped, loaded.repeated), (0, 1));
    }

    #[test]
    fn after_a_failed_sync_the_lines_since_the_last_good_one_are_written_again() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(LOG_FILE);
        let mut store = Store::open(root.path()).unwrap();
        store.append(&change(1)).unwrap();
        store.sync_log().unwrap();
        let synced = fs::metadata(&path).unwrap().len();
        store.append(&change(2)).unwrap();
        store.append(&change(3)).unwrap();
        store.fail_once(FailAt::SyncLog);
        assert!(store.sync_log().is_err());
        // The system dropped the pages the failed sync was for: the file
        // ends where the last good sync left it.
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(synced)
            .unwrap();
        // The next line goes after those lines, written again and synced.
        store.append(&change(4)).unwrap();
        store.sync_log().unwrap();
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4]);
        assert_eq!((loaded.skipped, loaded.repeated), (0, 0));
        // When they were not dropped after all, they are read once.
        store.append(&change(5)).unwrap();
        store.fail_once(FailAt::SyncLog);
        assert!(store.sync_log().is_err());
        store.sync_log().unwrap();
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4, 5]);
        assert_eq!(loaded.repeated, 1);
    }

    #[test]
    fn a_rotation_whose_directory_cannot_be_flushed_fails_the_log_until_it_is_whole() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        store.append(&change(1)).unwrap();
        store.log_lines = LOG_ROTATE_LINES;
        store.fail_once(FailAt::SyncRotation);
        assert!(store.append(&change(2)).is_err());
        assert!(store.log_failed, "not trusted until made whole");
        assert!(root.path().join(OLD_LOG_FILE).exists());
        // The next line makes it whole first (the line not yet flushed
        // written again, the directory flushed), and goes into the new log.
        store.append(&change(2)).unwrap();
        assert!(!store.log_failed);
        assert_eq!(count_lines(&root.path().join(LOG_FILE)).unwrap(), 2);
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 2]);
        assert_eq!(loaded.repeated, 1, "the line written again is read once");
    }

    #[test]
    fn saves_put_in_place_whose_directory_cannot_be_flushed_say_so() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let first = a_wall();
        store.write_save("keep", &first, false).unwrap();
        let mut second = a_wall();
        second.add("lfo", None, None).unwrap();
        mark_fixture_births(&mut second);
        // Saved over: the new save is in place, the flush said failed.
        store.fail_once(FailAt::SyncDir);
        let written = store.write_save("keep", &second, true).unwrap();
        assert!(written.replaced);
        assert!(written.unsynced.is_some());
        assert_eq!(*store.read_save("keep").unwrap().patch, second);
        // Deleted: gone, the flush said failed.
        store.fail_once(FailAt::SyncDir);
        let (gone, unsynced) = store.delete_save("keep").unwrap();
        assert_eq!(gone, "keep");
        assert!(unsynced.is_some());
        assert!(matches!(
            store.read_save("keep"),
            Err(SaveError::Unknown(_))
        ));
    }

    #[test]
    fn temporaries_a_crash_left_are_removed_on_opening() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let saves = dir.join(SAVES_DIR);
        fs::create_dir_all(&saves).unwrap();
        fs::write(dir.join("patch.json.tmp"), b"{").unwrap();
        fs::write(saves.join("before-load-20261009-143005.json.tmp"), b"{").unwrap();
        fs::write(saves.join(".imported.tmp"), b"half").unwrap();
        fs::write(saves.join("jam.json"), encode(&a_wall()).unwrap()).unwrap();
        let store = Store::open(dir).unwrap();
        assert!(!dir.join("patch.json.tmp").exists());
        let left: Vec<String> = fs::read_dir(&saves)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["jam.json"], "the saves themselves stay");
        assert!(store.prune_temporaries().unwrap().is_empty());
    }

    #[test]
    fn the_listing_says_what_a_load_would_find() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let saves = root.path().join(SAVES_DIR);
        fs::create_dir_all(&saves).unwrap();
        // A current file with a module this build cannot load: a load
        // leaves it (and its cable) out, and the listing counts as it.
        let mut file = serde_json::to_value(a_wall().to_file()).unwrap();
        let modules = file["modules"].as_array().unwrap().len();
        file["modules"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id": "gone1", "kind": "gone", "knobs": {}}));
        fs::write(saves.join("odd.json"), serde_json::to_vec(&file).unwrap()).unwrap();
        let read = store.read_save("odd").unwrap();
        let header = store
            .list_saves()
            .unwrap()
            .into_iter()
            .find(|listed| listed.name == "odd")
            .unwrap()
            .read
            .unwrap()
            .unwrap();
        assert_eq!(header.modules as usize, read.patch.modules().len());
        assert_eq!(header.modules as usize, modules);
        // A file a load refuses is listed as broken.
        fs::write(saves.join("bad.json"), b"[1, 2]").unwrap();
        assert!(store.read_save("bad").is_err());
        let listed = store.list_saves().unwrap();
        let bad = listed.iter().find(|listed| listed.name == "bad").unwrap();
        assert!(matches!(bad.read, Some(Err(_))), "{:?}", bad.read);
    }

    #[test]
    fn one_wall_alone_uses_a_state_directory() {
        let root = tempfile::tempdir().unwrap();
        let first = Store::open(root.path()).unwrap();
        let Err(refused) = Store::open(root.path()) else {
            panic!("a second store opened the same state");
        };
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            refused.to_string(),
            format!("another wall is using {}", root.path().display())
        );
        // Let go with the store.
        drop(first);
        Store::open(root.path()).unwrap();
    }

    #[test]
    fn a_file_too_large_to_be_a_wall_is_never_read_whole() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let saves = dir.join(SAVES_DIR);
        fs::create_dir_all(&saves).unwrap();
        let huge = vec![b' '; usize::try_from(MAX_WALL_FILE).unwrap() + 1];
        fs::write(saves.join("huge.json"), &huge).unwrap();
        let too_large = |reason: &str| reason.contains("too large to be a wall");
        // Listed as broken, refused as broken.
        let listed = store.list_saves().unwrap();
        assert!(
            matches!(&listed[0].read, Some(Err(reason)) if too_large(reason)),
            "{:?}",
            listed[0].read
        );
        assert!(matches!(
            store.read_save("huge"),
            Err(SaveError::Broken { reason, .. }) if too_large(&reason)
        ));
        // A copy that large is left out of the import.
        fs::write(dir.join("patch.huge2.json"), &huge).unwrap();
        let said = store.import_copies().unwrap();
        assert!(
            said.iter()
                .any(|line| line.starts_with("patch.huge2.json was left out") && too_large(line)),
            "{said:?}"
        );
    }

    #[test]
    fn one_listing_reads_only_so_many_saves_and_the_rest_come_by_ticks() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let total = HEADER_READS_PER_LISTING * 2 + 3;
        for n in 0..total {
            store
                .write_save(&format!("jam {n:02}"), &a_wall(), false)
                .unwrap();
            date(
                &root.path().join(SAVES_DIR).join(format!("jam {n:02}.json")),
                n as u64,
            );
        }
        // As a fresh start finds them: nothing read.
        let mut store = {
            drop(store);
            Store::open(root.path()).unwrap()
        };
        let unread = |listed: &[Listed]| listed.iter().filter(|l| l.read.is_none()).count();
        let listed = store.list_saves().unwrap();
        assert_eq!(listed.len(), total);
        assert_eq!(unread(&listed), total - HEADER_READS_PER_LISTING);
        // The newest are the ones read.
        assert!(
            listed[..HEADER_READS_PER_LISTING]
                .iter()
                .all(|l| l.read.is_some())
        );
        // Ticks read the rest, one at a time; once all are read, warming
        // stops.
        let mut ticks = 0;
        while store.warm_saves(1).unwrap() {
            ticks += 1;
        }
        // One read a tick, the rest looked at once and passed over.
        assert_eq!(ticks, total - HEADER_READS_PER_LISTING);
        assert_eq!(unread(&store.list_saves().unwrap()), 0);
        assert!(!store.warm_saves(1).unwrap());
    }

    #[test]
    fn a_saves_directory_that_will_not_read_is_said_once() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        store.write_save("jam", &a_wall(), false).unwrap();
        store.fail_once(FailAt::ListSaves);
        assert!(store.warm_saves(1).is_err());
        // The pass is over: every tick after is quiet.
        for _ in 0..100 {
            assert!(!store.warm_saves(1).unwrap());
        }
        // A listing still reads the saves.
        assert!(store.list_saves().unwrap()[0].read.is_some());
    }

    #[test]
    fn a_wall_too_large_for_its_file_is_never_written() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let mut wall = a_wall();
        // Words beyond any limit, to make the file too large.
        wall.speech.insert(
            "speak9".to_string(),
            crate::patch::Words {
                text: "a".repeat(usize::try_from(MAX_WALL_FILE).unwrap()),
                voice: None,
            },
        );
        assert!(matches!(
            store.write_save("huge", &wall, false),
            Err(SaveError::TooLarge(_))
        ));
        assert!(store.find_save("huge").unwrap().is_none());
        let refused = store.save_patch(&wall).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::FileTooLarge);
        assert!(!root.path().join(PATCH_FILE).exists());
        assert!(!root.path().join("patch.json.tmp").exists());
        // patch.json found that large (the daemon wrote it) is read, never
        // set aside.
        let mut big = serde_json::to_value(a_wall().to_file()).unwrap();
        big["padding"] = serde_json::json!("b".repeat(usize::try_from(MAX_WALL_FILE).unwrap()));
        fs::write(
            root.path().join(PATCH_FILE),
            serde_json::to_vec(&big).unwrap(),
        )
        .unwrap();
        assert!(matches!(store.load_patch().unwrap(), Loaded::Patch { .. }));
    }

    /// The catalogue facts [`WORST_MODULE_BYTES`] is a bound under.
    #[test]
    fn the_catalogue_is_within_what_the_wall_file_bound_assumes() {
        use crate::catalogue::Kind;
        assert!(Kind::all().count() <= MAX_KINDS);
        for kind in Kind::all() {
            let spec = kind.spec();
            assert!(spec.name.len() <= MAX_CATALOGUE_NAME, "{}", spec.name);
            let names = spec
                .knobs
                .iter()
                .map(|knob| knob.name)
                .chain(spec.inputs.iter().map(|port| port.name))
                .chain(spec.outputs.iter().map(|port| port.name));
            for name in names {
                assert!(name.len() <= MAX_CATALOGUE_NAME, "{}.{name}", spec.name);
            }
        }
    }

    #[test]
    fn a_load_too_large_or_with_a_log_not_ready_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        let old = a_wall();
        store.save_patch(&old).unwrap();
        let backup = encode(&old).unwrap();
        let mut huge = a_wall();
        huge.speech.insert(
            "speak9".to_string(),
            crate::patch::Words {
                text: "a".repeat(usize::try_from(MAX_WALL_FILE).unwrap()),
                voice: None,
            },
        );
        let refused = store.commit_load(Some(("before-load-x", &backup)), &huge, &a_load(1));
        assert!(
            matches!(refused, Err(LoadWriteError::TooLarge(_))),
            "{refused:?}"
        );
        // The log was due to rotate, and the rotation failed: said as that.
        store.log_lines = LOG_ROTATE_LINES;
        store.fail_once(FailAt::SyncRotation);
        let refused = store.commit_load(Some(("before-load-x", &backup)), &a_wall(), &a_load(1));
        assert!(
            matches!(refused, Err(LoadWriteError::LogNotReady(_))),
            "{refused:?}"
        );
        // Nothing of either was written.
        assert!(store.load_log().unwrap().changes.is_empty());
        assert!(store.find_save("before-load-x").unwrap().is_none());
        let Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
            panic!("no patch.json");
        };
        assert_eq!(*patch, old);
    }

    #[test]
    fn copies_a_cut_short_import_brought_in_are_said_brought_in() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let mut store = Store::open(dir).unwrap();
        fs::write(dir.join("patch.a.json"), encode(&a_wall()).unwrap()).unwrap();
        fs::write(dir.join("patch.b.json"), encode(&a_wall()).unwrap()).unwrap();
        // The first will not read; the second comes in; no mark.
        store.fail_once(FailAt::ReadCopy);
        store.import_copies().unwrap();
        // The next start: the first comes in, and the second is said to
        // have come in (its save is the copy, byte for byte); a save of
        // that name made otherwise is still said to be there already.
        let mut other = a_wall();
        other.add("lfo", None, None).unwrap();
        mark_fixture_births(&mut other);
        fs::write(dir.join("patch.c.json"), encode(&a_wall()).unwrap()).unwrap();
        store.write_save("c", &other, false).unwrap();
        let said = store.import_copies().unwrap();
        assert_eq!(
            said,
            [
                "brought patch.a.json in as the save \"a\"",
                "brought patch.b.json in as the save \"b\"",
                "patch.c.json was left out: the save \"c\" is there already",
            ]
        );
    }

    #[test]
    fn a_torn_line_stays_behind_in_the_log_rotated_away() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        store.append(&change(1)).unwrap();
        store.log.write_all(b"{\"seq\":2,\"at\"").unwrap();
        store.torn = true;
        // The log is full: the next line starts the new one.
        store.log_lines = LOG_ROTATE_LINES;
        store.append(&change(3)).unwrap();
        let fresh = fs::read(root.path().join(LOG_FILE)).unwrap();
        assert_eq!(fresh.first(), Some(&b'{'), "no blank line first");
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 3]);
        assert_eq!(loaded.skipped, 1);
    }

    #[test]
    fn the_log_reads_in_the_order_of_its_numbers() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        // A line written again after a failed sync, after a later one that
        // the system had kept.
        for seq in [1, 3, 2, 3] {
            store.append(&change(seq)).unwrap();
        }
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 2, 3]);
        assert_eq!(loaded.repeated, 1);
    }

    #[test]
    fn a_torn_log_line_never_takes_the_next_with_it() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        store.append(&change(1)).unwrap();
        // A write that failed part-way: half a line, no newline.
        store.log.write_all(b"{\"seq\":2,\"at\"").unwrap();
        store.torn = true;
        store.append(&change(3)).unwrap();
        let loaded = store.load_log().unwrap();
        let seqs: Vec<u64> = loaded.changes.iter().map(|change| change.seq).collect();
        assert_eq!(seqs, [1, 3]);
        assert_eq!(loaded.skipped, 1);
        assert!(!store.torn);
    }
}
