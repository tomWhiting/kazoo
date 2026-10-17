use std::fs;
use std::time::Instant;

use super::fixtures::{
    ask, at, birth, change, change_sequences, command, commit_of, commits, cutoff, files_of,
    fixture, history, kept, load, mixed_changes, module, moved_wall, real_wall, save, seed_file,
    snapshot, turn, turn_request, version_1, with_start, write_state,
};
use super::{RingsError, grow};
use crate::daemon::wall::Wall;
use crate::patch::{Patch, PatchFile};
use crate::protocol::{Request, What};
use crate::store::Store;
use crate::tree::{self, Birth, git};

#[test]
fn a_real_walls_log_grows_to_the_wall_it_kept() {
    let state = tempfile::tempdir().unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    mixed_changes(&mut wall);
    ask(&mut wall, &Request::Record { on: true });
    ask(&mut wall, &Request::Record { on: false });
    let named = |name: &str| Request::Save {
        name: name.into(),
        overwrite: false,
    };
    ask(&mut wall, &named("kept"));
    ask(&mut wall, &turn_request("vcf1", "cutoff", 444.0, None));
    let loaded = ask(
        &mut wall,
        &Request::Load {
            name: "kept".into(),
        },
    )
    .unwrap();
    ask(&mut wall, &Request::Undo { change: loaded });
    ask(&mut wall, &Request::New);
    let through = wall.patch().last_seq;
    for seq in through + 1..=120 {
        let bpm = 100.0 + f64::from(u32::try_from(seq).unwrap());
        wall.request(&format!("Seat{seq}"), &Request::tempo(bpm), Instant::now())
            .unwrap();
    }
    wall.close(Instant::now());
    let file = crate::migrate::read(&fs::read(state.path().join("patch.json")).unwrap())
        .unwrap()
        .0;
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    let expected = Patch::load(&file).unwrap().0.to_file();
    assert_eq!(
        files_of(tree.path(), "trunk"),
        tree::to_files(&expected).unwrap()
    );
    assert_eq!(report.changes, 120);
    assert_eq!((report.gaps, report.settles), (0, 0), "{report}");
    assert_eq!(change_sequences(tree.path()), (1..=120).collect::<Vec<_>>());
    assert!(!history(tree.path()).contains("Kazoo-Kind: gap"));
}

#[test]
fn the_wall_before_the_first_change_is_worked_back_from_the_earliest_file() {
    let state = tempfile::tempdir().unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    let began = wall.patch().to_file();
    mixed_changes(&mut wall);
    let through = wall.patch().last_seq;
    for seq in through + 1..=40 {
        let value = 500.0 + f64::from(u32::try_from(seq).unwrap());
        wall.request(
            &format!("Seat{seq}"),
            &turn_request("vcf1", "cutoff", value, Some(1.0)),
            Instant::now(),
        )
        .unwrap();
    }
    wall.close(Instant::now());
    assert_eq!(wall.patch().last_seq, 40);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    let all = commits(tree.path());
    assert_eq!(all[0].1, "start");
    assert_eq!(
        all[0].4,
        "the wall before change 1, worked back from patch.json"
    );
    let start = kept(tree.path(), &all[0].0);
    assert_eq!(start.modules, began.modules);
    assert!(start.modules.iter().all(|m| {
        m.birth.as_deref()
            == Some(
                &Birth::Found {
                    wall: began.wall,
                    seed: began.seed,
                    id: m.id.clone(),
                }
                .to_string(),
            )
    }));
    assert_eq!(start.cables, began.cables);
    assert_eq!(
        (start.tempo.to_bits(), start.seed),
        (began.tempo.to_bits(), began.seed)
    );
    assert_eq!(start.rack, began.rack);
    // The fingerprints before the first change are not worked back: none.
    assert!(start.dye.is_empty());
    // Replayed from there, they are the file's again: nothing to settle.
    assert_eq!((report.gaps, report.settles), (0, 0), "{report}");
    assert_eq!(change_sequences(tree.path()), (1..=40).collect::<Vec<_>>());
}

