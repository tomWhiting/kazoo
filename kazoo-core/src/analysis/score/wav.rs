//! Recorded takes and a whole-take, centred pYIN reference.

use std::path::Path;

use hound::{SampleFormat, WavReader};
use pyin::{Framing, PYINExecutor, PadMode};

use super::{SCORE_STEP, Take};
use crate::analysis::{PitchDetectorConfig, VOICE_MAX_HZ, VOICE_MIN_HZ};

fn config_error(path: &Path, reason: impl std::fmt::Display) -> crate::Error {
    crate::Error::Config(format!("{}: {reason}", path.display()))
}

fn whole(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::NAN, f64::from)
}

/// The reference at a step's centre sample, from centred frames `hop` apart:
/// voiced as the nearest frame is, and between two voiced frames
/// interpolated in octaves.
fn at_step(frames: &[Option<f64>], step: usize, hop: usize) -> Option<f32> {
    let centre = step * SCORE_STEP + SCORE_STEP / 2;
    let before = centre / hop;
    let offset = centre % hop;
    let nearest = if offset * 2 < hop { before } else { before + 1 };
    let hz = (*frames.get(nearest)?)?;
    let hz = match (frames[before], frames.get(before + 1).copied().flatten()) {
        (Some(low), Some(high)) => {
            let weight = whole(offset) / whole(hop);
            low.log2()
                .mul_add(1.0 - weight, high.log2() * weight)
                .exp2()
        }
        _ => hz,
    };
    Some(hz as f32)
}

/// Read channel zero and judge the entire take without streaming latency.
pub fn read_take(path: &Path) -> crate::Result<Take> {
    let mut reader = WavReader::open(path).map_err(|error| config_error(path, error))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(config_error(path, "WAV has no channels"));
    }
    let config = PitchDetectorConfig::for_rate(spec.sample_rate);
    let frame = config.frame_length;
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
    // The judge's own framing, run once over the whole take. pYIN's HMM lets
    // a pitch move at most 35.92 octaves a second, in whole semitones per
    // frame; at one frame per 32-sample step that rounds to none, so the path
    // could only change pitch through an unvoiced frame (a slide lagged, every
    // step dropped out), and its cost (frames x states squared) was 16 times
    // this. Each step is then read between the two frames around its centre.
    let mut executor = PYINExecutor::new(
        f64::from(VOICE_MIN_HZ),
        f64::from(VOICE_MAX_HZ),
        spec.sample_rate,
        config.frame_length,
        None,
        Some(config.hop_length),
        None,
    );
    let (_, f0, voiced, _) =
        executor.pyin(&input, f64::NAN, Framing::Center(PadMode::Constant(0.0)));
    let frames: Vec<Option<f64>> = f0
        .into_iter()
        .zip(voiced)
        .map(|(hz, voiced)| (voiced && hz.is_finite() && hz > 0.0).then_some(hz))
        .collect();
    let truth: Vec<_> = (0..samples.len().div_ceil(SCORE_STEP))
        .map(|step| at_step(&frames, step, config.hop_length))
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
