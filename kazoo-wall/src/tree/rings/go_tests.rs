use std::fs;
use std::path::Path;
use std::time::Instant;

use super::fixtures::{
    change, command, cutoff, fixture, kept, real_wall, save, seed_file, turn, with_start,
    write_state,
};
use super::grow;
use crate::daemon::wall::{Outcome, Wall};
use crate::protocol::{Change, Request, TreePlace, What};
use crate::tree::{git, thread::TreeHandle};

fn place(seq: Option<u64>) -> TreePlace {
    TreePlace {
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        branch: None,
        mark: Some("first light".into()),
        change: seq,
    }
}

fn go(seq: u64, to: Option<u64>, file: &crate::patch::PatchFile) -> Change {
    change(
        seq,
        What::Go {
            to: place(to),
            from: TreePlace {
                branch: Some("trunk".into()),
                ..place(Some(10))
            },
            backup: "before-go".into(),
            fingerprint: String::new(),
            modules: u32::try_from(file.modules.len()).unwrap(),
            cables: u32::try_from(file.cables.len()).unwrap(),
            tempo: file.tempo,
            by_console: true,
        },
    )
}

fn going(undo: bool) -> tempfile::TempDir {
    let (state, before, mut changes) = fixture(10);
    with_start(state.path());
    let fingerprint = save(state.path(), "before-go", &before, 11);
    let mut moment = seed_file();
    *cutoff(&mut moment) = 304.0;
    let mut went = go(11, Some(4), &moment);
    if let What::Go {
        fingerprint: hash, ..
    } = &mut went.what
    {
        *hash = fingerprint;
    }
    changes.push(went);
    for seq in 12..=14 {
        let to = f64::from(u32::try_from(seq).unwrap()) + 400.0;
        changes.push(turn(seq, *cutoff(&mut moment), to));
        *cutoff(&mut moment) = to;
    }
    moment.last_seq = 14;
    moment.wall = 11;
    if undo {
        let mut back = go(15, Some(10), &before);
        back.undoes = Some(11);
        changes.push(back);
        moment = before;
        moment.wall = 15;
        for seq in 16..=17 {
            let to = f64::from(u32::try_from(seq).unwrap()) + 500.0;
            changes.push(turn(seq, *cutoff(&mut moment), to));
            *cutoff(&mut moment) = to;
        }
        moment.last_seq = 17;
    }
    write_state(state.path(), &moment, &changes);
    state
}

fn going_without_a_save() -> tempfile::TempDir {
    let state = going(true);
    let store = crate::store::Store::open(state.path()).unwrap();
    let mut changes = store.load_log().unwrap().changes;
    drop(store);
    for change in &mut changes {
        if let What::Go { backup, .. } = &mut change.what {
            *backup = "tree:0123456789ab".into();
        }
    }
    let file = crate::migrate::read(&fs::read(state.path().join("patch.json")).unwrap())
        .unwrap()
        .0;
    fs::remove_file(state.path().join("saves/before-go.json")).unwrap();
    write_state(state.path(), &file, &changes);
    state
}

#[test]
fn tree_go_backups_are_counted_as_held_by_the_tree() {
    let state = going_without_a_save();
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert!(
        report.report.contains("2 go backups held by the tree"),
        "{report}"
    );
    assert!(report.report.contains("0 do not ()"), "{report}");
}

#[test]
fn an_undone_tree_go_is_proved_without_a_save() {
    let state = going_without_a_save();
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.gaps, 0, "{report}");
    assert_eq!(report.changes, 15, "{report}");
    assert!(
        !report.report.contains("does not hold its fingerprint"),
        "{report}"
    );
    assert!(
        (kept(tree.path(), "trunk")
            .modules
            .iter()
            .find(|m| m.id == "vcf1")
            .unwrap()
            .knobs["cutoff"]
            - 517.0)
            .abs()
            < f64::EPSILON
    );
}

