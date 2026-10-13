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
    fn push(&mut self, block: &[f32]) {
        self.cursor += block.len();
        self.latest = self
            .cursor
            .checked_sub(block.len() + self.delay)
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
        }
        for value in [
            delayed.octave_pct,
            delayed.gross_pct,
            delayed.missed_pct,
            delayed.false_pct,
        ] {
            assert!(value.abs() < f32::EPSILON);
        }
        // Continuous truth changes during a delay, so zero cents requires zero delay.
        let exact = score(&mut Oracle::new(&take, 0), &take);
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
    let result = score(&mut Oracle::new(&short_step, 0), &short_step);
    assert!(result.step_ms_median.abs() < f32::EPSILON, "{result:?}");
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
    let mut oracle = Oracle::new(&take, 0);
    oracle.multiplier = 2.0;
    let result = score(&mut oracle, &take);
    assert!((result.octave_pct - 100.0).abs() < f32::EPSILON);
    assert!((result.gross_pct - 0.0).abs() < f32::EPSILON);
}

#[test]
fn a_silent_tracker_misses_everything() {
    let mut take = take_named("sine_ladder", 48_000);
    cut(&mut take, 72_000);
    let mut oracle = Oracle::new(&take, 0);
    oracle.truth.fill(None);
    let result = score(&mut oracle, &take);
    assert!((result.missed_pct - 100.0).abs() < f32::EPSILON);
    assert!((result.false_pct - 0.0).abs() < f32::EPSILON);
    assert_eq!(result.onsets_missed, take.onsets.len());
    assert!(result.onset_ms_median.is_nan());
    assert!(crate::analysis::score::table(&[("silent", &take.name, result)]).contains(" | - | "));
}

#[test]
fn a_tracker_that_never_stops_is_false_voiced() {
    let mut take = take_named("noise_bursts", 48_000);
    cut(&mut take, 72_000);
    let mut oracle = Oracle::new(&take, 0);
    oracle.unvoiced = Some(220.0);
    assert!(score(&mut oracle, &take).false_pct > 90.0);
    let mut release = Oracle::new(&take, 0);
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
