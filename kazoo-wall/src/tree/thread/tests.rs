use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::daemon::wall::Wall;
use crate::engine::{Engine, EngineConfig, engine};
use crate::protocol::{Change, Request, What};
use crate::tree::{Files, Stamp, beats_between, from_files, git, thread};

const ALLOWED_GIT_COMMANDS: &[&str] = &[
    "version",
    "init",
    "config",
    "rev-parse",
    "symbolic-ref",
    "log",
    "fast-import",
    "repack",
];

fn receipt(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/kz-logs")
        .join(name)
}

fn command(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_string()
}

fn planted() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    dir
}

fn wall() -> (Wall, Engine) {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    (
        Wall::new(
            crate::seed::seed().unwrap(),
            control,
            None,
            Vec::new(),
            None,
        ),
        engine,
    )
}

fn turn(value: f64) -> Request {
    Request::Turn {
        module: "vcf1".into(),
        knob: "cutoff".into(),
        value,
        glide_beats: Some(4.0),
    }
}

fn ask(wall: &mut Wall, request: &Request, now: Instant) -> Change {
    wall.request("Tom", request, now).unwrap().change.unwrap()
}

fn commits(dir: &Path) -> Vec<String> {
    command(dir, &["log", "--reverse", "--format=%H"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn message(dir: &Path, sha: &str) -> String {
    command(dir, &["log", "-1", "--format=%B", sha])
}

fn files(dir: &Path, sha: &str) -> Files {
    command(dir, &["ls-tree", "-r", "--name-only", sha])
        .lines()
        .map(|path| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["show", &format!("{sha}:{path}")])
                .output()
                .unwrap();
            assert!(output.status.success());
            (path.to_string(), output.stdout)
        })
        .collect()
}

fn wait_through(wall: &mut Wall, seq: u64) {
    while wall.snapshot().tree.unwrap().through != Some(seq) {
        wall.wait_for_tree();
    }
}

/// The branch's tip, or `None` while it has no commit.
fn tip(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "-q", "--verify", "refs/heads/trunk^{commit}"])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_string())
}