#[test]
fn an_undone_tree_go_reads_its_unnumbered_source_moment() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let source_tree = root.path().join("tree");
    fs::create_dir(&state).unwrap();
    let (source_state, before, _) = fixture(10);
    with_start(source_state.path());
    grow(source_state.path(), &source_tree, false).unwrap();
    let commit = super::fixtures::commit_of(&source_tree, "change", 10);
    let mut original = go(11, Some(4), &before);
    if let What::Go { from, backup, .. } = &mut original.what {
        from.commit.clone_from(&commit);
        from.change = None;
        *backup = format!("tree:{}", &commit[..12]);
    }
    let mut undone = go(12, None, &before);
    undone.undoes = Some(11);
    let changes = vec![original, undone.clone()];
    let mut after = before.clone();
    after.last_seq = 12;
    write_state(&state, &after, &changes);
    let proofs = super::evidence::Proofs::read(&state, &changes).unwrap();
    let (restored, note) = proofs.incoming(&state, &undone, &changes).unwrap();
    assert_eq!(restored.modules, kept(&source_tree, &commit).modules);
    assert_eq!(restored.cables, before.cables);
    assert!(note.is_none());
}

fn branch_changes(tree: &Path, branch: &str) -> Vec<String> {
    command(
        tree,
        &[
            "log",
            "--reverse",
            "--format=%H|%(trailers:key=Kazoo-Kind,valueonly)",
            branch,
        ],
    )
    .lines()
    .filter_map(|line| line.strip_suffix("|change").map(str::to_string))
    .collect()
}

fn buds(tree: &Path) -> Vec<String> {
    command(
        tree,
        &["for-each-ref", "--format=%(refname)", "refs/heads/bud"],
    )
    .lines()
    .map(str::to_string)
    .collect()
}

#[test]
fn an_imported_go_grows_a_bud_from_its_moment() {
    let state = going(false);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 13, "{}", report.report);
    assert_eq!(report.gaps, 0, "{}", report.report);
    let trunk = branch_changes(tree.path(), "trunk");
    assert_eq!(trunk.len(), 10);
    let buds = buds(tree.path());
    assert_eq!(buds.len(), 1);
    assert!(buds[0].starts_with("refs/heads/bud/first+light/"));
    let bud = branch_changes(tree.path(), &buds[0]);
    assert_eq!(bud.len(), 7);
    assert_eq!(
        command(tree.path(), &["rev-parse", &format!("{}^", bud[4])]),
        trunk[3]
    );
    let moment = kept(tree.path(), &trunk[3]);
    let went = kept(tree.path(), &bud[4]);
    let mut expected = moment;
    *cutoff(&mut expected) = 412.0;
    assert_eq!(went.modules, expected.modules);
    assert_eq!(went.cables, expected.cables);
    assert_eq!(went.tempo.to_bits(), expected.tempo.to_bits());
    assert_eq!(command(tree.path(), &["symbolic-ref", "HEAD"]), buds[0]);
    assert!(
        !report.report.contains("are not in it"),
        "{}",
        report.report
    );
    let again = grow(state.path(), tree.path(), false).unwrap();
    assert!(again.report.contains("the first rings are in already"));
}

#[test]
fn an_imported_go_then_undo_comes_back_to_the_trunk() {
    let state = going(true);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 15, "{}", report.report);
    assert_eq!(report.gaps, 0, "{}", report.report);
    assert_eq!(branch_changes(tree.path(), "trunk").len(), 12);
    assert_eq!(buds(tree.path()).len(), 1);
    assert_eq!(
        command(tree.path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/trunk"
    );
    assert_eq!(kept(tree.path(), "trunk").last_seq, 17);
}

