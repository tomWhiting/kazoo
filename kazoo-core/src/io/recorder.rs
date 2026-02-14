//! Streaming disk recorder -- writes audio samples to a WAV file in real time.

use std::io::BufWriter;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavWriter};

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// DiskRecorder
// ---------------------------------------------------------------------------

/// Streams audio samples to a WAV file on disk.
///
/// Typical lifecycle:
///
/// ```text
/// let mut rec = DiskRecorder::new(path, 44_100, 2);
/// rec.start()?;                  // opens the file
/// rec.write_samples(&buf)?;      // called many times from the audio thread
/// rec.finish()?;                 // flushes and finalizes the WAV header
/// ```
///
/// The writer uses a [`BufWriter`] internally to reduce system-call overhead
/// and keep the audio callback latency low.
pub struct DiskRecorder {
    writer: Option<WavWriter<BufWriter<std::fs::File>>>,
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
}

impl std::fmt::Debug for DiskRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskRecorder")
            .field("path", &self.path)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("is_recording", &self.is_recording())
            .finish_non_exhaustive()
    }
}

impl DiskRecorder {
    /// Create a new recorder targeting `path` with the given format.
    ///
    /// No file is created until [`start`](Self::start) is called.
    #[must_use]
    pub const fn new(path: PathBuf, sample_rate: u32, channels: u16) -> Self {
        Self {
            writer: None,
            path,
            sample_rate,
            channels,
        }
    }

    /// Open the WAV file and begin recording.
    ///
    /// If a previous recording session was not finished, it is finalized first.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AudioFormat`] if the file cannot be created or the
    /// WAV spec is invalid, or [`Error::Config`] if `channels` or
    /// `sample_rate` is zero.
    pub fn start(&mut self) -> Result<()> {
        // Finish any previous session gracefully.
        if self.writer.is_some() {
            self.finish()?;
        }

        if self.channels == 0 {
            return Err(Error::Config("channel count must be > 0".into()));
        }
        if self.sample_rate == 0 {
            return Err(Error::Config("sample rate must be > 0".into()));
        }

        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };

        let file = std::fs::File::create(&self.path)?;
        let buf_writer = BufWriter::new(file);

        let writer = WavWriter::new(buf_writer, spec)
            .map_err(|e| Error::AudioFormat(format!("failed to create WAV writer: {e}")))?;

        self.writer = Some(writer);
        Ok(())
    }

    /// Write a block of interleaved samples to the open WAV file.
    ///
    /// Every sample is sanitized (NaN/Inf replaced with `0.0`) before
    /// writing.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Stream`] if the recorder has not been started,
    /// or [`Error::AudioFormat`] if a write fails.
    pub fn write_samples(&mut self, samples: &[f32]) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| Error::Stream("recorder not started".into()))?;

        for &sample in samples {
            writer
                .write_sample(crate::sanitize_sample(sample))
                .map_err(|e| Error::AudioFormat(format!("write error: {e}")))?;
        }
        Ok(())
    }

    /// Finalize the WAV header, flush all buffered data, and close the file.
    ///
    /// After this call, [`is_recording`](Self::is_recording) returns `false`.
    /// Calling `finish` when not recording is a harmless no-op.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AudioFormat`] if finalization fails.
    pub fn finish(&mut self) -> Result<()> {
        if let Some(writer) = self.writer.take() {
            writer
                .finalize()
                .map_err(|e| Error::AudioFormat(format!("failed to finalize WAV: {e}")))?;
        }
        Ok(())
    }

    /// Whether the recorder is currently capturing audio.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.writer.is_some()
    }

    /// The file path this recorder writes to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The configured sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The configured channel count.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }
}

impl Drop for DiskRecorder {
    fn drop(&mut self) {
        // Best-effort finalization so the file is always valid if possible.
        let _ = self.finish();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.wav");

        let mut rec = DiskRecorder::new(path.clone(), 44_100, 1);
        assert!(!rec.is_recording());

        rec.start().unwrap();
        assert!(rec.is_recording());

        // Write a 1-second sine wave.
        let samples: Vec<f32> = (0..44_100)
            .map(|i| (i as f32 / 44_100.0 * std::f32::consts::TAU * 440.0).sin() * 0.5)
            .collect();
        rec.write_samples(&samples).unwrap();

        rec.finish().unwrap();
        assert!(!rec.is_recording());

        // Verify the file is a valid WAV.
        let loaded = crate::io::file::read_wav(&path).unwrap();
        assert_eq!(loaded.channels, 1);
        assert_eq!(loaded.sample_rate, 44_100);
        assert_eq!(loaded.samples.len(), 44_100);
    }

