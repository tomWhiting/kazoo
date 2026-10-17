//! Worker-side pitch and spectral-flux analysis with callback-safe rings.

use super::TrackSettings;
use crate::analysis::{OnsetDetector, PitchDetector, PitchDetectorConfig, PitchEstimate};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

impl TrackSettings {
    /// Keep the original judge hop and rate scaling, with this profile's range and frame.
    #[must_use]
    pub const fn judge_config(self, rate: u32) -> PitchDetectorConfig {
        self.validate();
        let mut config = PitchDetectorConfig::for_rate(rate);
        let scale = config.frame_length / 2048;
        config.min_frequency = self.cycle.min_hz;
        config.max_frequency = self.cycle.max_hz;
        config.frame_length = self.judge_frame_48k * scale;
        config
    }
}

/// A verdict owns no memory, so consuming or dropping one never frees on the callback.
#[derive(Debug, Clone, Copy)]
pub enum JudgeResult {
    Pitch(PitchEstimate),
    Onset { strength: f32 },
}

/// Onset settings at 48 kHz, scaled with the voice judge at higher rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnsetConfig {
    pub onset_fft_size: usize,
    pub onset_threshold: f32,
}
impl Default for OnsetConfig {
    fn default() -> Self {
        Self {
            onset_fft_size: 1024,
            onset_threshold: 0.3,
        }
    }
}

#[derive(Debug, Default)]
struct JudgeStats {
    samples_dropped: AtomicU64,
    results_dropped: AtomicU64,
    pitch_runs_failed: AtomicU64,
    /// Why the first failed pYIN run failed, set once by the worker.
    first_pitch_failure: OnceLock<String>,
}

/// A judge's counters, shared: what [`Judge::stats`] reads, for whoever
/// reports on the judge (the engine's [`crate::engine::EngineStats`]).
/// Reading takes no lock and allocates nothing.
#[derive(Debug, Clone)]
pub struct JudgeCounters(Arc<JudgeStats>);

impl JudgeCounters {
    /// The counters now.
    #[must_use]
    pub fn snapshot(&self) -> JudgeStatsSnapshot {
        JudgeStatsSnapshot {
            samples_dropped: self.0.samples_dropped.load(Ordering::Relaxed),
            results_dropped: self.0.results_dropped.load(Ordering::Relaxed),
            pitch_runs_failed: self.0.pitch_runs_failed.load(Ordering::Relaxed),
        }
    }

    /// Why the first failed pYIN run failed, once one has.
    #[must_use]
    pub fn first_pitch_failure(&self) -> Option<&str> {
        self.0.first_pitch_failure.get().map(String::as_str)
    }
}
/// Ring overflow counters since this judge started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JudgeStatsSnapshot {
    pub samples_dropped: u64,
    pub results_dropped: u64,
    /// pYIN runs that failed, each reported as an unvoiced frame.
    pub pitch_runs_failed: u64,
}

/// Callback-side producer and a wake token for the worker.
pub struct JudgeFeed {
    samples: HeapProd<f32>,
    thread: Thread,
    stats: Arc<JudgeStats>,
}
/// Callback-side consumer of plain, memory-free verdicts.
pub struct JudgeResults {
    results: HeapCons<JudgeResult>,
}
/// Worker lifetime and shared overflow counters.
#[derive(Debug)]
pub struct Judge {
    handle: JoinHandle<()>,
    stats: Arc<JudgeStats>,
}
impl std::fmt::Debug for JudgeFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JudgeFeed").finish_non_exhaustive()
    }
}
impl std::fmt::Debug for JudgeResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JudgeResults").finish_non_exhaustive()
    }
}

/// Validate and construct detectors before starting the named judge worker.
pub fn spawn(
    pitch: PitchDetectorConfig,
    block: usize,
    onsets: OnsetConfig,
) -> crate::Result<(JudgeFeed, JudgeResults, Judge)> {
    let rate = pitch.sample_rate;
    let hop = pitch.hop_length;
    let mut detector = PitchDetector::new(pitch)?;
    if onsets.onset_fft_size < 4
        || !onsets.onset_threshold.is_finite()
        || !(0.0..=1.0).contains(&onsets.onset_threshold)
    {
        return Err(crate::Error::Config(
            "onset FFT must be at least four and threshold finite in 0..=1".into(),
        ));
    }
    let scale = PitchDetectorConfig::for_rate(rate).frame_length / 2048;
    let fft = onsets
        .onset_fft_size
        .checked_mul(scale)
        .ok_or_else(|| crate::Error::Config("onset FFT size overflow".into()))?;
    let mut onset = OnsetDetector::new(fft, fft / 4, rate as f32, onsets.onset_threshold);
    let capacity = usize::try_from(rate / 10)
        .map_err(|error| crate::Error::Config(error.to_string()))?
        .max(1);
    let (samples, mut input) = HeapRb::<f32>::new(capacity).split();
    let (mut output, results) = HeapRb::<JudgeResult>::new(64).split();
    let stats = Arc::new(JudgeStats::default());
    let worker_stats = Arc::clone(&stats);
    let mut buffer = vec![0.0; block.max(256)];
    let mut failed_runs = 0;
    let handle = std::thread::Builder::new()
        .name("kazoo-judge".into())
        .spawn(move || {
            loop {
                let count = input.pop_slice(&mut buffer);
                if count == 0 {
                    if !input.write_is_held() {
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(20));
                    continue;
                }
                // A detector call returns its newest pitch only, so never span multiple hops.
                for samples in buffer[..count].chunks(hop) {
                    if let Some(estimate) = detector.push_samples(samples) {
                        if output.try_push(JudgeResult::Pitch(estimate)).is_err() {
                            worker_stats.results_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let failed = detector.failed_runs();
                    if failed > failed_runs {
                        if let Some(why) = detector.last_failure() {
                            worker_stats
                                .first_pitch_failure
                                .get_or_init(|| why.to_owned());
                        }
                        worker_stats
                            .pitch_runs_failed
                            .fetch_add(failed - failed_runs, Ordering::Relaxed);
                        failed_runs = failed;
                    }
                    for event in onset.push_samples(samples) {
                        if output
                            .try_push(JudgeResult::Onset {
                                strength: event.strength,
                            })
                            .is_err()
                        {
                            worker_stats.results_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        })?;
    let thread = handle.thread().clone();
    Ok((
        JudgeFeed {
            samples,
            thread,
            stats: Arc::clone(&stats),
        },
        JudgeResults { results },
        Judge { handle, stats },
    ))
}
impl JudgeFeed {
    /// Push the fitting prefix, count the rest and wake once; no allocation or lock.
    pub fn push(&mut self, samples: &[f32]) {
        let count = self.samples.push_slice(samples);
        self.stats
            .samples_dropped
            .fetch_add((samples.len() - count) as u64, Ordering::Relaxed);
        self.thread.unpark();
    }
}
impl JudgeResults {
    /// Pop one memory-free verdict, if available.
    pub fn pop(&mut self) -> Option<JudgeResult> {
        self.results.try_pop()
    }
}
impl Judge {
    /// Read overflow counters without locking.
    #[must_use]
    pub fn stats(&self) -> JudgeStatsSnapshot {
        self.counters().snapshot()
    }

    /// The counters, to read from elsewhere while the judge runs.
    #[must_use]
    pub fn counters(&self) -> JudgeCounters {
        JudgeCounters(Arc::clone(&self.stats))
    }

    /// Join after dropping the feed; a wake makes producer shutdown immediate.
    pub fn join(self) -> std::thread::Result<()> {
        self.handle.thread().unpark();
        self.handle.join()
    }
}
