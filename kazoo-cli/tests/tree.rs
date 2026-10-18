//! `kzoo tree` on a Mac where no wall has played: it plants the tree and
//! says nothing has grown, and an error never reads as a crash.

use std::process::Command;

/// `kzoo tree --print` with `home` as the home folder and nothing else of
/// the wall's set.
fn tree_print(home: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kzoo"))
        .args(["tree", "--print"])
        .env("HOME", home)
        // Never the wall playing on this machine: its socket is in here.
        .env("KAZOO_WALL_RUNTIME_DIR", home.join("run"))
        .env_remove("KAZOO_TREE_DIR")
        .env_remove("KAZOO_WALL_STATE_DIR")
        .env_remove("KZOO_RUN_AS")
        .output()
        .unwrap()
}

#[test]
fn a_mac_that_never_played_plants_its_tree_and_says_nothing_has_grown() {
    let home = tempfile::tempdir().unwrap();
    let out = tree_print(home.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\n", kazoo_tree::NOTHING_YET)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = home.path().join(".kazoo").join("tree");
    assert!(kazoo_wall::tree::git::is_bare(&tree).unwrap());
    // Looked at again, the tree that is there is left as it is.
    let again = tree_print(home.path());
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(again.stdout, out.stdout);
}

#[test]
fn a_tree_that_cannot_be_planted_says_why_in_one_line() {
    let home = tempfile::tempdir().unwrap();
    // A file where the tree's folder would go: nothing can be planted.
    std::fs::write(home.path().join(".kazoo"), "not a folder").unwrap();
    let out = tree_print(home.path());
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.starts_with("kzoo: "), "{said}");
    assert!(said.contains("cannot be made"), "{said}");
    assert!(out.stdout.is_empty());
    for crash in ["Location", "Backtrace", "RUST_BACKTRACE", "   0: "] {
        assert!(!said.contains(crash), "{crash} in {said}");
    }
    assert_eq!(said.trim_end().lines().count(), 1, "{said}");
}

#[test]
fn a_wall_that_has_played_grows_the_tree_itself() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".kazoo").join("wall");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join(kazoo_wall::store::LOG_FILE), "{\"seq\":1}\n").unwrap();
    let out = tree_print(home.path());
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("the wall grows it as it starts"), "{said}");
    assert_eq!(said.trim_end().lines().count(), 1, "{said}");
    assert!(!home.path().join(".kazoo").join("tree").exists());
}
