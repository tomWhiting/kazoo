use serde::{Deserialize, Serialize};

/// What a connection is to the wall, as its hello answer says: its fields
/// sit at the top of [`super::HelloResult`] on the wire, as they always have.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HelloRole {
    /// Whether this connection is a console.
    pub console: bool,
    /// Whether this connection is a watcher.
    pub watcher: bool,
}
