//! Offline pitch tracker measurements against analytic or centred pYIN truth.

mod fused;
pub mod gate;
pub mod instrument;
mod metrics;

pub use fused::FusedUnderTest;
pub mod signals;
pub mod wav;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod high_notes;

use super::{PitchDetector, PitchDetectorConfig, cycle::CycleTracker};

/// Trajectories are compared every 32 samples (0.67 ms at 48 kHz).
pub const SCORE_STEP: usize = 32;

/// A pitch tracker fed one scoring step at a time.
pub trait TrackerUnderTest {
    fn name(&self) -> &'static str;
    fn push(&mut self, block: &[f32]);
    /// The latest pitch, or `None` when unvoiced.
    fn hz(&self) -> Option<f32>;
}

/// Samples and truth at the centre of each scoring step.
#[derive(Debug, Clone)]
pub struct Take {
    pub name: String,
    pub rate: u32,
    pub samples: Vec<f32>,
    pub truth: Vec<Option<f32>>,
    /// Articulations, including repeated syllables on a held pitch.
    pub onsets: Vec<usize>,
    /// Whether truth is analytic rather than an offline estimate.
    pub exact: bool,
}

/// An articulation or pitch change measured separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Onset,
    Step,
}

/// The latency of one event, including events that never landed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventLatency {
    pub at_sample: usize,
    pub kind: EventKind,
    pub truth_hz: f32,
    pub latency_ms: Option<f32>,
}

/// One tracker's figures on one take.
///
/// Errors are unshifted: they include the
/// tracker's latency the way a listener hears it. A latency runs from the
/// event's sample to the reading at which the tracker lands and stays, and
/// a reading is taken after each whole step, so a latency is never shorter
/// than the tracker's own and at most one step (0.67 ms at 48 kHz) longer.
/// Medians and maxima of an empty list are NaN.
#[derive(Debug, Clone)]
pub struct Score {
    /// Every onset and step, in sample order.
    pub events: Vec<EventLatency>,
    pub onset_ms_median: f32,
    pub onset_ms_max: f32,
    pub onsets_missed: usize,
    pub step_ms_median: f32,
    pub step_ms_max: f32,
    /// Step events in the truth.
    pub steps: usize,
    /// Step events the tracker never landed on before the note ended; they
    /// are not in the step latency figures.
    pub steps_missed: usize,
    pub cents_median: f32,
    pub cents_p95: f32,
    pub octave_pct: f32,
    pub gross_pct: f32,
    pub missed_pct: f32,
    pub false_pct: f32,
    /// Best reference alignment in milliseconds, for recorded takes only.
    pub aligned_lag_ms: Option<f32>,
}

/// The streaming voice judge, retaining its estimate between judge runs.
#[derive(Debug)]
pub struct PyinUnderTest {
    detector: PitchDetector,
    latest: Option<f32>,
}

impl PyinUnderTest {
    /// Build the judge for this take's sample rate.
    pub fn new(rate: u32) -> crate::Result<Self> {
        Ok(Self {
            detector: PitchDetector::new(PitchDetectorConfig::for_rate(rate))?,
            latest: None,
        })
    }
}

impl TrackerUnderTest for PyinUnderTest {
    fn name(&self) -> &'static str {
        "pYIN"
    }

    fn push(&mut self, block: &[f32]) {
        if let Some(estimate) = self.detector.push_samples(block) {
            self.latest = estimate.frequency;
        }
    }

    fn hz(&self) -> Option<f32> {
        self.latest
    }
}

/// The callback-safe cycle converter on the offline scoreboard.
#[derive(Debug)]
pub struct CycleUnderTest(CycleTracker);

impl CycleUnderTest {
    /// Build a converter for this take's sample rate.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self(CycleTracker::new(rate))
    }
}

impl TrackerUnderTest for CycleUnderTest {
    fn name(&self) -> &'static str {
        "cycle"
    }
    fn push(&mut self, block: &[f32]) {
        self.0.process(block);
    }
    fn hz(&self) -> Option<f32> {
        self.0.estimate().map(|e| e.hz)
    }
}

/// Feed a fresh tracker through a take, then measure its trajectory.
#[must_use]
pub fn score(tracker: &mut dyn TrackerUnderTest, take: &Take) -> Score {
    let estimates: Vec<_> = take
        .samples
        .chunks(SCORE_STEP)
        .map(|block| {
            tracker.push(block);
            tracker.hz().filter(|hz| hz.is_finite() && *hz > 0.0)
        })
        .collect();
    metrics::measure(take, &estimates)
}

fn number(value: f32) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "-".into()
    }
}

/// Format all measurements as one markdown table.
#[must_use]
pub fn table(rows: &[(&str, &str, Score)]) -> String {
    let mut output = String::from(
        "| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |\n|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for (tracker, take, score) in rows {
        let fields = [
            tracker.replace('|', "\\|").replace(['\r', '\n'], " "),
            take.replace('|', "\\|").replace(['\r', '\n'], " "),
            number(score.onset_ms_median),
            number(score.onset_ms_max),
            score.onsets_missed.to_string(),
            number(score.step_ms_median),
            number(score.step_ms_max),
            score.steps.to_string(),
            score.steps_missed.to_string(),
            number(score.cents_median),
            number(score.cents_p95),
            number(score.octave_pct),
            number(score.gross_pct),
            number(score.missed_pct),
            number(score.false_pct),
            score.aligned_lag_ms.map_or_else(|| "-".into(), number),
        ];
        output.push_str("| ");
        output.push_str(&fields.join(" | "));
        output.push_str(" |\n");
    }
    output
}
