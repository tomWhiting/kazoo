//! The daemon: the wall that plays forever.
//!
//! [`Daemon::start`] loads the patch (or seeds one), claims the control
//! socket and starts the control thread, which starts the audio. It runs in
//! any process: `kazoo wall serve` runs one until it is stopped, and tests
//! run one headless in a temporary directory:
//!
//! ```no_run
//! use kazoo_wall::daemon::{Daemon, DaemonConfig};
//! use kazoo_wall::protocol::client::WallClient;
//!
//! let dir = std::env::temp_dir().join("my-wall-test");
//! let daemon = Daemon::start(DaemonConfig::headless(&dir, &dir.join("state")))?;
//! let mut wall = WallClient::connect(daemon.socket(), "Tester", "a test", true)?;
//! let look = wall.look()?;
//! daemon.stop();
//! daemon.wait()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod audio;
pub mod control;
pub mod loops;
pub mod record;
pub mod server;
pub mod speech;
pub mod timing;
pub mod wall;

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kazoo_core::ipc::discovery;

pub use audio::AudioMode;

use crate::patch::Patch;
use crate::paths;
use crate::protocol::SOCKET_NAME;
use crate::protocol::client::{ClientError, WallClient};
use crate::seed::seed;
use crate::store::{Loaded, Store};
use wall::Wall;

/// Whether and how the wall joins the kazoo-mix desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskMode {
    /// Never.
    Off,
    /// The desk kazoo-mix advertises, whenever it is up.
    Discover,
    /// A desk on this socket.
    Socket(PathBuf),
}

/// How a daemon runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// The control socket.
    pub socket: PathBuf,
    /// Where the patch and the change log live.
    pub state_dir: PathBuf,
    /// Where the sound goes.
    pub audio: AudioMode,
    /// Whether to join the desk.
    pub desk: DeskMode,
    /// Where recordings go.
    pub recordings: PathBuf,
    /// The tree to attach, selected explicitly for headless tests.
    pub tree_dir: Option<PathBuf>,
    /// Whether the daemon plants the tree at `tree_dir` as it starts when
    /// there is none there (see `plant_tree`). On for every daemon that plays for
    /// people; off for test walls, which are made without one.
    pub plant_tree: bool,
    /// The reel store for tests; standard daemons use the configured reel root.
    pub reels_dir: Option<PathBuf>,
    /// Whether the wall refuses more work while rendering takes most of the
    /// time it has (see [`wall::LOAD_GUARD`]). On for every daemon that
    /// plays for people; a test daemon in a debug build, which renders far
    /// slower than real time, turns it off.
    pub load_guard: bool,
}

impl DaemonConfig {
    /// The standard daemon: socket, state and recordings where [`paths`]
    /// says (honouring their environment overrides), the default audio
    /// device, and the desk whenever it is up.
    ///
    /// # Errors
    ///
    /// Fails if there is no state or recordings directory to use (see
    /// [`paths::state_dir`] and [`paths::recordings_dir`]).
    pub fn standard() -> io::Result<Self> {
        let state_dir = paths::state_dir()?;
        let tree_dir = crate::tree::git::tree_dir(&state_dir);
        Ok(Self {
            socket: paths::socket_path(),
            state_dir,
            tree_dir,
            plant_tree: true,
            reels_dir: None,
            audio: AudioMode::Device { sample_rate: None },
            desk: DeskMode::Discover,
            recordings: paths::recordings_dir()?,
            load_guard: true,
        })
    }

    /// A daemon for tests: socket in `runtime_dir`, state in `state_dir`
    /// with recordings, tree and reels beneath it, no audio device (the
    /// headless loop at 48 kHz), no desk, and the load guard on.
    #[must_use]
    pub fn headless(runtime_dir: &Path, state_dir: &Path) -> Self {
        Self {
            socket: runtime_dir.join(SOCKET_NAME),
            state_dir: state_dir.to_path_buf(),
            tree_dir: Some(state_dir.join("tree")),
            plant_tree: false,
            reels_dir: Some(state_dir.join("reels")),
            audio: AudioMode::Headless {
                sample_rate: 48_000,
            },
            desk: DeskMode::Off,
            recordings: state_dir.join("recordings"),
            load_guard: true,
        }
    }
}

