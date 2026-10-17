//! The wall plants its own tree as it starts.

use std::path::Path;
use std::process::Command;

use kazoo_wall::daemon::{Daemon, DaemonConfig};
use kazoo_wall::protocol::Request;
use kazoo_wall::protocol::client::WallClient;

/// A test wall that plants its tree as a wall for people does.
fn planting(runtime: &Path, state: &Path) -> Daemon {
    Daemon::start(DaemonConfig {
        load_guard: false,
        plant_tree: true,
        ..DaemonConfig::headless(runtime, state)
    })
    .unwrap()
}

/// What git says in the tree, which must succeed.
fn git(tree: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(tree)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The commits on the tree's trunk.
fn trunk_commits(tree: &Path) -> usize {
    git(tree, &["rev-list", "--count", "refs/heads/trunk"])
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn a_new_wall_plants_its_tree_and_can_mark_at_once() {
    let (runtime, state) = super::dirs();
    let tree = state.path().join("tree");
    let daemon = planting(runtime.path(), state.path());
    assert!(kazoo_wall::tree::git::is_bare(&tree).unwrap());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    wall.turn("vcf1", "cutoff", 800.0, Some(0.0)).unwrap();
    wall.call(Request::Mark {
        name: "dawn".into(),
    })
    .unwrap();
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
    assert!(trunk_commits(&tree) >= 2, "the start and the turn");
}

#[test]
fn a_wall_with_a_log_and_no_tree_grows_it_from_the_log() {
    let (runtime, state) = super::dirs();
    let tree = state.path().join("tree");
    let daemon = super::start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    wall.turn("vcf1", "cutoff", 800.0, Some(0.0)).unwrap();
    wall.turn("vcf1", "cutoff", 700.0, Some(0.0)).unwrap();
    wall.tempo(100.0).unwrap();
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
    assert!(!tree.exists(), "a test wall plants nothing");

    let daemon = planting(runtime.path(), state.path());
    assert!(kazoo_wall::tree::git::is_bare(&tree).unwrap());
    assert!(
        trunk_commits(&tree) >= 3,
        "a moment for each change in the log"
    );
    git(&tree, &["rev-parse", "--verify", "refs/tags/rings/end"]);
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    wall.call(Request::Mark {
        name: "grown".into(),
    })
    .unwrap();
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn something_else_where_the_tree_goes_is_said_and_the_wall_plays() {
    let (runtime, state) = super::dirs();
    let tree = state.path().join("tree");
    std::fs::write(&tree, b"not a tree").unwrap();
    let daemon = planting(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    wall.turn("vcf1", "cutoff", 800.0, Some(0.0)).unwrap();
    let error = wall
        .call(Request::Mark {
            name: "dawn".into(),
        })
        .unwrap_err();
    assert!(error.to_string().contains("keeps no tree yet"), "{error}");
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
    assert_eq!(std::fs::read(&tree).unwrap(), b"not a tree");
}

#[test]
fn a_tree_that_cannot_be_planted_says_why_when_a_moment_is_marked() {
    let (runtime, state) = super::dirs();
    let file = state.path().join("a file");
    std::fs::write(&file, b"not a directory").unwrap();
    let daemon = Daemon::start(DaemonConfig {
        load_guard: false,
        plant_tree: true,
        tree_dir: Some(file.join("tree")),
        ..DaemonConfig::headless(runtime.path(), state.path())
    })
    .unwrap();
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let error = wall
        .call(Request::Mark {
            name: "dawn".into(),
        })
        .unwrap_err();
    assert!(error.to_string().contains("cannot be read"), "{error}");
    let tree = wall.look().unwrap().tree.unwrap();
    assert!(!tree.planted);
    assert!(tree.error.unwrap().contains("cannot be read"));
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}
