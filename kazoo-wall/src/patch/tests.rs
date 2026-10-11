//! Patch model tests: operations, size, undo and saving.

use super::*;

fn code(result: Result<What, WallError>) -> ErrorCode {
    result.unwrap_err().code
}

fn added(patch: &mut Patch, kind: &str) -> String {
    match patch.add(kind, None, None).unwrap() {
        What::Add { module } => module.id,
        other => panic!("{other:?}"),
    }
}

fn plugged(what: &What) -> u32 {
    match what {
        What::Patch { cable, .. } => cable.id,
        other => panic!("{other:?}"),
    }
}

/// A small wall: lfo1 → vcf1.cutoff, vco1 → vcf1.in.
fn small() -> Patch {
    let mut patch = Patch::empty(120.0);
    added(&mut patch, "vco");
    added(&mut patch, "vcf");
    added(&mut patch, "lfo");
    patch.plug("vco1.out", "vcf1.in", None, None).unwrap();
    patch
        .plug("lfo1.out", "vcf1.cutoff", Some(0.4), None)
        .unwrap();
    patch
}

#[test]
fn ids_count_per_kind_and_are_never_reused() {
    let mut patch = Patch::empty(120.0);
    assert_eq!(added(&mut patch, "vco"), "vco1");
    assert_eq!(added(&mut patch, "vco"), "vco2");
    assert_eq!(added(&mut patch, "lfo"), "lfo1");
    patch.remove("vco2", &[]).unwrap();
    assert_eq!(added(&mut patch, "vco"), "vco3");
    assert_eq!(kind_of_id("vco12"), Some(Kind::VCO));
    assert_eq!(kind_of_id("vco"), None);
    assert_eq!(kind_of_id("vco01"), None);
    assert_eq!(kind_of_id("zzz1"), None);
}

#[test]
fn turns_clamp_and_report_what_changed() {
    let mut patch = small();
    let what = patch.turn("vcf1", "resonance", 5.0, None).unwrap();
    assert_eq!(
        what,
        What::Turn {
            module: "vcf1".to_string(),
            knob: "resonance".to_string(),
            from: 0.2,
            to: 0.95,
            glide_beats: DEFAULT_GLIDE_BEATS,
        }
    );
    let what = patch.turn("vcf1", "cutoff", 800.0, Some(1_000.0)).unwrap();
    assert!(matches!(what, What::Turn { glide_beats, .. } if (glide_beats - 64.0).abs() < 1e-9));
    let err = patch.turn("vcf1", "cutof", 1.0, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnknownKnob);
    assert_eq!(
        err.message,
        "vcf1 has no knob 'cutof'; knobs: cutoff, resonance, mode, drive"
    );
    assert_eq!(
        code(patch.turn("vcf9", "cutoff", 1.0, None)),
        ErrorCode::UnknownModule
    );
    assert_eq!(
        code(patch.turn("vcf1", "cutoff", f64::NAN, None)),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(patch.turn("vcf1", "cutoff", 1.0, Some(f64::INFINITY))),
        ErrorCode::BadRequest
    );
}