/// Why the daemon could not start or stopped badly.
#[derive(Debug)]
pub struct DaemonError(String);

impl DaemonError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DaemonError {}

/// A running daemon.
#[derive(Debug)]
pub struct Daemon {
    socket: PathBuf,
    socket_inode: u64,
    stop: Arc<AtomicBool>,
    control: Option<JoinHandle<()>>,
}

impl Daemon {
    /// Start a daemon: claim the socket, plant the tree when `plant_tree`
    /// is set and there is none (growing it from a long change log takes
    /// a while, once, and connections wait meanwhile), load the patch
    /// (seeding one if there is none, and setting aside one that cannot
    /// be read), and start playing. Returns once the socket is accepting
    /// connections.
    ///
    /// # Errors
    ///
    /// Fails if the state directory cannot be used, an unreadable patch
    /// cannot be set aside, another daemon is serving on the socket, or a
    /// thread cannot start.
    pub fn start(config: DaemonConfig) -> Result<Self, DaemonError> {
        let tree_dir = config.tree_dir.clone();
        Self::start_with_tree(config, tree_dir)
    }

    pub(crate) fn start_with_tree(
        config: DaemonConfig,
        tree_dir: Option<PathBuf>,
    ) -> Result<Self, DaemonError> {
        // The socket first, so a daemon already serving it is found before
        // any state is touched. (One wall per state directory is the
        // store's lock: see `Store::open`.)
        let listener = claim(&config.socket)?;
        let socket_inode = fs::metadata(&config.socket)
            .map_err(|err| DaemonError::new(format!("the socket vanished: {err}")))?
            .ino();
        // Whatever fails from here lets the socket go again, as the daemon
        // never served it.
        let mut claimed = Claimed {
            socket: &config.socket,
            inode: socket_inode,
            kept: false,
        };
        // Before the state is opened: growing the tree reads the log under
        // the store's lock, which the wall then holds while it plays.
        let planting = if config.plant_tree {
            tree_dir
                .as_deref()
                .map(|dir| plant_tree(&config.state_dir, dir))
        } else {
            None
        };
        let Prepared {
            store,
            patch,
            changes,
            too_large,
        } = prepare(&config)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (inbox_tx, inbox) = std::sync::mpsc::channel();
        let accept = server::spawn_accept(listener, inbox_tx, Arc::clone(&stop))
            .map_err(|err| DaemonError::new(format!("the socket cannot be served: {err}")))?;
        eprintln!(
            "kazoo-wall: {} modules and {} cables, listening on {}",
            patch.modules().len(),
            patch.cables().len(),
            config.socket.display()
        );
        let recordings = config.recordings;
        let reels_dir = config.reels_dir;
        let load_guard = config.load_guard;
        let setup = control::Setup {
            audio: config.audio,
            desk: config.desk,
            tempo: patch.tempo,
            inbox,
            stop: Arc::clone(&stop),
            accept,
            build: Box::new(move |engine, link| {
                let mut wall =
                    Wall::new_with_reels(patch, engine, link, changes, Some(store), reels_dir);
                match (&tree_dir, planting) {
                    (Some(_), Some(Planting::Failed(why))) => wall.tree_unplanted(why),
                    (Some(dir), _) => wall.find_tree(dir),
                    (None, _) => {}
                }
                wall.set_recordings_dir(recordings);
                wall.set_load_guard(load_guard);
                if let Some(why) = &too_large {
                    wall.starts_too_large(why);
                }
                wall
            }),
        };
        let control = thread::Builder::new()
            .name("kazoo-wall-control".to_string())
            .spawn(move || control::run(setup))
            .map_err(|err| {
                // The accept thread stops with the daemon that never was.
                stop.store(true, Ordering::Release);
                DaemonError::new(format!("the control thread cannot start: {err}"))
            })?;
        // Served: the daemon lets it go when it stops.
        claimed.kept = true;
        Ok(Self {
            socket: config.socket.clone(),
            socket_inode,
            stop,
            control: Some(control),
        })
    }
}