#[test]
fn a_tempo_the_desk_moved_is_a_gap_and_the_walk_goes_on() {
    let (state, mut file, mut changes) = fixture(12);
    with_start(state.path());
    changes[3] = change(
        4,
        What::Tempo {
            from: 92.0,
            to: 60.0,
            desk: false,
            after_beats: 0.0,
            glide_beats: 0.0,
        },
    );
    changes[8] = change(
        9,
        What::Tempo {
            from: 120.0,
            to: 80.0,
            desk: false,
            after_beats: 0.0,
            glide_beats: 0.0,
        },
    );
    changes[4] = turn(5, 303.0, 305.0);
    changes[9] = turn(10, 308.0, 310.0);
    file.tempo = 80.0;
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!((report.gaps, report.changes), (1, 12), "{report}");
    let all = commits(tree.path());
    let gap = all.iter().position(|c| c.1 == "gap").unwrap();
    assert_eq!(
        all[gap].4,
        "the tempo was 120 BPM, not 60 BPM as the log has it since change 4"
    );
    assert_eq!(all[gap].2, 8);
    assert_eq!(all[gap].3, i64::try_from(at(9)).unwrap());
    assert_eq!((all[gap + 1].1.as_str(), all[gap + 1].2), ("change", 9));
    assert_eq!(
        kept(tree.path(), &all[gap].0).tempo.to_bits(),
        120.0_f64.to_bits()
    );
    assert_eq!(change_sequences(tree.path()), (1..=12).collect::<Vec<_>>());
    assert!(
        report
            .report
            .contains("gap before change 9: the tempo was 120 BPM")
    );
}

#[test]
fn a_knob_the_log_disagrees_on_says_since_when() {
    let (state, file, mut changes) = fixture(6);
    with_start(state.path());
    changes[4] = turn(5, 299.0, 305.0);
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    grow(state.path(), tree.path(), false).unwrap();
    assert!(
        history(tree.path())
            .contains("vcf1.cutoff was 299, not 304 as the log has it since change 4")
    );
}

#[test]
fn a_load_whose_wall_was_not_kept_is_one_gap_until_the_next_file() {
    let (state, file, mut changes) = fixture(30);
    with_start(state.path());
    changes[4] = change(
        5,
        What::Load {
            save: Some("gone".into()),
            backup: "missing".into(),
            fingerprint: "missing".into(),
            modules: 2,
            cables: 0,
            tempo: 92.0,
            by_console: false,
        },
    );
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!((report.changes, report.gaps), (4, 1), "{report}");
    assert_eq!(change_sequences(tree.path()), vec![1, 2, 3, 4]);
    let gap = commit_of(tree.path(), "gap", 30);
    let body = command(tree.path(), &["show", "-s", "--format=%B", &gap]);
    assert!(body.contains("Kazoo-Gap: 5-30"), "{body}");
    assert!(body.contains("(26 changes: Tom 26)"), "{body}");
    assert!(
        body.contains("the save \"gone\") was not kept exactly (it could not be read"),
        "{body}"
    );
    assert_eq!(
        files_of(tree.path(), &gap),
        tree::to_files(&Patch::load(&file).unwrap().0.to_file()).unwrap()
    );
}

#[test]
fn a_file_that_disagrees_with_the_log_is_adopted_by_a_gap_commit() {
    let (state, mut file, changes) = fixture(3);
    with_start(state.path());
    *cutoff(&mut file) = 999.0;
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.gaps, 1);
    assert!(history(tree.path()).contains(
        "after change 3 the wall was as patch.json has it, not as the log makes it: vcf1.cutoff differs"
    ));
    assert_eq!(
        cutoff(&mut kept(tree.path(), "trunk")).to_bits(),
        999.0_f64.to_bits()
    );
}

