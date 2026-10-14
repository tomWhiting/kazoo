use super::*;

fn batch(note: &str, changes: serde_json::Value) -> Request {
    let mut value = serde_json::json!({"op": "batch", "note": note});
    value["changes"] = changes;
    let request: Request = serde_json::from_value(value).unwrap();
    assert_eq!(
        serde_json::from_value::<Request>(serde_json::to_value(&request).unwrap()).unwrap(),
        request
    );
    request
}

#[test]
fn batch_applies_labels_glide_tempo_and_undo_as_one_change() {
    let (mut wall, mut engine) = wall();
    let before = wall.patch.to_file();
    let request = batch(
        "A moving voice",
        serde_json::json!([
            {"op":"add", "kind":"lfo", "as":"osc"},
            {"op":"patch", "from":"@osc.out", "to":"vcf1.cutoff"},
            {"op":"turn", "module":"@osc", "knob":"rate", "value":3.0, "glide_beats":4.0},
            {"op":"tempo", "bpm":140.0}
        ]),
    );
    let result = ask(&mut wall, &request).unwrap();
    let id = result["steps"][0]["module"].as_str().unwrap();
    assert_eq!(result["steps"][0]["as"], "osc");
    assert!(wall.patch.module(id).is_some());
    assert_eq!(wall.patch.tempo.to_bits(), 140.0_f64.to_bits());
    assert_eq!(wall.log.len(), 1);
    assert_eq!(
        result["change"]["what"]["changes"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(result["change"]["what"]["changes"][2]["glide_beats"], 4.0);
    assert!(
        result["change"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("Tom: \"A moving voice\" (4 changes:")
    );
    assert_no_alloc::assert_no_alloc(|| engine.render(&mut [0.0; 128], 2));
    let seq = result["change"]["seq"].as_u64().unwrap();
    let undone = ask(&mut wall, &Request::Undo { change: seq }).unwrap();
    assert_eq!(undone["change"]["undoes"], seq);
    assert_eq!(wall.log.len(), 2);
    let after = wall.patch.to_file();
    assert_eq!(after.modules, before.modules);
    assert_eq!(after.cables, before.cables);
    assert_eq!(after.tempo.to_bits(), before.tempo.to_bits());
}

#[test]
fn batch_failed_third_step_leaves_patch_tempo_and_engine_unchanged() {
    let (mut wall, mut engine) = wall();
    engine.render(&mut [0.0; 128], 2);
    let before = wall.patch.to_file();
    let backlog = wall.engine.queued_commands();
    let request = batch(
        "Refused idea",
        serde_json::json!([
            {"op":"tempo", "bpm":160.0},
            {"op":"turn", "module":"vcf1", "knob":"cutoff", "value":300.0},
            {"op":"turn", "module":"missing", "knob":"cutoff", "value":400.0}
        ]),
    );
    let refused = ask(&mut wall, &request).unwrap_err();
    assert_eq!(serde_json::to_value(&refused).unwrap()["step"], 2);
    assert_eq!(refused.code, ErrorCode::UnknownModule);
    assert_eq!(wall.patch.to_file(), before);
    assert_eq!(wall.engine.queued_commands(), backlog);
    engine.render(&mut [0.0; 128], 2);
    assert_eq!(wall.engine.shared().bpm().to_bits(), before.tempo.to_bits());
    assert!(wall.log.is_empty());
}

#[test]
fn batch_boundary_refuses_notes_ops_and_forward_labels_without_changes() {
    let (mut wall, mut engine) = wall();
    let before = wall.patch.to_file();
    for note in [
        "",
        "  ",
        &"x".repeat(201),
        "split\nline",
        "record\u{1e}",
        "tab\t",
        "escape\u{1b}",
    ] {
        let request = batch(note, serde_json::json!([{"op":"tempo","bpm":160.0}]));
        assert_eq!(
            ask(&mut wall, &request).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(wall.patch.to_file(), before);
    }
    for op in [
        serde_json::json!({"op":"undo","change":1}),
        serde_json::json!({"op":"go","moment":"trunk"}),
        serde_json::json!({"op":"load","name":"x"}),
        serde_json::json!({"op":"chain_add","name":"x"}),
        serde_json::json!({"op":"group","group":"x","modules":[]}),
        serde_json::json!({"op":"arrange","module":"vcf1","row":0}),
        serde_json::json!({"op":"batch","note":"nested","changes":[]}),
    ] {
        let request = batch(
            "Refused operation",
            serde_json::json!([{"op":"tempo","bpm":160.0},op]),
        );
        let err = ask(&mut wall, &request).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(serde_json::to_value(err).unwrap()["step"], 1);
        assert_eq!(wall.patch.to_file(), before);
    }
    for changes in [
        serde_json::json!([]),
        serde_json::json!([{"op":"turn","module":"@later","knob":"rate","value":1.0},{"op":"add","kind":"lfo","as":"later"}]),
        serde_json::json!([{"op":"add","kind":"lfo","as":"same"},{"op":"add","kind":"lfo","as":"same"}]),
        serde_json::json!([{"op":"add","kind":"lfo","as":"bad.label"}]),
    ] {
        assert_eq!(
            ask(&mut wall, &batch("Labels", changes)).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(wall.patch.to_file(), before);
    }
    engine.render(&mut [0.0; 128], 2);
    assert_eq!(wall.engine.shared().bpm().to_bits(), before.tempo.to_bits());
}

#[test]
fn batch_flood_guard_counts_one_request_for_ten_steps() {
    let (mut wall, engine) = wall();
    drop(engine);
    let now = Instant::now();
    let request = batch("One idea", serde_json::json!((0..10).map(|_|serde_json::json!({"op":"turn","module":"vcf1","knob":"cutoff","value":300.0})).collect::<Vec<_>>()));
    for _ in 0..30 {
        assert!(wall.request("Loop", &request, now).is_ok());
    }
    assert_eq!(
        wall.request("Loop", &request, now).unwrap_err().code,
        ErrorCode::SlowDown
    );
    assert_eq!(wall.log.len(), 30);
}

#[test]
fn batch_never_undone_step_refuses_whole_undo() {
    let what: What = serde_json::from_value(serde_json::json!({"op":"batch","note":"recorded","changes":[{"op":"record","on":true,"path":"local.wav"}]})).unwrap();
    assert_eq!(
        never_undone(&what).unwrap().message,
        "a recording cannot be undone; stop it with record"
    );
    let (mut wall, engine) = wall();
    drop(engine);
    let change = wall.new_change("Tom", what, None);
    let seq = change.seq;
    wall.log.push_back(change);
    let before = wall.patch.to_file();
    assert_eq!(
        ask(&mut wall, &Request::Undo { change: seq })
            .unwrap_err()
            .code,
        ErrorCode::NotAllowed
    );
    assert_eq!(wall.patch.to_file(), before);
}

#[test]
fn batch_add_then_remove_keeps_a_complete_moment() {
    let (mut wall, engine) = wall();
    drop(engine);
    let before = wall.patch.to_file();
    let result = ask(
        &mut wall,
        &batch(
            "Try a voice",
            serde_json::json!([
                {"op":"add", "kind":"lfo", "as":"voice"},
                {"op":"remove", "module":"@voice"}
            ]),
        ),
    )
    .unwrap();
    assert_eq!(wall.patch.to_file().modules, before.modules);
    assert_eq!(
        result["change"]["what"]["changes"][0]["module"]["birth"],
        result["change"]["what"]["changes"][1]["module"]["birth"]
    );
    assert!(result["change"]["what"]["changes"][0]["module"]["birth"].is_string());
}

#[test]
fn a_loop_added_in_a_batch_is_reckoned_at_its_worst_as_a_single_add_is() {
    let (mut single, _single_engine) = wall();
    ask(&mut single, &add("loop")).unwrap();
    let (mut batched, _batched_engine) = wall();
    ask(
        &mut batched,
        &batch("a loop", serde_json::json!([{"op":"add","kind":"loop"}])),
    )
    .unwrap();
    assert_eq!(batched.file_grown, single.file_grown);
}

#[test]
fn a_cable_that_replaces_one_in_a_batch_grows_nothing_as_a_single_patch() {
    let (mut single, _single_engine) = wall();
    let cable = single.patch.to_file().cables[0].clone();
    let (from, to) = (cable.from, cable.to);
    let replace = Request::Patch {
        from: from.clone(),
        to: to.clone(),
        amount: Some(0.5),
    };
    ask(&mut single, &replace).unwrap();
    let (mut batched, _batched_engine) = wall();
    ask(
        &mut batched,
        &batch(
            "a softer cable",
            serde_json::json!([{"op":"patch","from":from,"to":to,"amount":0.5}]),
        ),
    )
    .unwrap();
    assert_eq!(batched.file_grown, single.file_grown);
}

#[test]
fn a_batch_of_one_change_says_one_change() {
    let (mut wall, _engine) = wall();
    let result = ask(
        &mut wall,
        &batch("faster", serde_json::json!([{"op":"tempo","bpm":130.0}])),
    )
    .unwrap();
    let summary = result["change"]["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("Tom: \"faster\" (1 change: "),
        "{summary}"
    );
}

#[test]
fn a_batch_tempo_asked_of_the_desk_is_undone_once_the_desk_is_gone() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 100.0;
    let what = What::Batch {
        note: "faster".to_string(),
        changes: vec![What::Tempo {
            from: 120.0,
            to: 100.0,
            desk: true,
        }],
    };
    let change = wall.new_change("Tom", what, None);
    let seq = change.seq;
    wall.next_seq = seq + 1;
    wall.log.push_back(change);
    let undone = ask(&mut wall, &Request::Undo { change: seq }).unwrap();
    assert_eq!(wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
    assert_eq!(undone["change"]["what"]["changes"][0]["desk"], false);
}

#[test]
fn undoing_a_batch_tempo_starts_from_the_tempo_now_as_a_single_undo_does() {
    let (mut wall, _engine) = wall();
    let first = wall.patch.tempo;
    let made = ask(
        &mut wall,
        &batch("faster", serde_json::json!([{"op":"tempo","bpm":130.0}])),
    )
    .unwrap();
    ask(&mut wall, &Request::Tempo { bpm: 140.0 }).unwrap();
    let seq = made["change"]["seq"].as_u64().unwrap();
    let undone = ask(&mut wall, &Request::Undo { change: seq }).unwrap();
    assert_eq!(undone["change"]["what"]["changes"][0]["from"], 140.0);
    assert_eq!(undone["change"]["what"]["changes"][0]["to"], first);
    assert_eq!(wall.patch.tempo.to_bits(), first.to_bits());
}

#[test]
fn a_batch_is_kept_in_the_log_and_grows_one_knot_with_its_note_and_steps() {
    let (mut wall, _engine, dir) = saving_wall();
    let initial = wall.patch.to_file();
    let made = ask(
        &mut wall,
        &batch(
            "A moving voice",
            serde_json::json!([
                {"op":"add", "kind":"lfo", "as":"osc"},
                {"op":"patch", "from":"@osc.out", "to":"vcf1.cutoff"},
                {"op":"turn", "module":"@osc", "knob":"rate", "value":3.0, "glide_beats":4.0},
                {"op":"tempo", "bpm":140.0}
            ]),
        ),
    )
    .unwrap();
    let seq = made["change"]["seq"].as_u64().unwrap();
    ask(&mut wall, &Request::Undo { change: seq }).unwrap();
    let log = std::fs::read_to_string(dir.path().join("log.jsonl")).unwrap();
    let kept: Vec<Change> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(kept, wall.log.iter().cloned().collect::<Vec<_>>());
    let state = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(state.path().join("saves")).unwrap();
    std::fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    std::fs::write(
        state.path().join("patch.json"),
        serde_json::to_vec(&wall.patch.to_file()).unwrap(),
    )
    .unwrap();
    std::fs::write(state.path().join("log.jsonl"), log).unwrap();
    let tree = state.path().join("tree");
    let grown = crate::tree::rings::grow(state.path(), &tree, false).unwrap();
    assert_eq!(grown.gaps, 0, "{}", grown.report);
    assert_eq!(grown.settles, 0, "{}", grown.report);
    assert_eq!(grown.changes, 2, "{}", grown.report);
    let output = crate::tree::git::git_command("log", Some(&tree))
        .args([
            "--all",
            "--reverse",
            "--format=%s%x1f%(trailers:key=Kazoo-Kind,key=Kazoo-Note,key=Kazoo-Step,unfold)%x1e",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let records: Vec<&str> = text
        .split('\u{1e}')
        .map(str::trim)
        .filter(|record| record.contains("Kazoo-Kind: batch") || record.contains("undid"))
        .collect();
    assert_eq!(records.len(), 2, "{text}");
    let (subject, trailers) = records[0].split_once('\u{1f}').unwrap();
    assert_eq!(subject, "Tom: A moving voice");
    let trailers: Vec<&str> = trailers.lines().collect();
    assert_eq!(trailers[0], "Kazoo-Kind: batch");
    assert_eq!(trailers[1], "Kazoo-Note: A moving voice");
    assert_eq!(trailers.len(), 6, "{trailers:?}");
    assert!(
        trailers[2..]
            .iter()
            .all(|line| line.starts_with("Kazoo-Step: "))
    );
    let (undo, trailers) = records[1].split_once('\u{1f}').unwrap();
    assert!(undo.starts_with("Tom undid change "), "{undo}");
    assert_eq!(trailers.trim(), "Kazoo-Kind: change");
}

#[test]
fn a_batch_the_log_refuses_as_the_engine_grows_leaves_it_its_old_room() {
    let (mut wall, mut engine, _dir) = saving_wall();
    wall.set_load_guard(false);
    engine.render(&mut [0.0; 128], 2);
    let slots = wall.engine.slots();
    let queued = wall.engine.queued_commands();
    let before = wall.patch.to_file();
    let adds = slots - wall.rack.slots.len() + 1;
    let request = batch(
        "a crowd",
        serde_json::json!(
            (0..adds)
                .map(|_| serde_json::json!({"op":"add","kind":"lfo"}))
                .collect::<Vec<_>>()
        ),
    );
    fail_once(&mut wall, FailAt::AppendLog);
    let refused = ask(&mut wall, &request).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal, "{}", refused.message);
    assert_eq!(wall.patch.to_file().modules, before.modules);
    assert_eq!(wall.patch.to_file().cables, before.cables);
    assert_eq!(wall.engine.slots(), slots);
    assert_eq!(wall.engine.queued_commands(), queued);
    let made = ask(&mut wall, &request).unwrap();
    assert!(wall.engine.slots() > slots);
    for step in made["steps"].as_array().unwrap() {
        let id = step["module"].as_str().unwrap();
        assert!(wall.rack.slot_of(id).is_some(), "{id} has no slot");
    }
    wall.engine.pump();
    engine.render(&mut [0.0; 128], 2);
    assert_eq!(wall.engine.shared().slots(), wall.engine.slots());
}
