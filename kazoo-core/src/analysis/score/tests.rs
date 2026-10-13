use crate::analysis::score::{
    PyinUnderTest, SCORE_STEP, Take, TrackerUnderTest, score, signals, wav,
};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Oracle {
    truth: Vec<Option<f32>>,
    cursor: usize,
    delay: usize,
    multiplier: f32,
    unvoiced: Option<f32>,
    latest: Option<f32>,
}

impl Oracle {
    /// An oracle reading the truth of the step it has just taken: the
    /// scorer's own grid, one step late by the scorer's count.
    fn on_grid(take: &Take) -> Self {
        Self::new(take, SCORE_STEP)
    }

    fn new(take: &Take, delay: usize) -> Self {
        Self {
            truth: take.truth.clone(),
            cursor: 0,
            delay,
            multiplier: 1.0,
            unvoiced: None,
            latest: None,
        }
    }
}

impl TrackerUnderTest for Oracle {
    fn name(&self) -> &'static str {
        "oracle"
    }
    /// Reads the truth `delay` samples before the newest sample it has taken,
    /// so the scorer, which counts every sample taken, sees `delay` exactly.
    fn push(&mut self, block: &[f32]) {
        self.cursor += block.len();
        self.latest = self
            .cursor
            .checked_sub(self.delay)
            .and_then(|sample| self.truth.get(sample / SCORE_STEP).copied().flatten())
            .map(|hz| hz * self.multiplier)
            .or(self.unvoiced);
    }
    fn hz(&self) -> Option<f32> {
        self.latest
    }
}

fn take_named(name: &str, rate: u32) -> Take {
    signals::suite(rate)
        .into_iter()
        .find(|take| take.name == name)
        .expect("named take")
}

fn cut(take: &mut Take, samples: usize) {
    take.samples.truncate(samples);
    take.truth.truncate(take.samples.len().div_ceil(SCORE_STEP));
    take.onsets.retain(|&onset| onset < take.samples.len());
}

#[test]
fn an_oracle_scores_perfectly() {
    for name in ["steps", "vibrato", "glide"] {
        let mut take = take_named(name, 48_000);
        cut(&mut take, 72_000);
        let delayed = score(&mut Oracle::new(&take, 480), &take);
        assert!((delayed.onset_ms_median - 10.0).abs() <= 32.0 / 48.0);
        if name == "steps" {
            assert!((delayed.step_ms_median - 10.0).abs() <= 32.0 / 48.0);
            // Steps hold still between events, so a late oracle is exact
            // once each settle zone has passed.
            assert!(delayed.cents_median.abs() < 0.01, "{delayed:?}");
            assert!(delayed.cents_p95.abs() < 0.01, "{delayed:?}");
        }
        for value in [
            delayed.octave_pct,
            delayed.gross_pct,
            delayed.missed_pct,
            delayed.false_pct,
        ] {
            assert!(value.abs() < f32::EPSILON);
        }
        // Every figure is unshifted, so a delayed oracle reads a moving
        // pitch late; only an oracle on the scorer's grid can read 0 cents.
        let exact = score(&mut Oracle::on_grid(&take), &take);
        assert!(exact.cents_median.abs() < 0.01);
        assert!(exact.cents_p95.abs() < 0.01);
        for value in [
            exact.octave_pct,
            exact.gross_pct,
            exact.missed_pct,
            exact.false_pct,
        ] {
            assert!(value.abs() < f32::EPSILON);
        }
    }
    let mut truth = vec![Some(220.0); 14];
    truth.extend([Some(3000.0); 2]);
    let short_step = Take {
        name: "short high step".into(),
        rate: 48_000,
        samples: vec![0.0; truth.len() * SCORE_STEP],
        truth,
        onsets: vec![0],
        exact: true,
    };
    let result = score(&mut Oracle::on_grid(&short_step), &short_step);
    assert!(
        (result.step_ms_median - 32.0 / 48.0).abs() < 1e-4,
        "{result:?}"
    );
    let constant = Take {
        name: "constant reference".into(),
        rate: 44_100,
        samples: vec![0.0; 128 * SCORE_STEP],
        truth: vec![Some(220.0); 128],
        onsets: vec![0],
        exact: false,
    };
    let result = score(&mut Oracle::new(&constant, 960), &constant);
    assert!(
        result
            .aligned_lag_ms
            .is_some_and(|lag| (lag - 960.0 / 44.1).abs() <= 32.0 / 44.1),
        "{result:?}"
    );
}

