use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use super::super::*;
use crate::engine::{Engine, EngineConfig, engine};
use crate::protocol::{MarkResult, TreeResult};
use crate::tree::{git, thread::TreeHandle};

mod costs;
mod parts;
mod savings;

struct Rig {
    wall: Wall,
    engine: Engine,
    state: tempfile::TempDir,
    tree: tempfile::TempDir,
    now: Instant,
}

fn request(value: serde_json::Value) -> Request {
    serde_json::from_value(value).unwrap()
}

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
    String::from_utf8(output.stdout).unwrap().trim().into()
}

impl Rig {
    fn new(paused: Option<mpsc::Receiver<()>>) -> Self {
        let state = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        git::plant(tree.path()).unwrap();
        let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let mut wall = Wall::new(
            crate::seed::seed().unwrap(),
            control,
            None,
            Vec::new(),
            Some(Store::peek(state.path()).unwrap()),
        );
        let handle = paused.map_or_else(
            || TreeHandle::attach(tree.path()).unwrap(),
            |latch| TreeHandle::attach_paused(tree.path(), latch).unwrap(),
        );
        wall.attach_tree(handle);
        Self {
            wall,
            engine,
            state,
            tree,
            now: Instant::now(),
        }
    }

    fn ask(
        &mut self,
        seat: &str,
        console: bool,
        value: serde_json::Value,
    ) -> Result<Outcome, WallError> {
        let outcome = self
            .wall
            .request_as(seat, console, &request(value), self.now)?;
        let Some(ticket) = outcome.pending else {
            return Ok(outcome);
        };
        loop {
            self.wall.tree_test_wait();
            let answers = self.wall.tree_answers(self.now);
            if let Some((_, result)) = answers.into_iter().find(|(t, _)| *t == ticket) {
                return result;
            }
        }
    }

    /// Plays the engine until every swap sent has landed, as the audio
    /// callback does, for a go that waits for its landing to be answered.
    fn land(&mut self) {
        let shared = self.wall.engine.shared();
        let mut buffer = [0.0; crate::SUB_BLOCK * 2];
        while shared.unhushed() < self.wall.swaps_sent {
            self.engine.render(&mut buffer, 2);
        }
    }

    fn turn(&mut self, value: f64) -> Outcome {
        self.ask("Tom", true, serde_json::json!({"op":"turn", "module":"vcf1", "knob":"cutoff", "value":value, "glide_beats":0})).unwrap()
    }

    fn go(&mut self, moment: &str) -> Outcome {
        self.ask("Tom", true, serde_json::json!({"op":"go", "moment":moment}))
            .unwrap()
    }

    fn listing(&mut self) -> TreeResult {
        serde_json::from_value(
            self.ask("Tom", true, serde_json::json!({"op":"tree"}))
                .unwrap()
                .result,
        )
        .unwrap()
    }

    fn prepare(&mut self) {
        for n in 1..=10 {
            self.turn(f64::from(n).mul_add(100.0, 1000.0));
        }
    }
}

#[test]
fn a_go_puts_the_wall_at_the_moment_exactly() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let outcome = rig.go("#4");
    let What::Go { to, backup, .. } = outcome.change.unwrap().what else {
        panic!("not a go");
    };
    let paths = command(
        rig.tree.path(),
        &["ls-tree", "-r", "--name-only", &to.commit],
    );
    let mut files = crate::tree::Files::new();
    for path in paths.lines() {
        let output = git::git_command("show", Some(rig.tree.path()))
            .arg(format!("{}:{path}", to.commit))
            .output()
            .unwrap();
        assert!(output.status.success());
        files.insert(path.into(), output.stdout);
    }
    let mut expected = crate::tree::from_files(&files).unwrap();
    let actual = rig.wall.patch.to_file();
    expected.last_seq = actual.last_seq;
    expected.wall = actual.wall;
    assert_eq!(actual, expected);
    assert_eq!(to.change, Some(4));
    assert!(backup.starts_with("before-go-"));
    assert!(matches!(
        outcome.event,
        Some(Event::Loaded { save: None, .. })
    ));
    assert_eq!(
        rig.wall
            .patch
            .to_file()
            .modules
            .iter()
            .find(|m| m.id == "vcf1")
            .unwrap()
            .knobs["cutoff"]
            .to_bits(),
        1400.0_f64.to_bits()
    );
    let file = rig.wall.patch.to_file();
    assert!(file.modules.iter().all(|m| m.birth.is_some()));
}

