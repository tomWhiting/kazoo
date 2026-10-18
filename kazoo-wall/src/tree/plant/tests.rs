//! Planting an empty tree when someone else may be planting too, and
//! planting it for the tree's view.

use std::fs;
use std::path::Path;

use super::{Planting, ToLook, WallTree, plant_empty_with, plant_to_look, wall_tree};
use crate::store::{LOG_FILE, OLD_LOG_FILE};
use crate::tree::{TreeError, git};

/// What is in `dir`, the planting lock left out.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with(".planting.lock"))
        .collect();
    names.sort();
    names
}

#[test]
fn planters_at_once_take_turns_and_both_find_the_tree() {
    let home = tempfile::tempdir().unwrap();
    let tree = home.path().join("tree");
    let planters: Vec<_> = (0..4)
        .map(|_| {
            let tree = tree.clone();
            std::thread::spawn(move || plant_empty_with(&tree, git::plant))
        })
        .collect();
    for planter in planters {
        assert_eq!(planter.join().unwrap(), Ok(Planting::Empty));
    }
    assert!(git::is_bare(&tree).unwrap());
    assert_eq!(names(home.path()), ["tree"]);
}

#[test]
fn a_planting_that_fails_where_nothing_was_leaves_nothing() {
    let home = tempfile::tempdir().unwrap();
    let tree = home.path().join("tree");
    let planting = plant_empty_with(&tree, |dir| {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("HEAD"), "half").unwrap();
        Err(TreeError::Git("no git here".into()))
    });
    let Err(why) = planting else {
        panic!("{planting:?}");
    };
    assert!(
        why.contains("cannot be planted") && why.contains("no git here"),
        "{why}"
    );
    assert!(names(home.path()).is_empty(), "{:?}", names(home.path()));
}

#[test]
fn an_empty_folder_that_fails_to_plant_is_emptied_and_kept() {
    let home = tempfile::tempdir().unwrap();
    let tree = home.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let planting = plant_empty_with(&tree, |dir| {
        fs::write(dir.join("HEAD"), "half").unwrap();
        Err(TreeError::Git("no git here".into()))
    });
    assert!(planting.is_err(), "{planting:?}");
    assert!(tree.is_dir());
    assert!(names(&tree).is_empty());
}

#[test]
fn a_folder_with_something_else_in_it_is_never_touched() {
    let home = tempfile::tempdir().unwrap();
    let tree = home.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("mine.txt"), "keep me").unwrap();
    let planting = plant_empty_with(&tree, |_| panic!("nothing is planted over it"));
    let Err(why) = planting else {
        panic!("{planting:?}");
    };
    assert!(why.contains("is not a tree"), "{why}");
    assert_eq!(
        fs::read_to_string(tree.join("mine.txt")).unwrap(),
        "keep me"
    );
    assert_eq!(names(&tree), ["mine.txt"]);
}

#[test]
fn the_view_plants_an_empty_tree_where_the_wall_has_played_nothing() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".kazoo").join("wall");
    let tree = home.path().join(".kazoo").join("tree");
    assert_eq!(plant_to_look(&state, &tree), ToLook::Planted);
    assert!(git::is_bare(&tree).unwrap());
    assert!(!state.exists(), "the wall's state is the wall's to make");
    assert_eq!(plant_to_look(&state, &tree), ToLook::There);
    // An empty log is nothing played.
    let fresh = home.path().join("fresh");
    fs::create_dir_all(fresh.join("wall")).unwrap();
    fs::write(fresh.join("wall").join(LOG_FILE), "").unwrap();
    assert_eq!(
        plant_to_look(&fresh.join("wall"), &fresh.join("tree")),
        ToLook::Planted
    );
}

#[test]
fn the_view_leaves_a_tree_with_a_past_to_the_wall() {
    for log in [LOG_FILE, OLD_LOG_FILE] {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("wall");
        fs::create_dir_all(&state).unwrap();
        // After a rotation the new log is empty and the past is in the old.
        fs::write(state.join(LOG_FILE), "").unwrap();
        fs::write(state.join(log), "{\"seq\":1}\n").unwrap();
        let tree = home.path().join("tree");
        assert_eq!(plant_to_look(&state, &tree), ToLook::ForTheWall, "{log}");
        assert!(!tree.exists());
    }
}