#[test]
fn an_octave_high_oracle_is_all_octave_errors() {
    let mut take = take_named("sine_ladder", 48_000);
    cut(&mut take, 72_000);
    let mut oracle = Oracle::on_grid(&take);
    oracle.multiplier = 2.0;
    let result = score(&mut oracle, &take);
    assert!((result.octave_pct - 100.0).abs() < f32::EPSILON);
    assert!((result.gross_pct - 0.0).abs() < f32::EPSILON);
}

#[test]
fn a_silent_tracker_misses_everything() {
    let mut take = take_named("sine_ladder", 48_000);
    cut(&mut take, 72_000);
    let mut oracle = Oracle::on_grid(&take);
    oracle.truth.fill(None);
    let result = score(&mut oracle, &take);
    assert!((result.missed_pct - 100.0).abs() < f32::EPSILON);
    assert!((result.false_pct - 0.0).abs() < f32::EPSILON);
    assert_eq!(result.onsets_missed, take.onsets.len());
    assert!(result.onset_ms_median.is_nan());
    assert_eq!(result.events.len(), result.onsets_missed);
    assert!(result.events.iter().all(|event| event.latency_ms.is_none()));
    assert!(crate::analysis::score::table(&[("silent", &take.name, result)]).contains(" | - | "));
}

#[test]
fn a_tracker_that_never_stops_is_false_voiced() {
    let mut take = take_named("noise_bursts", 48_000);
    cut(&mut take, 72_000);
    let mut oracle = Oracle::on_grid(&take);
    oracle.unvoiced = Some(220.0);
    assert!(score(&mut oracle, &take).false_pct > 90.0);
    let mut release = Oracle::on_grid(&take);
    for i in 1..take.truth.len() {
        if take.truth[i].is_none() && take.truth[i - 1].is_some() {
            for pitch in release.truth.iter_mut().skip(i).take(45) {
                *pitch = Some(220.0);
            }
        }
    }
    assert!(score(&mut release, &take).false_pct.abs() < f32::EPSILON);
}

#[test]
fn the_suite_is_what_the_brief_says() {
    let suite = signals::suite(48_000);
    let repeated = signals::suite(48_000);
    let names = [
        "sine_ladder",
        "hum_low",
        "steps",
        "vibrato",
        "glide",
        "octave_bait",
        "vowel_ah",
        "noise_bursts",
        "whistle",
        "rearticulate",
    ];
    assert_eq!(suite.len(), 10);
    for (i, (take, repeat)) in suite.iter().zip(&repeated).enumerate() {
        assert_eq!(take.name, names[i]);
        assert!(
            take.samples
                .iter()
                .zip(&repeat.samples)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert!(take.samples.iter().all(|s| s.is_finite() && s.abs() <= 0.5));
        assert_eq!(take.truth.len(), take.samples.len().div_ceil(SCORE_STEP));
        assert!(take.exact);
        assert!(take.truth[..300].iter().all(Option::is_none));
        assert!(
            take.truth[take.truth.len() - 300..]
                .iter()
                .all(Option::is_none)
        );
        assert_eq!(take.onsets.len(), [6, 1, 1, 1, 1, 1, 1, 3, 1, 5][i]);
        let step_count = take
            .truth
            .windows(2)
            .filter(|p| match (p[0], p[1]) {
                (Some(a), Some(b)) => (1200.0 * (b / a).log2()).abs() >= 50.0,
                _ => false,
            })
            .count();
        assert_eq!(step_count, if take.name == "steps" { 6 } else { 0 });
        if take.name == "sine_ladder" {
            assert!(
                take.samples[28_800..33_600]
                    .iter()
                    .all(|sample| sample.abs() <= 0.000_316)
            );
        }
    }
    for (name, voiced_ms) in [
        ("sine_ladder", 2400),
        ("hum_low", 600),
        ("steps", 1750),
        ("vibrato", 1000),
        ("glide", 900),
        ("octave_bait", 600),
        ("vowel_ah", 800),
        ("noise_bursts", 900),
        ("whistle", 800),
        ("rearticulate", 1000),
    ] {
        let take = suite
            .iter()
            .find(|take| take.name == name)
            .expect("named take");
        assert_eq!(
            take.truth.iter().filter(|pitch| pitch.is_some()).count(),
            voiced_ms * 48 / SCORE_STEP
        );
    }
}

fn write_wav(path: &Path) -> crate::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| crate::Error::Config(e.to_string()))?;
    for i in 0..44_100_u32 {
        let sample = (f64::from(i) * 220.0 * std::f64::consts::TAU / 44_100.0).sin() * 16_000.0;
        writer
            .write_sample(sample as i16)
            .map_err(|e| crate::Error::Config(e.to_string()))?;
    }
    writer
        .finalize()
        .map_err(|e| crate::Error::Config(e.to_string()))
}

