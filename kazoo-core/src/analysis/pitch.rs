//! Pitch detection via the pYIN algorithm.
//!
//! Wraps the [`pyin`] crate's `PYINExecutor` to provide an incremental,
//! frame-based pitch detector suitable for real-time audio analysis. Incoming
//! samples are accumulated into an internal buffer; when a full frame is
//! available the pYIN algorithm runs and produces a [`PitchEstimate`].

use crate::{frequency_to_midi_note, sanitize_sample};
use pyin::{Framing, PYINExecutor, PadMode};

/// Configuration for the pitch detector.
#[derive(Debug, Clone)]
pub struct PitchDetectorConfig {
    /// Minimum detectable frequency in Hz (default 60.0).
    pub min_frequency: f32,
    /// Maximum detectable frequency in Hz (default 1000.0).
    pub max_frequency: f32,
    /// Audio sample rate in Hz (default 44100).
    pub sample_rate: u32,
    /// FFT frame length in samples (default 2048).
    pub frame_length: usize,
    /// Probability threshold above which a frame is considered voiced (default 0.3).
    pub voiced_threshold: f32,
}

impl Default for PitchDetectorConfig {
    fn default() -> Self {
        Self {
            min_frequency: 60.0,
            max_frequency: 1000.0,
            sample_rate: 44_100,
            frame_length: 2048,
            voiced_threshold: 0.3,
        }
    }
}

/// Result of pitch analysis for a single frame.
#[derive(Debug, Clone, Copy)]
pub struct PitchEstimate {
    /// Detected fundamental frequency in Hz, or `None` if the frame is
    /// unvoiced.
    pub frequency: Option<f32>,
    /// Probability that the frame contains a voiced signal (0.0 to 1.0).
    pub voiced_probability: f32,
    /// Nearest MIDI note number, or `None` if unvoiced or out of MIDI range.
    pub midi_note: Option<u8>,
}

/// Incremental pitch detector built on the pYIN algorithm.
///
/// Samples are pushed via [`push_samples`](Self::push_samples) and
/// accumulated into an internal ring buffer. Once a full frame is available
/// the detector runs pYIN and returns a [`PitchEstimate`].
pub struct PitchDetector {
    executor: PYINExecutor<f64>,
    config: PitchDetectorConfig,
    buffer: Vec<f64>,
    buffer_pos: usize,
}

impl std::fmt::Debug for PitchDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PitchDetector")
            .field("config", &self.config)
            .field("buffer_len", &self.buffer.len())
            .field("buffer_pos", &self.buffer_pos)
            .finish_non_exhaustive()
    }
}

impl PitchDetector {
    /// Create a new pitch detector with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid (e.g.
    /// `min_frequency >= max_frequency`, or frequencies outside the Nyquist
    /// range).
    pub fn new(config: PitchDetectorConfig) -> crate::Result<Self> {
        if !config.min_frequency.is_finite()
            || !config.max_frequency.is_finite()
            || config.min_frequency <= 0.0
            || config.min_frequency >= config.max_frequency
        {
            return Err(crate::Error::Config(format!(
                "invalid frequency range: [{}, {}]",
                config.min_frequency, config.max_frequency
            )));
        }

        let nyquist = f64::from(config.sample_rate) / 2.0;
        if f64::from(config.max_frequency) > nyquist {
            return Err(crate::Error::Config(format!(
                "max_frequency {} exceeds Nyquist {}",
                config.max_frequency, nyquist
            )));
        }

        if config.frame_length < 4 {
            return Err(crate::Error::Config(
                "frame_length must be at least 4".into(),
            ));
        }

        if config.sample_rate == 0 {
            return Err(crate::Error::Config("sample_rate must be > 0".into()));
        }

        let executor = PYINExecutor::new(
            f64::from(config.min_frequency),
            f64::from(config.max_frequency),
            config.sample_rate,
            config.frame_length,
            None, // win_length: default frame_length / 2
            None, // hop_length: default frame_length / 4
            None, // resolution: default 0.1
        );

        // The pYIN algorithm needs at least one full frame of audio.
        // With Center framing, the minimum input length is effectively 1 sample
        // (zero-padding handles the rest), but we accumulate a full frame to
        // provide a complete analysis window.
        let buffer = vec![0.0_f64; config.frame_length];

        Ok(Self {
            executor,
            config,
            buffer,
            buffer_pos: 0,
        })
    }

    /// Push audio samples into the detector.
    ///
    /// Returns `Some(PitchEstimate)` when a complete frame has been
    /// accumulated and analysed, or `None` if more samples are needed.
    /// NaN/Inf samples are sanitized to `0.0`.
    pub fn push_samples(&mut self, samples: &[f32]) -> Option<PitchEstimate> {
        let mut result = None;

        for &s in samples {
            let safe = f64::from(sanitize_sample(s));
            self.buffer[self.buffer_pos] = safe;
            self.buffer_pos += 1;

            if self.buffer_pos >= self.config.frame_length {
                result = Some(self.analyze_frame());
                self.buffer_pos = 0;
            }
        }

        result
    }