#[test]
fn a_file_without_a_knob_at_its_default_agrees() {
    let (state, mut file, changes) = fixture(3);
    with_start(state.path());
    let patch = Patch::load(&file).unwrap().0;
    let mut dye = crate::fingerprints::Dye::new();
    for change in &changes {
        dye.touch(&change.seat, &change.what, &patch);
    }
    file.dye = dye.deposits().clone();
    let vcf = file.modules.iter_mut().find(|m| m.id == "vcf1").unwrap();
    let spec = crate::catalogue::Kind::from_name("vcf").unwrap().spec();
    let knob = spec
        .knobs
        .iter()
        .find(|k| super::model::same(vcf.knobs[k.name], f64::from(k.default)))
        .unwrap();
    vcf.knobs.remove(knob.name);
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!((report.gaps, report.settles), (0, 0), "{report}");
}

#[test]
fn rows_moved_without_a_log_are_settled_by_their_own_commit() {
    let (state, saved) = moved_wall(true);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!((report.settles, report.gaps), (1, 0), "{report}");
    let settle = commit_of(tree.path(), "settle", saved);
    let file = crate::migrate::read(&fs::read(state.path().join("saves/moved.json")).unwrap())
        .unwrap()
        .0;
    assert_eq!(kept(tree.path(), &settle).rack, file.rack);
    assert!(
        command(tree.path(), &["show", "-s", "--format=%s", &settle]).starts_with(
            "the rack's rows, groups, fingerprints and words as moved.json kept them after change"
        )
    );
    let (still, _) = moved_wall(false);
    let other = tempfile::tempdir().unwrap();
    let report = grow(still.path(), other.path(), false).unwrap();
    assert_eq!((report.settles, report.gaps), (0, 0), "{report}");
}

#[test]
fn a_version_1_stretch_is_the_version_2_wall_its_migration_makes() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    version_1(&state);
    let daemon = crate::daemon::Daemon::start_with_tree(
        crate::daemon::DaemonConfig::headless(root.path(), &state),
        Some(root.path().join("tree")),
    )
    .unwrap();
    daemon.stop();
    daemon.wait().unwrap();
    let store = Store::open(&state).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no patch.json");
    };
    let changes = store.load_log().unwrap().changes;
    assert!(matches!(changes[5].what, What::Migrate { .. }));
    let (_engine, control) =
        crate::engine::engine(crate::engine::EngineConfig::new(48_000, 92.0, 0.0), None);
    let mut wall = Wall::new(*patch, control, None, changes, Some(store));
    for value in [3000.0, 4000.0, 5000.0, 6000.0, 7000.0] {
        ask(&mut wall, &turn_request("delay1", "highcut", value, None));
    }
    wall.close(Instant::now());
    drop(wall);
    let tree = root.path().join("tree");
    let report = grow(&state, &tree, false).unwrap();
    assert_eq!((report.changes, report.gaps), (11, 0), "{report}");
    let all = commits(&tree);
    assert!(
        all[0]
            .4
            .starts_with("the wall before change 1, worked back from patch.before-"),
        "{}",
        all[0].4
    );
    let right = |file: &PatchFile| -> Vec<u32> {
        file.cables
            .iter()
            .filter(|c| {
                c.from
                    .rsplit_once('.')
                    .is_some_and(|(_, port)| port == "right")
                    || c.to
                        .rsplit_once('.')
                        .is_some_and(|(_, port)| port == "right")
            })
            .map(|c| c.id)
            .collect()
    };
    let migrated = kept(&tree, &commit_of(&tree, "change", 6));
    assert!(!right(&migrated).is_empty());
    assert_ne!(migrated.seed, 0);
    for commit in all.iter().filter(|c| c.2 < 6) {
        let file = kept(&tree, &commit.0);
        let delay = file.modules.iter().find(|m| m.id == "delay1").unwrap();
        assert_eq!(delay.kind, "digital", "{}", commit.4);
        assert!(delay.knobs.contains_key("highcut"), "{}", commit.4);
        assert_eq!(right(&file), right(&migrated), "{}", commit.4);
        assert_eq!(file.seed, 0, "{}", commit.4);
    }
    // The high cut follows the old tone: change 1 turned it.
    let highcut = |rev: &str| {
        kept(&tree, rev)
            .modules
            .into_iter()
            .find(|m| m.id == "delay1")
            .unwrap()
            .knobs["highcut"]
    };
    assert!((highcut(&all[0].0) - highcut(&all[1].0)).abs() > f64::EPSILON);
}