/// What a starting wall did about its tree (see [`plant_tree`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Planting {
    /// A tree with a trunk was there already.
    Kept,
    /// The tree was grown from the change log.
    Grown,
    /// An empty tree was planted, to grow from this start.
    Empty,
    /// Another wall or an import held the state, so nothing was done; a
    /// tree that is there is attached as ever.
    Left,
    /// There is no tree, for this reason; the wall plays without one.
    Failed(String),
}

/// Plant the wall's tree at `dir` when it has none, so nobody has to:
/// grown from the change log in `state` as `kazoo wall tree import` grows
/// it, or planted empty when the log gives no wall to grow from (a new
/// wall, or one whose past cannot be worked out), to grow from this start.
/// A tree that is there with a trunk is left as it is. Whatever happens is
/// said in the daemon's log; a failure leaves nothing half planted.
fn plant_tree(state: &Path, dir: &Path) -> Planting {
    let planting = match grow_tree(state, dir) {
        Ok(planting) => planting,
        Err(why) => Planting::Failed(why),
    };
    match &planting {
        Planting::Kept | Planting::Grown => {}
        Planting::Left => eprintln!(
            "kazoo-wall: another wall or a tree import is using the wall's state, so the tree is left as it is"
        ),
        Planting::Empty => eprintln!(
            "kazoo-wall: the tree is planted at {}, and grows from this start",
            dir.display()
        ),
        Planting::Failed(why) => {
            eprintln!("kazoo-wall: there is no tree: {why}; the wall plays without it");
        }
    }
    planting
}

fn grow_tree(state: &Path, dir: &Path) -> Result<Planting, String> {
    let unreadable =
        |err: &dyn fmt::Display| format!("the tree at {} cannot be read ({err})", dir.display());
    if crate::tree::git::is_bare(dir).map_err(|err| unreadable(&err))?
        && crate::tree::rings::reference(dir, "refs/heads/trunk")
            .map_err(|err| unreadable(&err))?
            .is_some()
    {
        return Ok(Planting::Kept);
    }
    // What was there: a failure may take away only what it left.
    let before = match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                Before::Empty
            } else {
                Before::Occupied
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Before::Absent,
        Err(err) => return Err(unreadable(&err)),
    };
    if let Some(parent) = dir.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|err| format!("{} cannot be made ({err})", parent.display()))?;
    }
    if !state.is_dir() {
        return plant_empty(dir);
    }
    eprintln!(
        "kazoo-wall: growing the tree at {} from the change log (once; the wall answers when it is grown)",
        dir.display()
    );
    match crate::tree::rings::grow(state, dir, false) {
        Ok(rings) => {
            for line in rings.report.lines() {
                eprintln!("kazoo-wall: tree: {line}");
            }
            Ok(Planting::Grown)
        }
        Err(crate::tree::rings::RingsError::EmptyLog) => plant_empty(dir),
        Err(crate::tree::rings::RingsError::WallRunning) => Ok(Planting::Left),
        Err(crate::tree::rings::RingsError::NoWall(why)) => {
            clear(dir, before)?;
            eprintln!("kazoo-wall: the changes already in the log cannot go into the tree ({why})");
            plant_empty(dir)
        }
        Err(err) => Err(failed(dir, before, &err)),
    }
}

/// An empty tree at `dir`, unless one is planted there already.
fn plant_empty(dir: &Path) -> Result<Planting, String> {
    if !crate::tree::git::is_bare(dir)
        .map_err(|err| format!("the tree at {} cannot be read ({err})", dir.display()))?
    {
        crate::tree::git::plant(dir)
            .map_err(|err| format!("the tree cannot be planted at {} ({err})", dir.display()))?;
    }
    Ok(Planting::Empty)
}

