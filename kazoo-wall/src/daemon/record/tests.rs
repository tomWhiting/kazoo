//! Recorder tests: the file's shape and frames, names never overwritten,
//! a full file carried on without a gap, failures reported, and files
//! finished however the recorder is let go.

use std::time::{Duration, Instant};

use super::*;
use crate::MAX_KNOBS;
use crate::catalogue::Kind;
use crate::dsp::build;
use crate::engine::{CableTable, Command, Engine, EngineConfig, Route, engine, processing_order};

const RATE: u32 = 48_000;

/// An engine at `rate` playing an oscillator into an out, heard (so what
/// it renders is its master), settled on a sub-block boundary.
fn tone_at(rate: u32) -> (Engine, EngineControl) {
    let (mut engine, mut control) = engine(EngineConfig::new(rate, 120.0, 0.0), None);
    for (slot, kind) in [(0, Kind::VCO), (1, Kind::OUT)] {
        let mut knobs = [0.0; MAX_KNOBS];
        for (target, value) in knobs.iter_mut().zip(kind.spec().defaults()) {
            *target = value;
        }
        control.send(Command::Insert {
            slot,
            tag: u32::try_from(slot).unwrap() + 1,
            kind,
            module: build(kind, rate as f32),
            knobs,
        });
    }
    let mut table = CableTable::new();
    table.set(1, 0, Some(Route::new(0, 0, 1.0, 0, 1)));
    control.send(Command::Cables(Box::new(table)));
    control.send(Command::Order(Box::new(processing_order(
        &[0, 1],
        &[(0, 1)],
    ))));
    let mut settle = vec![0.0; 480 * 2];
    engine.render(&mut settle, 2);
    (engine, control)
}

fn tone() -> (Engine, EngineControl) {
    tone_at(RATE)
}

/// Render `frames` stereo frames, appending them to `heard`.
fn play(engine: &mut Engine, frames: usize, heard: &mut Vec<f32>) {
    let mut buffer = vec![0.0; frames * 2];
    engine.render(&mut buffer, 2);
    heard.extend_from_slice(&buffer);
}

/// Every sample in the WAV at `path`, after checking it is 32-bit float
/// stereo at `rate`.
fn samples(path: &Path, rate: u32) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, rate);
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    reader.samples::<f32>().map(Result::unwrap).collect()
}

fn same(got: &[f32], want: &[f32]) {
    assert_eq!(got.len(), want.len());
    for (index, (got, want)) in got.iter().zip(want).enumerate() {
        assert_eq!(got.to_bits(), want.to_bits(), "sample {index}");
    }
}

/// Wait for the writer to have written `frames` frames at `rate`.
fn caught_up(recorder: &Recorder, frames: u64, rate: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while recorder
        .status()
        .is_some_and(|status| status.seconds < frames as f64 / f64::from(rate))
    {
        assert!(Instant::now() < deadline, "the writer never caught up");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Wait for a recording to end on its own.
fn ended(recorder: &mut Recorder) -> Finished {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(finished) = recorder.poll() {
            return finished;
        }
        assert!(Instant::now() < deadline, "the recording never ended");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_writer_makes_a_float_stereo_wav_of_what_the_wall_played() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, mut control) = tone();
    let mut recorder = Recorder::new(dir.path().join("made"), &mut control);
    // Played before the recording: not in it.
    play(&mut engine, 960, &mut Vec::new());
    assert!(recorder.poll().is_none(), "nothing under way");
    let path = recorder.start("Tom", true).unwrap();
    assert!(control.shared().recording());
    assert!(
        path.starts_with(dir.path().join("made")),
        "the directory is made"
    );
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("wall-")
            && path.extension().is_some_and(|extension| extension == "wav")
            && name.len() == "wall-2026-09-27-203001.wav".len(),
        "{name}"
    );
    let mut heard = Vec::new();
    for _ in 0..10 {
        play(&mut engine, 480, &mut heard);
    }
    caught_up(&recorder, 4_800, RATE);
    let status = recorder.status().unwrap();
    assert_eq!(status.path, path);
    assert_eq!(status.seat, "Tom");
    assert_eq!(status.sample_rate, RATE);
    assert!((status.seconds - 0.1).abs() < 1e-9, "{}", status.seconds);
    assert!(recorder.poll().is_none(), "still under way");
    let finished = recorder.stop().unwrap();
    assert!(!control.shared().recording());
    assert_eq!(finished.ending, Ending::Asked);
    assert_eq!(finished.path, path);
    assert_eq!(finished.seat, "Tom");
    assert_eq!(finished.dropped, 0);
    assert!((finished.seconds - 0.1).abs() < 1e-9);
    assert!(recorder.status().is_none());
    let in_file = samples(&path, RATE);
    assert!(in_file.iter().any(|s| s.abs() > 0.3), "it is the tone");
    same(&in_file, &heard);
    assert_eq!(recorder.stop(), None, "nothing more to stop");
}

