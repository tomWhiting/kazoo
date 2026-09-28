//! Migration tests: Tom's real patch from before version 2, small patches
//! with every mapping, and forgiving reading.

use std::time::Instant;

use super::*;
use crate::daemon::wall::Wall;
use crate::engine::{EngineConfig, engine};
use crate::patch::Patch;
use crate::protocol::What;

/// Tom's "Airports" wall as it was saved by version 1 (26 Sep 2026): 47
/// modules and 55 cables, with two native delays and two native reverbs.
const AIRPORTS: &str = include_str!("../../tests/fixtures/airports-v1.json");

fn airports() -> (PatchFile, Vec<String>) {
    let (mut file, mut notes) = read(AIRPORTS.as_bytes()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    notes.extend(migrate(&mut file).unwrap());
    (file, notes)
}

fn record<'a>(file: &'a PatchFile, id: &str) -> &'a ModuleRecord {
    file.modules.iter().find(|m| m.id == id).unwrap()
}

#[test]
fn tom_s_patch_loads_whole_with_every_id_kept() {
    let original: PatchFile = serde_json::from_str(AIRPORTS).unwrap();
    let (file, notes) = airports();
    assert_eq!(file.version, FILE_VERSION);
    let (patch, left_out) = Patch::load(&file).unwrap();
    assert!(left_out.is_empty(), "{left_out:?}");
    // Every module, by the same id and name.
    assert_eq!(patch.modules().len(), original.modules.len());
    for old in &original.modules {
        let module = patch.module(&old.id).unwrap();
        assert_eq!(module.name, old.name, "{}", old.id);
    }
    // Every cable, by the same number, plus the right-hand partners.
    for old in &original.cables {
        assert!(
            patch.cables().iter().any(|c| c.id == old.id),
            "cable {}",
            old.id
        );
    }
    let added: Vec<_> = patch
        .cable_records()
        .into_iter()
        .filter(|c| c.id >= original.next_cable)
        .collect();
    let routes: Vec<(String, String)> = added
        .iter()
        .map(|c| (c.from.clone(), c.to.clone()))
        .collect();
    for route in [
        ("delay1.right", "reverb1.right"),
        ("reverb1.right", "out1.right"),
        ("delay2.right", "reverb2.right"),
        ("reverb2.right", "out2.right"),
    ] {
        assert!(
            routes.contains(&(route.0.to_string(), route.1.to_string())),
            "{route:?} not in {routes:?}"
        );
    }
    assert_eq!(added.len(), 4);
    // The chain Tom built: bus → delay2 "airport echo" → reverb2 "terminal"
    // → out2.
    let cable = |id: u32| {
        patch
            .cable_records()
            .into_iter()
            .find(|c| c.id == id)
            .unwrap()
    };
    assert_eq!(
        (cable(65).from, cable(65).to),
        ("mix3.out".into(), "delay2.left".into())
    );
    assert_eq!(
        (cable(66).from, cable(66).to),
        ("delay2.left".into(), "reverb2.left".into())
    );
    assert_eq!(
        (cable(67).from, cable(67).to),
        ("reverb2.left".into(), "out2.left".into())
    );
    // Knob cables follow their knobs.
    assert_eq!(cable(20).to, "delay1.feedback");
    assert_eq!(cable(21).to, "reverb1.decay");
    assert_eq!(cable(41).to, "reverb1.mix");
    assert_eq!(patch.module("delay2").unwrap().kind.name(), "digital");
    assert_eq!(patch.module("reverb2").unwrap().kind.name(), "plate");
    assert_eq!(notes.len(), 8, "{notes:?}");
    // New modules never take a migrated id, and the old kinds' counters
    // carry on.
    let mut patch = patch;
    let What::Add { module } = patch.add("digital", None, None).unwrap() else {
        panic!("not an add");
    };
    assert_eq!(module.id, "digital1");
    assert_eq!(patch.to_file().next_ids["delay"], 3);
}

