use super::{Engine, SUB_BLOCK, Wall};

pub(super) fn land(wall: &Wall, engine: &mut Engine) {
    let shared = wall.engine.shared();
    let mut buffer = [0.0; SUB_BLOCK * 2];
    while shared.unhushed() < wall.swaps_sent {
        engine.render(&mut buffer, 2);
    }
}

#[test]
fn a_load_says_the_bar_it_lands_on_as_the_console_counts_it() {
    let (mut wall, mut engine, _dir) = super::saving_wall();
    super::save(&mut wall, "here", false).unwrap();
    // 1.3 beats in at 120 BPM and 48 kHz: 24,000 frames a beat.
    let mut buffer = vec![0.0; 2 * 1_200];
    loop {
        if wall.engine.shared().beat() >= 1.3 {
            break;
        }
        engine.render(&mut buffer, 2);
    }
    let loaded = super::load_save(&mut wall, "here").unwrap();
    // The console counts bar 1 from beat 0, four beats a bar: the next bar
    // after beat 1.3 is beat 4, bar 2.
    assert!(
        loaded.notes.iter().any(|note| note == "lands on bar 2"),
        "{:?}",
        loaded.notes
    );
    land(&wall, &mut engine);
    let origin = wall.engine.shared().origin();
    assert_eq!(origin.to_bits(), 4.0_f64.to_bits());
}

#[test]
fn a_go_s_gap_commit_is_made_when_the_log_on_disk_cannot_be_read() {
    let tree = tempfile::tempdir().unwrap();
    let (mut wall, _engine, state) = super::saving_wall();
    crate::tree::git::plant(tree.path()).unwrap();
    wall.attach_tree(crate::tree::thread::TreeHandle::attach(tree.path()).unwrap());
    let log = state.path().join(crate::store::LOG_FILE);
    if log.exists() {
        std::fs::remove_file(&log).unwrap();
    }
    std::fs::create_dir(&log).unwrap();
    wall.whole_tree(crate::tree::thread::WholeKind::Gap);
    let error = wall.tree.as_ref().unwrap().error.clone();
    assert_eq!(error, None, "the gap commit waited on the log on disk");
}
