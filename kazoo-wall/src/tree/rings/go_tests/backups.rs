use super::*;

fn tree_goes() -> (tempfile::TempDir, Vec<Change>) {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let tree = root.path().join("tree");
    fs::create_dir(&state).unwrap();
    let (source, _, mut changes) = fixture(10);
    with_start(source.path());
    grow(source.path(), &tree, false).unwrap();
    let before_commit = super::super::fixtures::commit_of(&tree, "change", 10);
    let moment_commit = super::super::fixtures::commit_of(&tree, "change", 4);
    let before = kept(&tree, &before_commit);
    let moment = kept(&tree, &moment_commit);
    let mut original = go(11, Some(4), &moment);
    let mut undone = go(12, Some(10), &before);
    undone.undoes = Some(11);
    for (change, commit, seq, file) in [
        (&mut original, before_commit, 10, &before),
        (&mut undone, moment_commit, 4, &moment),
    ] {
        let What::Go {
            from,
            backup,
            fingerprint,
            ..
        } = &mut change.what
        else {
            panic!("not a go")
        };
        from.commit.clone_from(&commit);
        from.change = Some(seq);
        *backup = format!("tree:{}", &commit[..12]);
        *fingerprint = crate::patch::Patch::load(file).unwrap().0.fingerprint();
    }
    changes.extend([original, undone]);
    with_start(&state);
    let mut after = before;
    after.last_seq = 12;
    after.wall = 12;
    write_state(&state, &after, &changes);
    (root, changes)
}

fn damage(changes: &mut [Change], prefix: bool) {
    let What::Go {
        backup,
        fingerprint,
        ..
    } = &mut changes[10].what
    else {
        panic!("not a go")
    };
    if prefix {
        *backup = "tree:ffffffffffff".into();
    } else {
        let mut different = seed_file();
        *cutoff(&mut different) = 999.0;
        *fingerprint = crate::patch::Patch::load(&different)
            .unwrap()
            .0
            .fingerprint();
    }
}

#[test]
fn a_real_tree_go_and_undo_prove_both_backups() {
    let (root, _) = tree_goes();
    let target = root.path().join("imported");
    let report = grow(&root.path().join("state"), &target, false).unwrap();
    assert_eq!(report.gaps, 0, "{report}");
    assert!(
        report.report.contains("2 go backups held by the tree"),
        "{report}"
    );
    assert!(report.report.contains("0 do not ()"), "{report}");
}

#[test]
fn a_tree_backup_requires_its_existing_moments_fingerprint() {
    let (root, mut changes) = tree_goes();
    damage(&mut changes, false);
    let state = root.path().join("state");
    let proofs = super::super::evidence::Proofs::read(&state, &changes).unwrap();
    assert_eq!(proofs.tree_backups, 1);
    assert_eq!(proofs.bad_backups.len(), 1);
    let error = proofs.incoming(&state, &changes[11], &changes).unwrap_err();
    assert!(error.contains("fingerprint"), "{error}");
}

#[test]
fn a_held_tree_backup_requires_the_recorded_fingerprint() {
    held_backup_refuses(false);
}

#[test]
fn a_held_tree_backup_requires_its_source_prefix() {
    held_backup_refuses(true);
}

fn held_backup_refuses(prefix: bool) {
    let (root, mut changes) = tree_goes();
    damage(&mut changes, prefix);
    let state = root.path().join("state");
    let file = crate::migrate::read(&fs::read(state.join("patch.json")).unwrap())
        .unwrap()
        .0;
    write_state(&state, &file, &changes);
    let report = grow(&state, &root.path().join("imported"), false).unwrap();
    assert!(report.gaps > 0, "{report}");
    assert!(
        report.report.contains("1 go backups held by the tree"),
        "{report}"
    );
}
