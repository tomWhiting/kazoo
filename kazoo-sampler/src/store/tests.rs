//! Store tests: round trips, names, traversal, caps and damaged files.

use std::os::unix::fs::PermissionsExt;

use super::*;

fn store(limits: StoreLimits) -> (tempfile::TempDir, SampleStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SampleStore::open(dir.path().join("samples"), limits).unwrap();
    (dir, store)
}

fn name(text: &str) -> SampleName {
    SampleName::new(text).unwrap()
}

fn ramp(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let left: Vec<f32> = (0..frames).map(|n| (n as f32 * 0.01).sin() * 0.7).collect();
    let right: Vec<f32> = left.iter().map(|x| -x * 0.5).collect();
    (left, right)
}

/// Write a WAV of any format straight into the store's directory.
fn write_raw(store: &SampleStore, file: &str, spec: hound::WavSpec, samples: &[i32]) {
    let mut writer = hound::WavWriter::create(store.dir().join(file), spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn save_list_load_round_trip() {
    let (_dir, store) = store(StoreLimits::default());
    let (left, right) = ramp(4_800);
    let saved = store
        .save(&name("vox 1"), 48_000, &left, &right, Overwrite::Refuse)
        .unwrap();
    assert_eq!(saved.frames, 4_800);
    assert_eq!(
        (saved.rate, saved.channels, saved.bits, saved.float),
        (48_000, 2, 32, true)
    );

    let listing = store.list().unwrap();
    assert_eq!(listing.samples, vec![saved]);
    assert!(listing.unreadable.is_empty());

    let loaded = store.load(&name("vox 1"), 48_000).unwrap();
    assert_eq!(loaded.left(), left.as_slice());
    assert_eq!(loaded.right(), right.as_slice());
    assert_eq!(store.loaded_bytes(), loaded.bytes());
    drop(loaded);
    assert_eq!(store.loaded_bytes(), 0);
}

#[test]
fn the_directory_is_private() {
    let (_dir, store) = store(StoreLimits::default());
    let mode = fs::metadata(store.dir()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    // An existing, too-open directory is tightened.
    fs::set_permissions(store.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let again = SampleStore::open(store.dir(), StoreLimits::default()).unwrap();
    let mode = fs::metadata(again.dir()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn overwrite_is_refused_unless_asked_for() {
    let (_dir, store) = store(StoreLimits::default());
    let kick = name("kick");
    store
        .save(&kick, 48_000, &[0.5], &[0.5], Overwrite::Refuse)
        .unwrap();
    let refused = store.save(&kick, 48_000, &[0.1, 0.1], &[0.1, 0.1], Overwrite::Refuse);
    assert!(matches!(refused, Err(Error::Exists { .. })));
    assert_eq!(store.info(&kick).unwrap().frames, 1);
    store
        .save(&kick, 48_000, &[0.1, 0.1], &[0.1, 0.1], Overwrite::Replace)
        .unwrap();
    assert_eq!(store.info(&kick).unwrap().frames, 2);
    // No temporary files are left either way.
    let names: Vec<String> = fs::read_dir(store.dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["kick.wav".to_owned()]);
}

#[test]
fn delete_removes_and_missing_is_clear() {
    let (_dir, store) = store(StoreLimits::default());
    let snare = name("snare");
    store
        .save(&snare, 48_000, &[0.5], &[0.5], Overwrite::Refuse)
        .unwrap();
    store.delete(&snare).unwrap();
    assert!(matches!(store.delete(&snare), Err(Error::NotFound { .. })));
    assert!(matches!(
        store.load(&snare, 48_000),
        Err(Error::NotFound { .. })
    ));
    assert!(store.list().unwrap().samples.is_empty());
}

#[test]
fn traversal_is_impossible() {
    let (dir, store) = store(StoreLimits::default());
    for text in ["../escape", "..", "/etc/passwd", "a/../../b", ".hidden"] {
        assert!(matches!(SampleName::new(text), Err(Error::BadName { .. })));
    }
    // A symbolic link in the store is never followed, for reading or
    // writing.
    let outside = dir.path().join("outside.wav");
    let mut writer = hound::WavWriter::create(&outside, wav::stereo_float(48_000)).unwrap();
    writer.write_sample(0.5f32).unwrap();
    writer.write_sample(0.5f32).unwrap();
    writer.finalize().unwrap();
    std::os::unix::fs::symlink(&outside, store.dir().join("link.wav")).unwrap();
    let link = name("link");
    assert!(matches!(
        store.load(&link, 48_000),
        Err(Error::NotAFile { .. })
    ));
    assert!(matches!(
        store.save(&link, 48_000, &[0.1], &[0.1], Overwrite::Replace),
        Err(Error::NotAFile { .. })
    ));
    assert!(matches!(store.delete(&link), Err(Error::NotAFile { .. })));
    assert_eq!(store.list().unwrap().unreadable.len(), 1);
    assert!(outside.exists());
}

#[test]
fn length_cap_holds_for_saving_and_loading() {
    let limits = StoreLimits {
        max_seconds: 0.5,
        ..StoreLimits::default()
    };
    let (_dir, store) = store(limits);
    let (left, right) = ramp(48_000);
    let refused = store.save(&name("long"), 48_000, &left, &right, Overwrite::Refuse);
    assert!(matches!(refused, Err(Error::TooLong { .. })), "{refused:?}");
    // A long file put there some other way is refused on load too.
    write_raw(
        &store,
        "long.wav",
        hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
        &[0; 8_000],
    );
    assert!(matches!(
        store.load(&name("long"), 48_000),
        Err(Error::TooLong { .. })
    ));
    assert_eq!(store.loaded_bytes(), 0);
}

#[test]
fn memory_cap_holds_across_loaded_samples() {
    let frames = 48_000;
    let limits = StoreLimits {
        max_loaded_bytes: SampleData::bytes_for(frames) * 3 / 2,
        ..StoreLimits::default()
    };
    let (_dir, store) = store(limits);
    let (left, right) = ramp(frames);
    store
        .save(&name("a"), 48_000, &left, &right, Overwrite::Refuse)
        .unwrap();
    store
        .save(&name("b"), 48_000, &left, &right, Overwrite::Refuse)
        .unwrap();
    let first = store.load(&name("a"), 48_000).unwrap();
    let second = store.load(&name("b"), 48_000);
    assert!(
        matches!(second, Err(Error::MemoryFull { .. })),
        "{second:?}"
    );
    drop(first);
    assert!(store.load(&name("b"), 48_000).is_ok());
}

#[test]
fn damaged_files_give_clear_errors() {
    let (_dir, store) = store(StoreLimits::default());
    fs::write(
        store.dir().join("junk.wav"),
        b"this is not a wav file at all",
    )
    .unwrap();
    let junk = store.load(&name("junk"), 48_000);
    assert!(matches!(junk, Err(Error::Corrupt { .. })), "{junk:?}");

    // A real file cut short.
    let (left, right) = ramp(10_000);
    store
        .save(&name("cut"), 48_000, &left, &right, Overwrite::Refuse)
        .unwrap();
    let path = store.dir().join("cut.wav");
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    let cut = store.load(&name("cut"), 48_000);
    assert!(matches!(cut, Err(Error::Corrupt { .. })), "{cut:?}");

    fs::write(store.dir().join("notes.txt"), b"hi").unwrap();
    fs::write(store.dir().join("bad name!.wav"), b"x").unwrap();
    let listing = store.list().unwrap();
    assert!(listing.samples.is_empty());
    let files: Vec<&str> = listing.unreadable.iter().map(|u| u.file.as_str()).collect();
    assert_eq!(files.len(), 4);
    for file in ["junk.wav", "cut.wav", "notes.txt", "bad name!.wav"] {
        assert!(files.contains(&file), "{file} missing from {files:?}");
    }
    assert_eq!(store.loaded_bytes(), 0);
}

#[test]
fn any_format_loads_as_stereo_at_the_engine_rate() {
    let (_dir, store) = store(StoreLimits::default());
    // 16-bit mono at 44.1 kHz.
    let mono: Vec<i32> = (0..4_410)
        .map(|n| if n % 2 == 0 { 16_384 } else { -16_384 })
        .collect();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    write_raw(&store, "mono16.wav", spec, &mono);
    let loaded = store.load(&name("mono16"), 48_000).unwrap();
    assert_eq!(loaded.frames(), 4_800);
    assert_eq!(loaded.rate(), 48_000);
    assert_eq!(loaded.left(), loaded.right());

    // 24-bit, four channels: the first two are kept.
    let spec = hound::WavSpec {
        channels: 4,
        sample_rate: 48_000,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let quad: Vec<i32> = (0..100)
        .flat_map(|_| [4_194_304, -4_194_304, 8_000_000, 0])
        .collect();
    write_raw(&store, "quad24.wav", spec, &quad);
    let loaded = store.load(&name("quad24"), 48_000).unwrap();
    assert_eq!(loaded.frames(), 100);
    assert!(loaded.left().iter().all(|&x| (x - 0.5).abs() < 1e-6));
    assert!(loaded.right().iter().all(|&x| (x + 0.5).abs() < 1e-6));
    let info = store.info(&name("quad24")).unwrap();
    assert_eq!((info.channels, info.bits, info.float), (4, 24, false));
}

#[test]
fn bad_audio_and_rates_are_refused() {
    let (_dir, store) = store(StoreLimits::default());
    let n = name("x");
    assert!(store.save(&n, 48_000, &[], &[], Overwrite::Refuse).is_err());
    assert!(
        store
            .save(&n, 48_000, &[0.0], &[], Overwrite::Refuse)
            .is_err()
    );
    assert!(
        store
            .save(&n, 0, &[0.0], &[0.0], Overwrite::Refuse)
            .is_err()
    );
    store
        .save(&n, 48_000, &[f32::NAN], &[f32::INFINITY], Overwrite::Refuse)
        .unwrap();
    let loaded = store.load(&n, 48_000).unwrap();
    assert_eq!((loaded.left()[0], loaded.right()[0]), (0.0, 0.0));
    assert!(store.load(&n, 5).is_err());
    let bad = StoreLimits {
        max_seconds: f64::NAN,
        ..StoreLimits::default()
    };
    assert!(SampleStore::open(store.dir(), bad).is_err());
}

#[test]
fn stale_temporaries_are_cleared_and_fresh_ones_kept() {
    let (_dir, store) = store(StoreLimits::default());
    let stale = store.dir().join(".old.1.1.tmp");
    let fresh = store.dir().join(".new.1.2.tmp");
    fs::write(&stale, b"x").unwrap();
    fs::write(&fresh, b"x").unwrap();
    let old = SystemTime::now() - Duration::from_secs(7_200);
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    SampleStore::open(store.dir(), StoreLimits::default()).unwrap();
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert!(store.list().unwrap().samples.is_empty());
}

#[test]
fn the_default_place_is_under_home() {
    let dir = SampleStore::default_dir().unwrap();
    assert!(dir.ends_with(".kazoo/wall/samples"));
}