/// What was at the tree's place before the wall tried to grow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Before {
    /// Nothing: whatever is there now, a failure left.
    Absent,
    /// An empty directory (made by someone, perhaps a mount point): it
    /// stays, and only what is in it now goes.
    Empty,
    /// Something: it is never touched.
    Occupied,
}

/// Why growing failed, with what it left at `dir` taken away.
fn failed(dir: &Path, before: Before, err: &crate::tree::rings::RingsError) -> String {
    let why = format!("the tree could not be grown from the change log ({err})");
    match clear(dir, before) {
        Ok(()) => why,
        Err(also) => format!("{why}; {also}"),
    }
}

/// Take away what a failed grow left at `dir`, as `before` allows.
fn clear(dir: &Path, before: Before) -> Result<(), String> {
    let cannot = |err: io::Error| {
        format!(
            "what it left at {} cannot be taken away ({err})",
            dir.display()
        )
    };
    match before {
        Before::Occupied => Ok(()),
        Before::Absent => match fs::remove_dir_all(dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(cannot(err)),
        },
        Before::Empty => {
            for entry in fs::read_dir(dir).map_err(cannot)? {
                let path = entry.map_err(cannot)?.path();
                let removed = if path.is_dir() && !path.is_symlink() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                removed.map_err(cannot)?;
            }
            Ok(())
        }
    }
}

/// A socket claimed by a daemon starting: removed again (if it is still
/// the one it bound) unless the daemon started and keeps it.
struct Claimed<'a> {
    socket: &'a Path,
    inode: u64,
    kept: bool,
}

impl Drop for Claimed<'_> {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        if fs::metadata(self.socket).is_ok_and(|meta| meta.ino() == self.inode) {
            if let Err(gone) = fs::remove_file(self.socket) {
                eprintln!("kazoo-wall: the socket could not be removed: {gone}");
            }
        }
    }
}

/// How long a starting daemon waits for a wall that is stopping to let go
/// of the state directory.
const STATE_WAIT: Duration = Duration::from_secs(5);

/// Open the state directory `dir` (see [`Store::open`]), waiting up to
/// [`STATE_WAIT`] while another wall holds it: one asked to stop a moment
/// ago is still saving, and lets go when it has.
fn open_state(dir: &Path) -> Result<Store, DaemonError> {
    let deadline = Instant::now() + STATE_WAIT;
    loop {
        match Store::open(dir) {
            Ok(store) => return Ok(store),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                return Err(DaemonError::new(format!(
                    "the state directory {} cannot be used: {err}",
                    dir.display()
                )));
            }
        }
    }
}

/// Open the state, bring in the hand-made copies, load the patch and the
/// log, and say in the log what the wall starts on (see
/// [`wall::resume`]): the store, the patch and the changes to start with.
fn prepare(config: &DaemonConfig) -> Result<Prepared, DaemonError> {
    let mut store = open_state(&config.state_dir)?;
    // The hand-made copies of the patch become saves, once. A failure
    // here costs nothing that was there: the copies are left as they
    // are, and tried again on the next start.
    match store.import_copies() {
        Ok(said) => {
            for line in said {
                eprintln!("kazoo-wall: saves: {line}");
            }
        }
        Err(err) => eprintln!(
            "kazoo-wall: the hand-made copies of the patch could not all be brought in as \
                 saves ({err}); they are left as they are, and tried again next time"
        ),
    }
    let Found {
        mut patch,
        migrated,
        seeded,
        set_aside,
    } = load_patch(&store)?;
    let history = store
        .load_log()
        .map_err(|err| DaemonError::new(format!("the change log cannot be read: {err}")))?;
    if history.skipped > 0 {
        eprintln!(
            "kazoo-wall: {} unreadable lines in the change log were skipped",
            history.skipped
        );
    }
    if history.repeated > 0 {
        eprintln!(
            "kazoo-wall: {} lines in the change log repeated a change before them and were \
                 skipped",
            history.repeated
        );
    }
    let mut changes = history.changes;
    // Whatever the wall starts on that the log does not account for is
    // said in the log before anything plays.
    let too_large = wall::resume(
        &mut store,
        &mut patch,
        &mut changes,
        seeded,
        set_aside,
        migrated,
    )
    .map_err(|err| {
        DaemonError::new(format!(
            "the wall cannot say in its change log what it starts on ({err}), so it does not \
                 start: free some space in {} or make it writable, and start again",
            store.dir().display()
        ))
    })?;
    Ok(Prepared {
        store,
        patch,
        changes,
        too_large,
    })
}