    #[test]
    fn recorder_write_before_start_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_started.wav");
        let mut rec = DiskRecorder::new(path, 44_100, 1);
        assert!(rec.write_samples(&[0.0, 0.5]).is_err());
    }

    #[test]
    fn recorder_finish_when_not_started_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noop.wav");
        let mut rec = DiskRecorder::new(path, 44_100, 1);
        assert!(rec.finish().is_ok());
    }

    #[test]
    fn recorder_start_zero_channels_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zero_ch.wav");
        let mut rec = DiskRecorder::new(path, 44_100, 0);
        assert!(rec.start().is_err());
    }

    #[test]
    fn recorder_start_zero_sample_rate_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zero_sr.wav");
        let mut rec = DiskRecorder::new(path, 0, 1);
        assert!(rec.start().is_err());
    }

    #[test]
    fn recorder_sanitizes_nan_and_inf() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sanitize.wav");

        let mut rec = DiskRecorder::new(path.clone(), 44_100, 1);
        rec.start().unwrap();
        rec.write_samples(&[0.5, f32::NAN, f32::INFINITY, -0.3, f32::NEG_INFINITY])
            .unwrap();
        rec.finish().unwrap();

        let loaded = crate::io::file::read_wav(&path).unwrap();
        for s in &loaded.samples {
            assert!(s.is_finite(), "sample should be finite: {s}");
        }
    }

    #[test]
    fn recorder_double_start_finalizes_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("double_start.wav");

        let mut rec = DiskRecorder::new(path.clone(), 44_100, 1);
        rec.start().unwrap();
        rec.write_samples(&[0.1, 0.2, 0.3]).unwrap();

        // Starting again should finalize the first file, then open a new one.
        rec.start().unwrap();
        rec.write_samples(&[0.4, 0.5]).unwrap();
        rec.finish().unwrap();

        // The file should be valid and contain only the second session's samples.
        let loaded = crate::io::file::read_wav(&path).unwrap();
        assert_eq!(loaded.samples.len(), 2);
    }

    #[test]
    fn recorder_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");

        let mut rec = DiskRecorder::new(path.clone(), 48_000, 2);
        rec.start().unwrap();

        let mut samples = Vec::with_capacity(960);
        for i in 0..480 {
            let v = (i as f32 / 480.0 * std::f32::consts::TAU).sin() * 0.3;
            samples.push(v);
            samples.push(-v);
        }
        rec.write_samples(&samples).unwrap();
        rec.finish().unwrap();

        let loaded = crate::io::file::read_wav(&path).unwrap();
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.sample_rate, 48_000);
        assert_eq!(loaded.samples.len(), 960);
    }

    #[test]
    fn recorder_debug_format() {
        let rec = DiskRecorder::new(PathBuf::from("/tmp/test.wav"), 44_100, 2);
        let dbg = format!("{rec:?}");
        assert!(dbg.contains("DiskRecorder"));
        assert!(dbg.contains("44100"));
    }

    #[test]
    fn recorder_accessors() {
        let rec = DiskRecorder::new(PathBuf::from("/tmp/test.wav"), 48_000, 2);
        assert_eq!(rec.sample_rate(), 48_000);
        assert_eq!(rec.channels(), 2);
        assert_eq!(rec.path(), Path::new("/tmp/test.wav"));
    }

    #[test]
    fn recorder_drop_finalizes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drop.wav");

        {
            let mut rec = DiskRecorder::new(path.clone(), 44_100, 1);
            rec.start().unwrap();
            rec.write_samples(&[0.1, 0.2, 0.3]).unwrap();
            // Dropped without calling finish().
        }

        // File should still be valid thanks to Drop impl.
        let loaded = crate::io::file::read_wav(&path).unwrap();
        assert_eq!(loaded.samples.len(), 3);
    }
}
