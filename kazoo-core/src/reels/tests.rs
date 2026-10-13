use crate::reels::*;
use std::collections::BTreeSet;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::time::{Duration, SystemTime};

type TestResult = Result<(), Box<dyn Error>>;

fn make_old(path: &std::path::Path) -> io::Result<()> {
    File::open(path)?.set_modified(SystemTime::now() - Duration::from_secs(2 * 86_400))
}

#[test]
fn a_reel_round_trips_bit_for_bit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let mut samples: Vec<f32> = (0_u16..2_000).map(|n| f32::from(n) / 2_000.0).collect();
    samples[0] = -0.0;
    samples[1] = f32::from_bits(1);
    samples[2] = f32::MAX;
    let id = store.put(48_000, &samples)?;
    let reel = store.get(&id)?;
    assert_eq!(reel.rate, 48_000);
    assert_eq!(reel.frames(), 1_000);
    assert_eq!(
        reel.samples.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
        samples.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn the_id_is_pinned() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = reel_id(48_000, &[0.0, 0.0, 1.0, -1.0]);
    assert_eq!(
        id.to_string(),
        "b92d8a6bcb9815ef9c88f1b2f72d03b7159e55d2df73546f10525dcb20634545"
    );
    assert_eq!(id.short(), "b92d8a6b");
    assert_eq!(id.to_string().parse::<ReelId>()?, id);
    assert_ne!(id, reel_id(44_100, &[0.0, 0.0, 1.0, -1.0]));
    assert_eq!(store.put(48_000, &[0.0, 0.0, 1.0, -1.0])?, id);
    Ok(())
}

#[test]
fn the_same_samples_are_stored_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = store.put(48_000, &[0.25, -0.25])?;
    let before = fs::read(store.path(&id))?;
    assert_eq!(before.len(), 68 + 8);
    assert_eq!(store.put(48_000, &[0.25, -0.25])?, id);
    assert_eq!(fs::read(store.path(&id))?, before);
    let usage = store.usage()?;
    assert_eq!(usage.reels, 1);
    assert_eq!(usage.bytes, 76);
    Ok(())
}

#[test]
fn a_deduped_reel_gets_fresh_grace() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = store.put(48_000, &[0.25, -0.25])?;
    make_old(&store.path(&id))?;
    assert_eq!(store.put(48_000, &[0.25, -0.25])?, id);
    let now = SystemTime::now();
    let age = now.duration_since(fs::metadata(store.path(&id))?.modified()?)?;
    assert!(age < Duration::from_secs(60));
    let swept = store.sweep(SWEEP_GRACE, now)?;
    assert_eq!(swept.deleted, 0);
    assert_eq!(swept.kept, 1);
    assert!(store.contains(&id));
    Ok(())
}

#[test]
fn a_damaged_reel_is_refused_and_moved_aside() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = store.put(48_000, &[0.25, -0.25])?;
    let mut bytes = fs::read(store.path(&id))?;
    bytes[68] ^= 1;
    fs::write(store.path(&id), bytes)?;
    assert!(matches!(store.get(&id), Err(ReelError::Corrupt(found)) if found == id));
    let entries = fs::read_dir(dir.path())?.collect::<io::Result<Vec<_>>>()?;
    assert!(entries.iter().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with(&format!("{id}.corrupt-")) && name.ends_with(".wav")
    }));
    assert!(matches!(store.get(&id), Err(ReelError::Missing(found)) if found == id));
    Ok(())
}

#[test]
fn a_header_claiming_too_many_frames_is_refused_before_reading() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = reel_id(48_000, &[]);
    let frames = MAX_REEL_FRAMES + 1;
    let data_size = u32::try_from(frames * 8)?;
    let mut header = Vec::new();
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36_u32 + data_size).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16_u32.to_le_bytes());
    header.extend_from_slice(&3_u16.to_le_bytes());
    header.extend_from_slice(&2_u16.to_le_bytes());
    header.extend_from_slice(&48_000_u32.to_le_bytes());
    header.extend_from_slice(&384_000_u32.to_le_bytes());
    header.extend_from_slice(&8_u16.to_le_bytes());
    header.extend_from_slice(&32_u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_size.to_le_bytes());
    assert_eq!(header.len(), 44);
    fs::write(store.path(&id), header)?;
    assert!(matches!(store.get(&id), Err(ReelError::TooLong { frames: found }) if found == frames));
    Ok(())
}