fn logs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/kz-logs/voice-score-test")
}

#[test]
fn a_recorded_take_is_read_and_referenced() -> crate::Result<()> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("reference.wav");
    write_wav(&path)?;
    let take = wav::read_take(&path)?;
    assert_eq!(take.rate, 44_100);
    assert!(!take.exact);
    let middle = &take.truth[4410 / SCORE_STEP..39_690 / SCORE_STEP];
    let correct = middle
        .iter()
        .filter(|pitch| pitch.is_some_and(|hz| (1200.0 * (hz / 220.0).log2()).abs() < 5.0))
        .count();
    assert!(correct * 10 >= middle.len() * 9);
    let result = score(&mut Oracle::new(&take, 960), &take);
    assert!((result.aligned_lag_ms.expect("recorded lag") - 960.0 / 44.1).abs() <= 32.0 / 44.1);
    Ok(())
}

#[test]
fn the_fixtures_folder_may_be_empty() -> crate::Result<()> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;
    let empty = tempfile::tempdir_in(&dir)?;
    assert!(wav::fixture_takes(empty.path())?.is_empty());
    assert!(wav::fixture_takes(&empty.path().join("missing"))?.is_empty());
    write_wav(&empty.path().join("take.wav"))?;
    std::fs::write(empty.path().join("notes.txt"), "not audio")?;
    assert_eq!(wav::fixture_takes(empty.path())?.len(), 1);
    Ok(())
}

#[test]
fn the_judge_holds_its_floors() -> crate::Result<()> {
    let mut medians = Vec::new();
    for rate in [48_000, 44_100] {
        let mut take = take_named("sine_ladder", rate);
        let length =
            usize::try_from(rate).map_err(|error| crate::Error::Config(error.to_string()))? * 3 / 2;
        cut(&mut take, length);
        let result = score(&mut PyinUnderTest::new(rate)?, &take);
        assert!(result.cents_median <= 10.0, "{rate}: {result:?}");
        assert!(result.octave_pct.abs() < f32::EPSILON, "{rate}: {result:?}");
        assert!(result.missed_pct <= 10.0, "{rate}: {result:?}");
        medians.push(result.cents_median);
    }
    assert!((medians[0] - medians[1]).abs() <= 5.0);
    let mut noise = take_named("noise_bursts", 48_000);
    cut(&mut noise, 72_000);
    let result = score(&mut PyinUnderTest::new(48_000)?, &noise);
    assert!(result.false_pct <= 10.0, "{result:?}");
    Ok(())
}

/// Reads each listed pitch once it has taken that many samples.
#[derive(Debug)]
struct Switches {
    taken: usize,
    changes: Vec<(usize, Option<f32>)>,
}

impl TrackerUnderTest for Switches {
    fn name(&self) -> &'static str {
        "switches"
    }
    fn push(&mut self, block: &[f32]) {
        self.taken += block.len();
    }
    fn hz(&self) -> Option<f32> {
        self.changes
            .iter()
            .rev()
            .find(|(from, _)| *from <= self.taken)
            .and_then(|(_, hz)| *hz)
    }
}