/// What a start has ready for the wall (see [`prepare`]).
struct Prepared {
    store: Store,
    patch: Patch,
    changes: Vec<crate::protocol::Change>,
    /// Why the wall is too large for its file, when it is.
    too_large: Option<String>,
}

impl Daemon {
    /// The control socket.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The flag that stops the daemon, for signal handlers.
    #[must_use]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Ask the daemon to stop; [`Self::wait`] waits for it.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Wait until the daemon has stopped (by [`Self::stop`], a console's
    /// `shutdown`, or a signal handler setting [`Self::stop_flag`]): the
    /// patch is saved and the socket removed.
    ///
    /// # Errors
    ///
    /// Fails if the control thread panicked.
    pub fn wait(mut self) -> Result<(), DaemonError> {
        self.join()
    }

    fn join(&mut self) -> Result<(), DaemonError> {
        let Some(control) = self.control.take() else {
            return Ok(());
        };
        let joined = control.join();
        self.remove_socket();
        joined.map_err(|_| DaemonError::new("the control thread panicked"))
    }

    /// Remove the socket file, if it is still the one this daemon bound.
    fn remove_socket(&self) {
        let meta = match fs::metadata(&self.socket) {
            Ok(meta) => meta,
            Err(err) => {
                // Gone already is fine: nothing to remove.
                if err.kind() != io::ErrorKind::NotFound {
                    eprintln!("kazoo-wall: the socket could not be checked: {err}");
                }
                return;
            }
        };
        if meta.ino() != self.socket_inode {
            eprintln!("kazoo-wall: another daemon owns the socket now; leaving it");
        } else if let Err(err) = fs::remove_file(&self.socket) {
            eprintln!("kazoo-wall: the socket could not be removed: {err}");
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
        if let Err(err) = self.join() {
            eprintln!("kazoo-wall: {err}");
        }
    }
}

/// What loading the patch took: the file's version and the notes, when it
/// had to be brought up to date or had parts left out.
type LoadNotes = Option<(u32, Vec<String>)>;

/// The patch a start found, and what became of `patch.json` on the way.
struct Found {
    patch: Patch,
    /// The file's version and notes, when it was brought up to date.
    migrated: LoadNotes,
    /// Whether the patch is the seed.
    seeded: bool,
    /// Why `patch.json` could not be used, and the file it was moved to,
    /// when it was set aside.
    set_aside: Option<(String, String)>,
}

/// Load the patch, seeding one where there is none or it was set aside.
/// Nothing is written here: a patch that had to change (its original
/// kept beside it) or was seeded is written by [`wall::resume`], after
/// the notes saying so.
fn load_patch(store: &Store) -> Result<Found, DaemonError> {
    let loaded = store.load_patch().map_err(|err| {
        DaemonError::new(format!(
            "the patch in {} cannot be read and cannot be set aside ({err}); \
             move it away by hand and start again",
            store.dir().display()
        ))
    })?;
    let set_aside = match loaded {
        Loaded::Patch {
            patch,
            from_version,
            notes,
            backup,
        } => {
            if notes.is_empty() {
                return Ok(Found {
                    patch: *patch,
                    migrated: None,
                    seeded: false,
                    set_aside: None,
                });
            }
            for note in &notes {
                eprintln!("kazoo-wall: loading the patch: {note}");
            }
            if let Some(backup) = backup {
                eprintln!(
                    "kazoo-wall: the patch as it was is kept in {}",
                    backup.display()
                );
            }
            // Written by `wall::resume`, after the note saying so.
            return Ok(Found {
                patch: *patch,
                migrated: Some((from_version, notes)),
                seeded: false,
                set_aside: None,
            });
        }
        Loaded::Missing => {
            eprintln!("kazoo-wall: no patch yet; starting from the seed");
            None
        }
        // Said in the log as the wall's own change (by `wall::resume`),
        // where the console and every seat see it.
        Loaded::SetAside { reason, moved_to } => {
            let file = moved_to.file_name().map_or_else(
                || moved_to.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            Some((reason, file))
        }
    };
    let patch = seed().map_err(|err| DaemonError::new(format!("the seed patch failed: {err}")))?;
    // Written by `wall::resume`, after any note saying so.
    Ok(Found {
        patch,
        migrated: None,
        seeded: true,
        set_aside,
    })
}

/// Make sure `dir` is a private directory of ours.
///
/// It is created (mode 0700) if missing, refused if it is a symlink or
/// someone else owns it, and closed to everyone else if it is open. The
/// socket's directory is the wall's access control, and kazoo's runtime
/// directory is a shared path under `/tmp`, so this is checked every time.
///
/// # Errors
///
/// Fails if the directory cannot be created, is a symlink, belongs to
/// another user, or cannot be made private.
pub fn secure_dir(dir: &Path) -> Result<(), DaemonError> {
    let failed =
        |what: &str, err: &io::Error| DaemonError::new(format!("{} {what}: {err}", dir.display()));
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|err| failed("cannot be created", &err))?;
    let meta = fs::symlink_metadata(dir).map_err(|err| failed("cannot be checked", &err))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(DaemonError::new(format!(
            "{} is not a plain directory (a symlink?); refusing to put the wall's socket there",
            dir.display()
        )));
    }
    // Whoever creates a file here is this process's user: compare owners.
    let probe = dir.join(format!(".kazoo-wall-probe-{}", std::process::id()));
    let mine = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|file| file.metadata())
        .map_err(|err| failed("cannot be written", &err))?
        .uid();
    fs::remove_file(&probe).map_err(|err| failed("cannot be tidied", &err))?;
    if meta.uid() != mine {
        return Err(DaemonError::new(format!(
            "{} belongs to another user (uid {}); refusing to put the wall's socket there",
            dir.display(),
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        eprintln!(
            "kazoo-wall: {} was open to others (mode {:o}); making it private",
            dir.display(),
            meta.mode() & 0o777
        );
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|err| failed("cannot be made private", &err))?;
    }
    Ok(())
}

/// Claim the control socket in a private directory.
fn claim(socket: &Path) -> Result<std::os::unix::net::UnixListener, DaemonError> {
    if let Some(dir) = socket.parent() {
        secure_dir(dir)?;
    }
    let listener = discovery::claim_socket(socket).map_err(|err| {
        if err.kind() == io::ErrorKind::AddrInUse {
            DaemonError::new(format!("a wall is already playing on {}", socket.display()))
        } else {
            DaemonError::new(format!("{} cannot be claimed: {err}", socket.display()))
        }
    })?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).map_err(|err| {
        DaemonError::new(format!(
            "{} cannot be made private: {err}",
            socket.display()
        ))
    })?;
    Ok(listener)
}

