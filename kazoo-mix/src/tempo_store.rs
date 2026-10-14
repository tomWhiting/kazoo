use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::shared::{DEFAULT_BPM, MAX_BPM, MIN_BPM, Transport};

static NEXT: AtomicU64 = AtomicU64::new(0);

const SETTLE: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub struct TempoStore {
    path: PathBuf,
    pub(crate) bpm: f32,
    pub(crate) warning: Option<String>,
    revision: u64,
    pending: Option<Instant>,
    write_failed: bool,
}

impl TempoStore {
    pub(crate) fn open(path: PathBuf) -> Self {
        let (bpm, warning) = match read(&path) {
            Ok(bpm) => (bpm, None),
            Err(reason) => (
                DEFAULT_BPM,
                Some(format!(
                    "kazoo-mix: {}: {reason}; using {DEFAULT_BPM} BPM",
                    path.display()
                )),
            ),
        };
        Self {
            path,
            bpm,
            warning,
            revision: 0,
            pending: None,
            write_failed: false,
        }
    }

    pub(crate) fn standard() -> Result<Self, String> {
        let mix = std::env::var_os("KAZOO_MIX_DIR");
        let home = std::env::var_os("HOME");
        path_for(mix.as_deref(), home.as_deref()).map(Self::open)
    }

    pub(crate) fn poll(&mut self, transport: Transport, now: Instant) -> bool {
        if transport.revision != self.revision {
            self.revision = transport.revision;
            if transport.bpm.to_bits() != self.bpm.to_bits() || self.write_failed {
                self.bpm = transport.bpm;
                self.pending = Some(now);
            }
        }
        if self
            .pending
            .is_none_or(|at| now.saturating_duration_since(at) < SETTLE)
        {
            return false;
        }
        self.pending = None;
        match write(&self.path, self.bpm) {
            Ok(()) => {
                self.warning = None;
                self.write_failed = false;
            }
            Err(error) => {
                let warning = format!(
                    "kazoo-mix: {}: tempo could not be saved: {error}",
                    self.path.display()
                );
                eprintln!("{warning}");
                self.warning = Some(warning);
                self.write_failed = true;
            }
        }
        true
    }
}

fn path_for(mix: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(mix) = mix.filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(mix).join("tempo"));
    }
    home.filter(|dir| !dir.is_empty())
        .map(|home| PathBuf::from(home).join(".kazoo/mix/tempo"))
        .ok_or_else(|| "kazoo-mix: HOME is missing; cannot locate the tempo file".to_string())
}

fn read(path: &Path) -> Result<f32, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DEFAULT_BPM),
        Err(error) => return Err(format!("could not read the tempo: {error}")),
    };
    let bpm = contents
        .trim()
        .parse::<f32>()
        .map_err(|error| format!("tempo is not a decimal: {error}"))?;
    if !bpm.is_finite() {
        return Err("tempo must be finite".to_string());
    }
    if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
        return Err(format!(
            "tempo is outside the range {MIN_BPM}–{MAX_BPM} BPM"
        ));
    }
    Ok(bpm)
}

fn write(path: &Path, bpm: f32) -> io::Result<()> {
    if !bpm.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&bpm) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tempo must be finite and in range",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "tempo file has no directory")
    })?;
    fs::create_dir_all(parent)?;
    let (temporary, mut file) = loop {
        let temporary = parent.join(format!(
            ".tempo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) => {
                // A stale temporary from a reused process ID must not block saving.
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
    };
    let result = (|| {
        file.write_all(format!("{bpm}\n").as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        match fs::remove_file(&temporary) {
            Ok(()) => return result,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return result,
            Err(error) => {
                return Err(io::Error::other(format!(
                    "{result:?}; temporary tempo could not be removed: {error}"
                )));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests;