fn recorded(rate: u32, truth: Vec<Option<f32>>) -> Take {
    Take {
        name: "recorded".into(),
        rate,
        samples: vec![0.0; truth.len() * SCORE_STEP],
        onsets: Vec::new(),
        truth,
        exact: false,
    }
}

#[test]
fn latency_is_never_understated() {
    let mut truth = vec![None; 10];
    truth.extend([Some(220.0); 100]);
    truth.extend([Some(330.0); 100]);
    let take = Take {
        onsets: vec![10 * SCORE_STEP],
        exact: true,
        ..recorded(48_000, truth)
    };
    // 100 samples is not a whole number of steps, so a reading on the step
    // before the tracker could have known must not count.
    let late = 100;
    let mut tracker = Switches {
        taken: 0,
        changes: vec![
            (10 * SCORE_STEP + late, Some(220.0)),
            (110 * SCORE_STEP + late, Some(330.0)),
        ],
    };
    let result = score(&mut tracker, &take);
    let truth_ms = 100.0 / 48.0;
    for measured in [result.onset_ms_median, result.step_ms_median] {
        assert!(
            measured >= truth_ms && measured < truth_ms + 32.0 / 48.0,
            "{measured} ms for a {truth_ms} ms tracker: {result:?}"
        );
    }
}

#[test]
fn a_recorded_lag_is_set_by_the_notes_not_the_release() {
    let mut truth = vec![None; 20];
    truth.extend([Some(220.0); 100]);
    truth.extend([None; 100]);
    truth.extend([Some(330.0); 100]);
    truth.extend([None; 60]);
    let take = recorded(44_100, truth);
    // 960 samples late, and holding its last note through every gap.
    let mut oracle = Oracle::new(&take, 960);
    let mut held = None;
    for pitch in &mut oracle.truth {
        held = pitch.or(held);
        *pitch = held;
    }
    let result = score(&mut oracle, &take);
    assert!(
        result
            .aligned_lag_ms
            .is_some_and(|lag| (lag - 960.0 / 44.1).abs() <= 32.0 / 44.1),
        "{result:?}"
    );
    let mut silent = Oracle::on_grid(&take);
    silent.truth.fill(None);
    let result = score(&mut silent, &take);
    // A tracker that never agrees has no lag, not a perfect one.
    assert!(result.aligned_lag_ms.is_some_and(f32::is_nan), "{result:?}");
}
#[test]
fn a_step_never_landed_is_counted() {
    let take = take_named("steps", 48_000);
    let first = take.truth.iter().find_map(|pitch| *pitch);
    let mut oracle = Oracle::on_grid(&take);
    oracle.truth = take.truth.iter().map(|pitch| pitch.and(first)).collect();
    let result = score(&mut oracle, &take);
    // Only the last step, back to C3, lands on the note this tracker holds.
    assert_eq!(result.steps, 6, "{result:?}");
    assert_eq!(result.steps_missed, 5, "{result:?}");
    let table = crate::analysis::score::table(&[("held", &take.name, result)]);
    assert!(table.contains("steps missed"));
    let cells: Vec<_> = table
        .lines()
        .map(|line| line.matches('|').count())
        .collect();
    assert_eq!(cells, [17, 17, 17], "{table}");
}

fn write_float_wav(path: &Path, rate: u32, samples: &[f32]) -> crate::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| crate::Error::Config(e.to_string()))?;
    for &sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| crate::Error::Config(e.to_string()))?;
    }
    writer
        .finalize()
        .map_err(|e| crate::Error::Config(e.to_string()))
}

