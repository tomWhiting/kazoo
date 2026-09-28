//! The sample store: a directory of WAV files, one per sample.
//!
//! The store lives in `~/.kazoo/wall/samples/` by default, a directory only
//! its owner can open (mode 0700). Every sample is `<name>.wav`, where the
//! name is a checked [`SampleName`], so no request can reach a path outside
//! the directory, and a symbolic link in it is never followed. Writes go to a
//! hidden temporary file that is fsynced and then renamed, so a crash leaves
//! either the old sample or the new one, never half of one.
//!
//! Loading converts whatever the WAV holds (8, 16, 24 or 32-bit integer or
//! 32-bit float, any rate, any channel count) into stereo `f32` at the
//! engine's rate, and counts the memory against a cap that the loaded
//! samples share; a sample's share comes back when its last `Arc` drops.

pub(crate) mod wav;

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use crate::resample::Resampler;
use crate::sample::Reservation;
use crate::{Error, Result, SampleData, SampleName, rate_is_valid};

/// Temporary files older than this are left over from a crash and are
/// removed when a store opens.
const STALE_TEMP: Duration = Duration::from_secs(3_600);

/// Whether an existing sample may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    /// Fail with [`Error::Exists`] and leave the old sample alone.
    Refuse,
    /// Replace it atomically.
    Replace,
}

/// How much the store will hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoreLimits {
    /// The longest sample that may be saved, recorded or loaded, in seconds.
    pub max_seconds: f64,
    /// The memory all loaded samples may take together, in bytes.
    pub max_loaded_bytes: usize,
}

impl Default for StoreLimits {
    /// Ten minutes per sample; 2 GiB of loaded samples.
    fn default() -> Self {
        Self {
            max_seconds: 600.0,
            max_loaded_bytes: 2 << 30,
        }
    }
}

/// What a sample on disk is.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleInfo {
    /// Its name.
    pub name: SampleName,
    /// Length in frames.
    pub frames: u64,
    /// Frames per second.
    pub rate: u32,
    /// Channels in the file.
    pub channels: u16,
    /// Bits per sample in the file.
    pub bits: u16,
    /// Whether the samples are floating point.
    pub float: bool,
    /// Length in seconds.
    pub seconds: f64,
    /// Size of the file.
    pub file_bytes: u64,
}

/// A file in the store that is not a usable sample, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    /// The file's name, lossily decoded and cut to 64 characters.
    pub file: String,
    /// What is wrong with it.
    pub reason: String,
}

/// Everything in the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    /// The samples, by name.
    pub samples: Vec<SampleInfo>,
    /// Files that look like samples but cannot be used, so none goes
    /// unmentioned.
    pub unreadable: Vec<Unreadable>,
}

/// A directory of samples. Cheap to clone; clones share the memory budget.
#[derive(Debug, Clone)]
pub struct SampleStore {
    dir: Arc<PathBuf>,
    limits: StoreLimits,
    loaded: Arc<AtomicUsize>,
}