#[test]
fn one_cable_per_jack_and_patching_replaces() {
    let mut patch = small();
    let what = patch
        .plug("vco1.out", "vcf1.cutoff", Some(-3.0), None)
        .unwrap();
    match what {
        What::Patch { cable, replaced } => {
            assert_eq!(cable.id, 3);
            assert!((cable.amount + 1.0).abs() < 1e-9);
            assert_eq!(replaced.unwrap().from, "lfo1.out");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(patch.cables().len(), 2);
    // Fan-out is fine.
    patch.plug("vco1.out", "lfo1.rate", None, None).unwrap();
    assert_eq!(patch.cables().len(), 3);
}

#[test]
fn ports_are_checked() {
    let mut patch = small();
    let err = patch.plug("vcf1.in", "vco1.pitch", None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnknownPort);
    assert!(err.message.contains("is an input"), "{}", err.message);
    let err = patch.plug("vco1.out", "vcf1.out", None, None).unwrap_err();
    assert!(err.message.contains("is an output"), "{}", err.message);
    assert_eq!(
        code(patch.plug("vco1", "vcf1.in", None, None)),
        ErrorCode::UnknownPort
    );
    assert_eq!(
        code(patch.plug(".out", "vcf1.in", None, None)),
        ErrorCode::UnknownPort
    );
    assert_eq!(
        code(patch.plug("vco7.out", "vcf1.in", None, None)),
        ErrorCode::UnknownModule
    );
    assert_eq!(
        code(patch.plug("vco1.out", "vcf1.in", Some(f64::NAN), None)),
        ErrorCode::BadRequest
    );
    assert_eq!(code(patch.unplug(99, None)), ErrorCode::UnknownCable);
    assert_eq!(
        code(patch.unplug_jack("vco1.pitch")),
        ErrorCode::UnknownCable
    );
    assert!(patch.unplug_jack("vcf1.cutoff").is_ok());
}

#[test]
fn the_patch_has_no_fixed_number_of_modules_or_cables() {
    // Far past the 96 modules and 320 cables the wall once held: what the
    // machine can render is the daemon's to judge, not the patch's.
    const MIXERS: usize = 400;
    let mut patch = Patch::empty(120.0);
    for _ in 0..MIXERS {
        added(&mut patch, "mix");
    }
    assert_eq!(patch.modules().len(), MIXERS);
    for to in 1..=MIXERS {
        for jack in ["a", "b", "c", "d"] {
            patch
                .plug("mix1.out", &format!("mix{to}.{jack}"), None, None)
                .unwrap();
        }
    }
    assert_eq!(patch.cables().len(), MIXERS * 4);
    assert!(patch.is_plugged("mix400.d"));
    assert!(!patch.is_plugged("mix400.level_a"));
    assert!(!patch.is_plugged("nowhere.a"));
    // Replacing a cable leaves the count where it was.
    assert!(patch.plug("mix2.out", "mix1.a", None, None).is_ok());
    assert_eq!(patch.cables().len(), MIXERS * 4);
    // And it saves and loads whole.
    let (loaded, notes) = Patch::load(&patch.to_file()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(loaded.modules().len(), MIXERS);
    assert_eq!(loaded.cables().len(), MIXERS * 4);
}

#[test]
fn names_and_kinds_are_checked() {
    let mut patch = Patch::empty(120.0);
    assert_eq!(
        code(patch.add("theremin", None, None)),
        ErrorCode::UnknownKind
    );
    assert_eq!(code(patch.add("lfo", Some(""), None)), ErrorCode::BadName);
    assert_eq!(
        code(patch.add("lfo", Some("a name that is far too long"), None)),
        ErrorCode::BadName
    );
    assert_eq!(
        code(patch.add("lfo", Some("wobble\n"), None)),
        ErrorCode::BadName
    );
    assert!(patch.add("lfo", Some("slow wobble"), None).is_ok());
    assert!(!valid_name("Tom (midi)"));
    assert_eq!(printable("a\u{7}b"), "a\\u{7}b");
    assert_eq!(printable(&"x".repeat(50)).chars().count(), 41);
}

/// Undo `what`, check the patch is back where it was, then undo the undo and
/// check it is where it was after `what`.
fn undo_round_trip(patch: &mut Patch, what: &What, before: &Patch) {
    // A module brought back goes to the end of the list: compare by id.
    let sorted = |patch: &Patch| {
        let mut modules = patch.modules().to_vec();
        modules.sort_by(|a, b| a.id.cmp(&b.id));
        modules
    };
    let after = patch.clone();
    let inverse = patch.undo(what).unwrap();
    assert_eq!(sorted(patch), sorted(before), "{what:?}");
    assert_eq!(patch.cable_records(), before.cable_records(), "{what:?}");
    // Every module is back where it hung on the rack, too.
    assert_eq!(patch.rows(), before.rows(), "{what:?}");
    patch.undo(&inverse).unwrap();
    assert_eq!(sorted(patch), sorted(&after), "{inverse:?}");
    assert_eq!(patch.cable_records(), after.cable_records(), "{inverse:?}");
    assert_eq!(patch.rows(), after.rows(), "{inverse:?}");
}

#[test]
fn every_change_undoes_and_redoes() {
    let mut patch = small();
    let before = patch.clone();
    let what = patch.turn("vcf1", "cutoff", 300.0, Some(0.0)).unwrap();
    undo_round_trip(&mut patch, &what, &before);

    let before = patch.clone();
    let what = patch
        .plug("vco1.out", "vcf1.cutoff", Some(0.5), None)
        .unwrap();
    undo_round_trip(&mut patch, &what, &before);

    let before = patch.clone();
    let what = patch.unplug(1, None).unwrap();
    undo_round_trip(&mut patch, &what, &before);

    let before = patch.clone();
    let what = patch.add("env", Some("swell"), None).unwrap();
    undo_round_trip(&mut patch, &what, &before);

    let before = patch.clone();
    let what = patch.remove("vcf1", &[]).unwrap();
    // Cable 1 is out (the unplug above was redone): cable 3 goes with it.
    assert!(
        matches!(&what, What::Remove { cables, .. } if cables.len() == 1),
        "{what:?}"
    );
    undo_round_trip(&mut patch, &what, &before);
}

#[test]
fn undo_says_so_when_it_no_longer_applies() {
    let mut patch = small();
    let turned = patch.turn("lfo1", "rate", 2.0, None).unwrap();
    let unplugged = patch.unplug(2, None).unwrap();
    patch.remove("lfo1", &[]).unwrap();
    assert_eq!(code(patch.undo(&turned)), ErrorCode::UnknownModule);
    assert_eq!(code(patch.undo(&unplugged)), ErrorCode::UnknownModule);
    let added = patch.add("sh", None, None).unwrap();
    patch.undo(&added).unwrap();
    assert_eq!(code(patch.undo(&added)), ErrorCode::UnknownModule);
    assert_eq!(
        code(patch.undo(&What::Tempo {
            from: 1.0,
            to: 2.0,
            desk: false
        })),
        ErrorCode::Internal
    );
}

#[test]
fn restoring_skips_cables_whose_other_end_is_gone() {
    let mut patch = small();
    let removed = patch.remove("vcf1", &[]).unwrap();
    patch.remove("lfo1", &[]).unwrap();
    match patch.undo(&removed).unwrap() {
        What::Restore {
            cables, skipped, ..
        } => {
            assert_eq!(cables.len(), 1);
            assert_eq!(skipped.len(), 1);
        }
        other => panic!("{other:?}"),
    }
    // Restoring keeps the id and the numbering moves past it.
    assert!(patch.module("vcf1").is_some());
    assert_eq!(added(&mut patch, "vcf"), "vcf2");
}

#[test]
fn undoing_a_patch_that_replaced_a_cable_plugs_the_old_one_back() {
    let mut patch = small();
    let what = patch.plug("vco1.out", "vcf1.cutoff", None, None).unwrap();
    let new = plugged(&what);
    match patch.undo(&what).unwrap() {
        What::Unpatch { cable, replugged } => {
            assert_eq!(cable.id, new);
            assert_eq!(replugged.unwrap().id, 2);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(patch.cables().len(), 2);
    assert!(patch.cables().iter().any(|c| c.id == 2));
}

#[test]
fn files_round_trip() {
    let mut patch = small();
    patch.add("lfo", Some("slow wobble"), None).unwrap();
    patch.turn("vcf1", "cutoff", 612.0, None).unwrap();
    patch.last_seq = 57;
    let file = patch.to_file();
    let text = serde_json::to_string_pretty(&file).unwrap();
    let back: PatchFile = serde_json::from_str(&text).unwrap();
    let loaded = Patch::from_file(&back).unwrap();
    assert_eq!(loaded, patch);
}

#[test]
fn decisions_and_routers_round_trip_through_a_file() {
    let mut patch = Patch::empty(120.0);
    for kind in [
        "compare", "logic", "branch", "switch", "select", "divide", "math",
    ] {
        added(&mut patch, kind);
    }
    patch.turn("compare1", "hysteresis", 0.1, None).unwrap();
    patch.turn("branch1", "mode", 2.0, None).unwrap();
    patch.turn("switch1", "fade", 0.02, None).unwrap();
    patch.turn("select1", "select", 3.0, None).unwrap();
    patch.turn("divide1", "division", 7.0, None).unwrap();
    patch.turn("math1", "scale_b", -1.0, None).unwrap();
    patch.plug("math1.sum", "compare1.in", None, None).unwrap();
    patch.plug("compare1.trig", "logic1.b", None, None).unwrap();
    patch.plug("logic1.xor", "branch1.in", None, None).unwrap();
    patch.plug("branch1.b", "switch1.step", None, None).unwrap();
    patch
        .plug("divide1.count", "select1.select", Some(-0.5), None)
        .unwrap();
    patch
        .plug("switch1.out3", "select1.in2", None, None)
        .unwrap();
    patch
        .plug("select1.out", "math1.b", Some(0.7), None)
        .unwrap();
    let text = serde_json::to_string_pretty(&patch.to_file()).unwrap();
    let back: PatchFile = serde_json::from_str(&text).unwrap();
    assert_eq!(Patch::from_file(&back).unwrap(), patch);
    let (loaded, notes) = Patch::load(&back).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(loaded.fingerprint(), patch.fingerprint());
}

#[test]
fn broken_files_are_refused_with_a_reason() {
    let good = small().to_file();

    let mut file = good.clone();
    file.version = 9;
    assert!(Patch::from_file(&file).unwrap_err().contains("version"));

    let mut file = good.clone();
    file.modules.push(file.modules[0].clone());
    assert!(Patch::from_file(&file).unwrap_err().contains("twice"));

    let mut file = good.clone();
    file.modules[0].id = "Bad Id".to_string();
    assert!(
        Patch::from_file(&file)
            .unwrap_err()
            .contains("not a module id")
    );
    // An id need not name its module's kind (a migrated module keeps its
    // old id).
    let mut file = good.clone();
    file.modules[0].id = "delay9".to_string();
    file.cables
        .retain(|c| !c.from.starts_with("vco1") && !c.to.starts_with("vco1"));
    assert!(Patch::from_file(&file).is_ok());

    let mut file = good.clone();
    file.modules[0].name = Some("bad\tname".to_string());
    assert!(Patch::from_file(&file).is_err());

    let mut file = good.clone();
    file.cables[0].to = "vcf1.cutoff".to_string();
    assert!(
        Patch::from_file(&file)
            .unwrap_err()
            .contains("another cable is plugged into")
    );

    let mut file = good.clone();
    file.cables[0].from = "nowhere1.out".to_string();
    assert!(Patch::from_file(&file).is_err());

    // Counters for kinds the wall no longer has are kept, so their ids
    // are never given out again.
    let mut file = good.clone();
    file.next_ids.insert("theremin".to_string(), 4);
    assert_eq!(
        Patch::from_file(&file).unwrap().to_file().next_ids["theremin"],
        4
    );

    let mut file = good.clone();
    file.modules[0].knobs.insert("volume".to_string(), 1.0);
    assert!(Patch::from_file(&file).is_err());

    // Out-of-range knobs are held to range, and numbering moves past ids.
    let mut file = good;
    file.modules[0].knobs.insert("level".to_string(), 99.0);
    file.next_ids.clear();
    let mut patch = Patch::from_file(&file).unwrap();
    assert!((patch.module("vco1").unwrap().knobs[4] - 1.0).abs() < f32::EPSILON);
    assert_eq!(added(&mut patch, "vco"), "vco2");
}

#[test]
fn ids_read_back_whatever_the_kind_s_name() {
    assert_eq!(make_id("vco", 12), "vco12");
    assert_eq!(make_id("sampler12", 3), "sampler12_3");
    assert_eq!(split_id("vco12"), Some(("vco", 12)));
    assert_eq!(split_id("sampler12_3"), Some(("sampler12", 3)));
    assert_eq!(split_id("big_room4"), Some(("big_room", 4)));
    assert_eq!(split_id("vco"), None);
    assert_eq!(split_id("vco0"), None);
    assert_eq!(split_id("vco012"), None);
    assert!(valid_id("sampler12_3") && valid_id("delay2"));
    assert!(!valid_id("2vco") && !valid_id("Vco1") && !valid_id("") && !valid_id("a b"));
}

fn words(text: &str) -> Words {
    Words {
        text: text.to_string(),
        voice: None,
    }
}

#[test]
fn a_speaker_brought_back_says_its_words_again() {
    let mut patch = Patch::empty(120.0);
    let speaker = added(&mut patch, "speak");
    patch.speech.insert(speaker.clone(), words("hello there"));
    let What::Remove { module, cables, .. } = patch.remove(&speaker, &[]).unwrap() else {
        panic!("remove made no Remove");
    };
    assert!(!patch.speech.contains_key(&speaker));
    // Saved and loaded while it is away, the words are still kept.
    let mut patch = Patch::from_file(&patch.to_file()).unwrap();
    patch.restore(&module, &cables, None).unwrap();
    assert_eq!(patch.speech.get(&speaker), Some(&words("hello there")));
    // Kept once, not twice: removing and restoring again still works.
    let What::Remove { module, cables, .. } = patch.remove(&speaker, &[]).unwrap() else {
        panic!("remove made no Remove");
    };
    patch.restore(&module, &cables, None).unwrap();
    assert_eq!(patch.speech.get(&speaker), Some(&words("hello there")));
    assert!(patch.to_file().retired_speech.is_empty());
}

#[test]
fn only_the_latest_removed_speakers_words_are_kept() {
    let mut patch = Patch::empty(120.0);
    for index in 0..=MAX_RETIRED_WORDS {
        let speaker = added(&mut patch, "speak");
        patch
            .speech
            .insert(speaker.clone(), words(&format!("take {index}")));
        patch.remove(&speaker, &[]).unwrap();
    }
    let kept = patch.to_file().retired_speech;
    assert_eq!(kept.len(), MAX_RETIRED_WORDS);
    // The first removed is the one let go.
    assert!(kept.iter().all(|retired| retired.module != "speak1"));
    assert_eq!(
        kept.last().map(|retired| retired.words.text.as_str()),
        Some(&*format!("take {MAX_RETIRED_WORDS}"))
    );
}

#[test]
fn retired_words_for_a_module_on_the_wall_are_dropped_on_load() {
    let mut patch = Patch::empty(120.0);
    let speaker = added(&mut patch, "speak");
    let mut file = patch.to_file();
    file.retired_speech.push(RetiredWords {
        module: speaker,
        words: words("stale"),
    });
    let (_, notes) = Patch::load(&file).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
}

fn rows(patch: &Patch) -> Vec<Vec<&str>> {
    patch
        .rows()
        .iter()
        .map(|row| row.iter().map(String::as_str).collect())
        .collect()
}

fn place(row: usize, before: Option<&str>) -> Place {
    Place {
        row,
        before: before.map(str::to_string),
        own: false,
    }
}

fn own(row: usize) -> Place {
    Place {
        row,
        before: None,
        own: true,
    }
}

#[test]
fn a_new_module_hangs_beside_the_newest_of_its_group_or_on_a_new_row() {
    let mut patch = small();
    // One after another, each group its own row.
    assert_eq!(rows(&patch), vec![vec!["vco1"], vec!["vcf1"], vec!["lfo1"]]);
    added(&mut patch, "vco");
    added(&mut patch, "env");
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "vco2"], vec!["vcf1"], vec!["lfo1", "env1"]]
    );
    // Beside the newest of its group, wherever that has been moved.
    assert!(patch.arrange("env1", &place(0, None)).unwrap());
    added(&mut patch, "lfo");
    assert_eq!(
        rows(&patch),
        vec![
            vec!["vco1", "vco2", "env1", "lfo2"],
            vec!["vcf1"],
            vec!["lfo1"]
        ]
    );
    // A group not on the rack yet starts a new bottom row.
    added(&mut patch, "out");
    assert_eq!(rows(&patch).last(), Some(&vec!["out1"]));
    // Asked for a place, it goes there.
    patch
        .add("noise", None, Some(&place(1, Some("vcf1"))))
        .unwrap();
    assert_eq!(rows(&patch)[1], vec!["noise1", "vcf1"]);
    patch.add("vca", None, Some(&own(0))).unwrap();
    assert_eq!(rows(&patch)[0], vec!["vca1"]);
    patch.add("sh", None, Some(&place(99, None))).unwrap();
    assert_eq!(rows(&patch).last(), Some(&vec!["sh1"]));
    // Every module is in exactly one row, and no row is empty.
    let mut ids: Vec<&str> = rows(&patch).concat();
    ids.sort_unstable();
    let mut all: Vec<&str> = patch.modules().iter().map(|m| m.id.as_str()).collect();
    all.sort_unstable();
    assert_eq!(ids, all);
    assert!(patch.rows().iter().all(|row| !row.is_empty()));
}

#[test]
fn modules_move_along_rows_between_them_and_onto_rows_of_their_own() {
    let mut patch = small();
    added(&mut patch, "vco");
    // [vco1 vco2] [vcf1] [lfo1]
    assert!(patch.arrange("lfo1", &place(0, Some("vco2"))).unwrap());
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "lfo1", "vco2"], vec!["vcf1"]]
    );
    // The rows are counted as they stood: row 2 was past the end then.
    assert!(patch.arrange("vco1", &place(1, None)).unwrap());
    assert_eq!(
        rows(&patch),
        vec![vec!["lfo1", "vco2"], vec!["vcf1", "vco1"]]
    );
    assert!(patch.arrange("vcf1", &place(5, None)).unwrap());
    assert_eq!(
        rows(&patch),
        vec![vec!["lfo1", "vco2"], vec!["vco1"], vec!["vcf1"]]
    );
    // A row of its own, above the others.
    assert!(patch.arrange("vco2", &own(0)).unwrap());
    assert_eq!(
        rows(&patch),
        vec![vec!["vco2"], vec!["lfo1"], vec!["vco1"], vec!["vcf1"]]
    );
    // A module alone in its row, moved below that row: the rows as they
    // stood count, so it lands in what was the row below.
    assert!(patch.arrange("vco2", &place(2, None)).unwrap());
    assert_eq!(
        rows(&patch),
        vec![vec!["lfo1"], vec!["vco1", "vco2"], vec!["vcf1"]]
    );
    // Where it already is, or in front of itself: nothing moves.
    assert!(!patch.arrange("vco2", &place(1, None)).unwrap());
    assert!(!patch.arrange("vco1", &place(1, Some("vco2"))).unwrap());
    assert!(!patch.arrange("vco1", &place(0, Some("vco1"))).unwrap());
    assert!(!patch.arrange("lfo1", &own(0)).unwrap());
    // In front of a module another seat has taken away: the row's end.
    assert!(patch.arrange("vcf1", &place(0, Some("gone9"))).unwrap());
    assert_eq!(
        rows(&patch),
        vec![vec!["lfo1", "vcf1"], vec!["vco1", "vco2"]]
    );
    // Only modules on the wall move; nothing is logged (no What).
    let before = patch.clone();
    assert_eq!(
        patch.arrange("vco9", &place(0, None)).unwrap_err().code,
        ErrorCode::UnknownModule
    );
    assert_eq!(patch, before);
}

