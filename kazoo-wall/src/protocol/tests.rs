//! Protocol tests: every message survives a round trip through JSON, and
//! the wire shapes match the design.

use serde_json::json;

use super::*;

fn round_trip_request(request: Request) {
    let line = RequestLine { id: 7, request };
    let text = serde_json::to_string(&line).unwrap();
    let back: RequestLine = serde_json::from_str(&text).unwrap();
    assert_eq!(back, line, "{text}");
}

#[test]
fn every_request_round_trips() {
    for request in [
        Request::Hello {
            seat: "Waffles".to_string(),
            client: "kazoo-mcp 0.1.0".to_string(),
            console: false,
            watcher: false,
        },
        Request::Hello {
            seat: "meridian".to_string(),
            client: "meridian world".to_string(),
            console: false,
            watcher: true,
        },
        Request::Hello {
            seat: "Tom".to_string(),
            client: "kazoo-wall tui".to_string(),
            console: true,
            watcher: false,
        },
        Request::Look,
        Request::Catalogue,
        Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: 800.0,
            glide_beats: Some(4.0),
        },
        Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: 800.0,
            glide_beats: None,
        },
        Request::Patch {
            from: "lfo1.out".to_string(),
            to: "vcf1.cutoff".to_string(),
            amount: Some(0.4),
        },
        Request::Unpatch {
            cable: Some(12),
            to: None,
        },
        Request::Unpatch {
            cable: None,
            to: Some("vcf1.cutoff".to_string()),
        },
        Request::Add {
            kind: "lfo".to_string(),
            name: Some("slow wobble".to_string()),
            place: None,
        },
        Request::Add {
            kind: "lfo".to_string(),
            name: None,
            place: Some(Place {
                row: 2,
                before: Some("vcf1".to_string()),
                own: false,
            }),
        },
        Request::Remove {
            module: "lfo3".to_string(),
        },
        Request::Arrange {
            module: "lfo3".to_string(),
            row: 1,
            before: Some("vco2".to_string()),
            own: false,
        },
        Request::Arrange {
            module: "lfo3".to_string(),
            row: 4,
            before: None,
            own: true,
        },
        Request::Undo { change: 57 },
        Request::Log {
            before: None,
            limit: Some(50),
        },
        Request::Listen,
        Request::Tempo { bpm: 96.0 },
        Request::Subscribe,
        Request::Monitor { on: true },
        Request::Monitor { on: false },
        Request::Record { on: true },
        Request::Record { on: false },
        Request::Shutdown,
    ] {
        round_trip_request(request);
    }
}

#[test]
fn the_design_examples_parse() {
    let examples = [
        r#"{"id":1,"op":"hello","seat":"Waffles","client":"kazoo-mcp 0.1.0"}"#,
        r#"{"id":2,"op":"look"}"#,
        r#"{"id":4,"op":"turn","module":"vcf1","knob":"cutoff","value":800.0,"glide_beats":4.0}"#,
        r#"{"id":5,"op":"patch","from":"lfo1.out","to":"vcf1.cutoff","amount":0.4}"#,
        r#"{"id":6,"op":"unpatch","cable":12}"#,
        r#"{"id":6,"op":"unpatch","to":"vcf1.cutoff"}"#,
        r#"{"id":7,"op":"add","kind":"lfo","name":"slow wobble"}"#,
        r#"{"id":10,"op":"log","before":null,"limit":50}"#,
        r#"{"id":13,"op":"subscribe"}"#,
        r#"{"id":14,"op":"shutdown"}"#,
    ];
    for example in examples {
        let line: RequestLine = serde_json::from_str(example).unwrap();
        assert!(line.id > 0, "{example}");
    }
    let bad = [
        r#"{"id":1,"op":"dance"}"#,
        r#"{"op":"look"}"#,
        r#"{"id":1,"op":"turn"}"#,
    ];
    for example in bad {
        assert!(
            serde_json::from_str::<RequestLine>(example).is_err(),
            "{example}"
        );
    }
}