#[test]
fn the_view_says_what_is_in_the_way_of_planting() {
    let home = tempfile::tempdir().unwrap();
    // A file where the wall's folder would go: nothing has played there.
    fs::write(home.path().join(".kazoo"), "not a folder").unwrap();
    let state = home.path().join(".kazoo").join("wall");
    let tree = home.path().join(".kazoo").join("tree");
    let ToLook::Failed(why) = plant_to_look(&state, &tree) else {
        panic!("planted over a file");
    };
    assert!(why.contains("cannot be made"), "{why}");
}

#[test]
fn an_empty_mount_point_whose_parent_is_not_ours_is_planted_in_place() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let parent = home.path().join("volumes");
    let tree = parent.join("tree");
    fs::create_dir_all(&tree).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    let planting = plant_empty_with(&tree, git::plant);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(planting, Ok(Planting::Empty));
    assert!(git::is_bare(&tree).unwrap());
    assert_eq!(names(&parent), ["tree"]);
}

/// How a stand-in wall answers a look.
#[derive(Clone)]
enum Answer {
    /// With this tree.
    Tree(Option<crate::protocol::TreeHealth>),
    /// With an error.
    Refused,
    /// Never: the hello goes unanswered, as while a wall is starting.
    Silent,
}

/// A stand-in wall on `socket`, answering every line as `answer` says.
fn stand_in(socket: &Path, answer: Answer) {
    use crate::protocol::{ErrorCode, HelloResult, HelloRole, Request, Response, WallError};
    use std::io::{BufRead, BufReader, Write};
    let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            if matches!(answer, Answer::Silent) {
                // Read what is asked, and never answer it.
                std::thread::spawn(move || std::io::copy(&mut stream, &mut std::io::sink()));
                continue;
            }
            let answer = answer.clone();
            std::thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { return };
                    let asked: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let id = asked["id"].as_u64().unwrap();
                    let request: Request = serde_json::from_value(asked).unwrap();
                    let response = match (request, &answer) {
                        (Request::Hello { seat, .. }, _) => Response::success(
                            id,
                            &HelloResult {
                                daemon: "stand-in".to_string(),
                                seat,
                                role: HelloRole {
                                    console: false,
                                    watcher: true,
                                },
                                seats: Vec::new(),
                                revision: 0,
                                comments: false,
                                notes: false,
                            },
                        )
                        .unwrap(),
                        (Request::Look, Answer::Tree(tree)) => {
                            let mut look = crate::tui::tests::snapshot();
                            look.tree.clone_from(tree);
                            Response::success(id, &look).unwrap()
                        }
                        _ => Response::failure(
                            id,
                            WallError::new(ErrorCode::Internal, "the stand-in refuses"),
                        ),
                    };
                    let line = serde_json::to_string(&response).unwrap();
                    if writeln!(writer, "{line}").is_err() {
                        return;
                    }
                }
            });
        }
    });
}

fn asked(answer: Option<Answer>) -> WallTree {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("wall.sock");
    if let Some(answer) = answer {
        stand_in(&socket, answer);
    }
    wall_tree(&socket, std::time::Duration::from_millis(300))
}

#[test]
fn a_wall_says_whether_it_has_its_tree() {
    use crate::protocol::TreeHealth;
    assert_eq!(asked(None), WallTree::NoWall);
    assert_eq!(asked(Some(Answer::Silent)), WallTree::Starting);
    assert_eq!(
        asked(Some(Answer::Tree(Some(TreeHealth {
            planted: true,
            ..TreeHealth::default()
        })))),
        WallTree::Planted
    );
    assert_eq!(
        asked(Some(Answer::Tree(Some(TreeHealth {
            planted: false,
            error: Some("the disk is full".to_string()),
            ..TreeHealth::default()
        })))),
        WallTree::Unplanted(Some("the disk is full".to_string()))
    );
    assert_eq!(asked(Some(Answer::Tree(None))), WallTree::Unplanted(None));
    assert_eq!(
        asked(Some(Answer::Refused)),
        WallTree::Unclear("the stand-in refuses".to_string())
    );
}

#[test]
fn a_socket_left_behind_is_no_wall() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("wall.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    assert_eq!(
        wall_tree(&socket, std::time::Duration::from_millis(300)),
        WallTree::NoWall
    );
}