#[test]
fn a_removed_module_comes_back_where_it_hung() {
    let mut patch = small();
    added(&mut patch, "vco");
    added(&mut patch, "lfo");
    patch.arrange("vcf1", &place(0, Some("vco2"))).unwrap();
    // [vco1 vcf1 vco2] [lfo1 lfo2]
    let what = patch.remove("vcf1", &[]).unwrap();
    assert!(matches!(
        &what,
        What::Remove { place: Some(Place { row: 0, before: Some(next), own: false }), .. }
            if next == "vco2"
    ));
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "vco2"], vec!["lfo1", "lfo2"]]
    );
    patch.undo(&what).unwrap();
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "vcf1", "vco2"], vec!["lfo1", "lfo2"]]
    );

    // Alone in a middle row: it comes back on a row of its own there.
    patch.arrange("vcf1", &own(1)).unwrap();
    let before = patch.clone();
    let what = patch.remove("vcf1", &[]).unwrap();
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "vco2"], vec!["lfo1", "lfo2"]]
    );
    let back = patch.undo(&what).unwrap();
    assert_eq!(patch.rows(), before.rows());
    assert!(matches!(
        back,
        What::Restore {
            place: Some(Place {
                row: 1,
                own: true,
                ..
            }),
            ..
        }
    ));
    // Undoing the restore takes it away again, as it was.
    patch.undo(&back).unwrap();
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "vco2"], vec!["lfo1", "lfo2"]]
    );

    // A removal logged before the rows had places: it comes back where a
    // new module of its kind would go.
    let What::Remove { module, cables, .. } = patch.remove("vco2", &[]).unwrap() else {
        panic!("a removal");
    };
    let old = What::Remove {
        module,
        cables,
        replugged: Vec::new(),
        place: None,
    };
    patch.undo(&old).unwrap();
    assert_eq!(
        rows(&patch),
        vec![vec!["vco1", "vco2"], vec!["lfo1", "lfo2"]]
    );
}

