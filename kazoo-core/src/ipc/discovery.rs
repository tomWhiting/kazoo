//! Hub discovery: socket path resolution and PID file management.
//!
//! The hub writes a PID file containing the socket path and process ID.
//! Instruments read this file to discover the hub. If the hub is not
//! running, instruments fall back to standalone mode.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Default socket filename.
const SOCKET_NAME: &str = "hub.sock";

/// PID filename.
const PID_NAME: &str = "hub.pid";

/// Subdirectory under the runtime directory.
const KAZOO_DIR: &str = "kazoo";

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
// PID file management
// ---------------------------------------------------------------------------

/// Write a PID file containing the socket path and current process ID.
///
/// Creates the runtime directory if it does not exist.
pub fn write_pid_file(socket_path: &std::path::Path) -> io::Result<()> {
    let dir = runtime_dir();
    fs::create_dir_all(&dir)?;

    let pid = std::process::id();
    let contents = format!("{}\n{pid}\n", socket_path.display());
    fs::write(pid_file_path(), contents)
}

/// Read the PID file and return `(socket_path, pid)`.
///
/// Returns `None` if the file does not exist or is malformed.
#[must_use]
pub fn read_pid_file() -> Option<(PathBuf, u32)> {
    let contents = fs::read_to_string(pid_file_path()).ok()?;
    let mut lines = contents.lines();
    let socket_path = PathBuf::from(lines.next()?);
    let pid: u32 = lines.next()?.parse().ok()?;
    Some((socket_path, pid))
}

/// Remove the PID file. Best-effort; ignores errors.
pub fn remove_pid_file() {
    let _ = fs::remove_file(pid_file_path());
}

/// Remove the socket file. Best-effort; ignores errors.
pub fn remove_socket(path: &std::path::Path) {
    let _ = fs::remove_file(path);
}

/// Check whether the hub appears to be running.
///
/// Reads the PID file and checks whether a process with that PID exists.
/// This is a heuristic — the PID could have been reused by an unrelated
/// process — but it is good enough for local discovery.
#[must_use]
pub fn hub_is_running() -> bool {
    let Some((_, pid)) = read_pid_file() else {
        return false;
    };
    // On Unix, sending signal 0 checks process existence without
    // actually delivering a signal. We use std::process::Command
    // with `kill -0` to avoid unsafe.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Return the socket path from the PID file if the hub is running.
#[must_use]
pub fn discover_hub() -> Option<PathBuf> {
    let (socket_path, pid) = read_pid_file()?;
    let running = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if running {
        Some(socket_path)
    } else {
        // Stale PID file — clean it up.
        remove_pid_file();
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_path_contains_kazoo() {
        let path = default_socket_path();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("kazoo"),
            "socket path should contain 'kazoo': {path_str}"
        );
        assert!(
            path_str.ends_with("hub.sock"),
            "socket path should end with hub.sock: {path_str}"
        );
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
        let dir = runtime_dir();
        assert!(dir.is_absolute());
    }
}
