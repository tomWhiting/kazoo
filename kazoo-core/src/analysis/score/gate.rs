//! Measure gate transitions and articulations on the same scoring clock as pitch.
use super::{SCORE_STEP, Take, TrackerUnderTest};

/// A pitch tracker exposing a gate and cumulative articulation count.
pub trait GateUnderTest: TrackerUnderTest {
    fn gate(&self) -> bool;
    fn onsets(&self) -> u64;
}
/// Gate measurements, including missing transitions so an absent gate cannot pass.
#[derive(Debug, Clone, PartialEq)]
pub struct GateScore {
    pub rise_ms_max: f32,
    pub fall_ms_max: f32,
    pub noise_high_pct: f32,
    pub onsets_counted: u64,
    pub onsets_expected: usize,
    pub rises_missed: usize,
    pub falls_missed: usize,
    pub rise_targets_met: bool,
    pub fall_targets_met: bool,
}
/// Feed a fresh tracker and count transitions against analytic voiced spans.
#[must_use]
pub fn score_gate(tracker: &mut dyn GateUnderTest, take: &Take) -> GateScore {
    let initial_onsets = tracker.onsets();
    let gates: Vec<_> = take
        .samples
        .chunks(SCORE_STEP)
        .map(|block| {
            tracker.push(block);
            tracker.gate()
        })
        .collect();
    let mut score = GateScore {
        rise_ms_max: 0.0,
        fall_ms_max: 0.0,
        noise_high_pct: 0.0,
        onsets_counted: tracker.onsets().saturating_sub(initial_onsets),
        onsets_expected: take.onsets.len(),
        rises_missed: 0,
        falls_missed: 0,
        rise_targets_met: true,
        fall_targets_met: true,
    };
    let ms_per_step = SCORE_STEP as f32 * 1000.0 / take.rate as f32;
    let mut previous = None;
    let mut release_at = None;
    let mut noise = 0_u32;
    let mut high = 0_u32;
    for (step, &truth) in take.truth.iter().enumerate() {
        if let (None, Some(hz)) = (previous, truth) {
            let end = take.truth[step..]
                .iter()
                .position(Option::is_none)
                .map_or(take.truth.len(), |offset| step + offset);
            if let Some(offset) = gates[step..end].iter().position(|&gate| gate) {
                let ms = (offset + 1) as f32 * ms_per_step;
                score.rise_ms_max = score.rise_ms_max.max(ms);
                score.rise_targets_met &= ms <= 2000.0 / hz + 3.0;
            } else {
                score.rises_missed += 1;
                score.rise_targets_met = false;
            }
        }
        if let (Some(hz), None) = (previous, truth) {
            release_at = Some(step);
            let end = take.truth[step..]
                .iter()
                .position(Option::is_some)
                .map_or(take.truth.len(), |offset| step + offset);
            if let Some(offset) = gates[step..end].iter().position(|&gate| !gate) {
                let ms = (offset + 1) as f32 * ms_per_step;
                score.fall_ms_max = score.fall_ms_max.max(ms);
                score.fall_targets_met &= ms <= 2000.0 / hz + 15.0;
            } else {
                score.falls_missed += 1;
                score.fall_targets_met = false;
            }
        }
        if truth.is_none() && release_at.is_none_or(|at| (step - at) as f32 * ms_per_step >= 30.0) {
            noise += 1;
            high += u32::from(gates.get(step).copied().unwrap_or(false));
        }
        previous = truth;
    }
    if noise > 0 {
        score.noise_high_pct = 100.0 * high as f32 / noise as f32;
    }
    score
}
/// Render the gate measurements beside the pitch table.
#[must_use]
pub fn table(rows: &[(&str, &str, GateScore)]) -> String {
    let mut output = String::from(
        "| tracker | take | rise max ms | fall max ms | noise high % | onsets counted | onsets expected | rises missed | falls missed |\n|---|---|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for (tracker, take, score) in rows {
        let fields = [
            tracker.replace('|', "\\|"),
            take.replace('|', "\\|").replace(['\r', '\n'], " "),
            format!("{:.2}", score.rise_ms_max),
            format!("{:.2}", score.fall_ms_max),
            format!("{:.2}", score.noise_high_pct),
            score.onsets_counted.to_string(),
            score.onsets_expected.to_string(),
            score.rises_missed.to_string(),
            score.falls_missed.to_string(),
        ];
        output.push_str("| ");
        output.push_str(&fields.join(" | "));
        output.push_str(" |\n");
    }
    output
}