/// Run a daemon until SIGINT, SIGTERM or a console's `shutdown`.
///
/// # Errors
///
/// As [`Daemon::start`] and [`Daemon::wait`], or if the signal handlers
/// cannot be installed.
pub fn serve(config: DaemonConfig) -> Result<(), DaemonError> {
    let daemon = Daemon::start(config)?;
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, daemon.stop_flag()).map_err(|err| {
            DaemonError::new(format!("the stop signal {signal} cannot be caught: {err}"))
        })?;
    }
    daemon.wait()
}

/// Ask the daemon on `socket` to stop, as a console, and wait up to
/// `timeout` for it to have stopped.
///
/// Stopped is its socket file gone, which the daemon removes only once it
/// has saved and let go of its state, so a wall started right after finds
/// the state free.
///
/// # Errors
///
/// Fails if no daemon answers, it refuses, or its socket is still there
/// after `timeout`.
pub fn stop_daemon(socket: &Path, timeout: Duration) -> Result<(), ClientError> {
    let mut wall = WallClient::connect(socket, "Tom", "kazoo-wall stop", true)?;
    wall.shutdown()?;
    drop(wall);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match fs::symlink_metadata(socket) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(ClientError::Io(err)),
            Ok(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    Err(ClientError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "the wall was asked to stop, but after {} s it has not finished (its socket {} is \
             still there): it may still be saving; try again in a moment",
            timeout.as_secs(),
            socket.display()
        ),
    )))
}