fn birth_lineage_state() -> tempfile::TempDir {
    let state = tempfile::tempdir().unwrap();
    fs::create_dir_all(state.path().join("saves")).unwrap();
    with_start(state.path());
    let mut wall = seed_file();
    let mut changes = (1..=4)
        .map(|seq| {
            let to = 300.0 + f64::from(u32::try_from(seq).unwrap());
            let made = turn(seq, *cutoff(&mut wall), to);
            *cutoff(&mut wall) = to;
            made
        })
        .collect::<Vec<_>>();
    let mut vco3 = module(&wall, "vco1");
    vco3.id = "vco3".into();
    changes.push(change(
        5,
        What::Add {
            module: vco3.clone(),
        },
    ));
    wall.modules.push(vco3);
    changes.push(turn(6, 304.0, 306.0));
    *cutoff(&mut wall) = 306.0;
    wall.last_seq = 6;
    let kept_wall = wall.clone();
    save(state.path(), "kept", &kept_wall, 7);
    let mut lfo3 = module(&wall, "lfo1");
    lfo3.id = "lfo3".into();
    changes.push(change(
        7,
        What::Add {
            module: lfo3.clone(),
        },
    ));
    wall.modules.push(lfo3);
    for seq in 8..=11 {
        let to = 300.0 + f64::from(u32::try_from(seq).unwrap());
        changes.push(turn(seq, *cutoff(&mut wall), to));
        *cutoff(&mut wall) = to;
    }
    wall.last_seq = 11;
    let backup = save(state.path(), "backup12", &wall, 12);
    changes.push(load(12, "kept", "backup12", backup, &kept_wall));
    let mut loaded = kept_wall;
    changes.push(turn(13, 306.0, 313.0));
    *cutoff(&mut loaded) = 313.0;
    loaded.last_seq = 13;
    loaded.wall = 12;
    // A save of another wall, never grown here.
    let mut other = seed_file();
    other.wall = 99;
    other.seed = 7;
    other.last_seq = 50;
    save(state.path(), "other", &other, 14);
    let backup = save(state.path(), "backup14", &loaded, 14);
    changes.push(load(14, "other", "backup14", backup, &other));
    changes.push(turn(15, *cutoff(&mut other), 315.0));
    *cutoff(&mut other) = 315.0;
    other.last_seq = 15;
    other.wall = 14;
    write_state(state.path(), &other, &changes);
    state
}

#[test]
fn births_follow_the_log_and_cross_loads_of_the_same_wall() {
    let state = birth_lineage_state();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = grow(state.path(), first.path(), false).unwrap();
    let b = grow(state.path(), second.path(), false).unwrap();
    assert_eq!(a.trunk, b.trunk);
    assert_eq!((a.gaps, a.changes), (0, 15), "{a}");
    let tree = first.path();
    let born = birth(tree, &commit_of(tree, "change", 5), "vco3");
    assert!(born.starts_with("born 5.0 "), "{born}");
    assert_eq!(birth(tree, &commit_of(tree, "change", 12), "vco3"), born);
    assert_eq!(birth(tree, &commit_of(tree, "change", 13), "vco3"), born);
    assert!(birth(tree, &commit_of(tree, "change", 7), "lfo3").starts_with("born 7.0 "));
    assert!(
        !kept(tree, &commit_of(tree, "change", 12))
            .modules
            .iter()
            .any(|m| m.id == "lfo3")
    );
    let found = Birth::Found {
        wall: 99,
        seed: 7,
        id: "vco1".into(),
    };
    assert_eq!(
        birth(tree, &commit_of(tree, "change", 14), "vco1"),
        found.to_string()
    );
    assert_eq!(
        birth(tree, "trunk", "vco1"),
        Birth::Found {
            wall: 14,
            seed: 7,
            id: "vco1".into(),
        }
        .to_string()
    );
    let n = seed_file().modules.len();
    let births = format!(
        "births: {n} found at the start, 2 at adds, {} kept across loads, {n} found at loads\n",
        n + 1
    );
    assert!(a.report.contains(&births), "{a}");
}

