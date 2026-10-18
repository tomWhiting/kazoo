//! What `kzoo tree` and setup make of the tree and the wall's word on it.

use super::{TreeIs, tree_is};
use kazoo_wall::tree::plant::WallTree;

/// A wall's state folder (with a log when `played`) and an empty place for
/// the tree beside it.
fn home(played: bool) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("wall")).unwrap();
    if played {
        std::fs::write(
            home.path().join("wall").join(kazoo_wall::store::LOG_FILE),
            "{}\n",
        )
        .unwrap();
    }
    home
}

fn found(played: bool, said: WallTree) -> Result<TreeIs, String> {
    let home = home(played);
    tree_is(
        "kzoo",
        &home.path().join("wall"),
        &home.path().join("tree"),
        || said,
    )
    .map_err(|err| err.to_string())
}

#[test]
fn with_no_wall_the_past_decides() {
    assert_eq!(found(false, WallTree::NoWall), Ok(TreeIs::ToPlant));
    assert_eq!(found(true, WallTree::NoWall), Ok(TreeIs::ForTheWall));
}

#[test]
fn a_wall_without_its_tree_here_is_an_error_saying_what_to_do() {
    for played in [false, true] {
        let elsewhere = found(played, WallTree::Planted).unwrap_err();
        assert!(elsewhere.contains("somewhere other than"), "{elsewhere}");
        assert!(elsewhere.contains("KAZOO_TREE_DIR"), "{elsewhere}");
        let starting = found(played, WallTree::Starting).unwrap_err();
        assert!(starting.contains("look again in a moment"), "{starting}");
        let bare = found(played, WallTree::Unplanted(None)).unwrap_err();
        assert!(
            bare.starts_with("the wall is playing without a tree: stop it (kzoo wall stop)"),
            "{bare}"
        );
        let why = found(played, WallTree::Unplanted(Some("the disk is full".into()))).unwrap_err();
        assert!(
            why.starts_with("the wall is playing without a tree (the disk is full): "),
            "{why}"
        );
        let unclear = found(played, WallTree::Unclear("it hung up".into())).unwrap_err();
        assert!(unclear.contains("(it hung up)"), "{unclear}");
    }
}

#[test]
fn a_tree_there_is_there_without_asking_the_wall() {
    let home = home(true);
    let tree = home.path().join("tree");
    kazoo_wall::tree::git::plant(&tree).unwrap();
    assert_eq!(
        tree_is("kzoo", &home.path().join("wall"), &tree, || {
            panic!("the wall is not asked when the tree is there")
        })
        .unwrap(),
        TreeIs::There
    );
}