/// Whether a daemon is answering on `socket`.
#[must_use]
pub fn is_running(socket: &Path) -> bool {
    matches!(discovery::hub_listening(socket), Ok(true))
}

/// The executable and complete argument prefix used to start a wall daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeCommand {
    /// The program to spawn, independent of its file name.
    pub program: PathBuf,
    /// Arguments in order, including the command and any serve flags.
    pub args: Vec<OsString>,
}

/// Start `serve` as a detached daemon (its own process group, output to
/// `log`), unless one is already answering on `socket`, and wait up to
/// `timeout` for it to answer.
///
/// # Errors
///
/// Fails if the log cannot be opened, the process cannot start, or it is
/// not answering after `timeout`.
pub fn launch_detached(
    serve: &ServeCommand,
    socket: &Path,
    log: &Path,
    timeout: Duration,
) -> io::Result<()> {
    if is_running(socket) {
        return Ok(());
    }
    if let Some(dir) = log.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let output = OpenOptions::new().create(true).append(true).open(log)?;
    let mut child = Command::new(&serve.program)
        .args(&serve.args)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .process_group(0)
        .spawn()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_running(socket) {
            // Reap the daemon when it eventually exits, so it never lingers
            // as a zombie of this process.
            thread::Builder::new()
                .name("kazoo-wall-reaper".to_string())
                .spawn(move || {
                    if let Err(err) = child.wait() {
                        eprintln!("kazoo-wall: waiting for the daemon failed: {err}");
                    }
                })?;
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "the daemon exited ({status}); see {}",
                log.display()
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("the daemon did not answer in time; see {}", log.display()),
    ))
}

