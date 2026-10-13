//! Recorded takes and a whole-take, centred pYIN reference.

use std::path::Path;

use hound::{SampleFormat, WavReader};
use pyin::{Framing, PYINExecutor, PadMode};

use super::{SCORE_STEP, Take};
use crate::analysis::{PitchDetectorConfig, VOICE_MAX_HZ, VOICE_MIN_HZ};

fn config_error(path: &Path, reason: impl std::fmt::Display) -> crate::Error {
    crate::Error::Config(format!("{}: {reason}", path.display()))
}

/// Read channel zero and judge the entire take without streaming latency.
pub fn read_take(path: &Path) -> crate::Result<Take> {
    let mut reader = WavReader::open(path).map_err(|error| config_error(path, error))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(config_error(path, "WAV has no channels"));
    }
    let frame = PitchDetectorConfig::for_rate(spec.sample_rate).frame_length;
    let rate = f64::from(spec.sample_rate);
    let max_period = (rate / f64::from(VOICE_MIN_HZ)).ceil().min(f64::from(
        u32::try_from(frame / 2 - 1).map_err(|error| config_error(path, error))?,
    ));
    if rate < 2.0 * f64::from(VOICE_MAX_HZ)
        || max_period < (rate / f64::from(VOICE_MAX_HZ)).floor().max(1.0) + 2.0
    {
        return Err(config_error(
            path,
            "sample rate cannot support the voice reference range",
        ));
    }
    let channels = usize::from(spec.channels);
    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, bits @ (16 | 24)) => {
            let scale = if bits == 16 { 32_768.0 } else { 8_388_608.0 };
            let mut samples = Vec::new();
            for (index, sample) in reader.samples::<i32>().enumerate() {
                let sample = sample.map_err(|error| config_error(path, error))?;
                if index % channels == 0 {
                    samples.push(sample as f32 / scale);
                }
            }
            samples
        }
        (SampleFormat::Float, 32) => {
            let mut samples = Vec::new();
            for (index, sample) in reader.samples::<f32>().enumerate() {
                let sample = sample.map_err(|error| config_error(path, error))?;
                if !sample.is_finite() || sample.abs() > 1.0 {
                    return Err(config_error(
                        path,
                        "float sample is nonfinite or outside [-1, 1]",
                    ));
                }
                if index % channels == 0 {
                    samples.push(sample);
                }
            }
            samples
        }
        _ => {
            return Err(config_error(
                path,
                "expected 16/24-bit integer or 32-bit float WAV",
            ));
        }
    };
    if samples.is_empty() {
        return Err(config_error(path, "WAV has no samples"));
    }
    let input: Vec<f64> = samples.iter().copied().map(f64::from).collect();
    let mut executor = PYINExecutor::new(
        f64::from(VOICE_MIN_HZ),
        f64::from(VOICE_MAX_HZ),
        spec.sample_rate,
        frame,
        None,
        Some(SCORE_STEP),
        None,
    );
    let (_, f0, voiced, _) =
        executor.pyin(&input, f64::NAN, Framing::Center(PadMode::Constant(0.0)));
    let truth: Vec<_> = f0
        .into_iter()
        .zip(voiced)
        .take(samples.len().div_ceil(SCORE_STEP))
        .map(|(hz, voiced)| (voiced && hz.is_finite() && hz > 0.0).then_some(hz as f32))
        .collect();
    let onsets = truth
        .iter()
        .enumerate()
        .filter_map(|(index, hz)| {
            (hz.is_some() && (index == 0 || truth[index - 1].is_none()))
                .then_some(index * SCORE_STEP)
        })
        .collect();
    Ok(Take {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        rate: spec.sample_rate,
        samples,
        truth,
        onsets,
        exact: false,
    })
}

/// Read WAVs directly in a directory, sorted by filename.
pub fn fixture_takes(dir: &Path) -> crate::Result<Vec<Take>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(config_error(dir, error)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| config_error(dir, error))?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
            && entry
                .file_type()
                .map_err(|error| config_error(&path, error))?
                .is_file()
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths.iter().map(|path| read_take(path)).collect()
}