/// Shares of an analytic take's voiced steps, between two whole-step sample
/// positions, where its recorded reference is voiced, and where it is within
/// 50 cents.
fn reference_against_truth(name: &str, from: usize, to: usize) -> crate::Result<(f32, f32)> {
    let take = take_named(name, 48_000);
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.wav"));
    write_float_wav(&path, take.rate, &take.samples[from..to])?;
    let recorded = wav::read_take(&path)?;
    let truth = &take.truth[from / SCORE_STEP..to / SCORE_STEP];
    assert_eq!(recorded.truth.len(), truth.len());
    let (mut voiced, mut referenced, mut close) = (0_u16, 0_u16, 0_u16);
    for (truth, reference) in truth.iter().zip(&recorded.truth) {
        let Some(truth) = truth else { continue };
        voiced += 1;
        if let Some(reference) = reference {
            referenced += 1;
            close += u16::from((1200.0 * (reference / truth).log2()).abs() <= 50.0);
        }
    }
    let share = |count: u16| f32::from(count) / f32::from(voiced);
    Ok((share(referenced), share(close)))
}

#[test]
fn a_recorded_slide_and_a_step_are_referenced() -> crate::Result<()> {
    // From 50 ms before each note: the glide's held 110 Hz and its first
    // 250 ms of slide; steps' C3 and half its E3.
    let (voiced, close) = reference_against_truth("glide", 7_200, 31_200)?;
    assert!(
        voiced >= 0.98 && close >= 0.95,
        "glide: {voiced} voiced, {close} within 50 cents"
    );
    let (voiced, close) = reference_against_truth("steps", 7_200, 28_800)?;
    assert!(
        voiced >= 0.97 && close >= 0.95,
        "steps: {voiced} voiced, {close} within 50 cents"
    );
    Ok(())
}

fn short_tone(peak: f32) -> Vec<f32> {
    (0..4_410_u16)
        .map(|i| (f32::from(i) * 220.0 * std::f32::consts::TAU / 44_100.0).sin() * peak)
        .collect()
}

#[test]
fn a_float_take_louder_than_full_scale_is_read() -> crate::Result<()> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("loud-float.wav");
    // Float WAVs carry headroom: a peak of 1.5 is a valid recording.
    write_float_wav(&path, 44_100, &short_tone(1.5))?;
    let take = wav::read_take(&path)?;
    assert!(take.samples.iter().any(|sample| *sample > 1.0));
    write_float_wav(&path, 44_100, &[0.0, f32::NAN, 0.0])?;
    assert!(matches!(
        wav::read_take(&path),
        Err(crate::Error::Config(_))
    ));
    Ok(())
}

#[test]
fn a_linked_take_in_the_fixtures_folder_is_read() -> crate::Result<()> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;
    let folder = tempfile::tempdir_in(&dir)?;
    let elsewhere = tempfile::tempdir_in(&dir)?;
    let target = elsewhere.path().join("hum.wav");
    write_float_wav(&target, 44_100, &short_tone(0.5))?;
    std::os::unix::fs::symlink(&target, folder.path().join("hum-low.wav"))?;
    let takes = wav::fixture_takes(folder.path())?;
    assert_eq!(takes.len(), 1);
    assert_eq!(takes[0].name, "hum-low.wav");
    Ok(())
}

