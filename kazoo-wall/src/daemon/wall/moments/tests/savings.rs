use super::*;

#[test]
fn two_goes_from_tree_moments_keep_no_extra_save_and_undo_returns() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let before = rig.wall.patch.fingerprint();
    let at = rig.listing().here.unwrap();
    let count = rig.wall.saves_store().unwrap().list_saves().unwrap().len();
    let go = rig.go("#5").change.unwrap();
    let What::Go { backup, .. } = &go.what else {
        panic!("not a go")
    };
    assert_eq!(backup, &format!("tree:{}", &at.commit[..12]));
    rig.go("#6");
    assert_eq!(
        rig.wall.saves_store().unwrap().list_saves().unwrap().len(),
        count
    );
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"undo", "change":go.seq}),
    )
    .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
    assert_eq!(rig.listing().here.unwrap().commit, at.commit);
    rig.wall.close(rig.now);
}

#[test]
fn a_go_after_an_unsaved_edit_keeps_one_save() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let second = rig.go("#5").change.unwrap();
    assert!(matches!(second.what, What::Go { backup, .. } if backup.starts_with("tree:")));
    let count = rig.wall.saves_store().unwrap().list_saves().unwrap().len();
    rig.turn(3500.0);
    let go = rig.go("#6").change.unwrap();
    let What::Go { backup, .. } = go.what else {
        panic!("not a go")
    };
    assert!(backup.starts_with("before-go-"));
    assert_eq!(
        rig.wall.saves_store().unwrap().list_saves().unwrap().len(),
        count + 1
    );
    rig.wall.close(rig.now);
}

#[test]
fn undoing_a_tree_backup_survives_a_restart() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let before = rig.wall.patch.fingerprint();
    let go = rig.go("#5").change.unwrap();
    let What::Go { backup, .. } = &go.what else {
        panic!("not a go")
    };
    assert!(backup.starts_with("tree:"));
    rig.wall.close(rig.now);
    let store = Store::open(rig.state.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no wall")
    };
    let history = store.load_log().unwrap().changes;
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    rig.engine = engine;
    rig.wall = Wall::new(*patch, control, None, history, Some(store));
    rig.wall
        .attach_tree(TreeHandle::attach(rig.tree.path()).unwrap());
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"undo", "change":go.seq}),
    )
    .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
    rig.wall.close(rig.now);
}