#[test]
fn responses_and_events_have_the_design_shape() {
    let error = Response::failure(
        4,
        WallError::new(ErrorCode::UnknownKnob, "vcf1 has no knob 'cutof'"),
    );
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({"id":4,"ok":false,"error":{"code":"unknown_knob","message":"vcf1 has no knob 'cutof'"}})
    );
    let ok = Response::success(2, &ShutdownResult { stopping: true }).unwrap();
    assert_eq!(
        serde_json::to_value(&ok).unwrap(),
        json!({"id":2,"ok":true,"result":{"stopping":true}})
    );
    let event = Event::Seat {
        seat: "Vesper".to_string(),
        joined: true,
        seq: Some(57),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"event":"seat","seat":"Vesper","joined":true,"seq":57})
    );
    // A seat event from before seq still reads.
    let old: Event =
        serde_json::from_value(json!({"event":"seat","seat":"Vesper","joined":true})).unwrap();
    assert_eq!(
        old,
        Event::Seat {
            seat: "Vesper".to_string(),
            joined: true,
            seq: None
        }
    );
    let change = Change {
        seq: 58,
        at: "2026-09-26T12:00:01Z".to_string(),
        seat: "Tom".to_string(),
        what: What::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            from: 420.0,
            to: 800.0,
            glide_beats: 4.0,
        },
        summary: "Tom turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats".to_string(),
        undoes: None,
    };
    let event = Event::Change {
        change: Box::new(change),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["event"], "change");
    assert_eq!(value["change"]["what"]["op"], "turn");
    assert!(value["change"].get("undoes").is_none());
    let text = serde_json::to_string(&event).unwrap();
    assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Event(event));
    let text = serde_json::to_string(&ok).unwrap();
    assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Response(ok));
    assert!(ServerLine::parse("{").is_err());
    let fault = Event::Fault {
        summary: "vco2 produced NaN; reset".to_string(),
        seq: Some(3),
    };
    let text = serde_json::to_string(&fault).unwrap();
    assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Event(fault));
}

#[test]
fn every_error_code_has_its_wire_name() {
    let names: Vec<String> = ErrorCode::ALL
        .iter()
        .map(|code| {
            serde_json::to_value(code)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "bad_request",
            "not_hello",
            "bad_name",
            "unknown_module",
            "unknown_knob",
            "unknown_port",
            "unknown_kind",
            "unknown_cable",
            "unknown_change",
            "full",
            "slow_down",
            "not_allowed",
            "internal"
        ]
    );
}

#[test]
fn names_are_limited_to_the_safe_charset() {
    for good in [
        "Tom",
        "Waffles",
        "slow wobble",
        "a.b-c_d",
        "x",
        "123456789012345678901234",
    ] {
        assert!(valid_name(good), "{good}");
    }
    for bad in [
        "",
        "1234567890123456789012345",
        "Tom\n",
        "émile",
        "<b>",
        "a/b",
        "tab\there",
    ] {
        assert!(!valid_name(bad), "{bad:?}");
    }
}

#[test]
fn changes_undo_and_every_what_round_trip() {
    let module = ModuleRecord {
        id: "lfo3".to_string(),
        kind: "lfo".to_string(),
        name: None,
        knobs: std::iter::once(("rate".to_string(), 0.5)).collect(),
    };
    let cable = CableRecord {
        id: 3,
        from: "lfo3.out".to_string(),
        to: "vcf1.cutoff".to_string(),
        amount: 0.5,
    };
    for what in [
        What::Patch {
            cable: cable.clone(),
            replaced: None,
        },
        What::Unpatch {
            cable: cable.clone(),
            replugged: Some(cable.clone()),
        },
        What::Add {
            module: module.clone(),
        },
        What::Remove {
            module: module.clone(),
            cables: vec![cable.clone()],
            replugged: Vec::new(),
            place: Some(Place {
                row: 1,
                before: Some("vco1".to_string()),
                own: false,
            }),
        },
        What::Restore {
            module,
            cables: vec![cable.clone()],
            replaced: Vec::new(),
            skipped: vec![cable],
            place: None,
        },
        What::Tempo {
            from: 90.0,
            to: 100.0,
            desk: true,
        },
    ] {
        let change = Change {
            seq: 1,
            at: "2026-09-26T00:00:00Z".to_string(),
            seat: "Tom".to_string(),
            what,
            summary: String::new(),
            undoes: Some(9),
        };
        let text = serde_json::to_string(&change).unwrap();
        assert_eq!(serde_json::from_str::<Change>(&text).unwrap(), change);
    }
}

#[test]
fn knob_values_travel_as_the_numbers_people_meant() {
    assert_eq!(widen(0.35).to_string(), "0.35");
    assert_eq!(widen(0.2).to_string(), "0.2");
    assert!((widen(18_000.0) - 18_000.0).abs() < f64::EPSILON);
    assert!(widen(f32::NAN).is_nan());
    let exact: f32 = 0.1 + 0.2;
    assert!((widen(exact) as f32 - exact).abs() < f32::EPSILON);
}