/// Ticks the wall as the daemon does (every 20 ms) until `done` holds of
/// its tree health, or `limit` passes; says how long it took.
fn tick_until(
    wall: &mut Wall,
    limit: Duration,
    done: impl Fn(&crate::protocol::TreeHealth) -> bool,
) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < limit {
        wall.tick(Instant::now());
        if done(&wall.snapshot().tree.unwrap()) {
            return Some(started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Directories made read-only (0500) for a test, put back (0700) when it
/// ends, pass or fail, so the temporary tree can always be removed.
struct ReadOnly(Vec<PathBuf>);

impl ReadOnly {
    fn objects(tree: &Path) -> Self {
        let paths = vec![tree.join("objects"), tree.join("objects/pack")];
        for path in &paths {
            fs::set_permissions(path, fs::Permissions::from_mode(0o500)).unwrap();
        }
        Self(paths)
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        for path in &self.0 {
            if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o700)) {
                eprintln!("could not make {} writable again: {err}", path.display());
            }
        }
    }
}

#[test]
fn every_change_is_one_commit_with_its_trailers() {
    let dir = planted();
    let (mut wall, _engine) = wall();
    wall.attach_tree(thread::TreeHandle::attach(dir.path()).unwrap());
    let requests = [
        turn(400.0),
        Request::Patch {
            from: "lfo1.out".into(),
            to: "vcf1.cutoff".into(),
            amount: Some(0.2),
        },
        Request::Add {
            kind: "vco".into(),
            name: None,
            place: None,
        },
    ];
    let changes: Vec<_> = requests
        .iter()
        .map(|r| ask(&mut wall, r, Instant::now()))
        .collect();
    wall.close(Instant::now());
    let commits = commits(dir.path());
    assert_eq!(commits.len(), 4);
    assert_eq!(
        Stamp::parse(&message(dir.path(), &commits[0]))
            .unwrap()
            .kind,
        "start"
    );
    for (sha, change) in commits[1..].iter().zip(&changes) {
        let msg = message(dir.path(), sha);
        assert_eq!(msg.lines().next().unwrap(), change.summary);
        let stamp = Stamp::parse(&msg).unwrap();
        let clock = change.clock.as_ref().unwrap();
        assert_eq!(stamp.seq, change.seq);
        assert_eq!(stamp.beat, Some(clock.beat));
        assert_eq!(stamp.tempo, Some(clock.tempo));
        assert_eq!(stamp.clock.as_deref(), Some(clock.line.as_str()));
        let json = msg
            .lines()
            .find_map(|line| line.strip_prefix("Kazoo-Change: "))
            .unwrap();
        assert_eq!(serde_json::from_str::<Change>(json).unwrap(), *change);
        assert_eq!(
            command(
                dir.path(),
                &["log", "-1", "--format=%an <%ae>|%aI|%cn <%ce>|%cI", sha]
            ),
            format!(
                "Tom <tom@kazoo.invalid>|{}|kazoo-wall <kazoo-wall@kazoo.invalid>|{}",
                change.at, change.at
            )
        );
    }
    assert!(matches!(
        changes[0].what,
        What::Turn {
            glide_beats: 4.0,
            ..
        }
    ));
    fs::write(
        receipt("tree-log-fuller.txt"),
        command(dir.path(), &["log", "-3", "--format=fuller"]),
    )
    .unwrap();
}

#[test]
fn each_commit_holds_the_wall_after_its_change() {
    let dir = planted();
    let (mut wall, _engine) = wall();
    wall.attach_tree(thread::TreeHandle::attach(dir.path()).unwrap());
    let mut saved = vec![wall.patch().to_file()];
    for request in [
        turn(401.0),
        Request::Add {
            kind: "vco".into(),
            name: None,
            place: None,
        },
        Request::Remove {
            module: "vco2".into(),
        },
    ] {
        ask(&mut wall, &request, Instant::now());
        saved.push(wall.patch().to_file());
    }
    wall.close(Instant::now());
    for (sha, expected) in commits(dir.path()).iter().zip(saved) {
        assert_eq!(from_files(&files(dir.path(), sha)).unwrap(), expected);
    }
}

#[test]
fn two_commits_either_side_of_a_restart_are_on_different_clocks() {
    let dir = planted();
    for value in [402.0, 403.0] {
        let (mut wall, mut engine) = wall();
        wall.attach_tree(thread::TreeHandle::attach(dir.path()).unwrap());
        engine.render(&mut vec![0.0; 96_000], 2);
        ask(&mut wall, &turn(value), Instant::now());
        wall.close(Instant::now());
    }
    let stamps: Vec<_> = commits(dir.path())
        .iter()
        .map(|sha| Stamp::parse(&message(dir.path(), sha)).unwrap())
        .collect();
    assert_eq!(stamps.len(), 4);
    let kinds: Vec<_> = stamps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["start", "change", "start", "change"]);
    assert_ne!(stamps[1].clock, stamps[3].clock);
    assert_eq!(beats_between(&stamps[1], &stamps[3]), None);
    // Each run starts on its own line `<start>.0`, and B's turn is on B's.
    let line = |stamp: &Stamp| stamp.clock.clone().unwrap();
    let start_of = |stamp: &Stamp| line(stamp).split_once('.').unwrap().0.to_string();
    assert!(line(&stamps[0]).ends_with(".0"));
    assert!(line(&stamps[2]).ends_with(".0"));
    assert_ne!(start_of(&stamps[0]), start_of(&stamps[2]));
    assert_eq!(start_of(&stamps[2]), start_of(&stamps[3]));
    // One second rendered at the tempo, to within one sub-block of beats.
    let tempo = stamps[1].tempo.unwrap();
    let beats_rendered = tempo / 60.0;
    let sub_block_beats = crate::SUB_BLOCK as f64 / 48_000.0 * tempo / 60.0;
    let between = beats_between(&stamps[0], &stamps[1]).unwrap();
    assert!(
        (between - beats_rendered).abs() <= sub_block_beats,
        "{between} beats between A's start and A's turn; {beats_rendered} rendered"
    );
}