#[test]
fn non_finite_samples_and_mono_and_bad_rates_are_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(
            store.put(48_000, &[0.0, sample]),
            Err(ReelError::NotFinite { at: 1 })
        ));
    }
    assert!(matches!(
        store.put(48_000, &[0.0]),
        Err(ReelError::NotStereo)
    ));
    for rate in [0, 7_999, 384_001, u32::MAX] {
        assert!(
            matches!(store.put(rate, &[0.0, 0.0]), Err(ReelError::BadRate(found)) if found == rate)
        );
    }
    for rate in [8_000, 384_000] {
        let id = store.put(rate, &[])?;
        assert_eq!(store.get(&id)?.rate, rate);
    }
    let id = reel_id(48_000, &[]);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    hound::WavWriter::create(store.path(&id), spec)?.finalize()?;
    assert!(matches!(store.get(&id), Err(ReelError::WrongFormat(_))));
    Ok(())
}

#[test]
fn a_pin_list_cut_short_stops_the_sweep() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let a = store.put(48_000, &[0.25, -0.25])?;
    let b = store.put(48_000, &[0.5, -0.5])?;
    store.pin("wall", &BTreeSet::from([a]))?;
    fs::write(dir.path().join("pins/session-jam.txt"), format!("{a}\n"))?;
    make_old(&store.path(&b))?;
    assert!(
        matches!(store.sweep(SWEEP_GRACE, SystemTime::now()), Err(ReelError::PinUnreadable { holder }) if holder == "session-jam")
    );
    assert!(store.contains(&b));
    assert!(
        matches!(store.pinned_elsewhere("wall"), Err(ReelError::PinUnreadable { holder }) if holder == "session-jam")
    );
    fs::write(dir.path().join("pins/session-jam.txt"), "end\nextra\n")?;
    assert!(matches!(
        store.sweep(SWEEP_GRACE, SystemTime::now()),
        Err(ReelError::PinUnreadable { .. })
    ));
    assert!(store.contains(&b));
    Ok(())
}

#[test]
fn the_sweep_keeps_what_is_pinned_and_what_is_young() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let a = store.put(48_000, &[0.25, -0.25])?;
    let b = store.put(48_000, &[0.5, -0.5])?;
    let c = store.put(48_000, &[0.75, -0.75])?;
    make_old(&store.path(&a))?;
    make_old(&store.path(&b))?;
    let peaks = store.path(&b).with_extension("peaks");
    fs::write(&peaks, b"peaks")?;
    store.pin("wall", &BTreeSet::from([a]))?;
    store.pin("session-jam", &BTreeSet::from([a, c]))?;
    assert_eq!(store.pinned_elsewhere("wall")?, BTreeSet::from([a, c]));
    store.unpin("session-jam")?;
    store.unpin("session-jam")?;
    let swept = store.sweep(SWEEP_GRACE, SystemTime::now())?;
    assert_eq!(swept.deleted, 1);
    assert_eq!(swept.bytes, 76);
    assert_eq!(swept.kept, 2);
    assert!(store.contains(&a));
    assert!(!store.contains(&b));
    assert!(store.contains(&c));
    assert!(!peaks.exists());
    let c_peaks = store.path(&c).with_extension("peaks");
    fs::write(&c_peaks, b"peaks")?;
    assert!(store.remove(&c)?);
    assert!(!c_peaks.exists());
    assert!(!store.remove(&c)?);
    Ok(())
}

#[test]
fn stale_temporary_files_are_swept() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let old = dir.path().join("take.wav.tmp-1-0");
    let young = dir.path().join("take.wav.tmp-1-1");
    let old_pin = dir.path().join("pins/wall.txt.tmp-1-0");
    for path in [&old, &young, &old_pin] {
        File::create(path)?.write_all(b"partial")?;
    }
    make_old(&old)?;
    make_old(&old_pin)?;
    store.sweep(SWEEP_GRACE, SystemTime::now())?;
    assert!(!old.exists());
    assert!(!old_pin.exists());
    assert!(young.exists());
    Ok(())
}