#[cfg(test)]
mod plant_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::What;
    use crate::store::{FailAt, fail_on_open};

    /// The changes in the log in `state`.
    fn logged(state: &Path) -> Vec<crate::protocol::Change> {
        Store::open(state).unwrap().load_log().unwrap().changes
    }

    /// Start a daemon on `state` with the step `at` failing once, as a
    /// full disk would make it fail; returns why it would not start.
    fn failed_start(runtime: &Path, state: &Path, at: FailAt) -> String {
        fail_on_open(at);
        let Err(err) = Daemon::start(DaemonConfig::headless(runtime, state)) else {
            panic!("it started with {at:?} failing");
        };
        err.to_string()
    }

    #[test]
    fn a_start_that_cannot_log_what_it_did_to_the_patch_does_it_again_next_time() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        fs::create_dir_all(&state).unwrap();
        // A patch with a part that cannot load: starting brings it up to
        // date, and says so in the log.
        let mut file = serde_json::to_value(Patch::empty(100.0).to_file()).unwrap();
        file["modules"] = serde_json::json!([
            {"id": "vco1", "kind": "vco", "knobs": {"tune": 3.0}},
            {"id": "gone1", "kind": "gone", "knobs": {}}
        ]);
        let original = serde_json::to_vec(&file).unwrap();
        fs::write(state.join("patch.json"), &original).unwrap();
        for at in [FailAt::AppendLog, FailAt::SyncLog, FailAt::WritePatch] {
            let why = failed_start(root.path(), &state, at);
            assert!(
                why.contains("cannot say in its change log"),
                "{at:?}: {why}"
            );
            // patch.json as it was: the next start reads it the same way.
            assert_eq!(
                fs::read(state.join("patch.json")).unwrap(),
                original,
                "{at:?}"
            );
        }
        let daemon = Daemon::start(DaemonConfig::headless(root.path(), &state)).unwrap();
        daemon.stop();
        daemon.wait().unwrap();
        // Said in the log (each start that got that far said it too), and
        // patch.json brought up to date after it.
        let migrated = logged(&state)
            .into_iter()
            .filter(|change| matches!(change.what, What::Migrate { .. }))
            .count();
        assert!(migrated >= 1);
        let Loaded::Patch { patch, notes, .. } = Store::open(&state).unwrap().load_patch().unwrap()
        else {
            panic!("no patch.json");
        };
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(patch.modules().len(), 1);
    }

    #[test]
    fn a_seeded_start_that_cannot_log_it_leaves_no_patch_behind() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        // A log from a wall whose patch.json is gone.
        let mut store = Store::open(&state).unwrap();
        store
            .append(&crate::protocol::Change {
                notes: Vec::new(),
                seq: 1,
                at: "2026-10-09T00:00:00Z".to_string(),
                seat: "Tom".to_string(),
                summary: "Tom turned vcf1".to_string(),
                what: What::Tempo {
                    from: 120.0,
                    to: 100.0,
                    desk: false,
                    after_beats: 0.0,
                    glide_beats: 0.0,
                },
                undoes: None,
                clock: None,
                comment: None,
            })
            .unwrap();
        store.sync_log().unwrap();
        drop(store);
        failed_start(root.path(), &state, FailAt::AppendLog);
        assert!(
            !state.join("patch.json").exists(),
            "a seed is never on disk without the note saying it was seeded"
        );
        let daemon = Daemon::start(DaemonConfig::headless(root.path(), &state)).unwrap();
        daemon.stop();
        daemon.wait().unwrap();
        // Started on a wall the log does not know: nothing before holds.
        let notes: Vec<What> = logged(&state)
            .into_iter()
            .map(|change| change.what)
            .filter(|what| matches!(what, What::Resumed { .. }))
            .collect();
        assert_eq!(notes.len(), 1);
        assert!(matches!(notes[0], What::Resumed { from: None, .. }));
        assert!(state.join("patch.json").exists());
    }

    #[test]
    fn the_standard_config_honours_the_overrides() {
        // Only read here: the variables are set by the integration tests'
        // child processes, never in this process.
        let config = DaemonConfig::standard();
        if let Ok(config) = config {
            assert!(config.socket.ends_with(SOCKET_NAME));
            assert_eq!(config.audio, AudioMode::Device { sample_rate: None });
            assert!(config.load_guard, "a daemon that plays for people guards");
        } else {
            assert!(std::env::var_os("HOME").is_none());
        }
        let headless = DaemonConfig::headless(Path::new("/r"), Path::new("/s"));
        assert_eq!(headless.state_dir, Path::new("/s"));
        assert_eq!(headless.socket, Path::new("/r").join(SOCKET_NAME));
        assert_eq!(headless.desk, DeskMode::Off);
        assert_eq!(headless.recordings, Path::new("/s").join("recordings"));
        assert_eq!(headless.tree_dir, Some(Path::new("/s").join("tree")));
        assert_eq!(headless.reels_dir, Some(Path::new("/s").join("reels")));
        assert!(headless.load_guard);
    }

    #[test]
    fn the_socket_directory_is_made_private_and_symlinks_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let open = root.path().join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        secure_dir(&open).unwrap();
        assert_eq!(fs::metadata(&open).unwrap().mode() & 0o777, 0o700);
        let fresh = root.path().join("a").join("b");
        secure_dir(&fresh).unwrap();
        assert_eq!(fs::metadata(&fresh).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::read_dir(&fresh).unwrap().count(), 0);
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        let refused = secure_dir(&link).unwrap_err();
        assert!(refused.to_string().contains("symlink"), "{refused}");
    }
}