#[test]
fn change_ops_are_the_flood_guarded_ones() {
    assert!(Request::Tempo { bpm: 90.0 }.is_change());
    assert!(!Request::Look.is_change());
    assert!(!Request::Subscribe.is_change());
    // Whether the wall is heard is not a change to the wall.
    assert!(!Request::Monitor { on: true }.is_change());
    assert_eq!(Request::Monitor { on: false }.op(), "monitor");
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 4,
            request: Request::Monitor { on: true }
        })
        .unwrap(),
        json!({"id":4,"op":"monitor","on":true})
    );
    assert_eq!(Request::Undo { change: 1 }.op(), "undo");
}

#[test]
fn fingerprints_have_their_shape_and_old_snapshots_still_read() {
    let event = Event::Fingerprints {
        seq: 233,
        modules: std::iter::once((
            "vco4".to_string(),
            [("Tom".to_string(), 0.62), ("Cassio".to_string(), 0.38)]
                .into_iter()
                .collect(),
        ))
        .collect(),
        cables: std::iter::once(("41".to_string(), crate::fingerprints::Shares::new())).collect(),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"event":"fingerprints","seq":233,
               "modules":{"vco4":{"Cassio":0.38,"Tom":0.62}},
               "cables":{"41":{}}})
    );
    let text = serde_json::to_string(&event).unwrap();
    assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Event(event));
    // A hello result and a snapshot from before watchers and fingerprints.
    let hello: HelloResult = serde_json::from_value(json!({
        "daemon":"kazoo-wall 0.1.0","seat":"Cassio","console":false,"seats":["Cassio"],"revision":230
    }))
    .unwrap();
    assert!(!hello.watcher);
    let snapshot: Snapshot = serde_json::from_value(json!({
        "revision":1,"tempo":90.0,"beat":0.0,"clock":"own","on_desk":false,"seats":[],
        "modules":[],"cables":[],"levels":{"peak_l":-120.0,"peak_r":-120.0},"listen":null,
        "faults":{"count":0,"recent":[]}
    }))
    .unwrap();
    assert!(snapshot.fingerprints.is_empty());
    // A wall from before the monitor was always heard.
    assert!(snapshot.heard);
    // A watcher's hello says so; a seat's leaves the field out.
    let line: RequestLine = serde_json::from_value(
        json!({"id":1,"op":"hello","seat":"meridian","client":"world","watcher":true}),
    )
    .unwrap();
    assert!(matches!(line.request, Request::Hello { watcher: true, .. }));
}

#[test]
fn a_change_this_build_does_not_know_still_reads() {
    // A newer wall may log ops, and push events, that this build has never
    // heard of: they read as unknown rather than breaking the line.
    let line = json!({
        "event": "change",
        "change": {
            "seq": 9,
            "at": "2026-09-26T10:00:00Z",
            "seat": "Tom",
            "what": {"op": "juggle", "balls": 3},
            "summary": "Tom juggled"
        }
    });
    let read = ServerLine::parse(&line.to_string());
    assert!(read.is_ok(), "{read:?}");
    let page = json!({"changes": [line["change"].clone()], "more": false});
    let read: Result<LogPage, _> = serde_json::from_value(page);
    assert!(read.is_ok(), "{read:?}");
    let event = json!({"event": "weather", "sky": "clear"});
    let read = ServerLine::parse(&event.to_string());
    assert!(read.is_ok(), "{read:?}");
}

#[test]
fn recording_has_its_shape_and_old_snapshots_have_none() {
    // A watcher may not record: it counts as a change.
    assert!(Request::Record { on: true }.is_change());
    assert_eq!(Request::Record { on: false }.op(), "record");
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 7,
            request: Request::Record { on: true }
        })
        .unwrap(),
        json!({"id":7,"op":"record","on":true})
    );
    let stopped = What::Record {
        on: false,
        path: "/Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav".to_string(),
        seconds: Some(200.5),
        dropped: 0,
        continues: None,
        reason: None,
    };
    let value = serde_json::to_value(&stopped).unwrap();
    assert_eq!(
        value,
        json!({"op":"record","on":false,
               "path":"/Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav",
               "seconds":200.5,"dropped":0})
    );
    assert_eq!(serde_json::from_value::<What>(value).unwrap(), stopped);
    // A start reads without the stop's fields.
    let started: What =
        serde_json::from_value(json!({"op":"record","on":true,"path":"/tmp/a.wav"})).unwrap();
    assert_eq!(
        started,
        What::Record {
            on: true,
            path: "/tmp/a.wav".to_string(),
            seconds: None,
            dropped: 0,
            continues: None,
            reason: None,
        }
    );
    // A snapshot from before recording has none under way.
    let snapshot: Snapshot = serde_json::from_value(json!({
        "revision":1,"tempo":90.0,"beat":0.0,"clock":"own","on_desk":false,"seats":[],
        "modules":[],"cables":[],"levels":{"peak_l":-120.0,"peak_r":-120.0},"listen":null,
        "faults":{"count":0,"recent":[]}
    }))
    .unwrap();
    assert_eq!(snapshot.recording, None);
    let result: RecordResult =
        serde_json::from_value(json!({"on":false,"seconds":0.0,"dropped":0})).unwrap();
    assert_eq!(result.path, None);
    assert_eq!(result.change, None);
}

