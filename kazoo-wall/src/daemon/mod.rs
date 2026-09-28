//! The daemon: the wall that plays forever.
//!
//! [`Daemon::start`] loads the patch (or seeds one), claims the control
//! socket and starts the control thread, which starts the audio. It runs in
//! any process: `kazoo-wall serve` runs one until it is stopped, and tests
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
pub mod record;
pub mod server;
pub mod speech;
pub mod timing;
pub mod wall;

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
        Ok(Self {
            socket: paths::socket_path(),
            state_dir: paths::state_dir()?,
            audio: AudioMode::Device { sample_rate: None },
            desk: DeskMode::Discover,
            recordings: paths::recordings_dir()?,
        })
    }

    /// A daemon for tests: socket in `runtime_dir`, state in `state_dir`
    /// and recordings in its `recordings` directory, no audio device (the
    /// headless loop at 48 kHz), no desk.
    #[must_use]
    pub fn headless(runtime_dir: &Path, state_dir: &Path) -> Self {
        Self {
            socket: runtime_dir.join(SOCKET_NAME),
            state_dir: state_dir.to_path_buf(),
            audio: AudioMode::Headless {
                sample_rate: 48_000,
            },
            desk: DeskMode::Off,
            recordings: state_dir.join("recordings"),
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
    /// Start a daemon: load the patch (seeding one if there is none, and
    /// setting aside one that cannot be read), claim the socket, and start
    /// playing. Returns once the socket is accepting connections.
    ///
    /// # Errors
    ///
    /// Fails if the state directory cannot be used, an unreadable patch
    /// cannot be set aside, another daemon is serving on the socket, or a
    /// thread cannot start.
    pub fn start(config: DaemonConfig) -> Result<Self, DaemonError> {
        let store = Store::open(&config.state_dir).map_err(|err| {
            DaemonError::new(format!(
                "the state directory {} cannot be used: {err}",
                config.state_dir.display()
            ))
        })?;
        let (patch, load_notes) = load_patch(&store)?;
        let history = store
            .load_log()
            .map_err(|err| DaemonError::new(format!("the change log cannot be read: {err}")))?;
        if history.skipped > 0 {
            eprintln!(
                "kazoo-wall: {} unreadable lines in the change log were skipped",
                history.skipped
            );
        }
        let listener = claim(&config.socket)?;
        let socket_inode = fs::metadata(&config.socket)
            .map_err(|err| DaemonError::new(format!("the socket vanished: {err}")))?
            .ino();
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
        let changes = history.changes;
        let recordings = config.recordings;
        let setup = control::Setup {
            audio: config.audio,
            desk: config.desk,
            tempo: patch.tempo,
            inbox,
            stop: Arc::clone(&stop),
            accept,
            build: Box::new(move |engine, link| {
                let mut wall = Wall::new(patch, engine, link, changes, Some(store));
                wall.set_recordings_dir(recordings);
                if let Some((from_version, notes)) = load_notes {
                    let change = wall.record_load(from_version, notes);
                    eprintln!("kazoo-wall: {}", change.summary);
                }
                wall
            }),
        };
        let control = thread::Builder::new()
            .name("kazoo-wall-control".to_string())
            .spawn(move || control::run(setup))
            .map_err(|err| DaemonError::new(format!("the control thread cannot start: {err}")))?;
        Ok(Self {
            socket: config.socket,
            socket_inode,
            stop,
            control: Some(control),
        })
    }

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

/// Load the patch, seeding one where there is none or it was set aside. A
/// patch that had to change is saved in its new form at once (its original
/// is kept beside it).
fn load_patch(store: &Store) -> Result<(Patch, LoadNotes), DaemonError> {
    let loaded = store.load_patch().map_err(|err| {
        DaemonError::new(format!(
            "the patch in {} cannot be read and cannot be set aside ({err}); \
             move it away by hand and start again",
            store.dir().display()
        ))
    })?;
    let patch = match loaded {
        Loaded::Patch {
            patch,
            from_version,
            notes,
            backup,
        } => {
            if notes.is_empty() {
                return Ok((*patch, None));
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
            store.save_patch(&patch).map_err(|err| {
                DaemonError::new(format!("the updated patch cannot be saved: {err}"))
            })?;
            return Ok((*patch, Some((from_version, notes))));
        }
        Loaded::Missing => {
            eprintln!("kazoo-wall: no patch yet; starting from the seed");
            seed()
        }
        Loaded::SetAside { reason, moved_to } => {
            eprintln!(
                "kazoo-wall: the patch could not be used ({reason}); it is now {} and the wall starts from the seed",
                moved_to.display()
            );
            seed()
        }
    };
    let patch = patch.map_err(|err| DaemonError::new(format!("the seed patch failed: {err}")))?;
    store
        .save_patch(&patch)
        .map_err(|err| DaemonError::new(format!("the seed patch cannot be saved: {err}")))?;
    Ok((patch, None))
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
/// `timeout` for its socket to go quiet.
///
/// # Errors
///
/// Fails if no daemon answers, it refuses, or it is still answering after
/// `timeout`.
pub fn stop_daemon(socket: &Path, timeout: Duration) -> Result<(), ClientError> {
    let mut wall = WallClient::connect(socket, "Tom", "kazoo-wall stop", true)?;
    wall.shutdown()?;
    drop(wall);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_running(socket) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(ClientError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        "the wall is still answering after being asked to stop",
    )))
}

/// Whether a daemon is answering on `socket`.
#[must_use]
pub fn is_running(socket: &Path) -> bool {
    matches!(discovery::hub_listening(socket), Ok(true))
}

/// Start `exe serve` as a detached daemon (its own process group, output to
/// `log`), unless one is already answering on `socket`, and wait up to
/// `timeout` for it to answer.
///
/// # Errors
///
/// Fails if the log cannot be opened, the process cannot start, or it is
/// not answering after `timeout`.
pub fn launch_detached(exe: &Path, socket: &Path, log: &Path, timeout: Duration) -> io::Result<()> {
    if is_running(socket) {
        return Ok(());
    }
    if let Some(dir) = log.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let output = OpenOptions::new().create(true).append(true).open(log)?;
    let mut child = Command::new(exe)
        .arg("serve")
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
mod tests {
    use super::*;

    #[test]
    fn the_standard_config_honours_the_overrides() {
        // Only read here: the variables are set by the integration tests'
        // child processes, never in this process.
        let config = DaemonConfig::standard();
        if let Ok(config) = config {
            assert!(config.socket.ends_with(SOCKET_NAME));
            assert_eq!(config.audio, AudioMode::Device { sample_rate: None });
        } else {
            assert!(std::env::var_os("HOME").is_none());
        }
        let headless = DaemonConfig::headless(Path::new("/r"), Path::new("/s"));
        assert_eq!(headless.state_dir, Path::new("/s"));
        assert_eq!(headless.socket, Path::new("/r").join(SOCKET_NAME));
        assert_eq!(headless.desk, DeskMode::Off);
        assert_eq!(headless.recordings, Path::new("/s").join("recordings"));
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
