use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use super::{RingsError, grow};
use crate::patch::{Patch, PatchFile};
use crate::protocol::{Change, Request, What};
use crate::store::Store;
use crate::tree::{self, git};

fn command(tree: &Path, args: &[&str]) -> String {
    let output = git::git_command(args[0], Some(tree))
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn history(tree: &Path) -> String {
    command(tree, &["log", "--reverse", "--format=%B", "trunk"])
}

fn kept(tree: &Path, rev: &str) -> PatchFile {
    let paths = command(tree, &["ls-tree", "-r", "--name-only", rev]);
    let files = paths
        .lines()
        .map(|path| {
            (
                path.to_string(),
                command(tree, &["show", &format!("{rev}:{path}")]).into_bytes(),
            )
        })
        .collect();
    tree::from_files(&files).unwrap()
}

fn seed_file() -> PatchFile {
    let mut file = crate::seed::seed().unwrap().to_file();
    file.seed = 0;
    for module in &mut file.modules {
        module.birth = None;
    }
    file
}

fn change(seq: u64, what: What) -> Change {
    Change {
        seq,
        at: crate::change::utc(1_790_000_000 + seq),
        seat: "Tom".into(),
        summary: crate::change::summarise("Tom", &what, None),
        what,
        undoes: None,
        clock: None,
    }
}

fn turn(seq: u64, from: f64, to: f64) -> Change {
    change(
        seq,
        What::Turn {
            module: "vcf1".into(),
            knob: "cutoff".into(),
            from,
            to,
            glide_beats: 0.0,
        },
    )
}

fn write_state(state: &Path, file: &PatchFile, changes: &[Change]) {
    fs::create_dir_all(state.join("saves")).unwrap();
    fs::write(state.join("patch.json"), serde_json::to_vec(file).unwrap()).unwrap();
    let log = changes
        .iter()
        .map(|c| serde_json::to_string(c).unwrap() + "\n")
        .collect::<String>();
    fs::write(state.join("log.jsonl"), log).unwrap();
}

fn fixture(n: u64) -> (tempfile::TempDir, PatchFile, Vec<Change>) {
    let state = tempfile::tempdir().unwrap();
    let mut file = seed_file();
    let mut changes = Vec::new();
    let knob = file
        .modules
        .iter_mut()
        .find(|m| m.id == "vcf1")
        .unwrap()
        .knobs
        .get_mut("cutoff")
        .unwrap();
    for seq in 1..=n {
        let to = 300.0 + f64::from(u32::try_from(seq).unwrap());
        changes.push(turn(seq, *knob, to));
        *knob = to;
    }
    file.last_seq = n;
    write_state(state.path(), &file, &changes);
    (state, file, changes)
}

fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(path, path, &mut out);
    out
}

fn initial_changes(wall: &mut crate::daemon::wall::Wall, state: &Path, now: Instant) {
    wall.set_recordings_dir(state.join("recordings"));
    let scripted = vec![
        Request::Turn {
            module: "vcf1".into(),
            knob: "cutoff".into(),
            value: 333.0,
            glide_beats: Some(0.25),
        },
        Request::Patch {
            from: "lfo1.out".into(),
            to: "vcf1.cutoff".into(),
            amount: Some(0.3),
        },
        Request::Patch {
            from: "vco1.out".into(),
            to: "vcf1.cutoff".into(),
            amount: Some(0.2),
        },
        Request::Unpatch {
            cable: None,
            to: Some("vcf1.cutoff".into()),
        },
        Request::Add {
            kind: "vco".into(),
            name: None,
            place: None,
        },
    ];
    for request in scripted {
        wall.request("Tom", &request, now).unwrap();
    }
}

fn change_sequences(tree: &Path) -> Vec<u64> {
    let sequences = command(
        tree,
        &[
            "log",
            "--reverse",
            "--format=%(trailers:key=Kazoo-Kind,valueonly,separator=%x2C)|%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C)",
            "trunk",
        ],
    );
    sequences
        .lines()
        .filter_map(|line| line.strip_prefix("change|"))
        .map(|n| n.parse::<u64>().unwrap())
        .collect::<Vec<_>>()
}

