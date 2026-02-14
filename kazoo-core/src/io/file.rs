//! Audio file reading and writing (WAV via `hound`, multi-format via `symphonia`).

use std::fs::File;
use std::path::Path;

use hound::{SampleFormat, WavReader, WavWriter};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// AudioBuffer
// ---------------------------------------------------------------------------

/// A buffer of interleaved audio samples.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// Interleaved sample data.  For stereo, samples alternate L, R, L, R, ...
    pub samples: Vec<f32>,
    /// Number of audio channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl AudioBuffer {
    /// Create a new buffer with validation.
    ///
    /// Returns an error if `channels` or `sample_rate` is zero.
    pub fn new(samples: Vec<f32>, channels: u16, sample_rate: u32) -> Result<Self> {
        if channels == 0 {
            return Err(Error::AudioFormat("channel count must be > 0".into()));
        }
        if sample_rate == 0 {
            return Err(Error::AudioFormat("sample rate must be > 0".into()));
        }
        Ok(Self {
            samples,
            channels,
            sample_rate,
        })
    }

    /// Number of complete frames (one sample per channel = one frame).
    #[must_use]
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }
        self.samples.len() / self.channels as usize
    }

    /// Duration in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frame_count() as f64 / f64::from(self.sample_rate)
    }
}

// ---------------------------------------------------------------------------
// WAV reading (hound)
// ---------------------------------------------------------------------------

/// Read a WAV file into an [`AudioBuffer`].
///
/// Supports integer (8/16/24/32-bit) and 32-bit float WAV files.
///
/// # Errors
///
/// Returns [`Error::FileIo`] if the file cannot be opened, or
/// [`Error::AudioFormat`] if the WAV data is invalid.
pub fn read_wav(path: &Path) -> Result<AudioBuffer> {
    let reader = WavReader::open(path)
        .map_err(|e| Error::AudioFormat(format!("failed to open WAV file: {e}")))?;

    let spec = reader.spec();
    let channels = spec.channels;
    let sample_rate = spec.sample_rate;

    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| {
                s.map(crate::sanitize_sample)
                    .map_err(|e| Error::AudioFormat(format!("bad WAV sample: {e}")))
            })
            .collect::<Result<Vec<f32>>>()?,
        SampleFormat::Int => {
            let max_val = (1_i64 << (u32::from(spec.bits_per_sample) - 1)) as f32;
            if max_val == 0.0 {
                return Err(Error::AudioFormat("zero bits per sample".into()));
            }
            let inv = 1.0 / max_val;
            reader
                .into_samples::<i32>()
                .map(|s| {
                    s.map(|v| crate::sanitize_sample(v as f32 * inv))
                        .map_err(|e| Error::AudioFormat(format!("bad WAV sample: {e}")))
                })
                .collect::<Result<Vec<f32>>>()?
        }
    };

    AudioBuffer::new(samples, channels, sample_rate)
}

// ---------------------------------------------------------------------------
// WAV writing (hound)
// ---------------------------------------------------------------------------