#[test]
fn the_old_knobs_become_their_nearest_equivalents() {
    let (file, _) = airports();
    let echo = record(&file, "delay2");
    assert_eq!(echo.kind, "digital");
    assert!((echo.knobs["ltime"] - 1.7).abs() < 1e-6);
    assert!((echo.knobs["rtime"] - 1.7).abs() < 1e-6);
    assert!((echo.knobs["feedback"] - 0.45).abs() < 1e-6);
    assert!((echo.knobs["mix"] - 0.3).abs() < 1e-6);
    // Tone 0.35: the old loop filter sat at 200 Hz × 2^(0.35 × 6.5).
    let cutoff = 200.0 * (0.35_f64 * 6.5).exp2();
    assert!((echo.knobs["highcut"] - cutoff).abs() < 1.0);
    assert!(echo.knobs["cross"].abs() < f64::EPSILON);
    // delay1 was synced to a dotted eighth (the old 3/16).
    let dotted = record(&file, "delay1");
    let digital = Kind::from_name("digital").unwrap().spec();
    let lsync = &digital.knobs[digital.knob_index("lsync").unwrap()];
    // Positions are small whole numbers: exact.
    let label = lsync.labels[dotted.knobs["lsync"] as usize];
    assert_eq!(label, "1/8d");
    let terminal = record(&file, "reverb2");
    assert_eq!(terminal.kind, "plate");
    assert!((terminal.knobs["mix"] - 0.6).abs() < 1e-6);
    let decay = 0.5 * 16.0_f64.powf(0.95);
    let highcut = 16_000.0 * (-1.5_f64).exp2();
    assert!((terminal.knobs["decay"] - decay).abs() < 1e-3);
    assert!((terminal.knobs["highcut"] - highcut).abs() < 1.0);
    assert!(terminal.knobs["predelay"].abs() < f64::EPSILON);
}

/// Render `patch` for a second with only `out` open, and return the master
/// peak.
fn peak_through(patch: &Patch, out: &str) -> f32 {
    let (mut engine, control) = engine(EngineConfig::new(48_000, patch.tempo, 0.0), None);
    let mut wall = Wall::new(patch.clone(), control, None, Vec::new(), None);
    for other in ["out1", "out2"] {
        let level = if other == out { 1.0 } else { 0.0 };
        let turn = crate::protocol::Request::Turn {
            module: other.to_string(),
            knob: "level".to_string(),
            value: level,
            glide_beats: Some(0.0),
        };
        wall.request("Test", &turn, Instant::now()).unwrap();
    }
    let mut buffer = vec![0.0_f32; 4_800 * 2];
    let mut peak = 0.0_f32;
    for _ in 0..20 {
        engine.render(&mut buffer, 2);
        peak = buffer.iter().fold(peak, |m, s| m.max(s.abs()));
    }
    peak
}

#[test]
fn tom_s_patch_plays_through_both_outs() {
    let (file, _) = airports();
    let (patch, _) = Patch::load(&file).unwrap();
    let out1 = peak_through(&patch, "out1");
    let out2 = peak_through(&patch, "out2");
    assert!(out1 > 0.001, "out1 is silent: {out1}");
    assert!(out2 > 0.001, "out2 is silent: {out2}");
}

#[test]
fn a_current_patch_needs_nothing() {
    let mut file = crate::seed::seed().unwrap().to_file();
    assert!(migrate(&mut file).unwrap().is_empty());
    file.version = FILE_VERSION + 1;
    assert!(
        migrate(&mut file)
            .unwrap_err()
            .contains("not one this wall knows")
    );
    file.version = 0;
    assert!(migrate(&mut file).is_err());
}

#[test]
fn old_knob_names_are_renamed() {
    let text = r#"{"version":1,"tempo":90.0,"last_seq":0,"next_ids":{},"next_cable":1,
        "modules":[{"id":"mix1","kind":"mix","knobs":{"a":0.2,"d":0.9}},
                   {"id":"vco1","kind":"vco","knobs":{"fm":0.7}}],
        "cables":[]}"#;
    let (mut file, _) = read(text.as_bytes()).unwrap();
    migrate(&mut file).unwrap();
    assert!((record(&file, "mix1").knobs["level_a"] - 0.2).abs() < 1e-9);
    assert!((record(&file, "mix1").knobs["level_d"] - 0.9).abs() < 1e-9);
    assert!((record(&file, "vco1").knobs["fm_depth"] - 0.7).abs() < 1e-9);
    assert!(Patch::from_file(&file).is_ok());
}

