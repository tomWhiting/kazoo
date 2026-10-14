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

/// The failed writes kept to print at the exit; later ones are counted.
const FAILED_WRITES_KEPT: usize = 16;

#[derive(Debug)]
pub struct TempoStore {
    path: Result<PathBuf, String>,
    pub(crate) bpm: f32,
    pub(crate) warning: Option<String>,
    revision: u64,
    pending: Option<Instant>,
    write_failed: bool,
    last_written: f32,
    warning_at: Option<Instant>,
    failed_writes: Vec<String>,
    failed_writes_unkept: u64,
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
        let warning = match remove_leftovers(&path) {
            Ok(()) => warning,
            Err(error) => {
                let cleanup = format!(
                    "kazoo-mix: {}: temporary tempos could not be removed: {error}",
                    path.display()
                );
                Some(warning.map_or_else(
                    || cleanup.clone(),
                    |warning| format!("{warning}; {cleanup}"),
                ))
            }
        };
        let warning_at = warning.as_ref().map(|_| Instant::now());
        Self {
            path: Ok(path),
            bpm,
            warning,
            revision: 0,
            pending: None,
            write_failed: false,
            last_written: bpm,
            warning_at,
            failed_writes: Vec::new(),
            failed_writes_unkept: 0,
        }
    }

    pub(crate) fn standard() -> Self {
        let mix = std::env::var_os("KAZOO_MIX_DIR");
        let home = std::env::var_os("HOME");
        match path_for(mix.as_deref(), home.as_deref()) {
            Ok(path) => Self::open(path),
            Err(reason) => Self {
                path: Err(reason.clone()),
                bpm: DEFAULT_BPM,
                warning: Some(format!("{reason}; using {DEFAULT_BPM} BPM")),
                revision: 0,
                pending: None,
                write_failed: false,
                last_written: DEFAULT_BPM,
                warning_at: Some(Instant::now()),
                failed_writes: Vec::new(),
                failed_writes_unkept: 0,
            },
        }
    }

    pub(crate) fn status_warning(&self, now: Instant) -> Option<&str> {
        self.warning.as_deref().filter(|_| {
            self.warning_at
                .is_none_or(|at| now.saturating_duration_since(at) < Duration::from_secs(10))
        })
    }

    /// The failed writes to print once the terminal is back: the first
    /// [`FAILED_WRITES_KEPT`] that differ from the one before, then how many
    /// more there were.
    pub(crate) fn failed_writes(&self) -> Vec<String> {
        let mut lines = self.failed_writes.clone();
        if self.failed_writes_unkept > 0 {
            lines.push(format!(
                "kazoo-mix: the tempo failed to save {} more times",
                self.failed_writes_unkept
            ));
        }
        lines
    }

    fn keep_failed_write(&mut self, warning: String) {
        if self.failed_writes.len() < FAILED_WRITES_KEPT
            && self.failed_writes.last() != Some(&warning)
        {
            self.failed_writes.push(warning);
        } else {
            self.failed_writes_unkept = self.failed_writes_unkept.saturating_add(1);
        }
    }

    pub(crate) fn flush(&mut self, transport: Transport) -> Result<(), String> {
        self.bpm = transport.bpm;
        if self.pending.is_some() || self.bpm.to_bits() != self.last_written.to_bits() {
            self.save()?;
        }
        Ok(())
    }

    fn save(&mut self) -> Result<(), String> {
        self.pending = None;
        let result = match &self.path {
            Ok(path) => write(path, self.bpm).map_err(|error| {
                format!(
                    "kazoo-mix: {}: tempo could not be saved: {error}",
                    path.display()
                )
            }),
            Err(reason) => Err(format!(
                "tempo {} BPM could not be saved: {reason}",
                self.bpm
            )),
        };
        match result {
            Ok(()) => {
                self.last_written = self.bpm;
                self.warning = None;
                self.warning_at = None;
                self.write_failed = false;
                Ok(())
            }
            Err(warning) => {
                self.warning = Some(warning.clone());
                self.warning_at = None;
                self.write_failed = true;
                Err(warning)
            }
        }
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
        if let Err(warning) = self.save() {
            self.keep_failed_write(warning);
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

fn remove_leftovers(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut removed = false;
    for entry in entries {
        let entry = entry?;
        if entry.file_name().as_encoded_bytes().starts_with(b".tempo-") {
            fs::remove_file(entry.path())?;
            removed = true;
        }
    }
    if removed {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn create_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut directory = path;
    while !directory.as_os_str().is_empty() && !directory.try_exists()? {
        missing.push(directory);
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    fs::create_dir_all(path)?;
    // Each new directory entry must survive before the tempo is renamed.
    for directory in missing.into_iter().rev() {
        let parent = directory
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent)?.sync_all()?;
    }
    Ok(())
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
    create_directory(parent)?;
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