#[test]
fn a_full_queue_never_stops_the_wall_and_ends_in_one_gap_commit() {
    let dir = planted();
    let (release, latch) = mpsc::channel();
    let (mut wall, _engine) = wall();
    wall.attach_tree(thread::TreeHandle::attach_paused(dir.path(), latch).unwrap());
    let start = Instant::now();
    for n in 0..1_100 {
        ask(
            &mut wall,
            &turn(400.0 + (n % 100) as f64),
            start + Duration::from_millis(n * 400),
        );
    }
    let first = wall.snapshot().tree.unwrap().behind_from.unwrap();
    let last = wall.revision();
    release.send(()).unwrap();
    wait_through(&mut wall, last);
    assert_eq!(wall.snapshot().tree.unwrap().behind_from, None);
    wall.close(Instant::now());
    let tip = command(dir.path(), &["rev-parse", "HEAD"]);
    let stamp = Stamp::parse(&message(dir.path(), &tip)).unwrap();
    assert_eq!(stamp.kind, "gap");
    assert_eq!(stamp.gap, Some((first, last)));
    assert_eq!(
        from_files(&files(dir.path(), &tip)).unwrap(),
        wall.patch().to_file()
    );
}

#[test]
fn a_git_failure_is_said_and_the_tree_recovers() {
    let dir = planted();
    let (mut wall, _engine) = wall();
    wall.attach_tree(thread::TreeHandle::attach(dir.path()).unwrap());
    wait_through(&mut wall, 0);
    // Both `objects` and `objects/pack`: fast-import writes its packs into
    // the second, so only both make the checkpoint fail.
    let read_only = ReadOnly::objects(dir.path());
    let start = Instant::now();
    let change = ask(&mut wall, &turn(404.0), start);
    // Said within 2 s of the daemon's ticks, and the wall answers every
    // request while the tree is down.
    let mut asked = 0_u64;
    let said = tick_until(&mut wall, Duration::from_secs(2), |tree| {
        tree.error.is_some()
    });
    assert!(said.is_some(), "the tree's failure was not said within 2 s");
    for n in 1_u32..=5 {
        ask(
            &mut wall,
            &turn(404.0 + f64::from(n)),
            start + Duration::from_millis(u64::from(n) * 400),
        );
        asked += 1;
    }
    let last = change.seq + asked;
    assert_eq!(wall.revision(), last);
    drop(read_only);
    let recovered = tick_until(
        &mut wall,
        thread::RETRY_AFTER + Duration::from_secs(5),
        |tree| tree.through == Some(last) && tree.error.is_none(),
    );
    assert!(recovered.is_some(), "{:?}", wall.snapshot().tree);
    assert_eq!(wall.snapshot().tree.unwrap().behind_from, None);
    wall.close(Instant::now());
    let tip = tip(dir.path()).unwrap();
    let stamp = Stamp::parse(&message(dir.path(), &tip)).unwrap();
    assert_eq!(stamp.kind, "gap");
    assert_eq!(stamp.gap, Some((change.seq, last)));
    assert_eq!(
        from_files(&files(dir.path(), &tip)).unwrap(),
        wall.patch().to_file()
    );
}

#[test]
fn a_tree_that_fails_at_its_start_is_said_and_clears_with_nothing_behind() {
    let dir = planted();
    let (mut wall, _engine) = wall();
    let read_only = ReadOnly::objects(dir.path());
    wall.attach_tree(thread::TreeHandle::attach(dir.path()).unwrap());
    let said = tick_until(&mut wall, Duration::from_secs(2), |tree| {
        tree.error.is_some()
    });
    assert!(said.is_some(), "the tree's failure was not said within 2 s");
    drop(read_only);
    // No change was made while it was down: once git works again the tree
    // holds the wall, and says nothing is wrong or behind.
    let cleared = tick_until(
        &mut wall,
        thread::RETRY_AFTER + Duration::from_secs(5),
        |tree| tree.is_ok(),
    );
    assert!(cleared.is_some(), "{:?}", wall.snapshot().tree);
    wall.close(Instant::now());
    let tip = tip(dir.path()).unwrap();
    assert_eq!(
        from_files(&files(dir.path(), &tip)).unwrap(),
        wall.patch().to_file()
    );
}

