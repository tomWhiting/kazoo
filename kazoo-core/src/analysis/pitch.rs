//! Pitch detection via the pYIN algorithm.
//!
//! Wraps kazoo's pYIN tracker to provide an incremental,
//! frame-based pitch detector suitable for real-time audio analysis. Incoming
//! samples enter a sliding history window; every hop the pYIN algorithm
//! judges the newest frame with preceding frames as context.

use crate::{frequency_to_midi_note, sanitize_sample};

use super::pyin::{Framing, Pyin, Settings, Track};
use super::{VOICE_MAX_HZ, VOICE_MIN_HZ};

/// pYIN frames per run; the HMM picks the newest frame's octave with two frames of context behind it.
pub const JUDGE_FRAMES: usize = 3;

/// The judge only rules on octave and voicing: fusion folds within 300 cents
/// and flags disagreement past 100. pYIN rounds `1 / 0.34` up to three bins a
/// semitone, so the bins are 33.3 cents: inside both, and they cut pYIN's
/// states from 1518 to 456. A run of three frames then measured 1.3 to
/// 1.8 ms CPU with the pyin crate, and 0.19 to 0.34 ms with kazoo's own
/// (17 Oct 2026, release, a sine, noise and silence at 48 kHz), under the
/// 2.1 ms budget.
pub const JUDGE_RESOLUTION: f64 = 0.34;

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
    /// Semitones per pitch bin; pYIN requires a finite value in 0.01..1.0.
    pub resolution: f64,
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
            resolution: JUDGE_RESOLUTION,
            voiced_threshold: 0.3,
        }
    }

    /// Keep the voice judge's 512 × scale hop wherever pYIN can hold it,
    /// halving only where it cannot, down to 64 samples. If none builds,
    /// leave the configuration unchanged so [`PitchDetector::new`] reports why.
    #[must_use]
    pub fn with_hop_pyin_holds(self) -> Self {
        let mut hop_length = self.hop_length;
        while hop_length >= 64 {
            let candidate = Self { hop_length, ..self };
            if validate(&candidate).is_ok() {
                return candidate;
            }
            if hop_length == 64 {
                break;
            }
            hop_length = (hop_length / 2).max(64);
        }
        self
    }

    /// This configuration with the longest hop pYIN can hold: one run per
    /// window where it can, halving towards a quarter of the frame (pYIN's own
    /// hop) where it cannot. A long window at a low rate (2048 samples is 128 ms
    /// at 16 kHz) lets a pitch move further per hop than the range holds, and
    /// pYIN refuses that. If no hop down to a quarter frame builds, the hop
    /// stays the whole frame and [`PitchDetector::new`] reports why.
    #[must_use]
    pub fn with_longest_hop(self) -> Self {
        let mut hop_length = self.frame_length;
        while hop_length > 0 && hop_length >= self.frame_length / 4 {
            let candidate = Self { hop_length, ..self };
            if validate(&candidate).is_ok() {
                return candidate;
            }
            hop_length /= 2;
        }
        Self {
            hop_length: self.frame_length,
            ..self
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
    tracker: Pyin,
    config: PitchDetectorConfig,
    history: Vec<f64>,
    scratch: Vec<f64>,
    buffer_pos: usize,
    since_run: usize,
    failed_runs: u64,
    last_failure: Option<String>,
}

impl std::fmt::Debug for PitchDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PitchDetector")
            .field("config", &self.config)
            .field("history_len", &self.history.len())
            .field("buffer_pos", &self.buffer_pos)
            .field("since_run", &self.since_run)
            .field("failed_runs", &self.failed_runs)
            .field("last_failure", &self.last_failure)
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
        validate(&config)?;

        let history_len = (JUDGE_FRAMES - 1)
            .checked_mul(config.hop_length)
            .and_then(|context| config.frame_length.checked_add(context))
            .ok_or_else(|| crate::Error::Config("pitch history length overflow".into()))?;

        let tracker = Pyin::new(pyin_settings(&config))?;

        // A full silent history makes startup use the same path as every later hop.
        let history = vec![0.0_f64; history_len];
        let scratch = vec![0.0_f64; history_len];

        Ok(Self {
            tracker,
            config,
            history,
            scratch,
            buffer_pos: 0,
            since_run: 0,
            failed_runs: 0,
            last_failure: None,
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

    /// How many runs pYIN could not make since the detector was built; each
    /// was reported unvoiced.
    #[must_use]
    pub const fn failed_runs(&self) -> u64 {
        self.failed_runs
    }

    /// Why the last failed run failed, if one has.
    #[must_use]
    pub fn last_failure(&self) -> Option<&str> {
        self.last_failure.as_deref()
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
        // The history is one frame and two hops, which `new` checked pYIN
        // can read. A run that fails all the same is counted, its reason
        // kept, and the frame reported unvoiced.
        let track = match self.tracker.track(&self.scratch, f64::NAN, Framing::Valid) {
            Ok(track) => track,
            Err(error) => {
                self.failed_runs += 1;
                self.last_failure = Some(error.to_string());
                Track::default()
            }
        };
        let (Some(&frequency), Some(&voiced), Some(&probability)) = (
            track.f0.last(),
            track.voiced.last(),
            track.voiced_prob.last(),
        ) else {
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

/// Refuse a configuration [`PitchDetector::new`] cannot build a detector for.
fn validate(config: &PitchDetectorConfig) -> crate::Result<()> {
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

    if !config.resolution.is_finite() || !(0.01..1.0).contains(&config.resolution) {
        return Err(crate::Error::Config(
            "resolution must be finite, from 0.01 to under 1.0".into(),
        ));
    }

    check_pyin_limits(config)
}

/// The pYIN settings the detector runs: a window of half the frame.
fn pyin_settings(config: &PitchDetectorConfig) -> Settings {
    Settings {
        fmin: f64::from(config.min_frequency),
        fmax: f64::from(config.max_frequency),
        rate: config.sample_rate,
        frame_length: config.frame_length,
        win_length: config.frame_length / 2,
        hop_length: config.hop_length,
        resolution: config.resolution,
    }
}

/// Refuse a configuration pYIN cannot build a tracker for, or one with more
/// states than its ceiling ([`super::pyin::MAX_STATES`]), before anything is
/// allocated.
/// The caller has already checked the range, the Nyquist limit, a
/// `frame_length` of at least 4 and a hop in `1..=frame_length`.
fn check_pyin_limits(config: &PitchDetectorConfig) -> crate::Result<()> {
    pyin_settings(config).grid().map(drop)
}

#[cfg(test)]
mod tests;