#[test]
fn the_rack_has_its_shape_and_older_walls_and_consoles_still_read() {
    // A move is a request any seat but a watcher may make, flood-guarded
    // like a change.
    let arrange = Request::Arrange {
        module: "lfo3".to_string(),
        row: 1,
        before: Some("vco2".to_string()),
        own: false,
    };
    assert!(arrange.is_change());
    assert_eq!(arrange.op(), "arrange");
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 9,
            request: arrange,
        })
        .unwrap(),
        json!({"id":9,"op":"arrange","module":"lfo3","row":1,"before":"vco2"})
    );
    let line: RequestLine =
        serde_json::from_value(json!({"id":9,"op":"arrange","module":"lfo3","row":0})).unwrap();
    assert_eq!(
        line.request,
        Request::Arrange {
            module: "lfo3".to_string(),
            row: 0,
            before: None,
            own: false,
        }
    );
    // An add from an older seat has no place; one with a place says so.
    let line: RequestLine =
        serde_json::from_value(json!({"id":7,"op":"add","kind":"lfo"})).unwrap();
    assert!(matches!(line.request, Request::Add { place: None, .. }));
    let with = Request::Add {
        kind: "lfo".to_string(),
        name: None,
        place: Some(Place::end_of(3)),
    };
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 7,
            request: with
        })
        .unwrap(),
        json!({"id":7,"op":"add","kind":"lfo","place":{"row":3}})
    );

    // The rows event, and the rows in a snapshot.
    let event = Event::Rack {
        rows: vec![
            vec!["vco1".to_string(), "vco2".to_string()],
            vec!["lfo3".to_string()],
        ],
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"event":"rack","rows":[["vco1","vco2"],["lfo3"]]})
    );
    let text = serde_json::to_string(&event).unwrap();
    assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Event(event));
    let old_snapshot = json!({
        "revision":1,"tempo":90.0,"beat":0.0,"clock":"own","on_desk":false,"seats":[],
        "modules":[],"cables":[],"levels":{"peak_l":-120.0,"peak_r":-120.0},"listen":null,
        "faults":{"count":0,"recent":[]}
    });
    let snapshot: Snapshot = serde_json::from_value(old_snapshot.clone()).unwrap();
    assert_eq!(snapshot.rack, None, "a wall older than the rows");
    let mut new_snapshot = old_snapshot;
    new_snapshot["rack"] = json!([["vco1"]]);
    let snapshot: Snapshot = serde_json::from_value(new_snapshot).unwrap();
    assert_eq!(snapshot.rack, Some(vec![vec!["vco1".to_string()]]));
    let rows: ArrangeResult = serde_json::from_value(json!({"rows":[["a1"],["b1","c1"]]})).unwrap();
    assert_eq!(rows.rows.len(), 2);

    // A removal logged before the rows reads with no place; one logged
    // since carries it, and an older console reading it ignores it.
    let module = json!({"id":"lfo3","kind":"lfo","knobs":{}});
    let old: What =
        serde_json::from_value(json!({"op":"remove","module":&module,"cables":[]})).unwrap();
    assert!(matches!(old, What::Remove { place: None, .. }));
    let placed: What = serde_json::from_value(json!({
        "op":"remove","module":module,"cables":[],"place":{"row":1,"before":"vco2"}
    }))
    .unwrap();
    assert!(matches!(
        placed,
        What::Remove {
            place: Some(Place {
                row: 1,
                own: false,
                ..
            }),
            ..
        }
    ));
    let alone: Place = serde_json::from_value(json!({"row":2,"own":true})).unwrap();
    assert_eq!(
        alone,
        Place {
            row: 2,
            before: None,
            own: true
        }
    );
}