#[test]
fn abandoned_changes_are_not_grown() {
    let (state, mut file, mut changes) = fixture(8);
    with_start(state.path());
    changes[7] = change(8, What::Abandoned { change: 7 });
    *cutoff(&mut file) = 306.0;
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!((report.changes, report.gaps), (7, 0), "{report}");
    assert_eq!(change_sequences(tree.path()), vec![1, 2, 3, 4, 5, 6, 8]);
}

#[test]
fn a_file_after_an_abandoned_change_still_holds_the_wall() {
    let (state, mut file, mut changes) = fixture(8);
    with_start(state.path());
    changes[7] = change(8, What::Abandoned { change: 7 });
    *cutoff(&mut file) = 306.0;
    file.last_seq = 7;
    file.rack.reverse();
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.settles, 1, "{report}");
    assert_eq!(
        kept(tree.path(), &commit_of(tree.path(), "settle", 7)).rack,
        file.rack
    );
}

#[test]
fn a_dry_run_writes_nothing_in_the_tree() {
    let (state, _, _) = fixture(3);
    with_start(state.path());
    let parent = tempfile::tempdir().unwrap();
    let tree = parent.path().join("tree");
    let dry = grow(state.path(), &tree, true).unwrap();
    assert!(dry.report.contains("(dry run: grown in "), "{dry}");
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    git::plant(&tree).unwrap();
    let before = snapshot(&tree);
    let dry2 = grow(state.path(), &tree, true).unwrap();
    assert_eq!(snapshot(&tree), before);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
    let real = grow(state.path(), &tree, false).unwrap();
    for run in [&dry, &dry2] {
        assert_eq!(
            (&run.trunk, run.changes, run.gaps, run.settles),
            (&real.trunk, real.changes, real.gaps, real.settles)
        );
        let lines = |report: &str| {
            report
                .lines()
                .skip(1)
                .filter(|l| !l.starts_with("took "))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(&run.report), lines(&real.report));
    }
}

#[test]
fn running_twice_writes_nothing_the_second_time() {
    let (state, _, _) = fixture(3);
    with_start(state.path());
    let tree = tempfile::tempdir().unwrap();
    let first = grow(state.path(), tree.path(), false).unwrap();
    let before = snapshot(tree.path());
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(
        report.report,
        format!(
            "the first rings are in already: trunk holds changes 1 to 3 from the import (rings/end at {}), and 0 commits since\n",
            &first.trunk[..12]
        )
    );
    assert_eq!(snapshot(tree.path()), before);
}

#[test]
fn a_trunk_the_import_did_not_grow_is_refused() {
    let (state, _, _) = fixture(3);
    with_start(state.path());
    let tree = tempfile::tempdir().unwrap();
    git::plant(tree.path()).unwrap();
    let mut importer = git::Importer::new(tree.path(), &git::read(tree.path()).unwrap()).unwrap();
    let commit = git::Commit::change(&turn(1, 10.0, 20.0), Vec::new()).unwrap();
    let mark = importer.commit(&commit).unwrap();
    importer.checkpoint(mark).unwrap();
    importer.finish().unwrap();
    let before = snapshot(tree.path());
    assert!(matches!(
        grow(state.path(), tree.path(), false),
        Err(RingsError::HasTrunk(_))
    ));
    assert_eq!(snapshot(tree.path()), before);
}

