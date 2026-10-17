//! Protocol tests: every message survives a round trip through JSON, and
//! the wire shapes match the design.

use serde_json::json;

use super::*;

#[test]
fn old_lines_read_without_a_clock() -> Result<(), serde_json::Error> {
    let line = r#"{"seq":6400,"at":"2026-10-13T21:04:11Z","seat":"Tom","what":{"op":"turn","module":"vcf1","knob":"cutoff","from":420.0,"to":800.0,"glide_beats":0.0},"summary":"Tom turned vcf1 cutoff 420 Hz → 800 Hz at once"}"#;
    let change: Change = serde_json::from_str(line)?;
    assert_eq!(change.clock, None);
    let written = serde_json::to_value(&change)?;
    assert!(written.get("clock").is_none());
    assert_eq!(written, serde_json::from_str::<serde_json::Value>(line)?);
    Ok(())
}

fn round_trip_request(request: Request) {
    let line = RequestLine {
        id: 7,
        wall: None,
        request,
        comment: None,
    };
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
        Request::turn("vcf1".to_string(), "cutoff".to_string(), 800.0, Some(4.0)),
        Request::turn("vcf1".to_string(), "cutoff".to_string(), 800.0, None),
        Request::patch("lfo1.out".to_string(), "vcf1.cutoff".to_string(), Some(0.4)),
        Request::unpatch(Some(12), None),
        Request::unpatch(None, Some("vcf1.cutoff".to_string())),
        Request::add(
            "lfo".to_string(),
            Some("slow wobble".to_string()),
            None,
            None,
        ),
        Request::add(
            "lfo".to_string(),
            None,
            Some(Place {
                row: 2,
                before: Some("vcf1".to_string()),
                own: false,
            }),
            None,
        ),
        Request::remove("lfo3".to_string()),
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
        Request::tempo(96.0),
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
        notes: Vec::new(),
        seq: 58,
        at: "2026-09-26T12:00:01Z".to_string(),
        seat: "Tom".to_string(),
        what: What::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            from: 420.0,
            to: 800.0,
            glide_beats: 4.0,
            after_beats: 0.0,
            curve: crate::protocol::GlideCurve::Linear,
        },
        summary: "Tom turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats".to_string(),
        undoes: None,
        clock: None,
        comment: None,
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
    let notice = Event::Notice {
        summary: "loop1 is recording, so its take is kept first".to_string(),
    };
    let value = serde_json::to_value(&notice).unwrap();
    assert_eq!(value["event"], "notice");
    let text = serde_json::to_string(&notice).unwrap();
    assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Event(notice));
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
            "unknown_save",
            "save_exists",
            "broken_save",
            "wall_replaced",
            "slow_down",
            "not_allowed",
            "internal",
            "unknown_moment",
            "mark_exists",
            "not_found"
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
        birth: None,
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
            after_beats: 0.0,
            fade_beats: 0.0,
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
            after_beats: 0.0,
            glide_beats: 0.0,
        },
    ] {
        let change = Change {
            notes: Vec::new(),
            seq: 1,
            at: "2026-09-26T00:00:00Z".to_string(),
            seat: "Tom".to_string(),
            what,
            summary: String::new(),
            undoes: Some(9),
            clock: None,
            comment: None,
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
    // Not a number travels as 0: JSON has no such number.
    assert!(widen(f32::NAN).abs() < f64::EPSILON);
    assert!(widen(f32::INFINITY).abs() < f64::EPSILON);
    let exact: f32 = 0.1 + 0.2;
    assert!((widen(exact) as f32 - exact).abs() < f32::EPSILON);
}

#[test]
fn change_ops_are_the_flood_guarded_ones() {
    assert!(Request::tempo(90.0).is_change());
    assert!(!Request::Look.is_change());
    assert!(!Request::Subscribe.is_change());
    // Whether the wall is heard is not a change to the wall.
    assert!(!Request::Monitor { on: true }.is_change());
    assert_eq!(Request::Monitor { on: false }.op(), "monitor");
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 4,
            wall: None,
            request: Request::Monitor { on: true },
            comment: None,
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
    assert!(!hello.role.watcher);
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
            wall: None,
            request: Request::Record { on: true },
            comment: None,
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
            wall: None,
            request: arrange,
            comment: None,
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
    let with = Request::add("lfo".to_string(), None, Some(Place::end_of(3)), None);
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 7,
            wall: None,
            request: with,
            comment: None,
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
}

#[test]
fn a_removal_reads_with_its_place_or_without() {
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

#[test]
fn save_names_are_plain_visible_files() {
    let longest = "a".repeat(MAX_SAVE_NAME);
    for good in [
        "jam",
        "Tom fiddle 2026-09-29",
        "jam-2026-09-27-before-fresh-start",
        "detroit_blade.runner",
        "7",
        longest.as_str(),
    ] {
        assert!(valid_save_name(good), "{good}");
    }
    let too_long = "a".repeat(MAX_SAVE_NAME + 1);
    for bad in [
        "",
        " jam",
        "jam ",
        "jam.",
        ".jam",
        "..",
        "-jam",
        "_jam",
        "a/b",
        "../patch",
        "jam\n",
        "jäm",
        "jam!",
        too_long.as_str(),
    ] {
        assert!(!valid_save_name(bad), "{bad:?}");
    }
    assert!(is_auto_save("before-load-20261009-143005"));
    assert!(is_auto_save("Before-new-20261009-143005-2"));
    assert!(!is_auto_save("before"));
    assert!(!is_auto_save("jam-before-fresh-start"));
}

#[test]
fn saving_and_loading_have_their_shapes() {
    for (request, wire) in [
        (
            Request::Save {
                name: "jam".to_string(),
                overwrite: false,
            },
            json!({"id":7,"op":"save","name":"jam"}),
        ),
        (
            Request::Save {
                name: "jam".to_string(),
                overwrite: true,
            },
            json!({"id":7,"op":"save","name":"jam","overwrite":true}),
        ),
        (Request::Saves, json!({"id":7,"op":"saves"})),
        (
            Request::Load {
                name: "jam".to_string(),
            },
            json!({"id":7,"op":"load","name":"jam"}),
        ),
        (Request::New, json!({"id":7,"op":"new"})),
        (
            Request::DeleteSave {
                name: "jam".to_string(),
            },
            json!({"id":7,"op":"delete_save","name":"jam"}),
        ),
    ] {
        assert_eq!(wire["op"], request.op());
        // Saving, loading and deleting change the wall or its saves (a
        // watcher may do none of them); listing does not.
        assert_eq!(request.is_change(), request != Request::Saves);
        round_trip_request(request.clone());
        let line = RequestLine {
            id: 7,
            wall: None,
            request,
            comment: None,
        };
        assert_eq!(serde_json::to_value(&line).unwrap(), wire);
    }
}

#[test]
fn the_saves_events_and_results_have_their_shapes() {
    for (event, wire) in [
        (
            Event::Loaded {
                seq: 12,
                seat: "Tom".to_string(),
                save: Some("jam".to_string()),
                backup: "before-load-20261009-143005".to_string(),
            },
            json!({"event":"loaded","seq":12,"seat":"Tom","save":"jam",
                   "backup":"before-load-20261009-143005"}),
        ),
        (
            Event::Loaded {
                seq: 13,
                seat: "Tom".to_string(),
                save: None,
                backup: "before-new-20261009-143005".to_string(),
            },
            json!({"event":"loaded","seq":13,"seat":"Tom",
                   "backup":"before-new-20261009-143005"}),
        ),
        (
            Event::Saved {
                seat: "Waffles".to_string(),
                name: "jam".to_string(),
                replaced: false,
            },
            json!({"event":"saved","seat":"Waffles","name":"jam","replaced":false}),
        ),
        (
            Event::SaveDeleted {
                seat: "Tom".to_string(),
                name: "jam".to_string(),
            },
            json!({"event":"save_deleted","seat":"Tom","name":"jam"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&event).unwrap(), wire);
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(ServerLine::parse(&text).unwrap(), ServerLine::Event(event));
    }

    // The results.
    let info = SaveInfo {
        name: "jam".to_string(),
        saved_at: "2026-10-09T03:30:05Z".to_string(),
        tempo: 96.0,
        modules: 12,
        cables: 30,
        auto: false,
        version: 2,
    };
    let saves = SavesResult {
        saves: vec![info.clone()],
        broken: vec![BrokenSave {
            name: "torn".to_string(),
            saved_at: "2026-10-09T03:30:05Z".to_string(),
            problem: "it is not JSON".to_string(),
        }],
        pending: vec!["late".to_string()],
    };
    let back: SavesResult = serde_json::from_value(serde_json::to_value(&saves).unwrap()).unwrap();
    assert_eq!(back, saves);
    let none: SavesResult = serde_json::from_value(json!({"saves":[]})).unwrap();
    assert!(none.broken.is_empty());
    let answer = SaveResult {
        save: info,
        replaced: true,
        notes: vec!["the save is in place, but its directory could not be flushed".to_string()],
    };
    let back: SaveResult = serde_json::from_value(serde_json::to_value(&answer).unwrap()).unwrap();
    assert_eq!(back, answer);
    let deleted = DeleteSaveResult {
        name: "jam".to_string(),
        notes: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(&deleted).unwrap(),
        json!({"name":"jam"})
    );
    let mark = SaveMark {
        name: "jam".to_string(),
        revision: 40,
        changed: true,
    };
    assert_eq!(
        serde_json::to_value(&mark).unwrap(),
        json!({"name":"jam","revision":40,"changed":true})
    );
    // From an older wall, a mark without it reads as unchanged.
    let old: SaveMark = serde_json::from_value(json!({"name":"jam","revision":40})).unwrap();
    assert!(!old.changed);
}

#[test]
fn a_load_names_the_save_its_wall_is_kept_in_and_nothing_more() {
    let mut wall = crate::patch::Patch::empty(96.0);
    wall.add("speak", None, None).unwrap();
    wall.speech.insert(
        "speak1".to_string(),
        crate::patch::Words {
            text: "private words".to_string(),
            voice: None,
        },
    );
    let load = Change {
        notes: Vec::new(),
        seq: 12,
        at: "2026-10-09T03:30:05Z".to_string(),
        seat: "Tom".to_string(),
        what: What::Load {
            save: Some("jam".to_string()),
            backup: "before-load-20261009-143005".to_string(),
            fingerprint: wall.fingerprint(),
            modules: 3,
            cables: 2,
            tempo: 120.0,
            by_console: true,
        },
        summary: "Tom loaded the save \"jam\"".to_string(),
        undoes: None,
        clock: None,
        comment: None,
    };
    // The log line is small whatever the wall, and the words in the wall
    // it replaced are never in it: they stay in the save, privately.
    let line = serde_json::to_string(&load).unwrap();
    assert!(line.len() < 400, "{line}");
    assert!(!line.contains("private words"));
    let back: Change = serde_json::from_str(&line).unwrap();
    assert_eq!(back, load);
    assert_eq!(
        serde_json::to_value(&load).unwrap()["what"],
        json!({"op":"load","save":"jam","backup":"before-load-20261009-143005",
               "fingerprint":wall.fingerprint(),"modules":3,"cables":2,"tempo":120.0,
               "by_console":true})
    );
    // A seat's load leaves the flag out, and an older line without it
    // reads as a seat's.
    let mut seat = load.what;
    if let What::Load { by_console, .. } = &mut seat {
        *by_console = false;
    }
    let value = serde_json::to_value(&seat).unwrap();
    assert!(value.get("by_console").is_none(), "{value}");
    assert_eq!(serde_json::from_value::<What>(value).unwrap(), seat);
}

#[test]
fn a_request_says_which_wall_it_is_for() {
    let line = RequestLine {
        id: 3,
        wall: Some(41),
        request: Request::turn("vcf1".to_string(), "cutoff".to_string(), 800.0, None),
        comment: None,
    };
    let wire = serde_json::to_value(&line).unwrap();
    assert_eq!(
        wire,
        json!({"id":3,"wall":41,"op":"turn","module":"vcf1","knob":"cutoff","value":800.0})
    );
    assert_eq!(serde_json::from_value::<RequestLine>(wire).unwrap(), line);
    // Left out, it is none: older clients are not checked.
    let old: RequestLine = serde_json::from_value(json!({"id":3,"op":"look"})).unwrap();
    assert_eq!(old.wall, None);
    // Only requests naming modules or cables are checked.
    let names = [
        Request::turn("vcf1".to_string(), "cutoff".to_string(), 1.0, None),
        Request::patch("a1.out".to_string(), "b1.in".to_string(), None),
        Request::unpatch(Some(1), None),
        Request::add("lfo".to_string(), None, None, None),
        Request::remove("lfo1".to_string()),
        Request::Arrange {
            module: "lfo1".to_string(),
            row: 0,
            before: None,
            own: false,
        },
        Request::Speak {
            module: "speak1".to_string(),
            text: "hi".to_string(),
            voice: None,
        },
    ];
    assert!(names.iter().all(Request::acts_on_modules));
    for other in [
        Request::Look,
        Request::Undo { change: 3 },
        Request::tempo(90.0),
        Request::Record { on: true },
        Request::Load {
            name: "jam".to_string(),
        },
        Request::New,
        Request::Saves,
    ] {
        assert!(!other.acts_on_modules(), "{other:?}");
    }
    // A snapshot from before walls were replaced is wall 0.
    let snapshot: Snapshot = serde_json::from_value(json!({
        "revision":1,"tempo":90.0,"beat":0.0,"clock":"own","on_desk":false,"seats":[],
        "modules":[],"cables":[],"levels":{"peak_l":-120.0,"peak_r":-120.0},"listen":null,
        "faults":{"count":0,"recent":[]}
    }))
    .unwrap();
    assert_eq!(snapshot.wall, 0);
}

/// A reading of a quiet input, as a probe carries it.
fn quiet_input(connected: bool) -> Measured {
    Measured {
        connected,
        waiting: false,
        last: 0.0,
        low: 0.0,
        high: 0.0,
        mean: 0.0,
        rms_db: -120.0,
        peak_db: -120.0,
        hz: None,
        rhythm: Rhythm::None,
        shape: None,
        gate: GateMeasured::default(),
        min: None,
        max: None,
        faults: 0,
        faults_since_reset: 0,
    }
}

#[test]
fn a_probe_is_a_read_and_older_walls_have_none() {
    // A read anyone may make, naming a module or not.
    let one = Request::Probe {
        module: Some("scope1".to_string()),
        modules: Vec::new(),
    };
    assert!(!one.is_change() && !one.acts_on_modules());
    assert_eq!(one.op(), "probe");
    assert_eq!(
        serde_json::to_value(RequestLine {
            id: 7,
            wall: None,
            request: one.clone(),
            comment: None,
        })
        .unwrap(),
        json!({"id":7,"op":"probe","module":"scope1"})
    );
    round_trip_request(one);
    let every = Request::Probe {
        module: None,
        modules: Vec::new(),
    };
    round_trip_request(every.clone());
    let line: RequestLine = serde_json::from_value(json!({"id":8,"op":"probe"})).unwrap();
    assert_eq!(line.request, every);
    let several: RequestLine =
        serde_json::from_value(json!({"id":9,"op":"probe","modules":["scope17","readout20"]}))
            .unwrap();
    assert_eq!(
        several.request,
        Request::Probe {
            module: None,
            modules: vec!["scope17".to_string(), "readout20".to_string()],
        }
    );
    round_trip_request(several.request);

    // A snapshot from a wall older than probes has none, and a snapshot
    // with none sends none.
    let older: Snapshot = serde_json::from_value(json!({
        "revision":1,"tempo":90.0,"beat":0.0,"clock":"own","on_desk":false,"seats":[],
        "modules":[],"cables":[],"levels":{"peak_l":-120.0,"peak_r":-120.0},"listen":null,
        "faults":{"count":0,"recent":[]}
    }))
    .unwrap();
    assert!(older.probes.is_empty());
    assert!(
        serde_json::to_value(&older)
            .unwrap()
            .get("probes")
            .is_none()
    );
}

#[test]
fn probes_leave_out_what_they_do_not_have_and_read_back_whole() {
    // A reading leaves out what it does not have, and reads back whole.
    let quiet = serde_json::to_value(quiet_input(true)).unwrap();
    for absent in ["hz", "shape", "min", "max", "waiting"] {
        assert!(quiet.get(absent).is_none(), "{absent} in {quiet}");
    }
    assert_eq!(quiet["rhythm"], json!("none"));
    // A rhythm a newer wall names reads as none: the reading still reads.
    let mut newer = quiet.clone();
    newer["rhythm"] = json!("swinging");
    let read: Measured = serde_json::from_value(newer).unwrap();
    assert_eq!(read.rhythm, Rhythm::None);
    for (rhythm, name) in [
        (Rhythm::Steady, "steady"),
        (Rhythm::Wavering, "wavering"),
        (Rhythm::Paused, "paused"),
    ] {
        assert_eq!(serde_json::to_value(rhythm).unwrap(), json!(name));
    }
    assert_eq!(quiet["gate"], json!({"high": false}));
    let result = ProbeResult {
        probes: vec![
            Probe {
                module: "scope1".to_string(),
                kind: "scope".to_string(),
                age: 0.04,
                scope: Some(ScopeReading {
                    mode: "trigger".to_string(),
                    span: 0.02,
                    zoom: 1.0,
                    trigger: 0.0,
                    triggered: true,
                    points: 96,
                    sweeping: None,
                    a: vec![[-0.5, 0.25]; 96],
                    b: Vec::new(),
                    xy: Vec::new(),
                    input_a: Measured {
                        hz: Some(220.0),
                        rhythm: Rhythm::Steady,
                        shape: Some("sine".to_string()),
                        min: Some(-0.8),
                        max: Some(0.8),
                        ..quiet_input(true)
                    },
                    input_b: quiet_input(false),
                    words: "a: 220 Hz sine from -0.800 to +0.800; b unplugged; triggered on a \
                            rising through 0, 20 ms across"
                        .to_string(),
                }),
                readout: None,
            },
            Probe {
                module: "readout1".to_string(),
                kind: "readout".to_string(),
                age: 0.08,
                scope: None,
                readout: Some(ReadoutReading {
                    mode: "pitch".to_string(),
                    auto: true,
                    input: Measured {
                        last: 0.75,
                        ..quiet_input(true)
                    },
                    note: Some("A4".to_string()),
                    cents: Some(0.0),
                    pitch_hz: Some(440.0),
                    lines: vec!["A4".to_string(), "440 Hz".to_string()],
                }),
            },
            Probe {
                module: "scope2".to_string(),
                kind: "scope".to_string(),
                age: 0.0,
                scope: None,
                readout: None,
            },
        ],
        left_out: vec!["readout9".to_string()],
        missing: vec!["scope30".to_string()],
        dropped: 3,
    };
    let value = serde_json::to_value(&result).unwrap();
    assert!(
        value["probes"][0]["scope"].get("b").is_none(),
        "an empty trace is left out"
    );
    assert!(value["probes"][2].get("scope").is_none());
    assert_eq!(value["probes"][1]["readout"]["note"], json!("A4"));
    assert_eq!(
        serde_json::from_value::<ProbeResult>(value).unwrap(),
        result
    );
    // An answer listing every probe without any left out says nothing of
    // them.
    let none: ProbeResult = serde_json::from_value(json!({"probes":[]})).unwrap();
    assert!(none.left_out.is_empty());
}

/// A selector's `select` knob, as the catalogue sends it.
fn select_knob(jack: JackLaw) -> KnobInfo {
    KnobInfo {
        name: "select".to_string(),
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: String::new(),
        curve: Curve::Linear,
        stepped: true,
        labels: vec!["in1".into(), "in2".into(), "in3".into(), "in4".into()],
        jack,
    }
}

#[test]
fn a_spanning_jack_round_trips_and_an_unknown_law_still_reads() {
    let knob = select_knob(JackLaw::Span);
    let text = serde_json::to_string(&knob).unwrap();
    assert!(text.contains("\"jack\":\"span\""), "{text}");
    let back: KnobInfo = serde_json::from_str(&text).unwrap();
    assert_eq!(back, knob);
    // A law from a newer wall, plain or with data, reads as unknown: the
    // knob (and so the catalogue) still reads.
    for law in [json!("tides"), json!({"tides": {"depth": 2}})] {
        let mut value = serde_json::to_value(&knob).unwrap();
        value["jack"] = law.clone();
        let read: KnobInfo = serde_json::from_value(value).unwrap();
        assert_eq!(read, select_knob(JackLaw::Unknown), "{law}");
    }
    // A law this build knows, written wrongly, fails the knob: it is never
    // passed off as unknown. So does anything that is not a name or one
    // name with a body.
    for law in [
        json!({"octaves": "x"}),
        json!({"octaves": null}),
        json!("octaves"),
        json!({"span": 1}),
        json!({}),
        json!({"tides": 1, "waves": 2}),
        json!(5),
        json!([]),
        json!(true),
    ] {
        let mut value = serde_json::to_value(&knob).unwrap();
        value["jack"] = law.clone();
        assert!(serde_json::from_value::<KnobInfo>(value).is_err(), "{law}");
    }
}