#[test]
fn undoing_a_go_brings_back_the_wall_and_its_place() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let before = rig.wall.patch.fingerprint();
    let seq = rig.go("#4").change.unwrap().seq;
    rig.ask("Tom", true, serde_json::json!({"op":"undo", "change":seq}))
        .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
    rig.listing();
    assert_eq!(
        command(rig.tree.path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/trunk"
    );
    let seq = rig.turn(2800.0).change.unwrap().seq;
    assert_eq!(rig.listing().here.unwrap().change, Some(seq));
}

#[test]
fn the_first_change_after_a_go_grows_a_bud_and_none_grows_without_one() {
    let mut rig = Rig::new(None);
    rig.turn(1000.0);
    rig.ask("Tom", true, serde_json::json!({"op":"mark", "name":"dawn"}))
        .unwrap();
    rig.turn(2000.0);
    rig.go("dawn");
    let seq = rig.turn(3000.0).change.unwrap().seq;
    let tree = rig.listing();
    assert!(
        tree.branches
            .iter()
            .any(|b| b.name.starts_with("bud/dawn/") && b.tip.change == Some(seq))
    );
    let count = tree.branch_count;
    let go = rig.go("dawn").change.unwrap().seq;
    rig.ask("Tom", true, serde_json::json!({"op":"undo", "change":go}))
        .unwrap();
    assert_eq!(rig.listing().branch_count, count);
}

#[test]
fn a_seats_goes_share_the_load_allowance_and_unknown_moments_give_it_back() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let error = rig
        .ask(
            "seat",
            false,
            serde_json::json!({"op":"go", "moment":"nowhere"}),
        )
        .err()
        .unwrap();
    assert_eq!(format!("{:?}", error.code), "UnknownMoment");
    for moment in ["#4", "#5", "#6"] {
        rig.ask(
            "seat",
            false,
            serde_json::json!({"op":"go", "moment":moment}),
        )
        .unwrap();
    }
    assert_eq!(
        rig.ask("seat", false, serde_json::json!({"op":"new"}))
            .err()
            .unwrap()
            .code,
        ErrorCode::SlowDown
    );
    rig.go("#7");
}

#[test]
fn a_go_answered_after_other_changes_asks_the_tree_again() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let outcome = rig
        .wall
        .request_as(
            "Tom",
            true,
            &request(serde_json::json!({"op":"go", "moment":"#4"})),
            rig.now,
        )
        .unwrap();
    rig.turn(2500.0);
    loop {
        rig.wall.tree_test_wait();
        if let Some((_, answer)) = rig
            .wall
            .tree_answers(rig.now)
            .into_iter()
            .find(|(t, _)| Some(*t) == outcome.pending)
        {
            let What::Go { from, .. } = answer.unwrap().change.unwrap().what else {
                panic!("not a go");
            };
            assert_eq!(from.change, Some(11));
            break;
        }
    }
    let mut changing = Rig::new(None);
    changing.prepare();
    let pending = changing
        .wall
        .request_as(
            "Tom",
            true,
            &request(serde_json::json!({"op":"go", "moment":"#4"})),
            changing.now,
        )
        .unwrap()
        .pending
        .unwrap();
    for retry in 0..3 {
        while !changing
            .wall
            .tree_answered
            .iter()
            .any(|(ticket, _, _)| *ticket == pending)
        {
            changing.wall.tree_test_wait();
        }
        changing.turn(f64::from(retry).mul_add(100.0, 2600.0));
        let answers = changing.wall.tree_answers(changing.now);
        if retry < 2 {
            assert!(answers.is_empty());
        } else {
            assert_eq!(answers.len(), 1);
            assert_eq!(
                answers[0].1.as_ref().err().unwrap().code,
                ErrorCode::SlowDown
            );
        }
    }
}

