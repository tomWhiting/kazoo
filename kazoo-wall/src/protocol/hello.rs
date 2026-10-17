use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HelloRole {
    /// Whether this connection is a console.
    pub console: bool,
    /// Whether this connection is a watcher.
    pub watcher: bool,
}

impl std::ops::Deref for super::HelloResult {
    type Target = HelloRole;

    fn deref(&self) -> &Self::Target {
        &self.role
    }
}
