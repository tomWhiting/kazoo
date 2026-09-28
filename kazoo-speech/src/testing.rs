//! Helpers shared by the unit tests.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

/// A fresh directory under the system temp dir, removed on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "kazoo-speech-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove {}: {error}", self.0.display());
        }
    }
}
