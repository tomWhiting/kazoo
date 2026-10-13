use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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
                &["log", "-1", "--format=%an <%ae>|%aI|%cI", sha]
            ),
            format!("Tom <tom@kazoo.invalid>|{}|{}", change.at, change.at)
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
    assert_ne!(stamps[1].clock, stamps[3].clock);
    assert_eq!(beats_between(&stamps[1], &stamps[3]), None);
    assert!(stamps[2].clock.as_ref().unwrap().ends_with(".0"));
    assert!(
        (beats_between(&stamps[0], &stamps[1]).unwrap() - stamps[1].tempo.unwrap() / 60.0).abs()
            <= 2.0 * crate::SUB_BLOCK as f64 / 48_000.0
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
    let paths = [dir.path().join("objects"), dir.path().join("objects/pack")];
    for path in &paths {
        fs::set_permissions(path, fs::Permissions::from_mode(0o500)).unwrap();
    }
    let change = ask(&mut wall, &turn(404.0), Instant::now());
    while wall.snapshot().tree.unwrap().error.is_none() {
        wall.wait_for_tree();
    }
    ask(&mut wall, &turn(405.0), Instant::now());
    for path in &paths {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    wait_through(&mut wall, change.seq + 1);
    assert_eq!(wall.snapshot().tree.unwrap().error, None);
    wall.close(Instant::now());
    let tip = command(dir.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        Stamp::parse(&message(dir.path(), &tip)).unwrap().gap,
        Some((change.seq, change.seq + 1))
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
    assert!(
        wall.snapshot()
            .tree
            .unwrap()
            .error
            .unwrap()
            .contains("branch")
    );
    assert_eq!(command(dir.path(), &["rev-parse", "HEAD"]), stranger);
    wait_through(&mut wall, next.seq);
    command(
        dir.path(),
        &["merge-base", "--is-ancestor", &stranger, "HEAD"],
    );
    wall.close(Instant::now());
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

#[test]
fn the_tree_runs_only_these_git_commands() {
    assert_eq!(
        include_str!("../git.rs")
            .matches("Command::new(\"git\")")
            .count(),
        1
    );
    assert!(!include_str!("../thread.rs").contains("Command::new"));
    for source in [include_str!("../git.rs"), include_str!("../thread.rs")] {
        assert!(!source.contains("--force"));
        for line in source.lines() {
            if let Some(rest) = line.split("git_command(").nth(1) {
                if let Some(rest) = rest.strip_prefix('"') {
                    let name = rest.split('"').next().unwrap();
                    assert!(ALLOWED_GIT_COMMANDS.contains(&name), "{name}");
                }
            }
        }
    }
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

#[test]
fn evening_tree_cost() {
    let mut file = crate::migrate::read(include_bytes!("../fixtures/evening.json"))
        .unwrap()
        .0;
    let module = file.modules.iter().position(|m| m.kind == "vcf").unwrap();
    let dir = planted();
    let mut handle = thread::TreeHandle::attach(dir.path()).unwrap();
    let mut mirror = crate::tree::to_files(&file).unwrap();
    assert!(handle.offer(thread::TreeItem::Whole {
        kind: thread::WholeKind::Start,
        seq: file.last_seq,
        clock: None,
        files: mirror.clone(),
        generation: 1
    }));
    while handle.through != Some(file.last_seq) {
        handle.wait_event();
    }
    let mut items = Vec::with_capacity(1_000);
    let mut elapsed = Duration::ZERO;
    for n in 0..1_000 {
        file.last_seq += 1;
        let value = 400.0 + f64::from(n % 100);
        file.modules[module].knobs.insert("cutoff".into(), value);
        let started = Instant::now();
        let files = thread::files_diff(&file, &mut mirror).unwrap();
        elapsed += started.elapsed();
        let change = Change {
            seq: file.last_seq,
            at: "2026-10-13T21:04:11Z".into(),
            seat: "Tom".into(),
            what: What::Turn {
                module: file.modules[module].id.clone(),
                knob: "cutoff".into(),
                from: value - 1.0,
                to: value,
                glide_beats: 0.0,
            },
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
    let started = Instant::now();
    for item in items {
        assert!(handle.offer(item));
    }
    handle.finish().unwrap();
    let seconds = started.elapsed().as_secs_f64();
    let report = format!(
        "profile={} modules={} turns=1000 files_and_diff_mean_us={} commits_per_second={} elapsed_s={} checkpoint_after_s={} checkpoint_commits={}\n",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        file.modules.len(),
        elapsed.as_secs_f64() * 1_000.0,
        1_000.0 / seconds,
        seconds,
        thread::CHECKPOINT_AFTER.as_secs(),
        thread::CHECKPOINT_COMMITS
    );
    fs::write(
        receipt(&format!(
            "bench-tree-{}.txt",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        )),
        &report,
    )
    .unwrap();
    println!("{report}");
}
