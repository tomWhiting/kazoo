use super::*;

fn request(description: serde_json::Value, attachments: serde_json::Value) -> Request {
    let mut value =
        serde_json::json!({"op":"batch", "note":"Proof", "changes":[{"op":"tempo","bpm":140.0}]});
    value["description"] = description;
    value["attachments"] = attachments;
    serde_json::from_value(value).unwrap()
}

#[test]
fn notes_validation_boundaries() {
    let (mut wall, _engine) = wall();
    let before = wall.patch.to_file();
    let mut bad = vec![
        request(serde_json::json!(""), serde_json::json!([])),
        request(serde_json::json!("x".repeat(8193)), serde_json::json!([])),
    ];
    for name in [
        "",
        ".hidden",
        "description.md",
        "DESCRIPTION.MD",
        "bad/name",
        "bad name",
        "é",
        &"a".repeat(65),
    ] {
        bad.push(request(
            serde_json::Value::Null,
            serde_json::json!([{"name":name,"text":"proof"}]),
        ));
    }
    for text in [
        String::new(),
        "x".repeat(65537),
        "bad\rtext".into(),
        "bad\0text".into(),
    ] {
        bad.push(request(
            serde_json::Value::Null,
            serde_json::json!([{"name":"proof.md","text":text}]),
        ));
    }
    bad.push(request(
        serde_json::Value::Null,
        serde_json::json!([{"name":"a","text":"x"},{"name":"A","text":"y"}]),
    ));
    bad.push(request(
        serde_json::Value::Null,
        serde_json::json!(
            (0..9)
                .map(|n| serde_json::json!({"name":format!("n{n}"),"text":"x"}))
                .collect::<Vec<_>>()
        ),
    ));
    bad.push(request(serde_json::json!("x"), serde_json::json!([{"name":"a","text":"x".repeat(65536)},{"name":"b","text":"x".repeat(65536)}])));
    for request in bad {
        let refused = ask(&mut wall, &request).unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadRequest, "{refused:?}");
        assert_eq!(wall.patch.to_file(), before);
        assert!(wall.log.is_empty());
    }
}

#[test]
fn notes_store_read_summary_undo_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.store = Some(Store::open(dir.path()).unwrap());
    let request = request(
        serde_json::json!("# Why\n\tA proof"),
        serde_json::json!([{"name":"proof.md","text":"QED"},{"name":"same.txt","text":"QED"}]),
    );
    let result = ask(&mut wall, &request).unwrap();
    let seq = result["change"]["seq"].as_u64().unwrap();
    let notes = result["change"]["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 3);
    assert_eq!(notes[0]["name"], "description.md");
    assert_eq!(notes[1]["sha256"], notes[2]["sha256"]);
    assert_eq!(
        std::fs::read_dir(dir.path().join("notes")).unwrap().count(),
        2
    );
    let log = std::fs::read_to_string(dir.path().join("log.jsonl")).unwrap();
    assert!(!log.contains("QED"));
    assert!(!log.contains("# Why"));
    assert!(
        result["change"]["summary"]
            .as_str()
            .unwrap()
            .contains(", with notes: description.md, proof.md, same.txt")
    );
    let read: Request =
        serde_json::from_value(serde_json::json!({"op":"notes","seq":seq})).unwrap();
    let answer = ask(&mut wall, &read).unwrap();
    assert_eq!(answer["description"], "# Why\n\tA proof");
    assert_eq!(answer["attachments"][0]["text"], "QED");
    std::fs::remove_file(
        dir.path()
            .join("notes")
            .join(notes[1]["sha256"].as_str().unwrap()),
    )
    .unwrap();
    let answer = ask(&mut wall, &read).unwrap();
    assert!(
        answer["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "proof.md")
    );
    let undo = ask(&mut wall, &Request::Undo { change: seq }).unwrap();
    assert!(undo["change"].get("notes").is_none());
    let no_notes: Request =
        serde_json::from_value(serde_json::json!({"op":"notes","seq":seq+1})).unwrap();
    assert_eq!(
        serde_json::to_value(ask(&mut wall, &no_notes).unwrap_err()).unwrap()["code"],
        "not_found"
    );
}

#[test]
fn notes_failed_store_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.store = Some(Store::open(dir.path()).unwrap());
    std::fs::write(dir.path().join("notes"), "obstruction").unwrap();
    let before = wall.patch.to_file();
    let queued = wall.engine.queued_commands();
    let refused = ask(
        &mut wall,
        &request(serde_json::json!("proof"), serde_json::json!([])),
    )
    .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    assert_eq!(wall.patch.to_file(), before);
    assert_eq!(wall.engine.queued_commands(), queued);
    assert!(wall.log.is_empty());
}

#[test]
fn notes_step_limit_remains_4096() {
    let changes = vec![Request::Tempo { bpm: 140.0 }; 4096];
    assert!(crate::protocol::batch::validate("limit", &changes, &[]).is_ok());
    let changes = vec![Request::Tempo { bpm: 140.0 }; 4097];
    assert!(crate::protocol::batch::validate("limit", &changes, &[]).is_err());
}

#[test]
fn notes_exist_even_when_logging_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.store = Some(Store::open(dir.path()).unwrap());
    fail_once(&mut wall, FailAt::AppendLog);
    assert!(
        ask(
            &mut wall,
            &request(serde_json::json!("proof"), serde_json::json!([]))
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read_dir(dir.path().join("notes")).unwrap().count(),
        1
    );
}

#[test]
fn notes_summary_follows_the_change_count() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.store = Some(Store::open(dir.path()).unwrap());
    let made = ask(
        &mut wall,
        &request(
            serde_json::json!("Why"),
            serde_json::json!([{"name":"proof.md","text":"x".repeat(12*1024)}]),
        ),
    )
    .unwrap();
    let summary = made["change"]["summary"].as_str().unwrap();
    assert!(
        summary.contains("(1 change, with notes: description.md, proof.md (12 KB):"),
        "{summary}"
    );
}

#[test]
fn notes_read_changes_beyond_the_memory_window() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.store = Some(Store::open(dir.path()).unwrap());
    let made = ask(
        &mut wall,
        &request(serde_json::json!("Why"), serde_json::json!([])),
    )
    .unwrap();
    wall.log.clear();
    let read: Request =
        serde_json::from_value(serde_json::json!({"op":"notes","seq":made["change"]["seq"]}))
            .unwrap();
    assert_eq!(ask(&mut wall, &read).unwrap()["description"], "Why");
}

#[test]
fn notes_refuse_abandoned_batches_in_memory_and_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine) = wall();
    wall.store = Some(Store::open(dir.path()).unwrap());
    let made = ask(
        &mut wall,
        &request(serde_json::json!("Why"), serde_json::json!([])),
    )
    .unwrap();
    let seq = made["change"]["seq"].as_u64().unwrap();
    wall.not_made(seq);
    let read: Request =
        serde_json::from_value(serde_json::json!({"op":"notes","seq":seq})).unwrap();
    for disk in [false, true] {
        if disk {
            wall.log.clear();
        }
        let refused = ask(&mut wall, &read).unwrap_err();
        assert_eq!(serde_json::to_value(&refused).unwrap()["code"], "not_found");
        assert!(refused.message.contains("not made"), "{refused:?}");
    }
}