#[test]
fn a_go_to_where_the_wall_is_changes_nothing() {
    let mut rig = Rig::new(None);
    rig.turn(1000.0);
    let revision = rig.wall.revision();
    assert_eq!(
        rig.ask("Tom", true, serde_json::json!({"op":"go", "moment":"#1"}))
            .err()
            .unwrap()
            .code,
        ErrorCode::NotAllowed
    );
    assert_eq!(rig.wall.revision(), revision);
}

#[test]
fn a_seat_must_look_again_after_another_seats_go() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let old = rig.wall.snapshot().wall;
    rig.go("#4");
    assert_eq!(rig.wall.check_wall(Some(old), &request(serde_json::json!({"op":"turn", "module":"vcf1", "knob":"cutoff", "value":1800.0}))).unwrap_err().code, ErrorCode::WallReplaced);
}

#[test]
fn marks_are_answered_once_written_and_told_to_everyone() {
    let mut rig = Rig::new(None);
    rig.turn(1000.0);
    let outcome = rig
        .ask(
            "seat",
            false,
            serde_json::json!({"op":"mark", "name":"dawn"}),
        )
        .unwrap();
    let mark: MarkResult = serde_json::from_value(outcome.result).unwrap();
    assert_eq!(mark.at.change, Some(1));
    assert!(outcome.change.is_none());
    assert!(matches!(outcome.event, Some(Event::Marked { .. })));
    assert_eq!(
        format!(
            "{:?}",
            rig.ask(
                "seat",
                false,
                serde_json::json!({"op":"mark", "name":"DAWN"})
            )
            .err()
            .unwrap()
            .code
        ),
        "MarkExists"
    );
}

#[test]
fn a_seat_marks_three_times_a_minute_and_the_console_any_number() {
    let mut rig = Rig::new(None);
    for name in ["one", "two", "three"] {
        rig.ask("seat", false, serde_json::json!({"op":"mark", "name":name}))
            .unwrap();
    }
    assert_eq!(
        rig.ask(
            "seat",
            false,
            serde_json::json!({"op":"mark", "name":"four"})
        )
        .err()
        .unwrap()
        .code,
        ErrorCode::SlowDown
    );
    for name in ["four", "five", "six", "seven"] {
        rig.ask("Tom", true, serde_json::json!({"op":"mark", "name":name}))
            .unwrap();
    }
}

#[test]
fn every_save_by_hand_is_marked_and_the_walls_own_are_not() {
    let mut rig = Rig::new(None);
    rig.ask("Tom", true, serde_json::json!({"op":"save", "name":"jam"}))
        .unwrap();
    rig.turn(1500.0);
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"save", "name":"jam", "overwrite":true}),
    )
    .unwrap();
    rig.turn(2500.0);
    rig.ask("Tom", true, serde_json::json!({"op":"load", "name":"jam"}))
        .unwrap();
    let tags = command(rig.tree.path(), &["tag", "--list"]);
    rig.listing();
    let tags = format!("{tags}\n{}", command(rig.tree.path(), &["tag", "--list"]));
    assert!(tags.contains("save/jam/"));
    assert!(!tags.contains("before-load"));
    assert_eq!(
        rig.listing()
            .marks
            .iter()
            .filter(|m| m.name == "jam")
            .count(),
        2
    );
}