#[test]
fn a_real_walls_log_grows_to_the_wall_it_kept() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::peek(state.path()).unwrap();
    let (_engine, control) =
        crate::engine::engine(crate::engine::EngineConfig::new(48_000, 92.0, 0.0), None);
    let mut wall = crate::daemon::wall::Wall::new(
        crate::seed::seed().unwrap(),
        control,
        None,
        Vec::new(),
        Some(store),
    );
    let now = Instant::now();
    initial_changes(&mut wall, state.path(), now);
    let added = wall.snapshot().modules.last().unwrap().id.clone();
    let removed = wall
        .request("Tom", &Request::Remove { module: added }, now)
        .unwrap()
        .change
        .unwrap()
        .seq;
    wall.request("Tom", &Request::Undo { change: removed }, now)
        .unwrap();
    wall.request("Tom", &Request::Undo { change: 1 }, now)
        .unwrap();
    wall.request("Tom", &Request::Tempo { bpm: 100.0 }, now)
        .unwrap();
    wall.request("Tom", &Request::Record { on: true }, now)
        .unwrap();
    wall.request("Tom", &Request::Record { on: false }, now)
        .unwrap();
    wall.request(
        "Tom",
        &Request::Save {
            name: "kept".into(),
            overwrite: false,
        },
        now,
    )
    .unwrap();
    wall.request(
        "Tom",
        &Request::Turn {
            module: "vcf1".into(),
            knob: "cutoff".into(),
            value: 444.0,
            glide_beats: None,
        },
        now,
    )
    .unwrap();
    let loaded = wall
        .request(
            "Tom",
            &Request::Load {
                name: "kept".into(),
            },
            now,
        )
        .unwrap()
        .change
        .unwrap()
        .seq;
    wall.request("Tom", &Request::Undo { change: loaded }, now)
        .unwrap();
    wall.request("Tom", &Request::New, now).unwrap();
    let through = Store::peek(state.path())
        .unwrap()
        .load_log()
        .unwrap()
        .changes
        .last()
        .unwrap()
        .seq;
    for seq in through + 1..=120 {
        wall.request(
            &format!("Seat{seq}"),
            &Request::Tempo {
                bpm: 100.0 + f64::from(u32::try_from(seq).unwrap()),
            },
            now,
        )
        .unwrap();
    }
    wall.close(now);
    let file = crate::migrate::read(&fs::read(state.path().join("patch.json")).unwrap())
        .unwrap()
        .0;
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    let expected = Patch::load(&file).unwrap().0.to_file();
    assert_eq!(
        tree::to_files(&kept(tree.path(), "trunk")).unwrap(),
        tree::to_files(&expected).unwrap()
    );
    assert_eq!(report.changes, 120);
    assert_eq!(report.gaps, 0);
    assert_eq!(change_sequences(tree.path()), (1..=120).collect::<Vec<_>>());
}

#[test]
fn the_wall_before_the_first_change_is_worked_back_from_the_earliest_file() {
    let (state, _, _) = fixture(40);
    let tree = tempfile::tempdir().unwrap();
    grow(state.path(), tree.path(), false).unwrap();
    let root = command(tree.path(), &["rev-list", "--max-parents=0", "trunk"]);
    assert!(command(tree.path(), &["show", "-s", "--format=%s", &root]).contains("worked back"));
    let start = kept(tree.path(), &root);
    let seed = Patch::load(&seed_file()).unwrap().0.to_file();
    assert_eq!(start.modules, seed.modules);
}

#[test]
fn a_tempo_the_desk_moved_is_a_gap_and_the_walk_goes_on() {
    let (state, mut file, mut changes) = fixture(12);
    let mut start = seed_file();
    start.last_seq = 0;
    fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    changes[3] = change(
        4,
        What::Tempo {
            from: 92.0,
            to: 60.0,
            desk: false,
        },
    );
    changes[8] = change(
        9,
        What::Tempo {
            from: 120.0,
            to: 80.0,
            desk: false,
        },
    );
    // Keep the turns continuous when tempo changes replace them.
    changes[4] = turn(5, 303.0, 305.0);
    changes[9] = turn(10, 308.0, 310.0);
    file.tempo = 80.0;
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.gaps, 1);
    assert!(
        history(tree.path())
            .contains("the tempo was 120 BPM, not 60 BPM as the log has it since change 4")
    );
    assert_eq!(report.changes, 12);
}

#[test]
fn a_load_whose_wall_was_not_kept_is_one_gap_until_the_next_file() {
    let (state, file, mut changes) = fixture(30);
    fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&seed_file()).unwrap(),
    )
    .unwrap();
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
    assert_eq!(report.changes, 4);
    assert_eq!(report.gaps, 1);
    assert!(history(tree.path()).contains("Kazoo-Gap: 5-30"));
    assert!(history(tree.path()).contains("Tom 26"));
}

#[test]
fn a_file_that_disagrees_with_the_log_is_adopted_by_a_gap_commit() {
    let (state, mut file, changes) = fixture(3);
    fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&seed_file()).unwrap(),
    )
    .unwrap();
    file.modules
        .iter_mut()
        .find(|m| m.id == "vcf1")
        .unwrap()
        .knobs
        .insert("cutoff".into(), 999.0);
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.gaps, 1);
    assert!(history(tree.path()).contains("vcf1.cutoff"));
    assert_eq!(
        kept(tree.path(), "trunk")
            .modules
            .iter()
            .find(|m| m.id == "vcf1")
            .unwrap()
            .knobs["cutoff"]
            .to_bits(),
        999.0_f64.to_bits()
    );
}

#[test]
fn rows_moved_without_a_log_are_settled_by_their_own_commit() {
    let (state, mut file, changes) = fixture(3);
    fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&seed_file()).unwrap(),
    )
    .unwrap();
    file.rack.reverse();
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert!(report.settles > 0);
    assert!(history(tree.path()).contains("Kazoo-Kind: settle"));
    assert_eq!(kept(tree.path(), "trunk").rack, file.rack);
}