impl SampleStore {
    /// `~/.kazoo/wall/samples`.
    pub fn default_dir() -> Result<PathBuf> {
        match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => Ok(PathBuf::from(home).join(".kazoo/wall/samples")),
            Some(_) | None => Err(Error::NoHome),
        }
    }

    /// Open the store in its default place, creating it if need be.
    pub fn open_default(limits: StoreLimits) -> Result<Self> {
        Self::open(Self::default_dir()?, limits)
    }

    /// Open the store in `dir`, creating it (mode 0700) if need be, making
    /// sure only its owner can open it, and clearing temporary files left
    /// by a crash.
    pub fn open(dir: impl Into<PathBuf>, limits: StoreLimits) -> Result<Self> {
        let dir = dir.into();
        let context = format!("opening the sample store at {}", dir.display());
        if !(limits.max_seconds.is_finite() && limits.max_seconds > 0.0) {
            return Err(Error::BadAudio {
                reason: format!(
                    "a store's longest sample must be positive, not {}",
                    limits.max_seconds
                ),
            });
        }
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| Error::io(context.clone(), e))?;
        let meta = fs::metadata(&dir).map_err(|e| Error::io(context.clone(), e))?;
        if !meta.is_dir() {
            return Err(Error::io(
                context,
                std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory"),
            ));
        }
        if meta.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| Error::io(context.clone(), e))?;
        }
        let store = Self {
            dir: Arc::new(dir),
            limits,
            loaded: Arc::new(AtomicUsize::new(0)),
        };
        store.clear_stale_temps()?;
        Ok(store)
    }

    /// The store's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The store's limits.
    #[must_use]
    pub const fn limits(&self) -> StoreLimits {
        self.limits
    }

    /// The memory loaded samples take now, in bytes.
    #[must_use]
    pub fn loaded_bytes(&self) -> usize {
        self.loaded.load(Ordering::Acquire)
    }

    /// Every sample, sorted by name, and every file that should be one but
    /// is not usable.
    pub fn list(&self) -> Result<Listing> {
        let context = "listing the sample store";
        let mut listing = Listing::default();
        for entry in fs::read_dir(self.dir()).map_err(|e| Error::io(context, e))? {
            let entry = entry.map_err(|e| Error::io(context, e))?;
            let raw = entry.file_name();
            let file = raw.to_string_lossy();
            // Hidden files are the store's own temporaries (names never
            // start with a dot).
            if file.starts_with('.') {
                continue;
            }
            let shown: String = file.chars().take(64).collect();
            let Some(stem) = file.strip_suffix(".wav") else {
                listing.unreadable.push(Unreadable {
                    file: shown,
                    reason: "not a .wav file".to_owned(),
                });
                continue;
            };
            let found = SampleName::new(stem).and_then(|name| self.info(&name));
            match found {
                Ok(info) => listing.samples.push(info),
                Err(e) => listing.unreadable.push(Unreadable {
                    file: shown,
                    reason: e.to_string(),
                }),
            }
        }
        listing.samples.sort_by(|a, b| a.name.cmp(&b.name));
        listing.unreadable.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(listing)
    }

    /// What the sample `name` is, from its header.
    pub fn info(&self, name: &SampleName) -> Result<SampleInfo> {
        let (reader, file_bytes) = self.open_checked(name)?;
        let spec = reader.spec();
        let frames = u64::from(reader.duration());
        Ok(SampleInfo {
            name: name.clone(),
            frames,
            rate: spec.sample_rate,
            channels: spec.channels,
            bits: spec.bits_per_sample,
            float: spec.sample_format == hound::SampleFormat::Float,
            seconds: frames as f64 / f64::from(spec.sample_rate),
            file_bytes,
        })
    }

    /// Load the sample `name` as stereo `f32` at `engine_rate`, ready to
    /// play. Refused before anything is decoded if it is longer than the
    /// cap or would take the loaded samples over the memory cap. Runs off
    /// the audio thread: it reads, decodes, resamples and analyses.
    pub fn load(&self, name: &SampleName, engine_rate: u32) -> Result<Arc<SampleData>> {
        if !rate_is_valid(engine_rate) {
            return Err(Error::BadAudio {
                reason: format!("engine rate {engine_rate} Hz is out of range"),
            });
        }
        let (reader, _) = self.open_checked(name)?;
        let spec = reader.spec();
        if !rate_is_valid(spec.sample_rate) {
            return Err(Error::Unsupported {
                name: name.to_string(),
                reason: format!("a sample rate of {} Hz", spec.sample_rate),
            });
        }
        let frames = reader.duration() as usize;
        if frames == 0 {
            return Err(Error::Corrupt {
                name: name.to_string(),
                reason: "it has no audio in it".to_owned(),
            });
        }
        self.check_length(name, frames as f64 / f64::from(spec.sample_rate))?;
        let resampler = Resampler::new(spec.sample_rate, engine_rate)?;
        let needed = SampleData::bytes_for(resampler.output_len(frames));
        let reservation = Reservation::take(&self.loaded, needed, self.limits.max_loaded_bytes)
            .map_err(|available| Error::MemoryFull {
                name: name.to_string(),
                needed,
                available,
            })?;
        let (left, right) = wav::decode(reader, name)?;
        let (left, right) = (resampler.process(&left), resampler.process(&right));
        SampleData::build(name.clone(), engine_rate, left, right, Some(reservation)).map(Arc::new)
    }

    /// Save stereo audio at `rate` as the sample `name` (32-bit float), and
    /// say what was saved.
    pub fn save(
        &self,
        name: &SampleName,
        rate: u32,
        left: &[f32],
        right: &[f32],
        overwrite: Overwrite,
    ) -> Result<SampleInfo> {
        if !rate_is_valid(rate) {
            return Err(Error::BadAudio {
                reason: format!("sample rate {rate} Hz is out of range"),
            });
        }
        if left.is_empty() || left.len() != right.len() {
            return Err(Error::BadAudio {
                reason: format!(
                    "channels must be the same non-zero length (left {}, right {})",
                    left.len(),
                    right.len()
                ),
            });
        }
        self.check_length(name, left.len() as f64 / f64::from(rate))?;
        let dest = self.path_of(name);
        self.refuse_non_file(name, &dest)?;
        let mut temp = wav::TempWav::create(self.dir(), name, rate)?;
        temp.write(left, right)?;
        temp.commit(&dest, name, overwrite)?;
        self.info(name)
    }

    /// Delete the sample `name`.
    pub fn delete(&self, name: &SampleName) -> Result<()> {
        let path = self.existing(name)?;
        let context = format!("deleting '{name}'");
        fs::remove_file(&path).map_err(|e| Error::io(context.clone(), e))?;
        wav::sync_dir(self.dir(), &context)
    }

    /// Refuse a sample longer than the cap.
    pub(crate) fn check_length(&self, name: &SampleName, seconds: f64) -> Result<()> {
        if seconds > self.limits.max_seconds {
            return Err(Error::TooLong {
                name: name.to_string(),
                seconds,
                max_seconds: self.limits.max_seconds,
            });
        }
        Ok(())
    }

    /// Where `name` lives.
    pub(crate) fn path_of(&self, name: &SampleName) -> PathBuf {
        self.dir.join(name.file_name())
    }

    /// Open the sample `name` and check its header against the file: its
    /// reader and the file's size.
    fn open_checked(&self, name: &SampleName) -> Result<(wav::Reader, u64)> {
        let path = self.existing(name)?;
        let file_bytes = fs::metadata(&path)
            .map_err(|e| Error::io(format!("reading '{name}'"), e))?
            .len();
        let reader = wav::open(&path, name)?;
        let spec = reader.spec();
        let promised = u64::from(reader.duration())
            .saturating_mul(u64::from(spec.channels))
            .saturating_mul(u64::from(spec.bits_per_sample.div_ceil(8)));
        if promised > file_bytes {
            return Err(Error::Corrupt {
                name: name.to_string(),
                reason: format!(
                    "the header promises {promised} bytes of audio in a {file_bytes}-byte file"
                ),
            });
        }
        Ok((reader, file_bytes))
    }

    /// The path of `name`, which must exist as a regular file.
    fn existing(&self, name: &SampleName) -> Result<PathBuf> {
        let path = self.path_of(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => Ok(path),
            Ok(_) => Err(Error::NotAFile {
                name: name.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound {
                name: name.to_string(),
            }),
            Err(e) => Err(Error::io(format!("finding '{name}'"), e)),
        }
    }

    /// Refuse to write over anything at `path` that is not a regular file.
    pub(crate) fn refuse_non_file(&self, name: &SampleName, path: &Path) -> Result<()> {
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.file_type().is_file() => Err(Error::NotAFile {
                name: name.to_string(),
            }),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(format!("finding '{name}'"), e)),
        }
    }

    /// Remove temporary files a crashed writer left behind.
    fn clear_stale_temps(&self) -> Result<()> {
        let context = "clearing old temporary files from the sample store";
        let now = SystemTime::now();
        for entry in fs::read_dir(self.dir()).map_err(|e| Error::io(context, e))? {
            let entry = entry.map_err(|e| Error::io(context, e))?;
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if !(file.starts_with('.') && file.ends_with(wav::TEMP_SUFFIX)) {
                continue;
            }
            let meta = entry.metadata().map_err(|e| Error::io(context, e))?;
            let modified = meta.modified().map_err(|e| Error::io(context, e))?;
            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
            if meta.is_file() && age > STALE_TEMP {
                fs::remove_file(entry.path()).map_err(|e| Error::io(context, e))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
