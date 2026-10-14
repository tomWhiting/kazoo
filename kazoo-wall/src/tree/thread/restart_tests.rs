use super::*;
use crate::daemon::wall::Wall;
use crate::engine::{EngineConfig, engine};
use crate::patch::Patch;
use crate::protocol::TreePlace;

fn git_text(dir: &Path, args: &[&str]) -> String {
    let output = git::git_command(args[0], Some(dir))
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn change(seq: u64, go: bool) -> Change {
    let what = if go {
        let place: TreePlace = serde_json::from_value(serde_json::json!({"commit":"0000000000000000000000000000000000000000","branch":"trunk"})).unwrap();
        What::Go {
            to: place.clone(),
            from: place,
            backup: "kept".into(),
            fingerprint: "hash".into(),
            modules: 0,
            cables: 0,
            tempo: 120.0,
            by_console: true,
        }
    } else {
        What::Turn {
            module: "vcf1".into(),
            knob: "cutoff".into(),
            from: 400.0,
            to: 500.0,
            glide_beats: 0.0,
        }
    };
    Change {
        seq,
        at: "2026-10-13T09:01:07Z".into(),
        seat: "Tom".into(),
        what,
        summary: "changed".into(),
        undoes: None,
        clock: None,
    }
}

fn restart(history: Vec<Change>) -> String {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    let mut importer = Importer::new(dir.path(), &git::read(dir.path()).unwrap()).unwrap();
    importer
        .commit(&Commit::change(&change(1, false), Vec::new()).unwrap())
        .unwrap();
    importer
        .commit(&Commit::change(&change(2, false), Vec::new()).unwrap())
        .unwrap();
    importer.finish().unwrap();
    let newest = git_text(dir.path(), &["rev-parse", "trunk"]);
    git_text(dir.path(), &["update-ref", "refs/heads/held", &newest]);
    let oldest = git_text(dir.path(), &["rev-parse", "trunk~1"]);
    git_text(dir.path(), &["update-ref", "refs/heads/trunk", &oldest]);
    let mut file = crate::seed::seed().unwrap().to_file();
    file.last_seq = history.last().unwrap().seq;
    let (engine, control) = engine(EngineConfig::new(8_000, 120.0, 0.0), None);
    let mut wall = Wall::new(
        Patch::from_file(&file).unwrap(),
        control,
        None,
        history,
        None,
    );
    wall.attach_tree(TreeHandle::attach(dir.path()).unwrap());
    wall.close(std::time::Instant::now());
    drop(wall);
    drop(engine);
    git_text(dir.path(), &["log", "-1", "--format=%s"])
}

#[test]
fn moment_restart_gap_names_only_uncommitted_non_go_changes() {
    let history = (1..=6)
        .map(|seq| change(seq, seq == 3 || seq == 6))
        .collect();
    assert_eq!(
        restart(history),
        "the tree holds changes 4 to 5 as one commit"
    );
}

#[test]
fn moment_restart_without_missing_changes_names_the_restart() {
    assert_eq!(
        restart(vec![change(1, false), change(2, false), change(3, true)]),
        "the wall started again after change 3"
    );
}

#[test]
fn moment_restart_gap_of_one_change_names_it_in_the_singular() {
    let history = (1..=5)
        .map(|seq| change(seq, seq == 3 || seq == 5))
        .collect();
    assert_eq!(restart(history), "the tree holds change 4 as one commit");
}

#[test]
fn moment_a_gap_short_of_the_whole_commit_does_not_move_the_tree_back() {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    let mut handle = TreeHandle::attach(dir.path()).unwrap();
    // A whole commit after change 6 that named changes 4 and 5 (6 was a
    // go): the tree holds every change through 6.
    handle.hear(News::Committed { through: 6 });
    handle.hear(News::Gap { first: 4, last: 5 });
    assert_eq!(handle.through, Some(6));
}

#[test]
fn moment_nothing_after_the_tree_asks_git_nothing() {
    // Every candidate is at or before what the tree holds: no git run,
    // so no git repository is needed to answer.
    let nowhere = tempfile::tempdir().unwrap();
    assert_eq!(
        git::missing_changes(&nowhere.path().join("not-a-tree"), &[1, 2, 3], Some(3)).unwrap(),
        Vec::<u64>::new()
    );
}