#[test]
fn a_ref_moved_by_anyone_else_stops_the_tree_and_is_never_moved_back() {
    let dir = planted();
    let (mut wall, _engine) = wall();
    wall.attach_tree(thread::TreeHandle::attach(dir.path()).unwrap());
    let first = ask(&mut wall, &turn(406.0), Instant::now());
    wait_through(&mut wall, first.seq);
    let parent = command(dir.path(), &["rev-parse", "HEAD"]);
    let tree_spec = format!("{parent}^{{{}}}", "tree");
    let tree = command(dir.path(), &["rev-parse", &tree_spec]);
    let output = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args([
            "-c",
            "user.name=Stranger",
            "-c",
            "user.email=stranger@kazoo.invalid",
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            "stranger",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stranger = String::from_utf8(output.stdout).unwrap().trim().to_string();
    command(dir.path(), &["update-ref", "refs/heads/trunk", &stranger]);
    let next = ask(&mut wall, &turn(407.0), Instant::now());
    while wall.snapshot().tree.unwrap().error.is_none() {
        wall.wait_for_tree();
    }
    let error = wall.snapshot().tree.unwrap().error.unwrap();
    assert!(error.contains("refs/heads/trunk"), "{error}");
    assert_eq!(command(dir.path(), &["rev-parse", "HEAD"]), stranger);
    wait_through(&mut wall, next.seq);
    command(
        dir.path(),
        &["merge-base", "--is-ancestor", &stranger, "HEAD"],
    );
    wall.close(Instant::now());
    command(
        dir.path(),
        &["merge-base", "--is-ancestor", &stranger, "HEAD"],
    );
}

#[test]
fn closing_with_a_full_queue_still_finishes_the_tree() {
    let dir = planted();
    let (release, latch) = mpsc::channel();
    let (mut wall, _engine) = wall();
    wall.attach_tree(thread::TreeHandle::attach_paused(dir.path(), latch).unwrap());
    let start = Instant::now();
    for n in 0..1_100 {
        ask(
            &mut wall,
            &turn(400.0 + (n % 100) as f64),
            start + Duration::from_millis(n * 400),
        );
    }
    let first = wall.snapshot().tree.unwrap().behind_from.unwrap();
    let last = wall.revision();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        release.send(()).unwrap();
    });
    let closing = Instant::now();
    wall.close(Instant::now());
    assert!(closing.elapsed() < Duration::from_secs(6));
    releaser.join().unwrap();
    let tip = tip(dir.path()).expect("the tree holds nothing after close");
    let stamp = Stamp::parse(&message(dir.path(), &tip)).unwrap();
    assert_eq!(stamp.kind, "gap");
    assert_eq!(stamp.gap, Some((first, last)));
    assert_eq!(
        from_files(&files(dir.path(), &tip)).unwrap(),
        wall.patch().to_file()
    );
}

#[test]
fn no_tree_no_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.find_tree(&dir.path().join("tree"));
    assert!(!wall.snapshot().tree.unwrap().planted);
    ask(&mut wall, &turn(408.0), Instant::now());
    wall.close(Instant::now());
    assert!(!dir.path().join("tree").exists());
}

/// Git commands that move a ref backwards, or rewrite or prune history.
const FORBIDDEN_GIT_WORDS: &[&str] = &[
    "--force", "-f", "update-ref", "reset", "branch", "push", "tag", "gc", "--prune", "prune",
    "filter-branch", "replace", "fetch",
];