#[test]
fn a_go_to_a_moment_the_import_does_not_hold_is_said() {
    let (state, mut file, _) = fixture(1);
    file.last_seq = 9;
    write_state(state.path(), &file, &[]);
    let changes = [turn(10, 301.0, 302.0), go(11, Some(4), &file)];
    fs::write(
        state.path().join("log.jsonl"),
        changes
            .iter()
            .map(|c| serde_json::to_string(c).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 1);
    assert!(
        report
            .report
            .contains("the wall change 11 went to a moment the import does not hold (4)"),
        "{}",
        report.report
    );
}

fn ask(wall: &mut Wall, request: &Request) -> Outcome {
    let now = Instant::now();
    let outcome = wall.request_as("Tom", true, request, now).unwrap();
    let Some(ticket) = outcome.pending else {
        return outcome;
    };
    loop {
        wall.tree_test_wait();
        if let Some((_, answer)) = wall
            .tree_answers(now)
            .into_iter()
            .find(|(t, _)| *t == ticket)
        {
            return answer.unwrap();
        }
    }
}

fn rewrite(state: &Path, edit: impl FnOnce(&mut Vec<Change>)) {
    let mut changes: Vec<Change> = fs::read_to_string(state.join("log.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    edit(&mut changes);
    fs::write(
        state.join("log.jsonl"),
        changes
            .iter()
            .map(|c| serde_json::to_string(c).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}

#[test]
fn a_held_moment_must_match_each_of_the_go_counts() {
    for field in ["modules", "cables", "tempo"] {
        let state = going(false);
        rewrite(state.path(), |changes| {
            let What::Go {
                modules,
                cables,
                tempo,
                ..
            } = &mut changes[10].what
            else {
                panic!("not a go");
            };
            match field {
                "modules" => *modules += 1,
                "cables" => *cables += 1,
                "tempo" => *tempo += 1.0,
                _ => unreachable!(),
            }
        });
        let tree = tempfile::tempdir().unwrap();
        let report = grow(state.path(), tree.path(), false).unwrap();
        assert_eq!(report.changes, 10, "{}", report.report);
        assert!(report.report.contains("the wall change 11 went to change 4 but the moment's wall was not what the go put up"), "{}", report.report);
        assert!(buds(tree.path()).is_empty());
    }
}

#[test]
fn a_missing_go_target_can_be_proved_by_the_next_kept_wall() {
    for target in [None, Some(0)] {
        let state = going(false);
        rewrite(state.path(), |changes| {
            let What::Go { to, .. } = &mut changes[10].what else {
                panic!("not a go");
            };
            to.change = target;
        });
        let mut file = seed_file();
        *cutoff(&mut file) = 304.0;
        file.last_seq = 11;
        file.wall = 11;
        fs::write(
            state.path().join("patch.before-12.json"),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        let tree = tempfile::tempdir().unwrap();
        let report = grow(state.path(), tree.path(), false).unwrap();
        assert_eq!(report.changes, 13, "{}", report.report);
        assert!(
            report
                .report
                .contains("the wall is proved by a kept file, but its line is not held")
        );
        assert_eq!(buds(tree.path()).len(), 1);
    }
}

#[test]
fn an_undo_with_no_held_return_branch_grows_a_bud_and_says_so() {
    let state = going(true);
    rewrite(state.path(), |changes| {
        let What::Go { from, .. } = &mut changes[10].what else {
            panic!("not a go");
        };
        from.change = None;
    });
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 15, "{}", report.report);
    assert!(
        report
            .report
            .contains("gap at go 15: the wall is proved by a kept file, but its line is not held")
    );
    assert_eq!(buds(tree.path()).len(), 2);
    assert_eq!(branch_changes(tree.path(), "trunk").len(), 10);
}

#[test]
fn an_undone_go_requires_its_backups_fingerprint() {
    let state = going(true);
    let mut backup: crate::patch::PatchFile =
        serde_json::from_slice(&fs::read(state.path().join("saves/before-go.json")).unwrap())
            .unwrap();
    *cutoff(&mut backup) = 999.0;
    fs::write(
        state.path().join("saves/before-go.json"),
        serde_json::to_vec(&backup).unwrap(),
    )
    .unwrap();
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 13, "{}", report.report);
    assert!(
        report
            .report
            .contains("the backup of the wall load 11 replaced does not hold its fingerprint"),
        "{}",
        report.report
    );
}

#[test]
fn the_imported_tree_matches_the_live_tree() {
    for (later, undo) in [(3, false), (0, false), (0, true), (3, true)] {
        live_parity(later, undo);
    }
}

fn live_parity(later: u32, undo: bool) {
    let state = tempfile::tempdir().unwrap();
    let live = tempfile::tempdir().unwrap();
    git::plant(live.path()).unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    wall.attach_tree(TreeHandle::attach(live.path()).unwrap());
    for n in 1..=10 {
        ask(
            &mut wall,
            &super::fixtures::turn_request("vcf1", "cutoff", f64::from(n) + 300.0, None),
        );
        if n == 4 {
            ask(
                &mut wall,
                &Request::Mark {
                    name: "dawn".into(),
                },
            );
        }
    }
    let went = ask(
        &mut wall,
        &Request::Go {
            moment: "dawn".into(),
        },
    )
    .change
    .unwrap()
    .seq;
    for n in 1..=later {
        ask(
            &mut wall,
            &super::fixtures::turn_request("vcf1", "cutoff", f64::from(n) + 400.0, None),
        );
    }
    if undo {
        ask(&mut wall, &Request::Undo { change: went });
        for n in 1..=2 {
            ask(
                &mut wall,
                &super::fixtures::turn_request("vcf1", "cutoff", f64::from(n) + 500.0, None),
            );
        }
    }
    same_trees(wall, state.path(), live.path());
}

/// A batch the seat sends: two turns of the filter's cutoff under a note.
fn batch(wall: &mut Wall, note: &str, first: f64, second: f64) -> u64 {
    ask(
        wall,
        &Request::Batch {
            note: note.into(),
            changes: vec![
                super::fixtures::turn_request("vcf1", "cutoff", first, None),
                super::fixtures::turn_request("vcf1", "cutoff", second, None),
            ],
        },
    )
    .change
    .unwrap()
    .seq
}

#[test]
fn the_imported_tree_matches_the_live_tree_through_batches() {
    // T10a's batches are one change each: the import follows a batch, its
    // undo, a mark on a batch's moment, a go there and that go's undo
    // exactly as the live tree wrote them, with the model known throughout.
    let state = tempfile::tempdir().unwrap();
    let live = tempfile::tempdir().unwrap();
    git::plant(live.path()).unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    wall.attach_tree(TreeHandle::attach(live.path()).unwrap());
    for n in 1..=3 {
        ask(
            &mut wall,
            &super::fixtures::turn_request("vcf1", "cutoff", f64::from(n) + 300.0, None),
        );
    }
    let first = batch(&mut wall, "a darker filter", 250.0, 240.0);
    ask(
        &mut wall,
        &Request::Mark {
            name: "dawn".into(),
        },
    );
    for n in 1..=2 {
        ask(
            &mut wall,
            &super::fixtures::turn_request("vcf1", "cutoff", f64::from(n) + 600.0, None),
        );
    }
    let went = ask(
        &mut wall,
        &Request::Go {
            moment: "dawn".into(),
        },
    )
    .change
    .unwrap();
    let What::Go { to, .. } = &went.what else {
        panic!("not a go: {went:?}");
    };
    assert_eq!(
        to.change,
        Some(first),
        "the batch's moment has its change number"
    );
    let undone = batch(&mut wall, "brighter again", 700.0, 710.0);
    ask(&mut wall, &Request::Undo { change: undone });
    ask(
        &mut wall,
        &super::fixtures::turn_request("vcf1", "cutoff", 720.0, None),
    );
    ask(&mut wall, &Request::Undo { change: went.seq });
    batch(&mut wall, "back on the trunk", 730.0, 740.0);
    same_trees(wall, state.path(), live.path());
}

/// Close `wall`, import its log into a new tree, and prove that tree is the
/// live one at `live`: no gaps or settles, the same HEAD, and the same
/// branches by shape with the same files at every commit.
fn same_trees(mut wall: Wall, state: &Path, live: &Path) {
    ask(&mut wall, &Request::Tree { recent: None });
    wall.close(Instant::now());
    drop(wall);
    let imported = tempfile::tempdir().unwrap();
    let report = grow(state, imported.path(), false).unwrap();
    assert_eq!(report.gaps, 0, "{}", report.report);
    assert_eq!(report.settles, 0, "{}", report.report);
    assert!(
        !report.report.contains("are not in it"),
        "{}",
        report.report
    );
    assert_eq!(raw_files(imported.path(), "HEAD"), raw_files(live, "HEAD"));
    assert_eq!(
        head_line(imported.path()).as_deref().map(branch_shape),
        head_line(live).as_deref().map(branch_shape),
        "HEAD is on the same line in both trees (or detached in both)"
    );
    assert!(
        grow(state, imported.path(), false)
            .unwrap()
            .report
            .contains("the first rings are in already")
    );
    let live_buds = buds(live);
    let imported_buds = buds(imported.path());
    assert_eq!(imported_buds.len(), live_buds.len());
    for (actual, expected) in std::iter::once(("trunk", "trunk")).chain(
        imported_buds
            .iter()
            .zip(&live_buds)
            .map(|(a, b)| (a.as_str(), b.as_str())),
    ) {
        assert_eq!(branch_shape(actual), branch_shape(expected));
        let actual = command(imported.path(), &["rev-list", "--reverse", actual]);
        let expected = command(live, &["rev-list", "--reverse", expected]);
        let actual: Vec<_> = actual.lines().collect();
        let expected: Vec<_> = expected.lines().collect();
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) {
            assert_eq!(raw_files(imported.path(), a), raw_files(live, b));
        }
    }
}

/// The branch HEAD is on, or none when it is detached at a moment.
fn head_line(tree: &Path) -> Option<String> {
    let output = git::git_command("symbolic-ref", Some(tree))
        .args(["-q", "HEAD"])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_string())
}

fn raw_files(tree: &Path, commit: &str) -> String {
    command(tree, &["ls-tree", "-r", "--full-tree", commit])
}

fn branch_shape(branch: &str) -> &str {
    branch.rsplit_once('-').map_or(branch, |(prefix, suffix)| {
        if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            prefix
        } else {
            branch
        }
    })
}

#[test]
fn an_import_with_goes_is_one_durable_write() {
    // Tom, 3 Oct: a batch is one durable operation with one full flush,
    // never leaf by leaf. A log with goes is no different.
    let state = going(true);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 15, "{}", report.report);
    let packs = fs::read_dir(tree.path().join("objects/pack"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "pack")
        })
        .count();
    assert_eq!(packs, 1, "one flush writes one pack");
}