#[test]
fn a_new_recording_starts_from_now_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, mut control) = tone();
    let mut recorder = Recorder::new(dir.path().to_path_buf(), &mut control);
    let first = recorder.start("Tom", true).unwrap();
    let twice = recorder.start("Tom", true).unwrap_err();
    assert!(twice.contains("already recording"), "{twice}");
    let mut heard = Vec::new();
    play(&mut engine, 960, &mut heard);
    caught_up(&recorder, 960, RATE);
    recorder.stop().unwrap();
    // Left in the ring while nobody records: not in the next one.
    control.shared().set_recording(true);
    play(&mut engine, 480, &mut Vec::new());
    control.shared().set_recording(false);
    let second = recorder.start("Waffles", true).unwrap();
    let mut later = Vec::new();
    play(&mut engine, 480, &mut later);
    caught_up(&recorder, 480, RATE);
    let finished = recorder.stop().unwrap();
    assert_eq!(finished.seat, "Waffles");
    assert_ne!(first, second);
    same(&samples(&first, RATE), &heard);
    same(&samples(&second, RATE), &later);
    // The same start time's names go on counting.
    let (taken, _) = new_file(dir.path(), "2026-09-27-203001").unwrap();
    let (next, _) = new_file(dir.path(), "2026-09-27-203001").unwrap();
    let (third, _) = new_file(dir.path(), "2026-09-27-203001").unwrap();
    assert!(taken.ends_with("wall-2026-09-27-203001.wav"));
    assert!(next.ends_with("wall-2026-09-27-203001-2.wav"));
    assert!(third.ends_with("wall-2026-09-27-203001-3.wav"));
}

#[test]
fn a_full_file_carries_on_into_the_next_without_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, mut control) = tone();
    let mut recorder = Recorder::new(dir.path().to_path_buf(), &mut control);
    recorder.set_limit_frames(1_000);
    let first = recorder.start("Tom", true).unwrap();
    let mut heard = Vec::new();
    play(&mut engine, 1_600, &mut heard);
    let finished = ended(&mut recorder);
    assert_eq!(finished.ending, Ending::Full);
    assert!((finished.seconds - 1_000.0 / f64::from(RATE)).abs() < 1e-9);
    assert!(
        control.shared().recording(),
        "the engine records on for the next file"
    );
    let second = recorder.start("Tom", false).unwrap();
    play(&mut engine, 400, &mut heard);
    caught_up(&recorder, 1_000, RATE);
    recorder.stop().unwrap();
    let mut in_files = samples(&first, RATE);
    assert_eq!(in_files.len(), 1_000 * 2, "the first file holds its limit");
    in_files.extend(samples(&second, RATE));
    same(&in_files, &heard);
}

#[test]
fn a_recording_that_cannot_start_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"a file where the directory should be").unwrap();
    let (_engine, mut control) = tone();
    let mut recorder = Recorder::new(blocked.join("recordings"), &mut control);
    let why = recorder.start("Tom", true).unwrap_err();
    assert!(why.contains("cannot be made"), "{why}");
    assert!(!control.shared().recording());
    assert!(recorder.status().is_none());
    // An engine whose ring was taken already cannot be recorded.
    let (_engine, mut taken) = tone();
    assert!(taken.take_record().is_some());
    let mut recorder = Recorder::new(dir.path().to_path_buf(), &mut taken);
    let why = recorder.start("Tom", true).unwrap_err();
    assert!(why.contains("no record ring"), "{why}");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "only the blocking file"
    );
}

#[test]
fn a_full_file_that_cannot_carry_on_stops_the_engine_recording() {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("recordings");
    let (mut engine, mut control) = tone();
    let mut recorder = Recorder::new(files.clone(), &mut control);
    recorder.set_limit_frames(1_000);
    recorder.start("Tom", true).unwrap();
    play(&mut engine, 1_600, &mut Vec::new());
    assert_eq!(ended(&mut recorder).ending, Ending::Full);
    assert!(control.shared().recording());
    // The directory goes, and a file takes its place.
    std::fs::remove_dir_all(&files).unwrap();
    std::fs::write(&files, b"no longer a directory").unwrap();
    let why = recorder.start("Tom", false).unwrap_err();
    assert!(why.contains("cannot be made"), "{why}");
    assert!(!control.shared().recording(), "the engine stops recording");
}

#[test]
fn a_rebuilt_engine_ends_the_file_and_takes_the_new_ring() {
    let dir = tempfile::tempdir().unwrap();
    let (mut old, mut old_control) = tone();
    let mut recorder = Recorder::new(dir.path().to_path_buf(), &mut old_control);
    let path = recorder.start("Tom", true).unwrap();
    let mut heard = Vec::new();
    play(&mut old, 960, &mut heard);
    caught_up(&recorder, 960, RATE);
    // The old engine has stopped by the time the wall moves on.
    drop(old);
    let (mut new, mut new_control) = tone_at(44_100);
    let finished = recorder.attach(&mut new_control).unwrap();
    assert_eq!(finished.ending, Ending::Asked);
    assert_eq!(finished.path, path);
    assert!(!old_control.shared().recording());
    same(&samples(&path, RATE), &heard);
    assert_eq!(recorder.sample_rate(), 44_100);
    let carried = recorder.start("Tom", true).unwrap();
    assert!(new_control.shared().recording());
    // Whole sub-blocks: the ring takes the engine's master a sub-block at
    // a time, so it can run up to one ahead of what the device has had.
    let mut later = Vec::new();
    play(&mut new, 448, &mut later);
    caught_up(&recorder, 448, 44_100);
    recorder.stop().unwrap();
    same(&samples(&carried, 44_100), &later);
}

#[test]
fn a_recorder_let_go_finishes_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, mut control) = tone();
    let mut recorder = Recorder::new(dir.path().to_path_buf(), &mut control);
    let path = recorder.start("Tom", true).unwrap();
    let mut heard = Vec::new();
    play(&mut engine, 960, &mut heard);
    caught_up(&recorder, 960, RATE);
    drop(recorder);
    assert!(!control.shared().recording());
    same(&samples(&path, RATE), &heard);
}
