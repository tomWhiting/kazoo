use super::*;

#[test]
fn a_stopped_hub_wins_over_a_tempo_warning() {
    assert_eq!(
        status_line(true, Some("tempo failed"), || (
            "hub stopped".to_string(),
            true
        )),
        ("hub stopped".to_string(), true),
    );
    assert_eq!(
        status_line(false, Some("tempo failed"), || (
            "hub notice".to_string(),
            false
        )),
        ("tempo failed".to_string(), true),
    );
}

#[test]
fn startup_warnings_age_but_write_failures_stay_until_a_good_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tempo");
    std::fs::write(&path, "abc").unwrap();
    let mut store = TempoStore::open(path.clone());
    let now = Instant::now();
    assert!(store.status_warning(now + Duration::from_secs(9)).is_some());
    assert!(
        store
            .status_warning(now + Duration::from_secs(10))
            .is_none()
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let shared = SharedState::new();
    shared.set_tempo(96.0);
    store.poll(shared.transport(), now);
    store.poll(shared.transport(), now + Duration::from_millis(500));
    assert!(
        store
            .status_warning(now + Duration::from_secs(60))
            .is_some()
    );
    assert_eq!(store.failed_writes().len(), 1);
    std::fs::remove_dir(&path).unwrap();
    shared.set_tempo(100.0);
    store.poll(shared.transport(), now + Duration::from_secs(61));
    store.poll(shared.transport(), now + Duration::from_millis(61_500));
    assert!(
        store
            .status_warning(now + Duration::from_secs(62))
            .is_none()
    );
    assert_eq!(store.failed_writes().len(), 1);
}