/// Voiced spans of each take in ms from the end of the leading floor, and
/// the pitch at a few of those times, as the brief's table gives them.
#[test]
fn the_suite_is_voiced_and_pitched_where_the_table_says() {
    let spans: [(&str, &[(u32, u32)]); 10] = [
        (
            "sine_ladder",
            &[
                (0, 400),
                (500, 900),
                (1000, 1400),
                (1500, 1900),
                (2000, 2400),
                (2500, 2900),
            ],
        ),
        ("hum_low", &[(0, 600)]),
        ("steps", &[(0, 1750)]),
        ("vibrato", &[(0, 1000)]),
        ("glide", &[(0, 900)]),
        ("octave_bait", &[(0, 600)]),
        ("vowel_ah", &[(0, 800)]),
        ("noise_bursts", &[(0, 300), (500, 800), (1000, 1300)]),
        ("whistle", &[(0, 800)]),
        ("rearticulate", &[(0, 1000)]),
    ];
    let suite = signals::suite(48_000);
    for (name, voiced) in spans {
        let take = suite
            .iter()
            .find(|take| take.name == name)
            .expect("named take");
        for (step, pitch) in take.truth.iter().enumerate() {
            let centre = step * SCORE_STEP + SCORE_STEP / 2;
            let inside = centre.checked_sub(9_600).is_some_and(|sample| {
                voiced
                    .iter()
                    .any(|&(from, to)| sample >= from as usize * 48 && sample < to as usize * 48)
            });
            assert_eq!(pitch.is_some(), inside, "{name} step {step}");
        }
    }
    let pitch_at = |name: &str, ms: usize| {
        let take = suite
            .iter()
            .find(|take| take.name == name)
            .expect("named take");
        take.truth[(9_600 + ms * 48) / SCORE_STEP].expect("voiced")
    };
    let close = |hz: f32, expected: f32| (1200.0 * (hz / expected).log2()).abs() < 2.0;
    for (ms, hz) in [
        (200, 110.0),
        (700, 220.0),
        (1200, 440.0),
        (1700, 880.0),
        (2200, 1760.0),
        (2700, 3000.0),
    ] {
        assert!(
            close(pitch_at("sine_ladder", ms), hz),
            "sine_ladder at {ms} ms"
        );
    }
    for (ms, hz) in [
        (100, 130.81),
        (350, 164.81),
        (600, 196.0),
        (850, 261.63),
        (1100, 196.0),
        (1350, 164.81),
        (1600, 130.81),
    ] {
        assert!(close(pitch_at("steps", ms), hz), "steps at {ms} ms");
    }
    assert!(close(pitch_at("glide", 100), 110.0));
    assert!(close(pitch_at("glide", 450), 220.0));
    assert!(close(pitch_at("glide", 800), 440.0));
    assert!(close(pitch_at("hum_low", 300), 65.406));
    assert!(close(pitch_at("noise_bursts", 650), 330.0));
    assert!(close(
        pitch_at("whistle", 50),
        1500.0 * (20.0_f32 / 1200.0).exp2()
    ));
    assert!(close(pitch_at("vibrato", 0), 220.0));
}