#[test]
fn reading_keeps_whatever_it_can() {
    assert!(read(b"not json").unwrap_err().contains("not JSON"));
    assert!(read(b"[1, 2]").unwrap_err().contains("not a JSON object"));
    assert!(
        read(br#"{"modules": []}"#)
            .unwrap_err()
            .contains("no version")
    );
    let text = r#"{"version":2,"tempo":90.0,"last_seq":4,"next_ids":{"vco":2,"lfo":"x"},
        "next_cable":3,
        "modules":[{"id":"vco1","kind":"vco","knobs":{}}, {"oops":true},
                   {"id":"ghost1","kind":"ghost","knobs":{}},
                   {"id":"vcf1","kind":"vcf","knobs":{"cutoff":500.0,"wobble":1.0}}],
        "cables":[{"id":1,"from":"vco1.out","to":"vcf1.in","amount":1.0},
                  {"id":2,"from":"ghost1.out","to":"vcf1.cutoff","amount":1.0},
                  {"id":"three"}],
        "dye":{"vco1":{"Tom":1.0},"ghost1":{"Tom":1.0}}}"#;
    let (file, mut notes) = read(text.as_bytes()).unwrap();
    assert_eq!(notes.len(), 3, "{notes:?}");
    let (patch, left_out) = Patch::load(&file).unwrap();
    notes.extend(left_out);
    // What loaded: vco1, vcf1 (with its cutoff) and cable 1.
    assert_eq!(patch.modules().len(), 2);
    assert!((patch.module("vcf1").unwrap().knobs[0] - 500.0).abs() < 1e-3);
    assert_eq!(patch.cables().len(), 1);
    assert_eq!(patch.dye.deposits().len(), 1);
    // And every loss said out loud.
    let said = notes.join(" | ");
    for word in [
        "module entry 2",
        "id counter 'lfo'",
        "cable entry 3",
        "ghost1",
        "wobble",
        "cable 2",
    ] {
        assert!(said.contains(word), "{word} not in {said}");
    }
}

#[test]
fn reading_keeps_the_rack_s_rows() {
    let text = r#"{"version":2,"tempo":90.0,"last_seq":4,"next_ids":{"vco":3,"lfo":2},
        "next_cable":1,
        "modules":[{"id":"vco1","kind":"vco","knobs":{}},{"id":"vco2","kind":"vco","knobs":{}},
                   {"id":"lfo1","kind":"lfo","knobs":{}}],
        "cables":[],
        "rack":[["vco2","lfo1"],["vco1"]]}"#;
    let (mut file, notes) = read(text.as_bytes()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        file.rack,
        vec![
            vec!["vco2".to_string(), "lfo1".to_string()],
            vec!["vco1".to_string()]
        ]
    );
    assert!(migrate(&mut file).unwrap().is_empty());
    let patch = Patch::from_file(&file).unwrap();
    assert_eq!(patch.rows(), file.rack.as_slice());
    // Read back as written.
    let again = serde_json::to_vec(&patch.to_file()).unwrap();
    assert_eq!(read(&again).unwrap().0.rack, file.rack);

    // A row that is not a list of ids is left out, and said; the module
    // it held is hung again as it loads.
    let text = text.replace(
        r#"[["vco2","lfo1"],["vco1"]]"#,
        r#"[["vco2","lfo1"],[1,2]]"#,
    );
    let (file, notes) = read(text.as_bytes()).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("rack row entry 2"), "{notes:?}");
    let (patch, _) = Patch::load(&file).unwrap();
    assert_eq!(
        patch.rows(),
        [vec![
            "vco2".to_string(),
            "lfo1".to_string(),
            "vco1".to_string()
        ]]
    );
    // A file from before the rows reads with none, and says nothing of it.
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    old.as_object_mut().unwrap().remove("rack");
    let (file, notes) = read(old.to_string().as_bytes()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert!(file.rack.is_empty());
}
