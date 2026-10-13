//! Pitch detection via the pYIN algorithm.
//!
//! Wraps the [`pyin`] crate's `PYINExecutor` to provide an incremental,
//! frame-based pitch detector suitable for real-time audio analysis. Incoming
//! samples enter a sliding history window; every hop the pYIN algorithm
//! judges the newest frame with preceding frames as context.

use crate::{frequency_to_midi_note, sanitize_sample};
use pyin::{Framing, PYINExecutor};

use super::{VOICE_MAX_HZ, VOICE_MIN_HZ};

/// pYIN frames per run; the HMM picks the newest frame's octave with two frames of context behind it.
pub const JUDGE_FRAMES: usize = 3;

/// Configuration for the pitch detector.
#[derive(Debug, Clone)]
pub struct PitchDetectorConfig {
    /// Minimum detectable frequency in Hz (default 50.0).
    pub min_frequency: f32,
    /// Maximum detectable frequency in Hz (default 4000.0).
    pub max_frequency: f32,
    /// The stream's rate; [`crate::engine::start`] replaces it with the device's.
    pub sample_rate: u32,
    /// FFT frame length in samples (default 2048).
    pub frame_length: usize,
    /// Samples between judge runs.
    pub hop_length: usize,
    /// Probability threshold above which a frame is considered voiced (default 0.3).
    pub voiced_threshold: f32,
}

impl PitchDetectorConfig {
    /// Configure the voice judge with a window and hop scaled for the stream's rate.
    #[must_use]
    pub const fn for_rate(rate: u32) -> Self {
        let scale = if rate <= 48_000 {
            1
        } else if rate <= 96_000 {
            2
        } else if rate <= 192_000 {
            4
        } else {
            8
        };
        Self {
            min_frequency: VOICE_MIN_HZ,
            max_frequency: VOICE_MAX_HZ,
            sample_rate: rate,
            frame_length: 2048 * scale,
            hop_length: 512 * scale,
            voiced_threshold: 0.3,
        }
    }
}

impl Default for PitchDetectorConfig {
    fn default() -> Self {
        Self::for_rate(44_100)
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
/// accumulated into an internal ring buffer. Every hop the detector runs
/// pYIN and returns a [`PitchEstimate`] for the newest frame.
pub struct PitchDetector {
    executor: PYINExecutor<f64>,
    config: PitchDetectorConfig,
    history: Vec<f64>,
    scratch: Vec<f64>,
    buffer_pos: usize,
    since_run: usize,
}

impl std::fmt::Debug for PitchDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PitchDetector")
            .field("config", &self.config)
            .field("history_len", &self.history.len())
            .field("buffer_pos", &self.buffer_pos)
            .field("since_run", &self.since_run)
            .finish_non_exhaustive()
    }
}

impl PitchDetector {
    /// Create a new pitch detector with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid (e.g.
    /// `min_frequency >= max_frequency`, frequencies outside the Nyquist
    /// range, or a range, frame and hop that pyin cannot build a detector
    /// for).
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

        if config.hop_length == 0 || config.hop_length > config.frame_length {
            return Err(crate::Error::Config(
                "hop_length must be between 1 and frame_length".into(),
            ));
        }

        if config.sample_rate == 0 {
            return Err(crate::Error::Config("sample_rate must be > 0".into()));
        }

        check_pyin_limits(&config)?;

        let history_len = (JUDGE_FRAMES - 1)
            .checked_mul(config.hop_length)
            .and_then(|context| config.frame_length.checked_add(context))
            .ok_or_else(|| crate::Error::Config("pitch history length overflow".into()))?;

        let executor = PYINExecutor::new(
            f64::from(config.min_frequency),
            f64::from(config.max_frequency),
            config.sample_rate,
            config.frame_length,
            None, // win_length: default frame_length / 2
            Some(config.hop_length),
            None, // resolution: default 0.1
        );

        // A full silent history makes startup use the same path as every later hop.
        let history = vec![0.0_f64; history_len];
        let scratch = vec![0.0_f64; history_len];

