//! Studio session identity and runtime paths for `kazoo-mix`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use color_eyre::Result;
use kazoo_core::protocol::SessionId;

/// Runtime metadata for a mixer session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixSession {
    /// Stable session id for this mixer process.
    pub id: SessionId,
    /// Runtime directory used for sockets and ephemeral control files.
    pub runtime_dir: PathBuf,
    /// Unix control socket path.
    pub control_socket: PathBuf,
}

impl MixSession {
    /// Create a default local session runtime directory.
    pub fn create_default() -> Result<Self> {
        let id = generate_session_id();
        let runtime_root = runtime_root();
        let name = session_name(id);
        let runtime_dir = runtime_root.join(&name);
        fs::create_dir_all(&runtime_dir)?;
        let control_socket = socket_root().join(format!("km-{}.sock", short_session_suffix(id)));

        Ok(Self {
            id,
            runtime_dir,
            control_socket,
        })
    }
}

fn runtime_root() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(xdg).join("kazoo");
    }

    let user = env::var("USER").unwrap_or_else(|_| "user".to_string());
    env::temp_dir().join(format!("kazoo-{user}"))
}

fn socket_root() -> PathBuf {
    env::temp_dir()
}

fn generate_session_id() -> SessionId {
    let mut bytes = [0_u8; 16];
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    bytes[0..4].copy_from_slice(&pid.to_le_bytes());
    bytes[4..12].copy_from_slice(&(nanos as u64).to_le_bytes());
    bytes[12..16].copy_from_slice(&((nanos >> 64) as u32).to_le_bytes());
    SessionId(bytes)
}

fn session_name(id: SessionId) -> String {
    let mut out = String::with_capacity("session-".len() + 32);
    out.push_str("session-");
    for byte in id.0 {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn short_session_suffix(id: SessionId) -> String {
    let mut out = String::with_capacity(8);
    for byte in &id.0[..4] {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_is_stable_hex() {
        let id = SessionId([0xAB; 16]);
        assert_eq!(session_name(id), "session-abababababababababababababababab");
    }

    #[test]
    fn socket_path_is_short_for_unix_socket_limits() {
        let session = MixSession::create_default().unwrap();
        let len = session.control_socket.as_os_str().len();

        assert!(len < 100, "socket path too long: {len}");
    }
}
