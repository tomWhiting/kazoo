//! WAV reading and crash-safe WAV writing for the store.

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

use super::Overwrite;
use crate::{Error, Result, SampleName};

/// Suffix of every temporary file the store writes.
pub(crate) const TEMP_SUFFIX: &str = ".tmp";

/// A counter that keeps temporary names unique within this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The format every sample is saved in: stereo 32-bit float.
pub(crate) const fn stereo_float(rate: u32) -> WavSpec {
    WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    }
}

/// A WAV being written under a hidden temporary name in the store. It only
/// takes its real name in [`Self::commit`], after everything is on disk;
/// dropped uncommitted, the temporary file is removed.
pub(crate) struct TempWav {
    path: PathBuf,
    file: File,
    writer: Option<WavWriter<BufWriter<File>>>,
    context: String,
}

impl TempWav {
    /// Start a stereo float WAV at `rate` in `dir`, for the sample `name`.
    pub(crate) fn create(dir: &Path, name: &SampleName, rate: u32) -> Result<Self> {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            ".{name}.{}.{unique}{TEMP_SUFFIX}",
            std::process::id()
        ));
        let context = format!("writing '{name}'");
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| Error::io(context.clone(), e))?;
        let mut temp = Self {
            path,
            file,
            writer: None,
            context,
        };
        let handle = temp
            .file
            .try_clone()
            .map_err(|e| Error::io(temp.context.clone(), e))?;
        let writer = WavWriter::new(BufWriter::new(handle), stereo_float(rate))
            .map_err(|e| temp.wav_error(e))?;
        temp.writer = Some(writer);
        Ok(temp)
    }

    /// Append stereo frames. Non-finite samples are written as silence.
    pub(crate) fn write(&mut self, left: &[f32], right: &[f32]) -> Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(Error::BadAudio {
                reason: "the file is already finished".to_owned(),
            });
        };
        let mut failure = None;
        for (&l, &r) in left.iter().zip(right) {
            let pair = writer
                .write_sample(finite(l))
                .and_then(|()| writer.write_sample(finite(r)));
            if let Err(e) = pair {
                failure = Some(e);
                break;
            }
        }
        match failure {
            Some(e) => Err(self.wav_error(e)),
            None => Ok(()),
        }
    }

    /// Where the temporary file is.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Finish the header, flush and fsync, then give the file its real
    /// name `dest` and fsync the directory. With [`Overwrite::Refuse`] an
    /// existing sample is never touched: the new name is linked, which
    /// fails if it exists, and only then is the temporary name removed.
    pub(crate) fn commit(
        mut self,
        dest: &Path,
        name: &SampleName,
        overwrite: Overwrite,
    ) -> Result<()> {
        self.finish()?;
        match overwrite {
            Overwrite::Replace => {
                fs::rename(&self.path, dest).map_err(|e| Error::io(self.context.clone(), e))?;
            }
            Overwrite::Refuse => {
                fs::hard_link(&self.path, dest).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        Error::Exists {
                            name: name.to_string(),
                        }
                    } else {
                        Error::io(self.context.clone(), e)
                    }
                })?;
                fs::remove_file(&self.path).map_err(|e| Error::io(self.context.clone(), e))?;
            }
        }
        // The file is in place under its real name: nothing to clean up.
        self.path = PathBuf::new();
        if let Some(dir) = dest.parent() {
            sync_dir(dir, &self.context)?;
        }
        Ok(())
    }

    /// Finish the header, flush and fsync, keeping the temporary name.
    pub(crate) fn finish(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.take() {
            writer.finalize().map_err(|e| self.wav_error(e))?;
        }
        self.file
            .sync_all()
            .map_err(|e| Error::io(self.context.clone(), e))
    }

    fn wav_error(&self, error: hound::Error) -> Error {
        match error {
            hound::Error::IoError(e) => Error::io(self.context.clone(), e),
            other => Error::BadAudio {
                reason: format!("{}: {other}", self.context),
            },
        }
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        // Close the writer first; its own errors no longer matter since the
        // file is being thrown away.
        drop(self.writer.take());
        if let Err(e) = fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "kazoo-sampler: could not remove temporary file {}: {e}",
                    self.path.display()
                );
            }
        }
    }
}

/// Fsync a directory so a rename in it survives a crash.
pub(crate) fn sync_dir(dir: &Path, context: &str) -> Result<()> {
    File::open(dir)
        .and_then(|handle| handle.sync_all())
        .map_err(|e| Error::io(context.to_owned(), e))
}

/// Silence for NaN and infinity.
const fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

/// An open WAV file and its header, for reading.
pub(crate) type Reader = WavReader<BufReader<File>>;

/// Open `path` as a WAV and check the header makes sense.
pub(crate) fn open(path: &Path, name: &SampleName) -> Result<Reader> {
    let reader = WavReader::open(path).map_err(|e| hound_error(e, name, "opening"))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(corrupt(name, "the header says it has no channels"));
    }
    if spec.sample_rate == 0 {
        return Err(corrupt(name, "the header says its sample rate is 0 Hz"));
    }
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) | (SampleFormat::Int, 8 | 16 | 24 | 32) => Ok(reader),
        (format, bits) => Err(Error::Unsupported {
            name: name.to_string(),
            reason: format!("{bits}-bit {format:?} samples"),
        }),
    }
}

/// Every frame of `reader`, as stereo `f32` in -1 to 1: mono is doubled
/// into both channels, and a file with more than two channels gives its
/// first two (front left and right). Integer formats are scaled by their
/// full-scale value.
pub(crate) fn decode(mut reader: Reader, name: &SampleName) -> Result<(Vec<f32>, Vec<f32>)> {
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    let frames = reader.duration() as usize;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    let mut channel = 0;
    let mut push = |value: f32| {
        match (channel, channels) {
            (0, 1) => {
                left.push(value);
                right.push(value);
            }
            (0, _) => left.push(value),
            (1, _) => right.push(value),
            _ => {}
        }
        channel = (channel + 1) % channels;
    };
    if spec.sample_format == SampleFormat::Float {
        for sample in reader.samples::<f32>() {
            push(sample.map_err(|e| hound_error(e, name, "reading"))?);
        }
    } else {
        let scale = 1.0 / f64::from(1u32 << (spec.bits_per_sample - 1));
        for sample in reader.samples::<i32>() {
            let raw = sample.map_err(|e| hound_error(e, name, "reading"))?;
            push((f64::from(raw) * scale) as f32);
        }
    }
    if channel != 0 {
        return Err(corrupt(name, "the audio ends part-way through a frame"));
    }
    if left.len() != frames {
        return Err(corrupt(
            name,
            &format!(
                "the header promises {frames} frames but {} are there",
                left.len()
            ),
        ));
    }
    Ok((left, right))
}

fn corrupt(name: &SampleName, reason: &str) -> Error {
    Error::Corrupt {
        name: name.to_string(),
        reason: reason.to_owned(),
    }
}

/// A hound failure in words, with I/O kept as I/O.
fn hound_error(error: hound::Error, name: &SampleName, doing: &str) -> Error {
    match error {
        hound::Error::IoError(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            corrupt(name, "the file is cut short")
        }
        hound::Error::IoError(e) => Error::io(format!("{doing} '{name}'"), e),
        hound::Error::Unsupported => Error::Unsupported {
            name: name.to_string(),
            reason: "a WAV variant hound cannot read".to_owned(),
        },
        other => corrupt(name, &other.to_string()),
    }
}
