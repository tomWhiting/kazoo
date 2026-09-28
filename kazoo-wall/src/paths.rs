//! Where the wall keeps its socket, its state and its recordings.
//!
//! The socket lives in kazoo's runtime directory, beside the desk's (see
//! [`kazoo_core::ipc::discovery::runtime_dir`]); the patch and the change
//! log live in `~/.kazoo/wall/`; recordings go to `~/Music/kazoo-wall/`.
//! Each can be moved with an environment variable, which is how tests and
//! second instances keep apart.

use std::io;
use std::path::PathBuf;

use crate::protocol::SOCKET_NAME;

/// Overrides the directory holding the socket.
pub const RUNTIME_DIR_ENV: &str = "KAZOO_WALL_RUNTIME_DIR";

/// Overrides the directory holding the patch and the log.
pub const STATE_DIR_ENV: &str = "KAZOO_WALL_STATE_DIR";

/// Overrides the directory recordings go to.
pub const RECORDINGS_DIR_ENV: &str = "KAZOO_WALL_RECORDINGS_DIR";

/// The directory holding the daemon's socket: `$KAZOO_WALL_RUNTIME_DIR` if
/// set, else kazoo's runtime directory.
#[must_use]
pub fn runtime_dir() -> PathBuf {
    std::env::var_os(RUNTIME_DIR_ENV)
        .filter(|dir| !dir.is_empty())
        .map_or_else(kazoo_core::ipc::discovery::runtime_dir, PathBuf::from)
}

/// The daemon's socket.
#[must_use]
pub fn socket_path() -> PathBuf {
    runtime_dir().join(SOCKET_NAME)
}

/// The directory holding the patch and the change log:
/// `$KAZOO_WALL_STATE_DIR` if set, else `~/.kazoo/wall`.
///
/// # Errors
///
/// Fails if neither the override nor `$HOME` is set.
pub fn state_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(STATE_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".kazoo").join("wall"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("neither ${STATE_DIR_ENV} nor $HOME is set"),
            )
        })
}

/// The directory recordings go to: `$KAZOO_WALL_RECORDINGS_DIR` if set,
/// else `~/Music/kazoo-wall`.
///
/// # Errors
///
/// Fails if neither the override nor `$HOME` is set.
pub fn recordings_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(RECORDINGS_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join("Music").join("kazoo-wall"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("neither ${RECORDINGS_DIR_ENV} nor $HOME is set"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_named_for_the_wall() {
        assert!(socket_path().ends_with(SOCKET_NAME));
        assert!(runtime_dir().is_absolute() || std::env::var_os(RUNTIME_DIR_ENV).is_some());
    }

    #[test]
    fn recordings_go_to_the_music_folder() {
        // Only read here: the variables are never set in this process.
        match (
            recordings_dir(),
            std::env::var_os(RECORDINGS_DIR_ENV),
            std::env::var_os("HOME"),
        ) {
            (Ok(dir), None, Some(home)) => {
                assert_eq!(dir, PathBuf::from(home).join("Music").join("kazoo-wall"));
            }
            (Ok(dir), Some(set), _) => assert_eq!(dir, PathBuf::from(set)),
            (Err(_), None, None) => {}
            (found, set, home) => panic!("{found:?} with the override {set:?} and home {home:?}"),
        }
    }
}
