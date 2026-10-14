//! Held high notes use the same judge configuration as the engine.

use super::{
    CycleUnderTest, EventKind, FusedUnderTest, SCORE_STEP, Score, Take, TrackerUnderTest, score,
    signals, table,
};
use crate::analysis::PitchDetectorConfig;
use crate::engine::{EngineConfig, check_judge_range, judge_config};
use std::fmt::Write;
use std::path::Path;

struct Recorded<T> {
    tracker: T,
    said: Vec<Option<f32>>,
}

impl<T: TrackerUnderTest> TrackerUnderTest for Recorded<T> {
    fn name(&self) -> &'static str {
        self.tracker.name()
    }

    fn push(&mut self, block: &[f32]) {
        self.tracker.push(block);
        self.said.push(self.tracker.hz());
    }

    fn hz(&self) -> Option<f32> {
        self.tracker.hz()
    }
}

fn is_octave_error(hz: f32, truth: f32) -> bool {
    let cents = 1200.0 * (hz / truth).log2();
    let octaves = (cents / 1200.0).round();
    octaves != 0.0 && 1200.0_f32.mul_add(-octaves, cents).abs() <= 100.0
}

fn measure<T: TrackerUnderTest>(
    tracker: T,
    take: &Take,
    failures: &mut Vec<String>,
) -> (Score, usize) {
    let name = tracker.name();
    let mut recorded = Recorded {
        tracker,
        said: Vec::with_capacity(take.truth.len()),
    };
    let result = score(&mut recorded, take);
    let spliced = take.name.starts_with("splice_");
    let sung = take.name.starts_with("sung");
    let missed_limit = if spliced { 5.0 } else { 2.0 };
    if !(result.octave_pct.abs() <= f32::EPSILON
        && result.gross_pct <= 1.0
        && result.missed_pct <= missed_limit
        && result.cents_median <= 5.0
        && result.cents_p95 <= 25.0)
    {
        failures.push(format!("{name} {} accuracy: {result:?}", take.name));
    }
    let periods = if sung { 3.0 } else { 2.0 };
    let onsets = result
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Onset);
    let mut onset_count = 0;
    for event in onsets {
        onset_count += 1;
        let limit = periods * 1000.0 / event.truth_hz + 1.0;
        if !event.latency_ms.is_some_and(|ms| ms <= limit) {
            failures.push(format!(
                "{name} {} onset: {event:?}, limit {limit} ms",
                take.name
            ));
        }
    }
    if onset_count != take.onsets.len() || result.onsets_missed != 0 {
        failures.push(format!(
            "{name} {} onsets: {onset_count} measured, {} expected, {} missed",
            take.name,
            take.onsets.len(),
            result.onsets_missed
        ));
    }
    let settled = take.rate as usize / 10;
    let mut flips = 0;
    let mut first_flip = None;
    for (step, (said, truth)) in recorded.said.iter().zip(&take.truth).enumerate() {
        let start = step * SCORE_STEP;
        let note = take.onsets.iter().rev().find(|&&onset| onset <= start);
        if let (Some(&onset), Some(hz), Some(truth)) = (note, *said, *truth) {
            if start > onset + settled && is_octave_error(hz, truth) {
                flips += 1;
                if first_flip.is_none() {
                    first_flip = Some((start, hz, truth));
                }
            }
        }
    }
    if flips != 0 {
        failures.push(format!(
            "{name} {}: {flips} octave error steps after 100 ms; first {first_flip:?}",
            take.name
        ));
    }
    (result, flips)
}

fn write_table(test: &str, contents: &str) -> crate::Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/kz-logs/kz-v12");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{test}.md")), contents)?;
    Ok(())
}

