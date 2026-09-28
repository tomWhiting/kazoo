//! Hub discovery: where the hub's socket is, and claiming it safely.
//!
//! The hub writes a PID file naming its socket and process. Instruments read
//! it to find the socket, then simply connect: a hub that is not running
//! refuses the connection, so connecting is the liveness check. No process
//! is spawned and nothing is deleted on the instrument side.
//!
//! A starting hub claims the socket with [`claim_socket`]: it binds, and only
//! if the path is taken does it probe it. A socket something is listening on
//! belongs to a running hub and is left alone; a socket nobody answers is
//! left over from a hub that died, and is replaced.

use std::fs;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Default socket filename.
const SOCKET_NAME: &str = "hub.sock";

/// PID filename.
const PID_NAME: &str = "hub.pid";

/// Subdirectory under the runtime directory.
const KAZOO_DIR: &str = "kazoo";

/// Numbers each PID file write's temporary file, so two hubs starting in
/// one process cannot rename each other's file away.
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Return the runtime directory for kazoo state files.
///
/// Prefers `$XDG_RUNTIME_DIR/kazoo/` if set, otherwise `/tmp/kazoo/`.
#[must_use]
pub fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR").map_or_else(
        |_| PathBuf::from("/tmp").join(KAZOO_DIR),
        |xdg| PathBuf::from(xdg).join(KAZOO_DIR),
    )
}

/// Return the default socket path for the hub.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    runtime_dir().join(SOCKET_NAME)
}

/// Return the PID file path.
#[must_use]
pub fn pid_file_path() -> PathBuf {
    runtime_dir().join(PID_NAME)
}

// ---------------------------------------------------------------------------
// PID file
// ---------------------------------------------------------------------------

/// What a hub's PID file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubRecord {
    /// The hub's socket.
    pub socket: PathBuf,
    /// The hub's process id.
    pub pid: u32,
}

/// Write the PID file for a hub serving on `socket_path`, as this process.
///
/// The file is written to a temporary name and renamed into place, so a
/// reader never sees a half-written file.
///
/// # Errors
///
/// Fails if the runtime directory or file cannot be written.
pub fn write_pid_file(socket_path: &Path) -> io::Result<()> {
    let dir = runtime_dir();
    fs::create_dir_all(&dir)?;
    let contents = format!("{}\n{}\n", socket_path.display(), std::process::id());
    let temporary = dir.join(format!(
        "{PID_NAME}.{}.{}",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, pid_file_path())
}

/// Read the hub's PID file: `Ok(None)` if there is none.
///
/// # Errors
///
/// Fails if the file exists but cannot be read, or is malformed.
pub fn read_pid_file() -> io::Result<Option<HubRecord>> {
    let contents = match fs::read_to_string(pid_file_path()) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    parse_pid_file(&contents).map(Some)
}

fn parse_pid_file(contents: &str) -> io::Result<HubRecord> {
    let malformed = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("malformed hub PID file {}", pid_file_path().display()),
        )
    };
    let mut lines = contents.lines();
    let socket = lines
        .next()
        .filter(|line| !line.is_empty())
        .ok_or_else(malformed)?;
    let pid = lines
        .next()
        .ok_or_else(malformed)?
        .trim()
        .parse::<u32>()
        .map_err(|_| malformed())?;
    Ok(HubRecord {
        socket: PathBuf::from(socket),
        pid,
    })
}

/// Remove the PID file if it still names this process; another hub's file
/// is left alone. Returns whether a file was removed.
///
/// # Errors
///
/// Fails if the file cannot be read or removed.
pub fn remove_own_pid_file() -> io::Result<bool> {
    match read_pid_file()? {
        Some(record) if record.pid == std::process::id() => {
            fs::remove_file(pid_file_path())?;
            Ok(true)
        }
        Some(_) | None => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// Finding and claiming the hub socket
// ---------------------------------------------------------------------------

/// The socket an instrument should connect to: the one the hub advertises in
/// its PID file, or the default path when there is no PID file.
///
/// # Errors
///
/// Fails if the PID file exists but cannot be read or is malformed.
pub fn hub_socket() -> io::Result<PathBuf> {
    Ok(read_pid_file()?.map_or_else(default_socket_path, |record| record.socket))
}

/// Whether something is accepting connections on `socket`.
///
/// # Errors
///
/// Fails for errors other than the socket being absent or refusing, such as
/// a permissions problem.
pub fn hub_listening(socket: &Path) -> io::Result<bool> {
    match UnixStream::connect(socket) {
        Ok(_) => Ok(true),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

/// Bind `socket` for a new hub.
///
/// A socket that a running hub is listening on is never taken: that fails
/// with [`io::ErrorKind::AddrInUse`]. A leftover socket file that nothing
/// answers is removed and bound afresh.
///
/// # Errors
///
/// [`io::ErrorKind::AddrInUse`] if a hub is already serving there, or the
/// error from creating the directory, probing, removing or binding.
pub fn claim_socket(socket: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)?;
    }
    match UnixListener::bind(socket) {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            if hub_listening(socket)? {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("a kazoo hub is already serving on {}", socket.display()),
                ));
            }
            fs::remove_file(socket)?;
            UnixListener::bind(socket)
        }
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kzd-{name}-{}.sock", std::process::id()))
    }

    #[test]
    fn default_socket_path_contains_kazoo() {
        let path = default_socket_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("kazoo"), "{path_str}");
        assert!(path_str.ends_with("hub.sock"), "{path_str}");
    }

    #[test]
    fn pid_file_path_contains_kazoo() {
        let path = pid_file_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("kazoo"));
        assert!(path_str.ends_with("hub.pid"));
    }

    #[test]
    fn runtime_dir_is_absolute() {
        assert!(runtime_dir().is_absolute());
    }

    #[test]
    fn pid_files_parse_and_malformed_ones_are_errors() {
        assert_eq!(
            parse_pid_file("/tmp/kazoo/hub.sock\n4242\n").unwrap(),
            HubRecord {
                socket: PathBuf::from("/tmp/kazoo/hub.sock"),
                pid: 4242
            }
        );
        assert!(parse_pid_file("").is_err());
        assert!(parse_pid_file("/tmp/kazoo/hub.sock\nnot-a-pid\n").is_err());
        assert!(parse_pid_file("/tmp/kazoo/hub.sock\n").is_err());
    }

    #[test]
    fn a_live_socket_is_never_claimed_but_a_stale_one_is() {
        let path = temp_socket("claim");
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        let first = claim_socket(&path).unwrap();
        assert!(hub_listening(&path).unwrap());
        let refused = claim_socket(&path).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AddrInUse);

        // The first hub dies without cleaning up: its file stays behind.
        drop(first);
        assert!(path.exists());
        assert!(!hub_listening(&path).unwrap());
        let second = claim_socket(&path).unwrap();
        assert!(hub_listening(&path).unwrap());
        drop(second);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn nothing_listens_where_there_is_no_socket() {
        assert!(!hub_listening(&temp_socket("absent")).unwrap());
    }
}