#[test]
fn the_rows_are_saved_and_mended_as_they_load() {
    let mut patch = small();
    added(&mut patch, "vco");
    patch.arrange("lfo1", &place(0, Some("vco1"))).unwrap();
    let file = patch.to_file();
    assert_eq!(file.rack, patch.rows());
    let back: PatchFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
    assert_eq!(Patch::from_file(&back).unwrap().rows(), patch.rows());

    // A patch from before the rows: a row for each group, in the groups'
    // order, whatever order the modules were added in.
    let mut old = file.clone();
    old.rack.clear();
    let text = serde_json::to_string(&old).unwrap();
    assert!(!text.contains("\"rack\""), "no rows, no field: {text}");
    let loaded = Patch::from_file(&old).unwrap();
    assert_eq!(
        rows(&loaded),
        vec![vec!["vco1", "vco2"], vec!["lfo1"], vec!["vcf1"]]
    );

    // Rows naming modules not on the wall, naming one twice, or leaving
    // one out are mended, without a note: this is where modules hang, not
    // the patch's sound.
    let mut odd = file;
    odd.rack = vec![
        vec!["ghost1".to_string()],
        vec!["vcf1".to_string(), "vco1".to_string()],
        vec![],
        vec!["vco1".to_string(), "lfo1".to_string(), "lfo1".to_string()],
    ];
    let (loaded, notes) = Patch::load(&odd).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        rows(&loaded),
        vec![vec!["vcf1", "vco1", "vco2"], vec!["lfo1"]],
        "vco2 was in no row: it goes beside vco1"
    );
}