#[test]
fn holders_are_checked() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    for holder in [
        "../x",
        "",
        "Wall",
        "a_b",
        "é",
        "1234567890123456789012345678901234567890123456789",
    ] {
        assert_eq!(
            store
                .pin(holder, &BTreeSet::new())
                .expect_err("invalid holder")
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            store.unpin(holder).expect_err("invalid holder").kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(
            matches!(store.pinned_elsewhere(holder), Err(ReelError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
        );
    }
    for text in [
        "",
        "abcd",
        "B92D8A6BCB9815EF9C88F1B2F72D03B7159E55D2DF73546F10525DCB20634545",
        "../../../../../../../../../../../../../../../../../../../../../../..",
    ] {
        assert!(matches!(text.parse::<ReelId>(), Err(ReelError::BadId)));
    }
    store.pin("wall", &BTreeSet::new())?;
    assert_eq!(
        fs::read_to_string(dir.path().join("pins/wall.txt"))?,
        "end\n"
    );
    Ok(())
}


#[test]
fn an_existing_directory_is_made_private() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let reels = dir.path().join("reels");
    fs::create_dir_all(reels.join("pins"))?;
    fs::set_permissions(&reels, fs::Permissions::from_mode(0o755))?;
    fs::set_permissions(reels.join("pins"), fs::Permissions::from_mode(0o755))?;
    ReelStore::open(&reels)?;
    assert_eq!(fs::metadata(&reels)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(reels.join("pins"))?.permissions().mode() & 0o777,
        0o700
    );
    Ok(())
}

#[test]
fn a_reel_cut_short_is_the_wrong_format_not_a_disk_fault() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = store.put(48_000, &[0.25, -0.25, 0.5, -0.5])?;
    let bytes = fs::read(store.path(&id))?;
    fs::write(store.path(&id), &bytes[..bytes.len() - 4])?;
    let error = store.get(&id).expect_err("a cut-short reel must be refused");
    assert!(matches!(error, ReelError::WrongFormat(_)), "{error:?}");
    Ok(())
}

#[test]
fn an_unreadable_pin_list_names_what_it_stops() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    fs::write(dir.path().join("pins/session-jam.txt"), "")?;
    let error = store
        .pinned_elsewhere("wall")
        .expect_err("a list without end is unreadable");
    let text = error.to_string();
    assert!(!text.contains("swept"), "{text}");
    assert!(text.contains("session-jam"), "{text}");
    assert!(text.contains("no reel may be deleted"), "{text}");
    Ok(())
}

#[test]
fn a_damaged_reel_says_where_it_went() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let id = store.put(48_000, &[0.25, -0.25])?;
    let text = ReelError::Corrupt(id).to_string();
    assert!(!text.contains('<'), "{text}");
    assert!(text.contains(&format!("{id}.corrupt-")), "{text}");
    assert!(text.contains(&id.short()), "{text}");
    Ok(())
}

#[test]
fn malformed_files_are_errors_never_panics() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = ReelStore::open(dir.path())?;
    let samples: Vec<f32> = (0_u16..16).map(|n| f32::from(n) / 16.0).collect();
    let id = store.put(48_000, &samples)?;
    let good = fs::read(store.path(&id))?;
    for len in 0..good.len() {
        fs::write(store.path(&id), &good[..len])?;
        assert!(store.get(&id).is_err(), "cut to {len} bytes");
    }
    for at in 0..68 {
        for value in [0_u8, 1, 0x7f, 0x80, 0xff] {
            let mut bytes = good.clone();
            bytes[at] = value;
            fs::write(store.path(&id), &bytes)?;
            match store.get(&id) {
                Ok(reel) => assert_eq!(reel_id(reel.rate, &reel.samples), id),
                Err(error) => assert!(!error.to_string().is_empty()),
            }
        }
    }
    Ok(())
}
