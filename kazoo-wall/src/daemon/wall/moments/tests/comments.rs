//! KZ-T10c: a comment rides its change into the log and onto its moment.

use super::*;

/// The `Kazoo-Comment` of each of the tree's newest `count` commits on its
/// branch, oldest first (empty for one without).
fn comments(tree: &Path, count: usize) -> Vec<String> {
    trailer(tree, count, "Kazoo-Comment")
}

/// Trailer `key` of each of the tree's newest `count` commits on its
/// branch, oldest first, each one's values joined by `|` (empty for one
/// without).
fn trailer(tree: &Path, count: usize, key: &str) -> Vec<String> {
    command(
        tree,
        &[
            "log",
            &format!("-n{count}"),
            "--reverse",
            &format!("--format=%(trailers:key={key},valueonly,separator=|)%x1e"),
            "HEAD",
        ],
    )
    .split('\u{1e}')
    .map(str::trim)
    .take(count)
    .map(str::to_string)
    .collect()
}

fn turn(value: f64) -> serde_json::Value {
    serde_json::json!({"op":"turn", "module":"vcf1", "knob":"cutoff", "value":value, "glide_beats":0})
}

#[test]
fn a_comment_rides_its_change_into_the_log_and_onto_its_moment() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let made = rig
        .ask_commented(("Cally", false), turn(640.0), Some("darker under the pads"))
        .unwrap()
        .change
        .unwrap();
    assert_eq!(made.comment.as_deref(), Some("darker under the pads"));
    assert_eq!(
        made.said(),
        format!("{} \u{2014} \"darker under the pads\"", made.summary)
    );
    let logged = std::fs::read_to_string(rig.state.path().join("log.jsonl")).unwrap();
    let line = logged
        .lines()
        .map(|line| serde_json::from_str::<Change>(line).unwrap())
        .find(|change| change.seq == made.seq)
        .unwrap();
    assert_eq!(line.comment.as_deref(), Some("darker under the pads"));
    // The next change, with none, carries none.
    let plain = rig.turn(650.0).change.unwrap();
    assert_eq!(plain.comment, None);
    rig.wall.close(rig.now);
    assert_eq!(comments(rig.tree.path(), 2), ["darker under the pads", ""]);
}

#[test]
fn a_refused_change_keeps_no_comment_for_the_next() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let refused = rig.ask_commented(
        ("Cally", false),
        serde_json::json!({"op":"turn", "module":"nowhere1", "knob":"cutoff", "value":1.0}),
        Some("never made"),
    );
    assert!(refused.is_err());
    let next = rig.turn(700.0).change.unwrap();
    assert_eq!(next.comment, None);
}

#[test]
fn a_go_keeps_its_comment_for_the_change_made_when_the_tree_answers() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let went = rig
        .ask_commented(
            ("Tom", true),
            serde_json::json!({"op":"go", "moment":"#4"}),
            Some("back before the brass"),
        )
        .unwrap()
        .change
        .unwrap();
    assert!(matches!(went.what, What::Go { .. }), "{:?}", went.what);
    assert_eq!(went.comment.as_deref(), Some("back before the brass"));
    // Taken by the go: the next change has none.
    rig.land();
    let next = rig.turn(800.0).change.unwrap();
    assert_eq!(next.comment, None);
}

#[test]
fn each_step_of_a_batch_carries_its_own_comment_onto_its_moment() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let made = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({
                "op":"batch",
                "note":"Three cutoffs",
                "changes":[turn(300.0), turn(400.0), turn(500.0)],
                "comments":["  open it a little  ", null, "then all the way"],
            }),
        )
        .unwrap()
        .change
        .unwrap();
    let What::Batch { comments: kept, .. } = &made.what else {
        panic!("not a batch: {:?}", made.what);
    };
    assert_eq!(
        kept,
        &[
            Some("open it a little".to_string()),
            None,
            Some("then all the way".to_string())
        ]
    );
    // An undo of the batch says none of them: they said why each went on.
    let undone = rig
        .ask(
            "Tom",
            true,
            serde_json::json!({"op":"undo", "change":made.seq}),
        )
        .unwrap()
        .change
        .unwrap();
    let What::Batch { comments: none, .. } = &undone.what else {
        panic!("not a batch: {:?}", undone.what);
    };
    assert!(none.is_empty(), "{none:?}");
    rig.wall.close(rig.now);
    // Each part carries its own step's comment; the batch's own moment
    // lists every step's by its number, never one as the batch's reason.
    assert_eq!(
        comments(rig.tree.path(), 4),
        ["open it a little", "", "", ""]
    );
    assert_eq!(
        trailer(rig.tree.path(), 4, "Kazoo-Step-Comment"),
        ["", "", "1: open it a little|3: then all the way", ""]
    );
}