#[test]
fn an_import_that_stopped_before_trunk_finishes() {
    let (state, _, _) = fixture(3);
    with_start(state.path());
    let tree = tempfile::tempdir().unwrap();
    let result = grow(state.path(), tree.path(), false).unwrap();
    command(tree.path(), &["update-ref", "-d", "refs/heads/trunk"]);
    let before = snapshot(tree.path());
    let dry = grow(state.path(), tree.path(), true).unwrap();
    assert!(
        dry.report
            .starts_with("an import stopped before naming trunk"),
        "{dry}"
    );
    assert_eq!(snapshot(tree.path()), before);
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.trunk, result.trunk);
    assert!(
        report
            .report
            .starts_with("finished an import that stopped before naming trunk")
    );
    let mut after = snapshot(tree.path());
    assert_eq!(
        after.remove("refs/heads/trunk"),
        Some(format!("{}\n", result.trunk).into_bytes())
    );
    assert_eq!(after, before);
}

#[test]
fn the_import_refuses_while_the_wall_holds_its_lock() {
    let (state, _, _) = fixture(3);
    with_start(state.path());
    let held = Store::open(state.path()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let tree = parent.path().join("tree");
    assert!(matches!(
        grow(state.path(), &tree, false),
        Err(RingsError::WallRunning)
    ));
    assert!(!tree.exists());
    drop(held);
    grow(state.path(), &tree, false).unwrap();
}

#[test]
fn an_import_from_a_state_directory_that_is_not_there_makes_none() {
    let parent = tempfile::tempdir().unwrap();
    let state = parent.path().join("wall");
    let tree = parent.path().join("tree");
    assert!(matches!(
        grow(&state, &tree, false),
        Err(RingsError::Read(_))
    ));
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
}

#[test]
fn the_same_log_grows_the_same_trunk_anywhere() {
    let (state, _, _) = fixture(8);
    with_start(state.path());
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = grow(state.path(), first.path(), false).unwrap();
    let b = grow(state.path(), second.path(), false).unwrap();
    assert_eq!(a.trunk, b.trunk);
    for commit in commits(first.path()) {
        assert_eq!(commit.3, i64::try_from(at(commit.2.max(1))).unwrap());
    }
}

#[test]
fn the_report_says_where_trunk_ends_when_the_log_runs_on_unknown() {
    let (state, file, mut changes) = fixture(20);
    with_start(state.path());
    for seq in 21..=30 {
        let to = 300.0 + f64::from(u32::try_from(seq).unwrap());
        changes.push(turn(seq, to - 1.0, to));
    }
    changes[24] = load(25, "gone", "missing", "missing".into(), &file);
    let log = changes
        .iter()
        .map(|c| serde_json::to_string(c).unwrap() + "\n")
        .collect::<String>();
    fs::write(state.path().join("log.jsonl"), log).unwrap();
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    let line = report
        .report
        .lines()
        .find(|l| l.starts_with("trunk: "))
        .unwrap();
    assert_eq!(
        line,
        format!(
            "trunk: {} (change 24, {}); changes 25 to 30 are not in it: the wall change 25 put up (the save \"gone\") was not kept exactly (it could not be read: No such file or directory (os error 2)); patch.json is after change 20: the wall's next start says so",
            report.trunk,
            crate::change::utc(at(24))
        )
    );
}

#[test]
fn knobs_this_build_does_not_have_are_named_in_the_report() {
    let (state, mut file, changes) = fixture(3);
    let mut start = seed_file();
    for record in [&mut start, &mut file] {
        let vcf = record.modules.iter_mut().find(|m| m.id == "vcf1").unwrap();
        vcf.knobs.insert("warmth".into(), 0.5);
    }
    fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert!(
        report
            .report
            .contains("this build left out: 1 knobs, 0 modules (vcf1.warmth)\n"),
        "{report}"
    );
}