#[test]
fn a_version_1_stretch_is_the_version_2_wall_its_migration_makes() {
    let (state, mut file, changes) = fixture(5);
    file.version = 1;
    file.modules.push(crate::protocol::ModuleRecord {
        id: "delay9".into(),
        kind: "delay".into(),
        name: None,
        knobs: BTreeMap::from([
            ("tone".into(), 0.5),
            ("time".into(), 0.25),
            ("feedback".into(), 0.4),
            ("mix".into(), 0.5),
            ("sync".into(), 0.0),
        ]),
        birth: None,
    });
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    grow(state.path(), tree.path(), false).unwrap();
    let imported = kept(tree.path(), "trunk");
    assert_eq!(imported.seed, 0);
    let delay = imported.modules.iter().find(|m| m.id == "delay9").unwrap();
    assert_eq!(delay.kind, "digital");
    assert!(delay.knobs.contains_key("highcut"));
}

#[test]
fn births_follow_the_log_and_cross_loads_of_the_same_wall() {
    let (state, mut file, mut changes) = fixture(12);
    fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&seed_file()).unwrap(),
    )
    .unwrap();
    let mut newborn = file
        .modules
        .iter()
        .find(|m| m.id == "vco1")
        .unwrap()
        .clone();
    newborn.id = "vco3".into();
    changes[4] = change(
        5,
        What::Add {
            module: newborn.clone(),
        },
    );
    changes[5] = turn(6, 304.0, 306.0);
    file.modules.push(newborn);
    file.next_ids.insert("vco".into(), 4);
    write_state(state.path(), &file, &changes);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = grow(state.path(), first.path(), false).unwrap();
    let b = grow(state.path(), second.path(), false).unwrap();
    assert_eq!(a.trunk, b.trunk);
    assert!(
        kept(first.path(), "trunk")
            .modules
            .iter()
            .find(|m| m.id == "vco3")
            .unwrap()
            .birth
            .as_ref()
            .unwrap()
            .starts_with("born 5.0 ")
    );
}

#[test]
fn abandoned_changes_are_not_grown() {
    let (state, mut file, mut changes) = fixture(8);
    changes[7] = change(8, What::Abandoned { change: 7 });
    file.modules
        .iter_mut()
        .find(|m| m.id == "vcf1")
        .unwrap()
        .knobs
        .insert("cutoff".into(), 306.0);
    write_state(state.path(), &file, &changes);
    let tree = tempfile::tempdir().unwrap();
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.changes, 7);
    assert!(!history(tree.path()).contains("Kazoo-Seq: 7\n"));
}

#[test]
fn a_dry_run_writes_nothing_in_the_tree() {
    let (state, _, _) = fixture(3);
    let parent = tempfile::tempdir().unwrap();
    let tree = parent.path().join("tree");
    let dry = grow(state.path(), &tree, true).unwrap();
    assert!(!tree.exists());
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    git::plant(&tree).unwrap();
    let before = snapshot(&tree);
    let dry2 = grow(state.path(), &tree, true).unwrap();
    assert_eq!(snapshot(&tree), before);
    let real = grow(state.path(), &tree, false).unwrap();
    assert_eq!(dry.trunk, real.trunk);
    assert_eq!(dry2.trunk, real.trunk);
}

#[test]
fn running_twice_writes_nothing_the_second_time() {
    let (state, _, _) = fixture(3);
    let tree = tempfile::tempdir().unwrap();
    grow(state.path(), tree.path(), false).unwrap();
    let before = snapshot(tree.path());
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert!(report.report.contains("in already"));
    assert_eq!(snapshot(tree.path()), before);
}

#[test]
fn a_trunk_the_import_did_not_grow_is_refused() {
    let (state, _, _) = fixture(3);
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
    let tree = tempfile::tempdir().unwrap();
    let result = grow(state.path(), tree.path(), false).unwrap();
    command(tree.path(), &["update-ref", "-d", "refs/heads/trunk"]);
    let report = grow(state.path(), tree.path(), false).unwrap();
    assert_eq!(report.trunk, result.trunk);
    assert!(report.report.contains("stopped before naming trunk"));
}

#[test]
fn the_import_refuses_while_the_wall_holds_its_lock() {
    let (state, _, _) = fixture(3);
    let held = Store::open(state.path()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let tree = parent.path().join("tree");
    assert!(matches!(
        grow(state.path(), &tree, false),
        Err(RingsError::WallRunning)
    ));
    assert!(!tree.exists());
    drop(held);
}

#[test]
fn the_same_log_grows_the_same_trunk_anywhere() {
    let (state, _, _) = fixture(8);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    assert_eq!(
        grow(state.path(), first.path(), false).unwrap().trunk,
        grow(state.path(), second.path(), false).unwrap().trunk
    );
}
