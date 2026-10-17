//! `plant_tree`: what a starting wall does about its tree.

use std::path::Path;

use super::{Before, Daemon, DaemonConfig, Planting, failed, plant_tree};
use crate::protocol::client::WallClient;
use crate::store::{LOCK_FILE, LOG_FILE, Store};
use crate::tree::git;
use crate::tree::rings::{RingsError, reference};

/// A runtime directory short enough for a socket path, and a state one.
fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    let runtime = tempfile::Builder::new()
        .prefix("kw")
        .tempdir_in("/tmp")
        .unwrap();
    (runtime, tempfile::tempdir().unwrap())
}

/// A wall that has played and logged a change, with no tree planted.
fn played(runtime: &Path, state: &Path, plant: bool) {
    let daemon = Daemon::start(DaemonConfig {
        load_guard: false,
        plant_tree: plant,
        ..DaemonConfig::headless(runtime, state)
    })
    .unwrap();
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    wall.turn("vcf1", "cutoff", 800.0, Some(0.0)).unwrap();
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn a_new_mac_plants_its_tree_where_nothing_is_yet() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".kazoo").join("wall");
    let tree = home.path().join(".kazoo").join("tree");
    assert_eq!(plant_tree(&state, &tree), Planting::Empty);
    assert!(git::is_bare(&tree).unwrap());
}

#[test]
fn a_tree_with_a_trunk_is_kept() {
    let (runtime, state) = dirs();
    played(runtime.path(), state.path(), true);
    let tree = state.path().join("tree");
    let trunk = reference(&tree, "refs/heads/trunk").unwrap();
    assert!(trunk.is_some());
    assert_eq!(plant_tree(state.path(), &tree), Planting::Kept);
    assert_eq!(reference(&tree, "refs/heads/trunk").unwrap(), trunk);
}

#[test]
fn a_tree_planted_but_never_grown_is_grown_from_the_log() {
    let (runtime, state) = dirs();
    played(runtime.path(), state.path(), false);
    let tree = state.path().join("tree");
    git::plant(&tree).unwrap();
    assert_eq!(plant_tree(state.path(), &tree), Planting::Grown);
    assert!(reference(&tree, "refs/heads/trunk").unwrap().is_some());
    assert!(reference(&tree, "refs/tags/rings/end").unwrap().is_some());
}

#[test]
fn a_wall_holding_its_state_leaves_the_tree_alone() {
    let state = tempfile::tempdir().unwrap();
    let held = Store::open(state.path()).unwrap();
    let tree = state.path().join("tree");
    assert_eq!(plant_tree(state.path(), &tree), Planting::Left);
    assert!(!tree.exists());
    drop(held);
}

#[test]
fn a_log_whose_past_cannot_be_worked_out_plants_an_empty_tree() {
    let (runtime, state) = dirs();
    played(runtime.path(), state.path(), false);
    // Only the log is left: no kept wall file says what the wall was.
    for entry in std::fs::read_dir(state.path()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == LOG_FILE || name == LOCK_FILE {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
    }
    let tree = state.path().join("tree");
    assert_eq!(plant_tree(state.path(), &tree), Planting::Empty);
    assert!(git::is_bare(&tree).unwrap());
    assert!(reference(&tree, "refs/heads/trunk").unwrap().is_none());
}

#[test]
fn a_failed_grow_takes_away_only_what_it_left() {
    let parent = tempfile::tempdir().unwrap();
    let no_wall = || RingsError::NoWall("no wall".into());

    let absent = parent.path().join("absent");
    git::plant(&absent).unwrap();
    let why = failed(&absent, Before::Absent, &no_wall());
    assert!(why.contains("no wall"), "{why}");
    assert!(!absent.exists(), "a tree the grow made goes");

    let empty = parent.path().join("empty");
    git::plant(&empty).unwrap();
    failed(&empty, Before::Empty, &no_wall());
    assert!(empty.is_dir(), "a directory someone made stays");
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0, "emptied");

    let there = parent.path().join("there");
    git::plant(&there).unwrap();
    failed(&there, Before::Occupied, &no_wall());
    assert!(git::is_bare(&there).unwrap(), "what was there stays");
}

#[test]
fn something_else_where_the_tree_goes_is_never_touched() {
    let state = tempfile::tempdir().unwrap();
    let tree = state.path().join("tree");
    std::fs::write(&tree, b"not a tree").unwrap();
    let Planting::Failed(why) = plant_tree(state.path(), &tree) else {
        panic!("planted over a file");
    };
    assert!(why.contains("cannot be read"), "{why}");
    assert_eq!(std::fs::read(&tree).unwrap(), b"not a tree");
}