#[test]
fn cycle_meets_its_targets() {
    use super::{CycleUnderTest, EventKind};
    let mut failures = Vec::new();
    for mut take in signals::suite(48_000) {
        cut(&mut take, 72_000);
        let result = score(&mut CycleUnderTest::new(48_000), &take);
        let accurate = match take.name.as_str() {
            "sine_ladder" | "steps" | "hum_low" | "whistle" => {
                result.onsets_missed == 0
                    && result.cents_median <= 3.0
                    && result.cents_p95 <= 10.0
                    && result.octave_pct <= f32::EPSILON
                    && result.missed_pct <= 2.0
                    && (take.name != "steps" || result.steps_missed == 0)
            }
            "vibrato" => result.cents_median <= 8.0 && result.cents_p95 <= 20.0,
            "glide" => result.cents_median <= 10.0 && result.cents_p95 <= 25.0,
            "vowel_ah" | "octave_bait" => result.octave_pct <= 5.0,
            "noise_bursts" => result.false_pct <= 2.0,
            _ => true,
        };
        if !accurate {
            failures.push(format!("{} accuracy: {result:?}", take.name));
        }
        let periods = match take.name.as_str() {
            "sine_ladder" | "steps" | "hum_low" | "whistle" => Some(2.0),
            "vowel_ah" | "octave_bait" => Some(3.0),
            _ => None,
        };
        if let Some(periods) = periods {
            for event in &result.events {
                if event.kind == EventKind::Step && take.name != "steps" {
                    continue;
                }
                let limit = periods * 1_000.0 / event.truth_hz + 1.0;
                if !event.latency_ms.is_some_and(|ms| ms <= limit) {
                    failures.push(format!("{}: {event:?}, limit {limit}", take.name));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A tracker under test that keeps what it said at every scoring step, so
/// one pass both scores a take and shows where any octave error fell.
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

/// The scoreboard's own octave error (`metrics.rs`): within 100 cents of a
/// whole, nonzero number of octaves from the truth.
fn is_octave_error(hz: f32, truth: f32) -> bool {
    let cents = 1200.0 * (hz / truth).log2();
    let octaves = (cents / 1200.0).round();
    octaves != 0.0 && 1200.0_f32.mul_add(-octaves, cents).abs() <= 100.0
}

#[test]
fn voice_meets_its_targets() -> crate::Result<()> {
    use super::{EventKind, FusedUnderTest};
    let mut failures = Vec::new();
    for take in signals::suite(48_000) {
        let mut recorded = Recorded {
            tracker: FusedUnderTest::new(48_000)?,
            said: Vec::with_capacity(take.truth.len()),
        };
        let result = score(&mut recorded, &take);
        let accurate = match take.name.as_str() {
            "sine_ladder" | "steps" | "hum_low" | "whistle" => {
                result.onsets_missed == 0
                    && result.cents_median <= 3.0
                    && result.cents_p95 <= 10.0
                    && result.octave_pct <= f32::EPSILON
                    && result.missed_pct <= 2.0
                    && (take.name != "steps" || result.steps_missed == 0)
            }
            "vibrato" => result.cents_median <= 8.0 && result.cents_p95 <= 20.0,
            "glide" => result.cents_median <= 10.0 && result.cents_p95 <= 25.0,
            "vowel_ah" | "octave_bait" => result.octave_pct <= 2.0,
            "noise_bursts" => result.false_pct <= 1.0,
            _ => true,
        };
        if !accurate {
            failures.push(format!("{} accuracy: {result:?}", take.name));
        }
        let periods = match take.name.as_str() {
            "sine_ladder" | "steps" | "hum_low" | "whistle" => Some(2.0),
            "vowel_ah" | "octave_bait" => Some(3.0),
            _ => None,
        };
        if let Some(periods) = periods {
            for event in &result.events {
                if event.kind == EventKind::Step && take.name != "steps" {
                    continue;
                }
                let limit = periods * 1_000.0 / event.truth_hz + 1.0;
                if !event.latency_ms.is_some_and(|ms| ms <= limit) {
                    failures.push(format!("{}: {event:?}, limit {limit}", take.name));
                }
            }
        }
        if matches!(take.name.as_str(), "octave_bait" | "vowel_ah") {
            // No octave error later than 100 ms after the note's onset.
            let settled = usize::try_from(take.rate / 10).unwrap_or(usize::MAX);
            for (step, (said, truth)) in recorded.said.iter().zip(&take.truth).enumerate() {
                let start = step * SCORE_STEP;
                let note = take.onsets.iter().rev().find(|&&onset| onset <= start);
                if let (Some(&onset), Some(hz), Some(truth)) = (note, *said, *truth) {
                    if start >= onset + settled && is_octave_error(hz, truth) {
                        failures.push(format!(
                            "{}: octave error {hz} Hz for {truth} Hz at sample {start}",
                            take.name
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
#[test]
fn voice_gate_meets_its_targets() -> crate::Result<()> {
    use super::{FusedUnderTest, gate::score_gate};
    let mut failures = Vec::new();
    for take in signals::suite(48_000) {
        let onsets = match take.name.as_str() {
            "noise_bursts" => Some((3, 3)),
            "rearticulate" => Some((5, 5)),
            "sine_ladder" => Some((6, 6)),
            _ => None,
        };
        let edges = matches!(
            take.name.as_str(),
            "sine_ladder" | "steps" | "hum_low" | "whistle"
        );
        if onsets.is_none() && !edges {
            continue;
        }
        let result = score_gate(&mut FusedUnderTest::new(48_000)?, &take);
        if edges && !(result.rise_targets_met && result.fall_targets_met) {
            failures.push(format!("{} edges: {result:?}", take.name));
        }
        if take.name == "noise_bursts" && result.noise_high_pct > 1.0 {
            failures.push(format!("{} noise: {result:?}", take.name));
        }
        // The take's own count and the brief's must agree, and the gate
        // must count exactly that many.
        if let Some((counted, expected)) = onsets {
            if result.onsets_counted != counted || result.onsets_expected != expected {
                failures.push(format!(
                    "{} onsets, expected {expected}: {result:?}",
                    take.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