/// Write an [`AudioBuffer`] to a WAV file in 32-bit float format.
///
/// # Errors
///
/// Returns [`Error::FileIo`] / [`Error::AudioFormat`] on I/O or encoding
/// failures.
pub fn write_wav(path: &Path, buffer: &AudioBuffer) -> Result<()> {
    if buffer.channels == 0 {
        return Err(Error::AudioFormat("channel count must be > 0".into()));
    }
    if buffer.sample_rate == 0 {
        return Err(Error::AudioFormat("sample rate must be > 0".into()));
    }

    let spec = hound::WavSpec {
        channels: buffer.channels,
        sample_rate: buffer.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };

    let mut writer = WavWriter::create(path, spec)
        .map_err(|e| Error::AudioFormat(format!("failed to create WAV file: {e}")))?;

    for &sample in &buffer.samples {
        writer
            .write_sample(crate::sanitize_sample(sample))
            .map_err(|e| Error::AudioFormat(format!("failed to write WAV sample: {e}")))?;
    }

    writer
        .finalize()
        .map_err(|e| Error::AudioFormat(format!("failed to finalize WAV file: {e}")))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-format reading (symphonia)
// ---------------------------------------------------------------------------

/// Read any supported audio file (WAV, MP3, FLAC, OGG, AAC, etc.) into an
/// [`AudioBuffer`].
///
/// Uses the `symphonia` decoder pipeline, which probes the file format
/// automatically.  The entire file is decoded into memory.
///
/// # Errors
///
/// Returns [`Error::FileIo`] if the file cannot be opened, or
/// [`Error::AudioFormat`] if decoding fails.
pub fn read_audio_file(path: &Path) -> Result<AudioBuffer> {
    let file = File::open(path)?;

    // Build a hint from the file extension so symphonia can skip probing bytes
    // when the extension is unambiguous.
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let metadata_opts: MetadataOptions = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| Error::AudioFormat(format!("failed to probe audio format: {e}")))?;

    let mut format = probed.format;

    // Select the first track that has a codec we can decode.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| Error::AudioFormat("no decodable track found".into()))?;

    let codec_params = track.codec_params.clone();
    let track_id = track.id;

    let channels = codec_params
        .channels
        .map_or(1, |ch| ch.count() as u16)
        .max(1);

    let sample_rate = codec_params
        .sample_rate
        .unwrap_or(crate::DEFAULT_SAMPLE_RATE);

    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &decoder_opts)
        .map_err(|e| Error::AudioFormat(format!("failed to create decoder: {e}")))?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break; // end of stream
            }
            Err(e) => {
                return Err(Error::AudioFormat(format!("packet read error: {e}")));
            }
        };

        // Skip packets that do not belong to our chosen track.
        if packet.track_id() != track_id {
            continue;
        }

        let audio_buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(symphonia::core::errors::Error::DecodeError(msg)) => {
                // Non-fatal: skip corrupt frame.
                eprintln!("decode warning (skipping frame): {msg}");
                continue;
            }
            Err(e) => {
                return Err(Error::AudioFormat(format!("decode error: {e}")));
            }
        };

        let spec = *audio_buf.spec();
        let duration = audio_buf.capacity();
        if duration == 0 {
            continue;
        }

        let mut sample_buf = SampleBuffer::<f32>::new(duration as u64, spec);
        sample_buf.copy_interleaved_ref(audio_buf);
        let interleaved = sample_buf.samples();

        // Sanitize every sample coming out of the decoder.
        all_samples.extend(interleaved.iter().copied().map(crate::sanitize_sample));
    }

    AudioBuffer::new(all_samples, channels, sample_rate)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn audio_buffer_new_valid() {
        let buf = AudioBuffer::new(vec![0.0; 100], 2, 44_100).unwrap();
        assert_eq!(buf.frame_count(), 50);
        assert!((buf.duration_seconds() - 50.0 / 44_100.0).abs() < 1e-10);
    }

    #[test]
    fn audio_buffer_zero_channels_rejected() {
        assert!(AudioBuffer::new(vec![0.0], 0, 44_100).is_err());
    }

    #[test]
    fn audio_buffer_zero_sample_rate_rejected() {
        assert!(AudioBuffer::new(vec![0.0], 1, 0).is_err());
    }

    #[test]
    fn audio_buffer_empty_samples() {
        let buf = AudioBuffer::new(Vec::new(), 1, 44_100).unwrap();
        assert_eq!(buf.frame_count(), 0);
        assert!((buf.duration_seconds() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wav_roundtrip_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_mono.wav");

        let original_samples: Vec<f32> = (0..1000)
            .map(|i| (i as f32 / 1000.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();

        let buf = AudioBuffer::new(original_samples.clone(), 1, 44_100).unwrap();
        write_wav(&path, &buf).unwrap();
        assert!(path.exists());

        let loaded = read_wav(&path).unwrap();
        assert_eq!(loaded.channels, 1);
        assert_eq!(loaded.sample_rate, 44_100);
        assert_eq!(loaded.samples.len(), original_samples.len());

        for (a, b) in loaded.samples.iter().zip(original_samples.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "sample mismatch: loaded={a}, original={b}"
            );
        }
    }

    #[test]
    fn wav_roundtrip_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_stereo.wav");

        let mut samples = Vec::with_capacity(2000);
        for i in 0..1000 {
            let v = (i as f32 / 1000.0 * std::f32::consts::TAU).sin() * 0.3;
            samples.push(v); // left
            samples.push(-v); // right
        }

        let buf = AudioBuffer::new(samples.clone(), 2, 48_000).unwrap();
        write_wav(&path, &buf).unwrap();

        let loaded = read_wav(&path).unwrap();
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.sample_rate, 48_000);
        assert_eq!(loaded.samples.len(), samples.len());

        for (a, b) in loaded.samples.iter().zip(samples.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "sample mismatch: loaded={a}, original={b}"
            );
        }
    }

    #[test]
    fn wav_roundtrip_via_symphonia() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_symphonia.wav");

        let original: Vec<f32> = (0..500)
            .map(|i| (i as f32 / 500.0 * std::f32::consts::TAU).sin() * 0.4)
            .collect();

        let buf = AudioBuffer::new(original.clone(), 1, 44_100).unwrap();
        write_wav(&path, &buf).unwrap();

        // Read back via the multi-format reader.
        let loaded = read_audio_file(&path).unwrap();
        assert_eq!(loaded.channels, 1);
        assert_eq!(loaded.sample_rate, 44_100);
        assert_eq!(loaded.samples.len(), original.len());

        for (a, b) in loaded.samples.iter().zip(original.iter()) {
            assert!(
                (a - b).abs() < 1e-5,
                "symphonia roundtrip mismatch: loaded={a}, original={b}"
            );
        }
    }

    #[test]
    fn write_wav_zero_channels_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.wav");
        let buf = AudioBuffer {
            samples: vec![0.0],
            channels: 0,
            sample_rate: 44_100,
        };
        assert!(write_wav(&path, &buf).is_err());
    }

    #[test]
    fn write_wav_zero_sample_rate_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad2.wav");
        let buf = AudioBuffer {
            samples: vec![0.0],
            channels: 1,
            sample_rate: 0,
        };
        assert!(write_wav(&path, &buf).is_err());
    }

    #[test]
    fn read_wav_nonexistent_file() {
        let result = read_wav(Path::new("/tmp/__no_such_file_kazoo__.wav"));
        assert!(result.is_err());
    }

    #[test]
    fn read_audio_file_nonexistent() {
        let result = read_audio_file(Path::new("/tmp/__no_such_file_kazoo__.flac"));
        assert!(result.is_err());
    }

    #[test]
    fn read_wav_corrupt_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.wav");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"this is not a valid wav file").unwrap();
        drop(f);
        assert!(read_wav(&path).is_err());
    }

    #[test]
    fn read_audio_file_corrupt_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.mp3");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"garbage data").unwrap();
        drop(f);
        assert!(read_audio_file(&path).is_err());
    }

    #[test]
    fn wav_handles_nan_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nan.wav");

        let samples = vec![0.5, f32::NAN, -0.3, f32::INFINITY, 0.0];
        let buf = AudioBuffer {
            samples,
            channels: 1,
            sample_rate: 44_100,
        };
        write_wav(&path, &buf).unwrap();

        let loaded = read_wav(&path).unwrap();
        for s in &loaded.samples {
            assert!(s.is_finite(), "NaN/Inf should have been sanitized: {s}");
        }
    }
}
