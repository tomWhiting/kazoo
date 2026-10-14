use super::*;
use crate::callback::AudioCallback;
use crate::engine::MixerEngine;
use crate::shared::{DESK_CHANNELS, SharedState};
use crate::test_support::{assert_f64_eq, assert_float_eq};
use std::fs;
use std::sync::Arc;
use std::time::Duration;

fn directory() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/kz-logs")
        .join(format!("tempo-{}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_restarted_desk_keeps_its_tempo() {
    let dir = directory();
    let path = dir.join("tempo");
    fs::write(&path, "96\n").unwrap();
    let store = TempoStore::open(path_for(Some(dir.as_os_str()), None).unwrap());
    let (shared, engine) = crate::run::desk_transport(store.bpm, 48_000).unwrap();
    assert_float_eq(shared.transport().bpm, 96.0);
    assert_f64_eq(crate::hub::tempo::initial_song(&shared).bpm, 96.0);
    assert_f64_eq(engine.song().bpm, 96.0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn no_file_is_the_default() {
    let dir = directory();
    let store = TempoStore::open(dir.join("missing"));
    assert_float_eq(store.bpm, DEFAULT_BPM);
    assert!(store.warning.is_none());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_bad_file_is_the_default_and_said() {
    let dir = directory();
    let path = dir.join("tempo");
    for (contents, reason) in [
        ("abc", "decimal"),
        ("NaN", "finite"),
        ("0", "range"),
        ("9999", "range"),
    ] {
        fs::write(&path, contents).unwrap();
        let store = TempoStore::open(path.clone());
        assert_float_eq(store.bpm, DEFAULT_BPM);
        let warning = store.warning.as_ref().expect("bad tempo must be reported");
        assert!(warning.contains(path.to_str().unwrap()), "{warning}");
        assert!(warning.contains(reason), "{warning}");
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_tempo_is_written_once_it_settles() {
    let dir = directory();
    let path = dir.join("tempo");
    let mut store = TempoStore::open(path.clone());
    let shared = SharedState::new();
    let now = Instant::now();
    let mut writes = 0;
    for (step, bpm) in [100.0, 96.5, 90.0].into_iter().enumerate() {
        shared.set_tempo(bpm);
        writes += usize::from(store.poll(
            shared.transport(),
            now + Duration::from_millis(step as u64 * 100),
        ));
    }
    writes += usize::from(store.poll(shared.transport(), now + Duration::from_millis(699)));
    assert_eq!(writes, 0);
    writes += usize::from(store.poll(shared.transport(), now + Duration::from_millis(700)));
    writes += usize::from(store.poll(shared.transport(), now + Duration::from_secs(1)));
    assert_eq!(writes, 1);
    assert_eq!(fs::read_to_string(path).unwrap(), "90\n");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_callback_never_writes() {
    let source = include_str!("../callback.rs");
    assert!(!source.contains("std::fs"));
    assert!(!source.contains("tempo_store"));
    let shared = Arc::new(SharedState::new());
    let mut callback = AudioCallback::new(
        MixerEngine::new(DESK_CHANNELS, 48_000).unwrap(),
        Arc::clone(&shared),
        2,
    );
    let mut data = [0.0_f32; 512];
    shared.set_tempo(96.0);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        callback.process(&mut data);
        callback.process(&mut data);
        callback.process(&mut data);
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert_f64_eq(callback.engine().song().bpm, 96.0);
    assert!(data.iter().all(|sample| sample.is_finite()));
}

#[test]
fn the_store_uses_the_configured_directory_or_home() {
    assert_eq!(
        path_for(Some(OsStr::new("desk")), Some(OsStr::new("home"))).unwrap(),
        PathBuf::from("desk/tempo")
    );
    assert_eq!(
        path_for(Some(OsStr::new("")), Some(OsStr::new("home"))).unwrap(),
        PathBuf::from("home/.kazoo/mix/tempo")
    );
    assert!(path_for(None, None).is_err());
}

#[test]
fn a_failed_write_is_reported_and_retried_on_the_next_change() {
    let dir = directory();
    let path = dir.join("blocked/tempo");
    fs::write(dir.join("blocked"), "a file").unwrap();
    let mut store = TempoStore::open(path.clone());
    let shared = SharedState::new();
    let now = Instant::now();
    shared.set_tempo(96.0);
    assert!(!store.poll(shared.transport(), now));
    assert!(store.poll(shared.transport(), now + SETTLE));
    assert!(
        store
            .warning
            .as_ref()
            .unwrap()
            .contains(path.to_str().unwrap())
    );
    fs::remove_file(dir.join("blocked")).unwrap();
    assert!(!store.poll(shared.transport(), now + SETTLE * 2));
    shared.set_tempo(96.0);
    assert!(!store.poll(shared.transport(), now + SETTLE * 3));
    assert!(store.poll(shared.transport(), now + SETTLE * 4));
    assert!(store.warning.is_none());
    assert_eq!(fs::read_to_string(path).unwrap(), "96\n");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn readers_only_see_complete_tempo_files() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    let dir = directory();
    let path = dir.join("tempo");
    write(&path, 120.0).unwrap();
    let finished = AtomicBool::new(false);
    let (ready, started) = mpsc::sync_channel(0);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let initial = fs::read_to_string(&path).unwrap();
            assert_eq!(initial, "120\n");
            ready.send(()).unwrap();
            while !finished.load(Ordering::Acquire) {
                let contents = fs::read_to_string(&path).unwrap();
                assert!(contents.ends_with('\n'));
                let bpm = contents.trim().parse::<f32>().unwrap();
                assert!((MIN_BPM..=MAX_BPM).contains(&bpm));
                assert_eq!(contents.lines().count(), 1);
            }
        });
        started.recv().unwrap();
        for bpm in 90..110 {
            write(&path, bpm as f32).unwrap();
        }
        finished.store(true, Ordering::Release);
        reader.join().unwrap();
    });
    assert_eq!(fs::read_to_string(path).unwrap(), "109\n");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn play_changes_do_not_delay_a_pending_tempo_save() {
    let dir = directory();
    let mut store = TempoStore::open(dir.join("tempo"));
    let shared = SharedState::new();
    let now = Instant::now();
    shared.set_tempo(96.0);
    assert!(!store.poll(shared.transport(), now));
    shared.set_playing(true);
    assert!(!store.poll(shared.transport(), now + Duration::from_millis(400)));
    assert!(store.poll(shared.transport(), now + SETTLE));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unreadable_file_is_reported_without_replacing_it() {
    let dir = directory();
    let path = dir.join("tempo");
    fs::create_dir(&path).unwrap();
    let store = TempoStore::open(path.clone());
    assert_float_eq(store.bpm, DEFAULT_BPM);
    let warning = store.warning.unwrap();
    assert!(warning.contains(path.to_str().unwrap()));
    assert!(warning.contains("could not read"));
    assert!(path.is_dir());
    fs::remove_dir_all(dir).unwrap();
}