#[test]
fn an_import_that_fails_after_a_go_names_no_line() {
    // Nothing the import wrote is a line until it has written all of it:
    // a failure part way leaves no trunk, so the import can run again.
    let state = going(false);
    rewrite(state.path(), |changes| {
        changes[12].seat = "not\na seat".into();
    });
    let tree = tempfile::tempdir().unwrap();
    assert!(grow(state.path(), tree.path(), false).is_err());
    assert_eq!(
        command(tree.path(), &["for-each-ref", "--format=%(refname)"]),
        "",
        "no ref is named by an import that failed"
    );
    rewrite(state.path(), |changes| {
        changes[12].seat = "Tom".into();
    });
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 13, "{}", report.report);
}

#[test]
fn a_go_while_the_wall_is_not_known_keeps_the_gap_it_is_in() {
    // Change 11 is one the model cannot follow; the go at 12 to change 4
    // must not hide that 11 is not in the tree: the kept file after 14
    // closes the gap from 11, as it would without the go.
    let (state, _, mut changes) = fixture(10);
    with_start(state.path());
    let mut moment = seed_file();
    *cutoff(&mut moment) = 304.0;
    changes.push(change(11, What::Unknown));
    changes.push(go(12, Some(4), &moment));
    for seq in 13..=14 {
        let to = f64::from(u32::try_from(seq).unwrap()) + 400.0;
        changes.push(turn(seq, *cutoff(&mut moment), to));
        *cutoff(&mut moment) = to;
    }
    moment.last_seq = 14;
    moment.wall = 12;
    fs::write(
        state.path().join("patch.before-15.json"),
        serde_json::to_vec(&moment).unwrap(),
    )
    .unwrap();
    write_state(state.path(), &moment, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert!(
        report.report.contains("changes 11 to 14"),
        "{}",
        report.report
    );
    assert_eq!(report.changes, 10, "{}", report.report);
}

#[test]
fn a_tree_whose_wall_went_on_elsewhere_is_in_already() {
    // After an import that ends on a bud, the live wall goes back to the
    // trunk and grows another bud there: the import is still in.
    let state = going(false);
    let tree = tempfile::tempdir().unwrap();
    grow(state.path(), tree.path(), false).unwrap();
    command(tree.path(), &["branch", "bud/elsewhere", "trunk~5"]);
    command(
        tree.path(),
        &["symbolic-ref", "HEAD", "refs/heads/bud/elsewhere"],
    );
    let again = grow(state.path(), tree.path(), false).unwrap();
    assert!(
        again.report.contains("the first rings are in already"),
        "{}",
        again.report
    );
}
