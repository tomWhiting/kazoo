use std::fs;
use std::time::Instant;

use super::fixtures::{
    birth, change, command, commit_of, fixture, kept, load, module, save, seed_file, snapshot,
    with_start, write_state,
};
use super::{RingsError, grow};
use crate::daemon::wall::Wall;
use crate::patch::Patch;
use crate::protocol::What;
use crate::store::Store;
use crate::tree::{self, Birth, git};

#[test]
fn an_import_ends_with_the_births_the_daemon_will_find() {
    let (state, mut file, mut changes) = fixture(1);
    with_start(state.path());
    let mut added = module(&file, "vco1");
    added.id = "vco3".into();
    changes.push(change(
        2,
        What::Add {
            module: added.clone(),
        },
    ));
    file.modules.push(added);
    file.last_seq = 2;
    write_state(state.path(), &file, &changes);
    let before = fs::read(state.path().join("patch.json")).unwrap();
    let tree = tempfile::tempdir().unwrap();
    grow(state.path(), tree.path(), false).unwrap();
    let daemon = Patch::load(&file).unwrap().0.to_file();
    assert_eq!(kept(tree.path(), "trunk").modules, daemon.modules);
    assert!(
        birth(tree.path(), &commit_of(tree.path(), "change", 2), "vco3").starts_with("born 2.0 ")
    );
    assert_eq!(
        birth(tree.path(), "trunk", "vco3"),
        module(&daemon, "vco3").birth.unwrap()
    );
    assert_eq!(fs::read(state.path().join("patch.json")).unwrap(), before);
    let imported = tree::to_files(&kept(tree.path(), "trunk")).unwrap();
    let tip = command(tree.path(), &["rev-parse", "trunk"]);
    let (engine, control) = crate::engine::engine(
        crate::engine::EngineConfig::new(48_000, file.tempo, 0.0),
        None,
    );
    let mut wall = Wall::new(
        Patch::load(&file).unwrap().0,
        control,
        None,
        changes,
        Some(Store::open(state.path()).unwrap()),
    );
    wall.find_tree(tree.path());
    wall.close(Instant::now());
    assert_eq!(command(tree.path(), &["rev-parse", "trunk^"]), tip);
    assert_eq!(
        command(
            tree.path(),
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Kazoo-Kind,valueonly)",
                "trunk"
            ]
        ),
        "start"
    );
    assert_eq!(
        tree::to_files(&kept(tree.path(), "trunk")).unwrap(),
        imported
    );
    drop(engine);
}

#[test]
fn a_snapshot_after_a_load_keeps_the_named_saves_births() {
    let (state, _, mut changes) = fixture(1);
    with_start(state.path());
    let mut saved = seed_file();
    saved.wall = 99;
    saved.seed = 7;
    let original = Birth::Born {
        seq: 55,
        n: 0,
        at: crate::change::utc(super::fixtures::at(55)),
        print: 123,
        salt: 456,
    };
    saved
        .modules
        .iter_mut()
        .find(|m| m.id == "vco1")
        .unwrap()
        .birth = Some(original.to_string());
    save(state.path(), "named", &saved, 2);
    changes.push(load(2, "named", "missing", "missing".into(), &saved));
    let mut snapshot = saved.clone();
    snapshot.wall = 2;
    snapshot.last_seq = 2;
    for record in &mut snapshot.modules {
        record.birth = None;
    }
    write_state(state.path(), &snapshot, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    let loaded = commit_of(tree.path(), "change", 2);
    assert_eq!(birth(tree.path(), &loaded, "vco1"), original.to_string());
    assert_eq!(
        birth(tree.path(), &loaded, "vcf1"),
        Birth::Found {
            wall: 99,
            seed: 7,
            id: "vcf1".into(),
        }
        .to_string()
    );
    assert_eq!(report.gaps, 0, "{report}");
    assert_eq!(
        kept(tree.path(), "trunk").modules,
        Patch::load(&snapshot).unwrap().0.to_file().modules
    );
}

#[test]
fn a_bare_tree_whose_head_is_not_trunk_is_refused() {
    let (state, _, _) = fixture(1);
    with_start(state.path());
    let tree = tempfile::tempdir().unwrap();
    git::plant(tree.path()).unwrap();
    fs::write(tree.path().join("HEAD"), "ref: refs/heads/other\n").unwrap();
    let before = snapshot(tree.path());
    for dry_run in [false, true] {
        let error = grow(state.path(), tree.path(), dry_run).unwrap_err();
        assert!(matches!(error, RingsError::NotTrunk(_)), "{error}");
        assert!(error.to_string().contains("refs/heads/other"), "{error}");
        assert_eq!(snapshot(tree.path()), before);
    }
}

#[test]
fn a_bare_tree_with_a_detached_head_is_refused() {
    let (state, _, _) = fixture(1);
    with_start(state.path());
    let tree = tempfile::tempdir().unwrap();
    git::plant(tree.path()).unwrap();
    let head = "0123456789abcdef0123456789abcdef01234567";
    fs::write(tree.path().join("HEAD"), format!("{head}\n")).unwrap();
    let before = snapshot(tree.path());
    let error = grow(state.path(), tree.path(), false).unwrap_err();
    assert!(matches!(error, RingsError::NotTrunk(_)), "{error}");
    assert!(error.to_string().contains(head), "{error}");
    assert_eq!(snapshot(tree.path()), before);
}