/// Every string literal inside each `.arg(` or `.args(` call in `source`.
fn arg_literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for call in [".arg(", ".args("] {
        for (at, _) in source.match_indices(call) {
            let mut depth = 0_usize;
            let mut end = at + call.len();
            for (i, c) in source[at + call.len() - 1..].char_indices() {
                match c {
                    '(' | '[' => depth += 1,
                    ')' | ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = at + call.len() - 1 + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inside = &source[at + call.len()..end];
            found.extend(
                inside
                    .split('"')
                    .skip(1)
                    .step_by(2)
                    .map(str::to_string),
            );
        }
    }
    found
}

#[test]
fn the_tree_runs_only_these_git_commands() {
    let git_source = include_str!("../git.rs");
    let thread_source = include_str!("../thread.rs");
    assert_eq!(git_source.matches("Command::new(").count(), 1);
    assert_eq!(git_source.matches("Command::new(\"git\")").count(), 1);
    assert!(!thread_source.contains("Command::new"));
    let mut subcommands = Vec::new();
    for source in [git_source, thread_source] {
        assert!(!source.contains("--force"));
        for line in source.lines() {
            if let Some(rest) = line.split("git_command(").nth(1) {
                if let Some(rest) = rest.strip_prefix('"') {
                    subcommands.push(rest.split('"').next().unwrap().to_string());
                }
            }
        }
        for literal in arg_literals(source) {
            assert!(
                !FORBIDDEN_GIT_WORDS.contains(&literal.as_str()),
                "{literal} is passed to git"
            );
        }
    }
    for name in &subcommands {
        assert!(ALLOWED_GIT_COMMANDS.contains(&name.as_str()), "{name}");
    }
    // Every allowed command is used, so the list says what the tree runs.
    for name in ALLOWED_GIT_COMMANDS {
        assert!(subcommands.iter().any(|s| s == name), "{name} is never run");
    }
    // The subcommand is only ever given through `git_command`.
    assert!(
        git_source
            .lines()
            .filter(|line| line.contains("git_command(") && !line.contains("fn git_command"))
            .all(|line| line.contains("git_command(\"")),
        "a git subcommand is not a literal"
    );
}

#[test]
fn git_runs_free_of_the_callers_repository() {
    let command = git::git_command("version", None);
    let removed: Vec<_> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect();
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ] {
        assert!(removed.iter().any(|k| k == key), "{key} is passed to git");
    }
    // A directory inside a bare repository is not itself the tree.
    let dir = planted();
    assert!(git::is_bare(dir.path()).unwrap());
    assert!(!git::is_bare(&dir.path().join("objects")).unwrap());
    assert!(!git::is_bare(&dir.path().join("refs")).unwrap());
}

#[test]
fn the_tree_repacks_and_keeps_every_commit() {
    let dir = planted();
    let (news, _heard) = mpsc::channel();
    let planted = git::read(dir.path()).unwrap();
    let mut writer = thread::Writer::new(dir.path(), &planted).unwrap();
    let mut file = crate::seed::seed().unwrap().to_file();
    let mut mirror = crate::tree::to_files(&file).unwrap();
    writer
        .item(
            thread::TreeItem::Whole {
                kind: thread::WholeKind::Start,
                seq: 0,
                clock: None,
                files: mirror.clone(),
                generation: 1,
            },
            dir.path(),
            &news,
        )
        .unwrap();
    let module = file.modules.iter().position(|m| m.id == "vcf1").unwrap();
    let rounds = thread::REPACK_EVERY + 6;
    for seq in 1..=rounds {
        file.last_seq = seq;
        file.modules[module]
            .knobs
            .insert("cutoff".into(), 400.0 + seq as f64);
        let diff = thread::files_diff(&file, &mut mirror).unwrap();
        let change = Change {
            seq,
            at: "2026-10-13T21:04:11Z".into(),
            seat: "Tom".into(),
            what: What::Unknown,
            summary: format!("turn {seq}"),
            undoes: None,
            clock: None,
        };
        writer
            .item(
                thread::TreeItem::Change {
                    change: Box::new(change),
                    files: diff,
                    generation: 1,
                },
                dir.path(),
                &news,
            )
            .unwrap();
        writer.checkpoint(dir.path(), &news).unwrap();
    }
    assert_eq!(writer.checkpoints, rounds);
    writer.finish(dir.path(), &news).unwrap();
    // The 64th checkpoint repacked: fewer packs than checkpoints remain,
    // every object is sound, and every commit is still on the branch.
    let packs = fs::read_dir(dir.path().join("objects/pack"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "pack")
        })
        .count();
    assert!(packs < 16, "{packs} packs after {rounds} checkpoints");
    command(dir.path(), &["fsck", "--strict", "--no-dangling"]);
    assert_eq!(commits(dir.path()).len() as u64, rounds + 1);
    let tip = tip(dir.path()).unwrap();
    assert_eq!(from_files(&files(dir.path(), &tip)).unwrap(), file);
}

