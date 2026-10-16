//! KZ-T10c: a comment rides its change into the log and onto its moment.

use super::*;

/// The `Kazoo-Comment` of each of the tree's newest `count` commits on its
/// branch, oldest first (empty for one without).
fn comments(tree: &Path, count: usize) -> Vec<String> {
    command(
        tree,
        &[
            "log",
            &format!("-n{count}"),
            "--reverse",
            "--format=%(trailers:key=Kazoo-Comment,valueonly)%x1e",
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
    assert_eq!(
        comments(rig.tree.path(), 4),
        ["open it a little", "", "then all the way", ""]
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
    assert_eq!(live, ["darker", "first", "second"]);
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
}
