use std::time::Instant;

use kazoo_wall::daemon::{Daemon, DaemonConfig};

use super::*;
use crate::history::History;

/// Read the tree at `dir` until it holds at least `count` moments.
fn moments(dir: &Path, count: usize) -> History {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match History::read(dir) {
            Ok(history) if history.moments.len() >= count => return history,
            Ok(_) | Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(history) => panic!("only {} moments grew", history.moments.len()),
            Err(err) => panic!("the tree could not be read: {err}"),
        }
    }
}

#[test]
fn a_go_puts_the_wall_at_a_moment_and_says_so() {
    let dir = tempfile::tempdir().expect("a folder");
    // A test daemon keeps its tree inside its own state (KZ-T04g).
    let state = dir.path().join("state");
    let tree = state.join("tree");
    std::fs::create_dir_all(&state).expect("a state folder");
    kazoo_wall::tree::git::plant(&tree).expect("planted");
    let mut config = DaemonConfig::headless(dir.path(), &state);
    // The tests run side by side, so the machine is busy: the wall's guard
    // against overloading it would refuse the go for that, not for the go.
    config.load_guard = false;
    let daemon = Daemon::start(config).expect("a wall");
    let mut wall = WallClient::connect(daemon.socket(), "Tester", "a test", false).expect("in");
    let look = wall.look().expect("a look");
    let (module, knob) = look
        .modules
        .iter()
        .find_map(|m| {
            m.knobs
                .iter()
                .find(|k| !k.stepped && k.max > k.min)
                .map(|k| (m.id.clone(), k.clone()))
        })
        .expect("a knob to turn");
    let middle = knob.min.midpoint(knob.max);
    for value in [middle, knob.min.midpoint(middle)] {
        wall.turn(&module, &knob.name, value, Some(0.0))
            .expect("turned");
    }
    let history = moments(&tree, 2);
    let first = &history.moments[0].id;
    let words = go(daemon.socket(), first).expect("gone");
    assert!(
        words.starts_with(&format!("the wall is at {}", short(first))),
        "{words}"
    );
    assert!(words.contains("saved as"), "{words}");

    let refused = go(daemon.socket(), "0123456789abcdef0123456789abcdef01234567")
        .expect_err("no such moment");
    assert!(
        refused.starts_with("the wall would not go to 0123456789ab"),
        "{refused}"
    );
    drop(wall);
    daemon.stop();
    daemon.wait().expect("stopped");
}

#[test]
fn a_go_with_no_wall_playing_says_so() {
    let dir = tempfile::tempdir().expect("a folder");
    let why = go(&dir.path().join("nobody.sock"), "0123456789abcdef").expect_err("no wall");
    assert!(
        why.starts_with("the wall could not be reached to go to 0123456789ab"),
        "{why}"
    );
    assert!(why.ends_with("is it playing?"), "{why}");
}

#[test]
fn goes_send_off_the_screen_and_come_back_in_words() {
    let dir = tempfile::tempdir().expect("a folder");
    let goes = Goes::to(dir.path().join("nobody.sock"));
    assert_eq!(goes.heard(), None);
    goes.send("0123456789abcdef");
    let deadline = Instant::now() + Duration::from_secs(10);
    let words = loop {
        if let Some(words) = goes.heard() {
            break words;
        }
        assert!(Instant::now() < deadline, "the go never came back");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(words.contains("could not be reached"), "{words}");
}
