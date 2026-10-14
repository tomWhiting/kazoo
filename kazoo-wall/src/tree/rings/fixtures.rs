use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use crate::daemon::wall::Wall;
use crate::patch::{PatchFile, hash_bytes};
use crate::protocol::{Change, ModuleRecord, Request, What};
use crate::store::Store;
use crate::tree::{self, git};

pub(super) fn command(tree: &Path, args: &[&str]) -> String {
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

pub(super) fn history(tree: &Path) -> String {
    command(tree, &["log", "--reverse", "--format=%B", "trunk"])
}

/// Every commit on trunk, oldest first: id, kind, seq, author date, subject.
pub(super) fn commits(tree: &Path) -> Vec<(String, String, u64, i64, String)> {
    command(
        tree,
        &[
            "log",
            "--reverse",
            "--format=%H|%(trailers:key=Kazoo-Kind,valueonly,separator=%x2C)|%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C)|%at|%s",
            "trunk",
        ],
    )
    .lines()
    .map(|line| {
        let mut parts = line.splitn(5, '|');
        let mut next = || parts.next().unwrap().to_string();
        let (id, kind, seq, at, subject) = (next(), next(), next(), next(), next());
        (id, kind, seq.parse().unwrap(), at.parse().unwrap(), subject)
    })
    .collect()
}

pub(super) fn change_sequences(tree: &Path) -> Vec<u64> {
    commits(tree)
        .into_iter()
        .filter(|c| c.1 == "change")
        .map(|c| c.2)
        .collect()
}

/// The id of the commit of kind `kind` after change `seq`.
pub(super) fn commit_of(tree: &Path, kind: &str, seq: u64) -> String {
    commits(tree)
        .into_iter()
        .find(|c| c.1 == kind && c.2 == seq)
        .unwrap_or_else(|| panic!("no {kind} commit for {seq}"))
        .0
}

pub(super) fn kept(tree: &Path, rev: &str) -> PatchFile {
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

pub(super) fn files_of(tree: &Path, rev: &str) -> tree::Files {
    tree::to_files(&kept(tree, rev)).unwrap()
}

pub(super) fn seed_file() -> PatchFile {
    let mut file = crate::seed::seed().unwrap().to_file();
    file.seed = 0;
    for module in &mut file.modules {
        module.birth = None;
    }
    file
}

pub(super) const fn at(seq: u64) -> u64 {
    1_790_000_000 + seq
}

pub(super) fn change(seq: u64, what: What) -> Change {
    Change {
        seq,
        at: crate::change::utc(at(seq)),
        seat: "Tom".into(),
        summary: crate::change::summarise("Tom", &what, None),
        what,
        undoes: None,
        clock: None,
    }
}

pub(super) fn turn_of(seq: u64, module: &str, knob: &str, from: f64, to: f64) -> Change {
    change(
        seq,
        What::Turn {
            module: module.into(),
            knob: knob.into(),
            from,
            to,
            glide_beats: 0.0,
        },
    )
}

pub(super) fn turn(seq: u64, from: f64, to: f64) -> Change {
    turn_of(seq, "vcf1", "cutoff", from, to)
}

pub(super) fn cutoff(file: &mut PatchFile) -> &mut f64 {
    file.modules
        .iter_mut()
        .find(|m| m.id == "vcf1")
        .unwrap()
        .knobs
        .get_mut("cutoff")
        .unwrap()
}

pub(super) fn write_state(state: &Path, file: &PatchFile, changes: &[Change]) {
    fs::create_dir_all(state.join("saves")).unwrap();
    fs::write(state.join("patch.json"), serde_json::to_vec(file).unwrap()).unwrap();
    let log = changes
        .iter()
        .map(|c| serde_json::to_string(c).unwrap() + "\n")
        .collect::<String>();
    fs::write(state.join("log.jsonl"), log).unwrap();
}

/// A save written as the wall writes one, last written before `before`'s
/// time; returns its fingerprint.
pub(super) fn save(state: &Path, name: &str, file: &PatchFile, before: u64) -> String {
    let path = state.join("saves").join(format!("{name}.json"));
    let bytes = serde_json::to_vec(file).unwrap();
    fs::write(&path, &bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(at(before) * 1000 - 500))
        .unwrap();
    hash_bytes(&bytes)
}

/// Changes 1 to `n` turning `vcf1.cutoff` on the seed; `patch.json` after
/// them, and `patch.before-1.json` (the seed, after change 0) beside it.
pub(super) fn fixture(n: u64) -> (tempfile::TempDir, PatchFile, Vec<Change>) {
    let state = tempfile::tempdir().unwrap();
    let mut file = seed_file();
    let mut changes = Vec::new();
    for seq in 1..=n {
        let to = 300.0 + f64::from(u32::try_from(seq).unwrap());
        changes.push(turn(seq, *cutoff(&mut file), to));
        *cutoff(&mut file) = to;
    }
    file.last_seq = n;
    write_state(state.path(), &file, &changes);
    (state, file, changes)
}

pub(super) fn with_start(state: &Path) {
    fs::write(
        state.join("patch.before-1.json"),
        serde_json::to_vec(&seed_file()).unwrap(),
    )
    .unwrap();
}

pub(super) fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
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

/// A seeded wall on `state`'s store, as the daemon runs one.
pub(super) fn real_wall(state: &Path) -> (crate::engine::Engine, Wall) {
    let store = Store::peek(state).unwrap();
    let (engine, control) =
        crate::engine::engine(crate::engine::EngineConfig::new(48_000, 92.0, 0.0), None);
    let mut wall = Wall::new(
        crate::seed::seed().unwrap(),
        control,
        None,
        Vec::new(),
        Some(store),
    );
    wall.set_recordings_dir(state.join("recordings"));
    (engine, wall)
}

pub(super) fn ask(wall: &mut Wall, request: &Request) -> Option<u64> {
    wall.request("Tom", request, Instant::now())
        .unwrap()
        .change
        .map(|c| c.seq)
}

pub(super) fn turn_request(module: &str, knob: &str, value: f64, glide: Option<f64>) -> Request {
    Request::Turn {
        module: module.into(),
        knob: knob.into(),
        value,
        glide_beats: glide,
    }
}

/// Turns with glides, a patch, a patch that replaces it, an unpatch, an
/// add patched in, its removal and the removal's undo (a restore), a
/// turn's undo and an own-clock tempo.
pub(super) fn mixed_changes(wall: &mut Wall) {
    ask(wall, &turn_request("vcf1", "cutoff", 333.0, Some(0.25)));
    for (from, amount) in [("lfo1.out", 0.3), ("vco1.out", 0.2)] {
        ask(
            wall,
            &Request::Patch {
                from: from.into(),
                to: "vcf1.drive".into(),
                amount: Some(amount),
            },
        );
    }
    ask(
        wall,
        &Request::Unpatch {
            cable: None,
            to: Some("vcf1.drive".into()),
        },
    );
    ask(
        wall,
        &Request::Add {
            kind: "vco".into(),
            name: None,
            place: None,
        },
    );
    let added = wall.patch().to_file().modules.last().unwrap().id.clone();
    ask(
        wall,
        &Request::Patch {
            from: format!("{added}.out"),
            to: "mix1.c".into(),
            amount: Some(0.5),
        },
    );
    let removed = ask(wall, &Request::Remove { module: added }).unwrap();
    ask(wall, &Request::Undo { change: removed });
    ask(wall, &Request::Undo { change: 1 });
    ask(wall, &Request::Tempo { bpm: 100.0 });
}

pub(super) fn moved_wall(arrange: bool) -> (tempfile::TempDir, u64) {
    let state = tempfile::tempdir().unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    let named = |name: &str| Request::Save {
        name: name.into(),
        overwrite: false,
    };
    ask(&mut wall, &turn_request("vcf1", "cutoff", 700.0, None));
    ask(&mut wall, &named("plain"));
    ask(&mut wall, &turn_request("vcf1", "cutoff", 750.0, None));
    if arrange {
        let arranged = wall.request(
            "Tom",
            &Request::Arrange {
                module: "vco1".into(),
                row: 0,
                before: None,
                own: true,
            },
            Instant::now(),
        );
        assert!(arranged.unwrap().change.is_none());
    }
    ask(&mut wall, &named("moved"));
    let saved = wall.patch().last_seq;
    ask(&mut wall, &turn_request("vcf1", "cutoff", 800.0, None));
    wall.close(Instant::now());
    (state, saved)
}

/// A version-1 `patch.json` after five turns of its own log: a vco, a
/// delay and a reverb in a chain to an out, as the wall kept it on 26 Sep.
pub(super) fn version_1(state: &Path) {
    let file = serde_json::json!({
        "version": 1, "tempo": 92.0, "last_seq": 5, "next_cable": 4,
        "next_ids": {"vco": 2, "delay": 2, "reverb": 2, "out": 2},
        "modules": [
            {"id": "vco1", "kind": "vco", "knobs": {"level": 0.6}},
            {"id": "delay1", "kind": "delay",
             "knobs": {"time": 0.3, "sync": 0.0, "feedback": 0.45, "tone": 0.7, "mix": 0.3}},
            {"id": "reverb1", "kind": "reverb", "knobs": {"size": 0.6, "damping": 0.4, "mix": 0.25}},
            {"id": "out1", "kind": "out", "knobs": {"level": 0.75}}
        ],
        "cables": [
            {"id": 1, "from": "vco1.out", "to": "delay1.in", "amount": 1.0},
            {"id": 2, "from": "delay1.out", "to": "reverb1.in", "amount": 1.0},
            {"id": 3, "from": "reverb1.out", "to": "out1.left", "amount": 1.0}
        ]
    });
    let changes = [
        turn_of(1, "delay1", "tone", 0.5, 0.7),
        turn_of(2, "delay1", "feedback", 0.4, 0.45),
        turn_of(3, "reverb1", "size", 0.5, 0.6),
        turn_of(4, "vco1", "level", 0.7, 0.6),
        turn_of(5, "out1", "level", 0.8, 0.75),
    ];
    fs::create_dir_all(state).unwrap();
    fs::write(state.join("patch.json"), file.to_string()).unwrap();
    let log = changes
        .iter()
        .map(|c| serde_json::to_string(c).unwrap() + "\n")
        .collect::<String>();
    fs::write(state.join("log.jsonl"), log).unwrap();
}

pub(super) fn module(file: &PatchFile, id: &str) -> ModuleRecord {
    file.modules.iter().find(|m| m.id == id).unwrap().clone()
}

pub(super) fn birth(tree: &Path, rev: &str, id: &str) -> String {
    module(&kept(tree, rev), id).birth.unwrap()
}

pub(super) fn load(
    seq: u64,
    save: &str,
    backup: &str,
    fingerprint: String,
    file: &PatchFile,
) -> Change {
    change(
        seq,
        What::Load {
            save: Some(save.into()),
            backup: backup.into(),
            fingerprint,
            modules: u32::try_from(file.modules.len()).unwrap(),
            cables: u32::try_from(file.cables.len()).unwrap(),
            tempo: file.tempo,
            by_console: false,
        },
    )
}