        Ok(Self {
            executor,
            config,
            history,
            scratch,
            buffer_pos: 0,
            since_run: 0,
        })
    }

    /// Push audio samples into the detector.
    ///
    /// Returns `Some(PitchEstimate)` every hop, or `None` if more samples
    /// are needed. A call spanning several hops runs the judge several
    /// times and returns only the newest estimate.
    /// NaN/Inf samples are sanitized to `0.0`.
    pub fn push_samples(&mut self, samples: &[f32]) -> Option<PitchEstimate> {
        let mut result = None;

        for &s in samples {
            let safe = f64::from(sanitize_sample(s));
            self.history[self.buffer_pos] = safe;
            self.buffer_pos += 1;
            if self.buffer_pos == self.history.len() {
                self.buffer_pos = 0;
            }
            self.since_run += 1;

            if self.since_run == self.config.hop_length {
                result = Some(self.analyze_frame());
                self.since_run = 0;
            }
        }

        result
    }

    /// Reset the detector, clearing the internal buffer.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.buffer_pos = 0;
        self.since_run = 0;
    }

    /// Return the current configuration.
    #[must_use]
    pub const fn config(&self) -> &PitchDetectorConfig {
        &self.config
    }

    /// Judge the newest valid frame with earlier frames as HMM context.
    fn analyze_frame(&mut self) -> PitchEstimate {
        let oldest_len = self.history.len() - self.buffer_pos;
        self.scratch[..oldest_len].copy_from_slice(&self.history[self.buffer_pos..]);
        self.scratch[oldest_len..].copy_from_slice(&self.history[..self.buffer_pos]);
        let (_, f0, voiced_flag, voiced_prob) =
            self.executor.pyin(&self.scratch, f64::NAN, Framing::Valid);
        let (Some(&frequency), Some(&voiced), Some(&probability)) =
            (f0.last(), voiced_flag.last(), voiced_prob.last())
        else {
            return PitchEstimate {
                frequency: None,
                voiced_probability: 0.0,
                midi_note: None,
            };
        };
        let voiced_probability = if probability.is_finite() {
            probability.clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let frequency = if voiced
            && probability >= f64::from(self.config.voiced_threshold)
            && frequency.is_finite()
            && frequency > 0.0
        {
            Some(frequency as f32)
        } else {
            None
        };
        PitchEstimate {
            frequency,
            voiced_probability,
            midi_note: frequency.and_then(frequency_to_midi_note),
        }
    }
}

/// Refuse a configuration that pyin's `PYINExecutor::new` would panic on.
///
/// Mirrors pyin 1.2.0's own arithmetic (`pyin.rs:135-177`) with its defaults
/// of `win_length = frame_length / 2` and a resolution of 0.1 semitone (10
/// bins per semitone), so the two never disagree. The caller has already
/// checked the range, the Nyquist limit, a `frame_length` of at least 4 and a
/// hop in `1..=frame_length`.
fn check_pyin_limits(config: &PitchDetectorConfig) -> crate::Result<()> {
    let rate = f64::from(config.sample_rate);
    let fmin = f64::from(config.min_frequency);
    let fmax = f64::from(config.max_frequency);

    // The longest period pyin searches is `frame_length - win_length - 1`.
    let longest = config.frame_length - config.frame_length / 2 - 1;
    let longest = u32::try_from(longest)
        .map(f64::from)
        .map_err(|_| crate::Error::Config("frame_length too large for pyin".into()))?;
    let min_period = (rate / fmax).floor().max(1.0);
    let max_period = (rate / fmin).ceil().min(longest);
    if max_period < min_period + 2.0 {
        return Err(crate::Error::Config(format!(
            "frame_length {} is too short for {fmin} Hz at {rate} Hz: pyin needs \
             periods {min_period} to {max_period} to span at least 2 samples",
            config.frame_length
        )));
    }

    let pitch_bins = (12.0 * 10.0 * (fmax / fmin).log2()).floor() + 1.0;
    if pitch_bins < 2.0 {
        return Err(crate::Error::Config(format!(
            "range [{fmin}, {fmax}] Hz holds fewer than 2 pyin pitch bins"
        )));
    }

    let hop = u32::try_from(config.hop_length)
        .map(f64::from)
        .map_err(|_| crate::Error::Config("hop_length too large for pyin".into()))?;
    let transition_width = (35.92 * 12.0 * hop / rate).round() * 10.0 + 1.0;
    if pitch_bins < transition_width {
        return Err(crate::Error::Config(format!(
            "hop_length {} at {rate} Hz lets a pitch move {transition_width} bins \
             per hop, more than the range's {pitch_bins} bins",
            config.hop_length
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
