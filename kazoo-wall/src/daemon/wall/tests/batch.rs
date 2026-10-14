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
