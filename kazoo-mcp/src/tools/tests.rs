use super::*;

#[tokio::test]
async fn wall_undo_renders_replacements_and_notes_its_own_change() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    for what in ["go", "load"] {
        let dir = tempfile::Builder::new()
            .prefix("mu")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("wall.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let destination =
            serde_json::json!({"commit": "a".repeat(40), "branch": "trunk", "change": 1});
        let change = serde_json::json!({
            "seq": 3, "at": "2026-10-13T10:00:00Z", "seat": "Tom", "summary": "undid replacement", "undoes": 2,
            "what": {"op": what, "to": destination, "from": destination, "save": "dawn", "backup": "kept-wall", "fingerprint": "abc", "modules": 1, "cables": 0, "tempo": 120.0, "by_console": true},
        });
        let fixture =
            serde_json::json!({"change": change, "backup": "kept-wall", "notes": ["kept tempo"]});
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            for expected in ["hello", "undo"] {
                let request: serde_json::Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["op"], expected);
                let result = if expected == "hello" {
                    serde_json::json!({"daemon": "stub", "seat": "Tom", "console": false, "seats": ["Tom"], "revision": 0})
                } else {
                    fixture.clone()
                };
                let response =
                    serde_json::json!({"id": request["id"], "ok": true, "result": result});
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let (feed, mut heard) = mpsc::unbounded_channel();
        let (peer, receiver) = watch::channel(None);
        let seat = Seat::new(Arc::new(Wall::new(socket, "Tom".into())), feed, peer);
        let result = seat
            .wall_undo(Parameters(UndoArgs {
                change: 2,
                comment: None,
            }))
            .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let text = result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("kept-wall"), "{text}");
        assert!(text.contains("kept tempo"), "{text}");
        if what == "go" {
            assert!(
                text.contains("The wall is back on trunk: the next change carries it on."),
                "{text}"
            );
        } else {
            assert!(!text.contains("The wall is"), "{text}");
        }
        assert!(matches!(heard.recv().await, Some(Feed::Own(3))));
        server.await.unwrap();
        drop(receiver);
    }
}