#[test]
fn stamps_read_back_exactly() {
    let change = Change {
        seq: 7,
        at: "2026-10-13T21:04:11Z".into(),
        seat: "Tom W.".into(),
        what: What::Unknown,
        summary: "a\nb".into(),
        undoes: None,
        clock: Some(crate::protocol::ClockStamp {
            beat: 18734.25,
            tempo: 120.0,
            line: "9c41d07a3be2f610.3".into(),
        }),
    };
    let commit = git::Commit::change(&change, Vec::new()).unwrap();
    let stamp = Stamp::parse(&commit.message).unwrap();
    assert_eq!(
        stamp,
        Stamp {
            kind: "change".into(),
            seq: 7,
            beat: Some(18734.25),
            tempo: Some(120.0),
            clock: Some("9c41d07a3be2f610.3".into()),
            gap: None
        }
    );
    assert!(Stamp::parse("a message without trailers").is_err());
    assert!(Stamp::parse("Kazoo-Kind: change\nKazoo-Seq: 1\nKazoo-Beat: NaN").is_err());
}

/// BENCH: what `Wall::apply` pays for the tree on the evening wall (the
/// patch turned into files and compared with the mirror, as `feed_tree`
/// does), and the commits a second the tree thread sustains for the same
/// turns with its checkpoints. Written under `target/kz-logs`.
#[test]
fn evening_tree_cost() {
    let evening = crate::migrate::read(include_bytes!("../fixtures/evening.json"))
        .unwrap()
        .0;
    let mut patch = crate::patch::Patch::load(&evening).unwrap().0;
    let module = evening
        .modules
        .iter()
        .find(|m| m.kind == "vcf")
        .unwrap()
        .id
        .clone();
    let dir = planted();
    let mut handle = thread::TreeHandle::attach(dir.path()).unwrap();
    let mut mirror = crate::tree::to_files(&patch.to_file()).unwrap();
    assert!(handle.offer(thread::TreeItem::Whole {
        kind: thread::WholeKind::Start,
        seq: patch.last_seq,
        clock: None,
        files: mirror.clone(),
        generation: 1
    }));
    while handle.through != Some(patch.last_seq) {
        handle.wait_event();
    }
    let mut items = Vec::with_capacity(1_000);
    let mut elapsed = Duration::ZERO;
    for n in 0..1_000 {
        let value = 400.0 + f64::from(n % 100);
        let what = patch.turn(&module, "cutoff", value, Some(0.0)).unwrap();
        patch.last_seq += 1;
        let started = Instant::now();
        let files = thread::files_diff(&patch.to_file(), &mut mirror).unwrap();
        elapsed += started.elapsed();
        assert_eq!(files.len(), 2, "a turn changes its module and wall.toml");
        let change = Change {
            seq: patch.last_seq,
            at: "2026-10-13T21:04:11Z".into(),
            seat: "Tom".into(),
            what,
            summary: "Tom turned cutoff".into(),
            undoes: None,
            clock: None,
        };
        items.push(thread::TreeItem::Change {
            change: Box::new(change),
            files,
            generation: 1,
        });
    }
    // Offered at once, the thread takes them back to back; `finish`
    // checkpoints the last of them and waits for git to exit.
    let started = Instant::now();
    for item in items {
        assert!(handle.offer(item));
    }
    handle.finish().unwrap();
    let seconds = started.elapsed().as_secs_f64();
    assert_eq!(commits(dir.path()).len(), 1_001);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let report = format!(
        "profile={profile} modules={} turns=1000 files_and_diff_mean_us={:.1} \
         commits_per_second={:.0} elapsed_s={seconds:.3} checkpoint_after_s={} \
         checkpoint_commits={}\n",
        evening.modules.len(),
        elapsed.as_secs_f64() * 1_000.0,
        1_000.0 / seconds,
        thread::CHECKPOINT_AFTER.as_secs(),
        thread::CHECKPOINT_COMMITS
    );
    fs::write(receipt(&format!("bench-tree-{profile}.txt")), &report).unwrap();
    println!("{report}");
}