#[test]
fn a_batch_refuses_comments_that_are_not_one_per_step_or_break_the_rules() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let short = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({"op":"batch", "note":"two", "changes":[turn(300.0), turn(400.0)], "comments":["one"]}),
        )
        .unwrap_err();
    assert_eq!(short.code, ErrorCode::BadRequest);
    assert!(short.message.contains("one per step"), "{}", short.message);
    let broken = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({"op":"batch", "note":"two", "changes":[turn(300.0), turn(400.0)], "comments":[null, "two\nlines"]}),
        )
        .unwrap_err();
    assert_eq!(broken.step, Some(1));
    assert!(broken.message.contains("one line"), "{}", broken.message);
    // All null is no comments at all.
    let made = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({"op":"batch", "note":"two", "changes":[turn(300.0), turn(400.0)], "comments":[null, null]}),
        )
        .unwrap()
        .change
        .unwrap();
    let What::Batch { comments: none, .. } = &made.what else {
        panic!("not a batch");
    };
    assert!(none.is_empty());
}

#[test]
fn an_import_of_the_log_writes_the_same_comments_as_the_live_tree() {
    let mut rig = Rig::new(None);
    let initial = rig.wall.patch.to_file();
    rig.prepare();
    rig.ask_commented(("Cally", false), turn(640.0), Some("darker"))
        .unwrap();
    rig.ask(
        "Cally",
        false,
        serde_json::json!({
            "op":"batch",
            "note":"Two cutoffs",
            "changes":[turn(300.0), turn(400.0)],
            "comments":["first", "second"],
        }),
    )
    .unwrap();
    rig.wall.close(rig.now);
    let live = comments(rig.tree.path(), 3);
    assert_eq!(live, ["darker", "first", ""]);
    let steps = trailer(rig.tree.path(), 1, "Kazoo-Step-Comment");
    assert_eq!(steps, ["1: first|2: second"]);
    let state = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(state.path().join("saves")).unwrap();
    std::fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    std::fs::write(
        state.path().join("patch.json"),
        serde_json::to_vec(&rig.wall.patch.to_file()).unwrap(),
    )
    .unwrap();
    std::fs::copy(
        rig.state.path().join("log.jsonl"),
        state.path().join("log.jsonl"),
    )
    .unwrap();
    let tree = state.path().join("tree");
    let grown = crate::tree::rings::grow(state.path(), &tree, false).unwrap();
    assert_eq!(grown.gaps, 0, "{}", grown.report);
    assert_eq!(comments(&tree, 3), live);
    assert_eq!(trailer(&tree, 1, "Kazoo-Step-Comment"), steps);
}

/// Ask `value` from Tom's console with `comment`; the change it made.
fn commented(rig: &mut Rig, value: serde_json::Value, comment: &str) -> Change {
    rig.ask_commented(("Tom", true), value, Some(comment))
        .unwrap()
        .change
        .unwrap()
}

#[test]
fn every_kind_of_change_that_takes_a_comment_keeps_it() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let mut made = Vec::new();
    let tempo = commented(
        &mut rig,
        serde_json::json!({"op":"tempo", "bpm":90.0}),
        "slower",
    );
    made.push((tempo, "slower"));
    let added = commented(
        &mut rig,
        serde_json::json!({"op":"add", "kind":"loop"}),
        "a loop",
    );
    let loop_id = match &added.what {
        What::Add { module, .. } => module.id.clone(),
        other => panic!("not an add: {other:?}"),
    };
    made.push((added, "a loop"));
    let erased = commented(
        &mut rig,
        serde_json::json!({"op":"loop", "module":loop_id, "loop_op":{"op":"erase"}}),
        "clean tape",
    );
    made.push((erased, "clean tape"));
    let patched = commented(
        &mut rig,
        serde_json::json!({"op":"patch", "from":"lfo1.out", "to":"vcf1.resonance"}),
        "wobble",
    );
    made.push((patched, "wobble"));
    let unpatched = commented(
        &mut rig,
        serde_json::json!({"op":"unpatch", "to":"vcf1.resonance"}),
        "still",
    );
    made.push((unpatched, "still"));
    let removed = commented(
        &mut rig,
        serde_json::json!({"op":"remove", "module":loop_id}),
        "no loop",
    );
    let removed_seq = removed.seq;
    made.push((removed, "no loop"));
    let undone = commented(
        &mut rig,
        serde_json::json!({"op":"undo", "change":removed_seq}),
        "the loop back",
    );
    made.push((undone, "the loop back"));
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"chain_save", "name":"voice", "modules":["vco1","vcf1"]}),
    )
    .unwrap();
    let chained = commented(
        &mut rig,
        serde_json::json!({"op":"chain_add", "name":"voice"}),
        "a second voice",
    );
    made.push((chained, "a second voice"));
    rig.ask("Tom", true, serde_json::json!({"op":"save", "name":"jam"}))
        .unwrap();
    let blank = commented(&mut rig, serde_json::json!({"op":"new"}), "from nothing");
    let blank_seq = blank.seq;
    made.push((blank, "from nothing"));
    rig.land();
    let loaded = commented(
        &mut rig,
        serde_json::json!({"op":"load", "name":"jam"}),
        "the jam again",
    );
    made.push((loaded, "the jam again"));
    rig.land();
    let unloaded = commented(
        &mut rig,
        serde_json::json!({"op":"undo", "change":blank_seq}),
        "not the blank",
    );
    made.push((unloaded, "not the blank"));
    rig.land();
    for (change, comment) in &made {
        assert_eq!(
            change.comment.as_deref(),
            Some(*comment),
            "{}",
            change.summary
        );
    }
    // And every one is in the log with it.
    let logged = std::fs::read_to_string(rig.state.path().join("log.jsonl")).unwrap();
    for (change, comment) in &made {
        let line = logged
            .lines()
            .map(|line| serde_json::from_str::<Change>(line).unwrap())
            .find(|logged| logged.seq == change.seq)
            .unwrap();
        assert_eq!(line.comment.as_deref(), Some(*comment));
    }
}