#[tokio::test]
async fn a_listed_save_tag_looks_up_that_moment() {
    use kazoo_wall::protocol::TreeResult;
    let runtime = tempfile::Builder::new()
        .prefix("mt")
        .tempdir_in("/tmp")
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("wall");
    // A test daemon keeps its tree inside its own state (KZ-T04g), never
    // beside it where a person's tree would be.
    std::fs::create_dir_all(&state).unwrap();
    kazoo_wall::tree::git::plant(&state.join("tree")).unwrap();
    let daemon = kazoo_wall::daemon::Daemon::start(kazoo_wall::daemon::DaemonConfig {
        load_guard: false,
        ..kazoo_wall::daemon::DaemonConfig::headless(runtime.path(), &state)
    })
    .unwrap();
    let wall = Wall::new(daemon.socket().to_path_buf(), "Tom".into());
    wall.ask::<SaveResult>(Request::Save {
        name: "low end".into(),
        overwrite: false,
    })
    .await
    .unwrap();
    wall.ask::<MarkResult>(Request::Mark {
        name: "low end".into(),
    })
    .await
    .unwrap();
    let Answer::Read(list) = wall
        .ask::<TreeResult>(Request::Tree { recent: None })
        .await
        .unwrap()
    else {
        panic!("unreadable tree")
    };
    let save = list
        .marks
        .iter()
        .find(|mark| mark.kind == kazoo_wall::protocol::MarkKind::Save)
        .unwrap();
    let rendered = render::tree(&list);
    let prefix = "(save; wall_go \"";
    let tag = rendered
        .split_once(prefix)
        .expect("save supplies lookup form")
        .1
        .split_once('"')
        .unwrap()
        .0;
    assert_eq!(tag, save.tag);
    wall.ask::<ChangeResult>(Request::Turn {
        module: "vcf1".into(),
        knob: "cutoff".into(),
        value: 1234.0,
        glide_beats: Some(0.0),
    })
    .await
    .unwrap();
    // The mark's answer waits until the tree has recorded the turn.
    let Answer::Read(moved) = wall
        .ask::<MarkResult>(Request::Mark {
            name: "after turn".into(),
        })
        .await
        .unwrap()
    else {
        panic!("unreadable mark")
    };
    assert_ne!(moved.at.commit, save.moment.commit);
    let Answer::Read(result) = wall
        .ask::<LoadResult>(Request::Go { moment: tag.into() })
        .await
        .unwrap()
    else {
        panic!("unreadable go")
    };
    let kazoo_wall::protocol::What::Go { to, .. } = result.change.what else {
        panic!("go destination absent")
    };
    assert_eq!(to.commit, save.moment.commit);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn wall_loop_needs_exactly_one_of_erase_and_from() {
    for json in [
        r#"{"module":"loop1"}"#,
        r#"{"module":"loop1","erase":false}"#,
        r#"{"module":"loop1","erase":true,"from":"mouth/dry"}"#,
        r#"{"module":"loop1","erase":true,"from":null}"#,
    ] {
        let args: LoopArgs = serde_json::from_str(json).unwrap();
        assert_eq!(
            args.request().unwrap_err(),
            "Give exactly one of erase: true or from (a desk strip label, or null for patched inputs)."
        );
    }
    for (json, op) in [
        (r#"{"module":"loop1","erase":true}"#, LoopOp::Erase),
        (
            r#"{"module":"loop1","from":"mouth/dry"}"#,
            LoopOp::From {
                from: Some("mouth/dry".into()),
            },
        ),
        (
            r#"{"module":"loop1","from":null}"#,
            LoopOp::From { from: None },
        ),
        (
            r#"{"module":"loop1","erase":false,"from":null}"#,
            LoopOp::From { from: None },
        ),
    ] {
        let args: LoopArgs = serde_json::from_str(json).unwrap();
        assert_eq!(
            args.request().unwrap(),
            Request::Loop {
                module: "loop1".into(),
                op
            }
        );
    }
    assert!(serde_json::from_str::<LoopArgs>(r#"{"module":"loop1","from":42}"#).is_err());
}

#[test]
fn a_batch_carries_each_steps_comment_beside_it() {
    let args: super::batch::BatchArgs = serde_json::from_value(serde_json::json!({
        "note": "brass",
        "changes": [
            {"op": "tempo", "bpm": 70.0, "comment": "half-time"},
            {"op": "turn", "module": "vcf1", "knob": "cutoff", "value": 500.0},
        ],
    }))
    .unwrap();
    let Request::Batch {
        changes, comments, ..
    } = args.request().unwrap()
    else {
        panic!("a batch");
    };
    assert_eq!(comments, [Some("half-time".to_string()), None]);
    assert_eq!(
        changes[0],
        Request::Tempo { bpm: 70.0 },
        "the step is the single tool's"
    );
    let none: super::batch::BatchArgs = serde_json::from_value(serde_json::json!({
        "note": "plain",
        "changes": [{"op": "tempo", "bpm": 70.0}],
    }))
    .unwrap();
    let Request::Batch { comments, .. } = none.request().unwrap() else {
        panic!("a batch");
    };
    assert!(comments.is_empty());
}

#[test]
fn every_tool_that_makes_a_change_takes_a_comment() {
    let tools = Seat::all_tools().list_all();
    for name in [
        "wall_turn",
        "wall_patch",
        "wall_unpatch",
        "wall_add",
        "wall_remove",
        "wall_chain",
        "wall_undo",
        "wall_tempo",
        "wall_loop",
        "wall_record",
        "wall_load",
        "wall_go",
    ] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        let schema = serde_json::Value::Object(tool.input_schema.as_ref().clone());
        assert!(
            schema["properties"]["comment"].is_object(),
            "{name} takes no comment"
        );
    }
}

/// Every tool, in the router's order.
const TOOLS: [&str; 25] = [
    "wall_add",
    "wall_batch",
    "wall_catalogue",
    "wall_chain",
    "wall_go",
    "wall_group",
    "wall_listen",
    "wall_load",
    "wall_log",
    "wall_look",
    "wall_loop",
    "wall_mark",
    "wall_notes",
    "wall_patch",
    "wall_probe",
    "wall_record",
    "wall_remove",
    "wall_save",
    "wall_saves",
    "wall_speak",
    "wall_tempo",
    "wall_tree",
    "wall_turn",
    "wall_undo",
    "wall_unpatch",
];

#[test]
fn the_router_holds_every_tool_with_a_description_and_schema() {
    let tools = Seat::all_tools().list_all();
    let mut names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, TOOLS);
    for tool in &tools {
        let about = tool.description.as_deref().unwrap_or_default();
        assert!(about.len() > 60, "{} is barely described", tool.name);
        assert_eq!(
            tool.input_schema.get("type").and_then(|kind| kind.as_str()),
            Some("object"),
            "{}",
            tool.name
        );
    }
    let turn = tools
        .iter()
        .find(|tool| tool.name == "wall_turn")
        .map(|tool| serde_json::Value::Object(tool.input_schema.as_ref().clone()))
        .unwrap_or_default();
    let required = turn["required"].as_array().cloned().unwrap_or_default();
    assert_eq!(required, ["module", "knob", "value"]);
    let glide = turn["properties"]["glide_beats"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(glide.contains("beats"), "{glide}");
}

#[test]
fn arguments_are_strict() {
    assert!(serde_json::from_str::<TurnArgs>(r#"{"module":"vcf1","knob":"cutoff"}"#).is_err());
    assert!(
        serde_json::from_str::<TurnArgs>(
            r#"{"module":"vcf1","knob":"cutoff","value":800,"glide":4}"#
        )
        .is_err(),
        "an unknown field is refused, not ignored"
    );
    let unpatch: UnpatchArgs = serde_json::from_str(r#"{"to":"vcf1.cutoff"}"#).unwrap();
    assert_eq!(unpatch.to.as_deref(), Some("vcf1.cutoff"));
    assert_eq!(unpatch.cable, None);
    assert!(serde_json::from_str::<NoArgs>("{}").is_ok());
    assert!(
        serde_json::from_str::<RecordArgs>("{}").is_err(),
        "on is required"
    );
    let record: RecordArgs = serde_json::from_str(r#"{"on":true}"#).unwrap();
    assert!(record.on);
    // A seat cannot ask to save over a save: there is no such field.
    assert!(
        serde_json::from_str::<SaveArgs>(r#"{"name":"jam","overwrite":true}"#).is_err(),
        "overwrite is the console's"
    );
    let probe: ProbeArgs = serde_json::from_str("{}").unwrap();
    assert_eq!(probe.module, None);
    let probe: ProbeArgs = serde_json::from_str(r#"{"module":"scope1"}"#).unwrap();
    assert_eq!(probe.module.as_deref(), Some("scope1"));
    assert!(serde_json::from_str::<ProbeArgs>(r#"{"modules":["scope1"]}"#).is_err());
    let save: SaveArgs = serde_json::from_str(r#"{"name":"jam"}"#).unwrap();
    assert_eq!(save.name, "jam");
    assert!(
        serde_json::from_str::<LoadArgs>("{}").is_err(),
        "name is required"
    );
    assert!(serde_json::from_str::<MarkArgs>("{}").is_err());
    let mark: MarkArgs = serde_json::from_str(r#"{"name":"dawn"}"#).unwrap();
    assert_eq!(mark.name, "dawn");
    assert!(serde_json::from_str::<MarkArgs>(r#"{"name":"dawn","overwrite":true}"#).is_err());
    assert!(serde_json::from_str::<GoArgs>("{}").is_err());
    let go: GoArgs = serde_json::from_str(r##"{"moment":"#57"}"##).unwrap();
    assert_eq!(go.moment, "#57");
    let tree: TreeArgs = serde_json::from_str("{}").unwrap();
    assert_eq!(tree.recent, None);
    let tree: TreeArgs = serde_json::from_str(r#"{"recent":50}"#).unwrap();
    assert_eq!(tree.recent, Some(50));
    assert!(serde_json::from_str::<TreeArgs>(r#"{"limit":5}"#).is_err());
}

/// Each property in `schema` with no description, by its path; the op tag
/// (a constant) says itself.
fn undescribed(schema: &serde_json::Value, path: &str, missing: &mut Vec<String>) {
    match schema {
        serde_json::Value::Object(object) => {
            if let Some(serde_json::Value::Object(properties)) = object.get("properties") {
                for (name, property) in properties {
                    let constant =
                        property.get("const").is_some() || property.get("enum").is_some();
                    if !constant && property.get("description").is_none() {
                        missing.push(format!("{path}.{name}"));
                    }
                }
            }
            for (key, value) in object {
                undescribed(value, &format!("{path}/{key}"), missing);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                undescribed(item, &format!("{path}[{index}]"), missing);
            }
        }
        _ => {}
    }
}

#[test]
fn wall_batch_describes_its_note_its_changes_and_every_field_of_a_step() {
    let tools = Seat::all_tools().list_all();
    let batch = tools
        .iter()
        .find(|tool| tool.name == "wall_batch")
        .map(|tool| serde_json::Value::Object(tool.input_schema.as_ref().clone()))
        .unwrap_or_default();
    let mut missing = Vec::new();
    undescribed(&batch, "", &mut missing);
    assert!(missing.is_empty(), "undescribed: {missing:?}\n{batch:#}");
    let text = batch.to_string();
    assert!(text.contains("@label"), "{batch:#}");
}

#[test]
fn notes_schema_roundtrip_preserves_texts_and_readback() {
    let value = serde_json::json!({"note":"Proof", "description":"# Why\nline", "attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"tempo","bpm":140.0}]});
    let args: super::batch::BatchArgs = serde_json::from_value(value).unwrap();
    let request = args.request().unwrap();
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["description"], "# Why\nline");
    assert_eq!(wire["attachments"][0]["text"], "QED");
    assert_eq!(serde_json::from_value::<Request>(wire).unwrap(), request);
    let tools = Seat::all_tools().list_all();
    assert!(tools.iter().any(|tool| tool.name == "wall_notes"));
    let batch = tools.iter().find(|tool| tool.name == "wall_batch").unwrap();
    let schema = serde_json::Value::Object(batch.input_schema.as_ref().clone());
    assert!(
        schema["properties"]["attachments"]["description"]
            .as_str()
            .unwrap()
            .contains("read back with wall_notes")
    );
}