#[test]
fn any_arrangement_one_move_away_has_a_place_that_makes_it() {
    let text = |rows: &[&[&str]]| -> Vec<Vec<String>> {
        rows.iter()
            .map(|row| row.iter().map(|id| (*id).to_string()).collect())
            .collect()
    };
    let starts = [
        text(&[&["a1", "b1", "c1"], &["d1"], &["e1", "f1"]]),
        text(&[&["a1"], &["b1"], &["c1"]]),
        text(&[&["a1", "b1"]]),
    ];
    for from in &starts {
        let ids: Vec<String> = from.concat();
        for id in &ids {
            let mut places = Vec::new();
            for row in 0..=from.len() + 1 {
                places.push(Place::end_of(row));
                places.push(own(row));
                for before in &ids {
                    places.push(place(row, Some(before)));
                }
            }
            for asked in places {
                let mut to = from.clone();
                arrange_rows(&mut to, id, &asked);
                assert!(to.iter().all(|row| !row.is_empty()));
                assert_eq!(to.concat().len(), ids.len(), "{to:?}");
                let found = place_for(from, &to, id).expect("it is in a row");
                let mut made = from.clone();
                arrange_rows(&mut made, id, &found);
                assert_eq!(made, to, "{id} to {asked:?} from {from:?}: {found:?}");
            }
        }
    }
    assert_eq!(place_for(&starts[0], &starts[1], "z9"), None);
}