#[test]
fn an_undo_of_a_go_keeps_its_comment_for_when_the_tree_answers() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let went = rig.go("#4").change.unwrap();
    rig.land();
    let back = commented(
        &mut rig,
        serde_json::json!({"op":"undo", "change":went.seq}),
        "where we were",
    );
    assert_eq!(back.undoes, Some(went.seq));
    assert_eq!(back.comment.as_deref(), Some("where we were"));
}

#[test]
fn a_recording_stopped_keeps_its_comment_even_while_the_log_owes() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.ask(
        "Cally",
        false,
        serde_json::json!({"op":"record", "on":true}),
    )
    .unwrap();
    // The stop's line fails: the stop is owed, with its comment.
    rig.wall
        .store_mut()
        .unwrap()
        .fail_once(crate::store::FailAt::AppendLog);
    let stopped = rig
        .ask_commented(
            ("Cally", false),
            serde_json::json!({"op":"record", "on":false}),
            Some("end of take"),
        )
        .unwrap();
    assert!(stopped.change.is_none(), "owed: no number yet");
    // A batch after it: the owed stop is paid first, with its comment, and
    // the batch still grows a moment for each step.
    let batch = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({"op":"batch", "note":"Three cutoffs", "changes":[turn(300.0), turn(400.0), turn(500.0)]}),
        )
        .unwrap()
        .change
        .unwrap();
    let stop = rig
        .wall
        .log
        .iter()
        .find(|change| matches!(change.what, What::Record { on: false, .. }))
        .unwrap()
        .clone();
    assert_eq!(stop.comment.as_deref(), Some("end of take"));
    assert!(stop.seq < batch.seq);
    rig.wall.close(rig.now);
    assert_eq!(
        trailer(rig.tree.path(), 4, "Kazoo-Kind"),
        ["change", "part", "part", "batch"]
    );
    assert_eq!(comments(rig.tree.path(), 4)[0], "end of take");
    // Each moment holds the wall it was made to: the stop, paid before the
    // batch was staged, holds none of the batch's work.
    let cutoffs: Vec<String> = (0..4)
        .rev()
        .map(|back| {
            command(
                rig.tree.path(),
                &["show", &format!("HEAD~{back}:modules/vcf1.toml")],
            )
            .lines()
            .find(|line| line.trim_start().starts_with("cutoff"))
            .unwrap()
            .trim()
            .to_string()
        })
        .collect();
    assert_eq!(
        cutoffs,
        [
            "cutoff = 2000.0",
            "cutoff = 300.0",
            "cutoff = 400.0",
            "cutoff = 500.0"
        ]
    );
}

#[test]
fn a_large_batch_grows_a_moment_for_every_step() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let steps: Vec<serde_json::Value> = (0..200).map(|n| turn(300.0 + f64::from(n))).collect();
    let mut comments_given = vec![serde_json::Value::Null; 200];
    comments_given[150] = serde_json::json!("halfway up");
    rig.ask(
        "Cally",
        false,
        serde_json::json!({"op":"batch", "note":"a long rise", "changes":steps, "comments":comments_given}),
    )
    .unwrap();
    rig.wall.close(rig.now);
    let kinds = trailer(rig.tree.path(), 200, "Kazoo-Kind");
    assert_eq!(kinds.iter().filter(|kind| *kind == "part").count(), 199);
    assert_eq!(kinds[199], "batch");
    assert_eq!(comments(rig.tree.path(), 200)[150], "halfway up");
}

#[test]
fn a_tree_at_a_part_holds_through_the_change_before_its_batch() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let before = rig.wall.revision();
    let batch = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({"op":"batch", "note":"Three cutoffs", "changes":[turn(300.0), turn(400.0), turn(500.0)]}),
        )
        .unwrap()
        .change
        .unwrap();
    rig.wall.close(rig.now);
    let part = command(rig.tree.path(), &["rev-parse", "HEAD~1"]);
    assert_eq!(
        git::through_at(rig.tree.path(), part.trim()).unwrap(),
        Some(before)
    );
    assert_eq!(
        git::through_at(rig.tree.path(), "HEAD").unwrap(),
        Some(batch.seq)
    );
}