fn holds(
    test: &str,
    rate: u32,
    selected: impl Fn(&Take) -> bool,
    engine: bool,
) -> crate::Result<()> {
    let takes: Vec<_> = signals::high_suite(rate)
        .into_iter()
        .filter(selected)
        .collect();
    let mut rows = Vec::new();
    let mut flip_rows = Vec::new();
    let mut failures = Vec::new();
    for take in &takes {
        if !engine {
            let (result, flips) = measure(CycleUnderTest::new(rate), take, &mut failures);
            rows.push(("cycle", take.name.as_str(), result));
            flip_rows.push(("cycle", take.name.as_str(), flips));
        }
        let tracker = if engine {
            FusedUnderTest::with_judge(rate, judge_config(&EngineConfig::default().pitch, rate))?
        } else {
            FusedUnderTest::new(rate)?
        };
        let name = if engine { "engine voice" } else { "voice" };
        let (result, flips) = measure(tracker, take, &mut failures);
        rows.push((name, take.name.as_str(), result));
        flip_rows.push((name, take.name.as_str(), flips));
    }
    let mut contents = table(&rows);
    contents.push_str("\n| tracker | take | octave error steps after 100 ms |\n|---|---|---:|\n");
    for (name, take, flips) in flip_rows {
        writeln!(contents, "| {name} | {take} | {flips} |")
            .map_err(|error| crate::Error::Config(format!("format high-note table: {error}")))?;
    }
    write_table(test, &contents)?;
    assert!(!takes.is_empty(), "the test must select high-note takes");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn the_engine_judge_is_the_voice_judge() -> crate::Result<()> {
    let mut failures = Vec::new();
    let mut contents = table(&[]);
    for rate in [22_050, 44_100, 48_000, 96_000, 192_000] {
        let config = judge_config(&EngineConfig::default().pitch, rate);
        let voice = PitchDetectorConfig::for_rate(rate);
        writeln!(
            contents,
            "\nRate {rate}: engine {config:?}; voice {voice:?}"
        )
        .map_err(|error| crate::Error::Config(format!("format judge config: {error}")))?;
        if !((config.min_frequency - voice.min_frequency).abs() < f32::EPSILON
            && (config.max_frequency - voice.max_frequency).abs() < f32::EPSILON
            && (config.resolution - voice.resolution).abs() < f64::EPSILON
            && config.frame_length == voice.frame_length
            && config.hop_length == voice.hop_length
            && (config.voiced_threshold - voice.voiced_threshold).abs() < f32::EPSILON)
        {
            failures.push(format!("rate {rate}: engine {config:?}; voice {voice:?}"));
        }
    }
    write_table("the_engine_judge_is_the_voice_judge", &contents)?;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn a_narrow_judge_is_refused() -> crate::Result<()> {
    let narrow = PitchDetectorConfig {
        min_frequency: 60.0,
        max_frequency: 1000.0,
        ..PitchDetectorConfig::for_rate(48_000)
    };
    let refused = check_judge_range(&narrow);
    let accepted = check_judge_range(&PitchDetectorConfig::for_rate(48_000));
    let contents = format!("{}\nNarrow: {refused:?}\nVoice: {accepted:?}\n", table(&[]));
    write_table("a_narrow_judge_is_refused", &contents)?;
    assert!(
        matches!(&refused, Err(crate::Error::Config(message)) if message.contains("must cover")),
        "the narrow judge must be refused with the range requirement"
    );
    assert!(
        accepted.is_ok(),
        "the voice judge must be accepted: {accepted:?}"
    );
    Ok(())
}

#[test]
fn high_notes_hold_through_the_engine_judge() -> crate::Result<()> {
    holds(
        "high_notes_hold_through_the_engine_judge",
        48_000,
        |take| take.name.starts_with("whistle_") || take.name.starts_with("sung2_"),
        true,
    )
}

#[test]
fn high_whistles_hold() -> crate::Result<()> {
    holds(
        "high_whistles_hold",
        48_000,
        |take| take.name.starts_with("whistle_"),
        false,
    )
}

#[test]
fn high_sung_seconds_hold() -> crate::Result<()> {
    holds(
        "high_sung_seconds_hold",
        48_000,
        |take| take.name.starts_with("sung2_"),
        false,
    )
}

#[test]
fn high_sung_thirds_hold() -> crate::Result<()> {
    holds(
        "high_sung_thirds_hold",
        48_000,
        |take| take.name.starts_with("sung3_"),
        false,
    )
}

#[test]
fn high_splices_hold() -> crate::Result<()> {
    holds(
        "high_splices_hold",
        48_000,
        |take| take.name.starts_with("splice_"),
        false,
    )
}

#[test]
fn high_notes_hold_at_96k() -> crate::Result<()> {
    holds(
        "high_notes_hold_at_96k",
        96_000,
        |take| matches!(take.name.as_str(), "whistle_2200" | "sung2_2200"),
        false,
    )
}