#[test]
fn a_file_brings_in_nothing_past_todays_limits() {
    let mut patch = Patch::empty(120.0);
    patch.add("speak", None, None).unwrap();
    patch.add("vcf", None, None).unwrap();
    let mut file = patch.to_file();
    // Words longer than a speaker may be given, here and retired.
    file.speech.insert(
        "speak1".to_string(),
        Words {
            text: "a".repeat(kazoo_speech::tts::MAX_TEXT_CHARS + 1),
            voice: None,
        },
    );
    file.retired_speech.push(RetiredWords {
        module: "speak9".to_string(),
        words: Words {
            text: "fine".to_string(),
            voice: Some("v".repeat(kazoo_speech::tts::MAX_VOICE_CHARS + 1)),
        },
    });
    // Numbering for a kind this build has not (kept: it may come back),
    // and for a name no kind could have (dropped).
    file.next_ids.insert("gone".to_string(), 4);
    file.next_ids.insert("Not A Kind!".repeat(10), 4);
    // Dye from more seats than a module keeps, and from a seat whose name
    // is longer than a seat's may be.
    let mut seats = std::collections::BTreeMap::new();
    for n in 0..crate::fingerprints::MAX_DYE_SEATS + 2 {
        seats.insert(
            format!("seat{n}"),
            1.0 + f64::from(u8::try_from(n).unwrap()),
        );
    }
    seats.insert("x".repeat(crate::protocol::MAX_NAME + 1), 5.0);
    file.dye.insert("vcf1".to_string(), seats);
    let (loaded, notes) = Patch::load(&file).unwrap();
    assert!(loaded.speech.is_empty());
    assert!(loaded.retired_speech.is_empty());
    assert_eq!(loaded.next_ids.get("gone"), Some(&4));
    assert_eq!(loaded.next_ids.len(), file.next_ids.len() - 1);
    let dyed = &loaded.dye.deposits()["vcf1"];
    assert_eq!(dyed.len(), crate::fingerprints::MAX_DYE_SEATS);
    assert!(!dyed.contains_key("seat0") && !dyed.contains_key("seat1"));
    // Each said, the words without quoting them.
    for said in [
        "words for 'speak1' longer than a speaker may be given, dropped",
        "kept words for 'speak9' longer than a speaker may be given, dropped",
        "which no kind could have, dropped",
        "from a seat whose name is longer than a seat's may be, dropped",
        "the faintest dye of 2 seats on vcf1 dropped",
    ] {
        assert!(
            notes.iter().any(|note| note.contains(said)),
            "{said}: {notes:?}"
        );
    }
    assert!(!notes.iter().any(|note| note.contains("aaaa")), "{notes:?}");
}

#[test]
fn however_much_junk_a_file_holds_its_notes_fit_one_answer() {
    let mut file = Patch::empty(120.0).to_file();
    // Names that print as long as a name can: 40 characters, each written
    // out as an escape.
    let junk = |n: usize| format!("{n}{}", "\u{10ffff}".repeat(60));
    for n in 0..1_500 {
        file.next_ids.insert(junk(n), 1);
        let mut seats = std::collections::BTreeMap::new();
        seats.insert(junk(n), 1.0);
        file.dye.insert(junk(n), seats);
        file.speech.insert(junk(n), words("lost"));
    }
    let bytes = serde_json::to_vec(&file).unwrap();
    assert!(
        bytes.len() as u64 <= crate::store::MAX_WALL_FILE,
        "{}",
        bytes.len()
    );
    let (_, notes) = Patch::load(&file).unwrap();
    assert_eq!(notes.len(), MAX_LOAD_NOTES);
    assert_eq!(
        notes.last().unwrap(),
        &format!("and {} more like these", 4_500 - 31)
    );
    let said: usize = notes.iter().map(String::len).sum();
    assert!(said < crate::protocol::MAX_SERVER_LINE / 64, "{said}");
}