#[test]
fn the_tree_answers_within_five_seconds_or_the_seat_is_told() {
    let (release, latch) = mpsc::channel();
    let mut rig = Rig::new(Some(latch));
    let outcome = rig
        .wall
        .request_as(
            "seat",
            false,
            &request(serde_json::json!({"op":"tree"})),
            rig.now,
        )
        .unwrap();
    let mark = rig
        .wall
        .request_as(
            "seat",
            false,
            &request(serde_json::json!({"op":"mark", "name":"late"})),
            rig.now,
        )
        .unwrap();
    let go = rig
        .wall
        .request_as(
            "seat",
            false,
            &request(serde_json::json!({"op":"go", "moment":"nowhere"})),
            rig.now,
        )
        .unwrap();
    let answers = rig.wall.tree_answers(rig.now + Duration::from_secs(6));
    assert_eq!(answers.len(), 3);
    assert_eq!(Some(answers[0].0), outcome.pending);
    assert!(
        answers[0]
            .1
            .as_ref()
            .err()
            .unwrap()
            .message
            .contains("did not answer within 5 s (the tree)")
    );
    assert!(
        answers
            .iter()
            .any(|(ticket, _)| Some(*ticket) == mark.pending)
    );
    assert!(
        answers
            .iter()
            .any(|(ticket, _)| Some(*ticket) == go.pending)
    );
    assert_eq!(
        rig.wall.mark_buckets["seat"].tokens.to_bits(),
        2.0_f64.to_bits()
    );
    assert_eq!(
        rig.wall.load_buckets["seat"].tokens.to_bits(),
        3.0_f64.to_bits()
    );
    release.send(()).unwrap();
    rig.listing();
    assert!(rig.wall.tree_answers(rig.now).is_empty());
}

#[test]
fn a_wall_with_no_tree_says_so() {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(
        crate::seed::seed().unwrap(),
        control,
        None,
        Vec::new(),
        None,
    );
    for value in [
        serde_json::json!({"op":"mark", "name":"dawn"}),
        serde_json::json!({"op":"go", "moment":"#4"}),
        serde_json::json!({"op":"tree"}),
    ] {
        let error = wall
            .request_as("Tom", true, &request(value), Instant::now())
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.starts_with("this wall keeps no tree yet"));
    }
    drop(engine);
}

#[test]
fn watchers_read_the_tree_and_may_not_mark_or_go() {
    assert!(request(serde_json::json!({"op":"mark", "name":"dawn"})).is_change());
    assert!(request(serde_json::json!({"op":"go", "moment":"dawn"})).is_change());
    assert!(!request(serde_json::json!({"op":"tree"})).is_change());
}

#[test]
fn a_go_survives_a_restart_as_a_load_does() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let before = rig.wall.patch.fingerprint();
    let go = rig.go("#4").change.unwrap().seq;
    rig.wall.close(rig.now);
    drop(rig.wall);
    let mut store = Store::open(rig.state.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no wall");
    };
    let mut patch = *patch;
    let mut history = store.load_log().unwrap().changes;
    resume(&mut store, &mut patch, &mut history, false, None, None).unwrap();
    assert!(
        !history
            .iter()
            .any(|c| matches!(c.what, What::Resumed { .. }))
    );
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    rig.engine = engine;
    rig.wall = Wall::new(patch, control, None, history, Some(store));
    rig.wall
        .attach_tree(TreeHandle::attach(rig.tree.path()).unwrap());
    rig.ask("Tom", true, serde_json::json!({"op":"undo", "change":go}))
        .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
}

#[test]
fn a_change_from_before_a_go_cannot_be_undone_after_it() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let error = rig
        .ask("Tom", true, serde_json::json!({"op":"undo", "change":1}))
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::NotAllowed);
    assert!(error.message.contains("replaced since (change 11:"));
}

#[test]
fn the_backups_goes_need_are_not_pruned() {
    let mut rig = Rig::new(None);
    rig.turn(1000.0);
    rig.turn(2000.0);
    let mut backups = Vec::new();
    for n in 0..25 {
        let moment = if n % 2 == 0 { "#1" } else { "#2" };
        let What::Go { backup, .. } = rig.go(moment).change.unwrap().what else {
            panic!("not a go");
        };
        backups.push(backup);
    }
    rig.wall.prune();
    for backup in backups {
        if let Some(commit) = backup.strip_prefix("tree:") {
            assert_eq!(
                command(rig.tree.path(), &["cat-file", "-t", commit]),
                "commit"
            );
        } else {
            assert!(rig.wall.saves_store().unwrap().read_save(&backup).is_ok());
        }
    }
}