    /// Reset the detector, clearing the internal buffer.
    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.buffer_pos = 0;
    }

    /// Return the current configuration.
    #[must_use]
    pub const fn config(&self) -> &PitchDetectorConfig {
        &self.config
    }

    /// Run the pYIN algorithm on the current buffer contents.
    fn analyze_frame(&mut self) -> PitchEstimate {
        let framing = Framing::Center(PadMode::Constant(0.0));
        let fill_unvoiced = f64::NAN;

        let (_timestamps, f0, _voiced_flag, voiced_prob) =
            self.executor.pyin(&self.buffer, fill_unvoiced, framing);

        // The pYIN output contains one or more frame estimates. We take the
        // last one as the most representative for the current buffer position.
        if f0.is_empty() {
            return PitchEstimate {
                frequency: None,
                voiced_probability: 0.0,
                midi_note: None,
            };
        }

        // Average over all returned frames (typically 1-4 for a single frame of audio).
        let mut sum_freq = 0.0_f64;
        let mut sum_prob = 0.0_f64;
        let mut voiced_count = 0usize;
        let total = f0.len();

        for (i, &freq) in f0.iter().enumerate() {
            let prob = if i < voiced_prob.len() {
                voiced_prob[i]
            } else {
                0.0
            };
            sum_prob += prob;
            if freq.is_finite() && freq > 0.0 {
                sum_freq += freq;
                voiced_count += 1;
            }
        }

        let avg_prob = if total > 0 {
            (sum_prob / total as f64) as f32
        } else {
            0.0
        };
        let avg_prob = if avg_prob.is_finite() {
            avg_prob.clamp(0.0, 1.0)
        } else {
            0.0
        };

        if voiced_count > 0 && avg_prob >= self.config.voiced_threshold {
            let avg_freq = (sum_freq / voiced_count as f64) as f32;
            let freq = if avg_freq.is_finite() && avg_freq > 0.0 {
                Some(avg_freq)
            } else {
                None
            };
            PitchEstimate {
                frequency: freq,
                voiced_probability: avg_prob,
                midi_note: freq.and_then(frequency_to_midi_note),
            }
        } else {
            PitchEstimate {
                frequency: None,
                voiced_probability: avg_prob,
                midi_note: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_sine(frequency: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate;
                (2.0 * PI * frequency * t).sin()
            })
            .collect()
    }

    #[test]
    fn detects_440hz_sine() {
        let config = PitchDetectorConfig {
            min_frequency: 60.0,
            max_frequency: 1000.0,
            sample_rate: 44_100,
            frame_length: 2048,
            voiced_threshold: 0.15,
        };
        let mut detector = PitchDetector::new(config).expect("valid config");

        // Generate several frames worth of 440 Hz sine.
        let samples = generate_sine(440.0, 44100.0, 2048 * 4);

        let mut estimates = Vec::new();
        for chunk in samples.chunks(512) {
            if let Some(est) = detector.push_samples(chunk) {
                estimates.push(est);
            }
        }

        // We should have gotten at least one estimate.
        assert!(
            !estimates.is_empty(),
            "should have produced at least one pitch estimate"
        );

        // Find any voiced estimate.
        let voiced: Vec<_> = estimates.iter().filter(|e| e.frequency.is_some()).collect();
        assert!(
            !voiced.is_empty(),
            "at least one estimate should be voiced for a clean sine"
        );

        for est in &voiced {
            let freq = est.frequency.unwrap();
            // pYIN should be within 5% of the true frequency.
            let error_pct = ((freq - 440.0) / 440.0).abs() * 100.0;
            assert!(
                error_pct < 5.0,
                "detected {freq} Hz, expected ~440 Hz (error {error_pct:.1}%)"
            );
        }

        // Check MIDI note for the voiced estimate closest to 440.
        let best = voiced
            .iter()
            .min_by(|a, b| {
                let da = (a.frequency.unwrap() - 440.0).abs();
                let db = (b.frequency.unwrap() - 440.0).abs();
                da.partial_cmp(&db).unwrap()
            })
            .unwrap();
        if let Some(midi) = best.midi_note {
            // A4 = MIDI 69
            assert!(
                (67..=71).contains(&midi),
                "MIDI note should be near 69, got {midi}"
            );
        }
    }

    #[test]
    fn silence_is_unvoiced() {
        let config = PitchDetectorConfig::default();
        let mut detector = PitchDetector::new(config).expect("valid config");

        let silence = vec![0.0_f32; 2048 * 4];
        let mut estimates = Vec::new();
        for chunk in silence.chunks(512) {
            if let Some(est) = detector.push_samples(chunk) {
                estimates.push(est);
            }
        }

        for est in &estimates {
            assert!(
                est.frequency.is_none(),
                "silence should be unvoiced, got freq {:?}",
                est.frequency
            );
        }
    }

    #[test]
    fn reset_clears_buffer() {
        let config = PitchDetectorConfig::default();
        let mut detector = PitchDetector::new(config).expect("valid config");

        // Push partial frame.
        detector.push_samples(&[0.5; 100]);
        assert!(detector.buffer_pos > 0);

        detector.reset();
        assert_eq!(detector.buffer_pos, 0);
    }

    #[test]
    fn invalid_config_returns_error() {
        // min >= max
        let config = PitchDetectorConfig {
            min_frequency: 1000.0,
            max_frequency: 100.0,
            ..PitchDetectorConfig::default()
        };
        assert!(PitchDetector::new(config).is_err());

        // max > Nyquist
        let config = PitchDetectorConfig {
            max_frequency: 30000.0,
            sample_rate: 44100,
            ..PitchDetectorConfig::default()
        };
        assert!(PitchDetector::new(config).is_err());
    }

    #[test]
    fn nan_samples_are_sanitized() {
        let config = PitchDetectorConfig::default();
        let mut detector = PitchDetector::new(config).expect("valid config");

        let mut samples = vec![f32::NAN; 2048];
        samples.extend_from_slice(&[f32::INFINITY; 2048]);
        // Should not panic.
        for chunk in samples.chunks(512) {
            let _ = detector.push_samples(chunk);
        }
    }
}
