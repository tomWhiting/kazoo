use super::*;

fn timed_wall() -> (Wall, Engine) {
    let (mut wall, engine) = super::wall();
    wall.patch.tempo = 120.0;
    wall.engine.send(Command::Tempo(120.0));
    (wall, engine)
}

fn request(value: serde_json::Value) -> Request {
    serde_json::from_value(value).unwrap()
}

#[test]
fn scheduled_turn_records_timing_and_holds_before_moving() {
    let (mut wall, mut engine) = timed_wall();
    engine.render(&mut [0.0; 64], 2);
    let slot = wall.rack.slot_of("vcf1").unwrap();
    let knob = wall
        .patch
        .module("vcf1")
        .unwrap()
        .kind
        .spec()
        .knob_index("cutoff")
        .unwrap();
    let before = wall.engine.knob(slot, knob).unwrap();
    let result = ask(&mut wall, &request(serde_json::json!({"op":"turn","module":"vcf1","knob":"cutoff","value":300.0,"after_beats":1.0,"glide_beats":1.0,"curve":"ease_in"}))).unwrap();
    assert_eq!(result["change"]["what"]["after_beats"], 1.0);
    assert_eq!(result["change"]["what"]["curve"], "ease_in");
    for _ in 0..750 {
        engine.render(&mut [0.0; 64], 2);
    }
    assert_eq!(
        (wall.engine.knob(slot, knob).unwrap()).to_bits(),
        (before).to_bits()
    );
    for _ in 0..750 {
        engine.render(&mut [0.0; 64], 2);
    }
    assert_eq!(
        (wall.engine.knob(slot, knob).unwrap()).to_bits(),
        (300.0_f32).to_bits()
    );
}

#[test]
fn scheduling_refuses_invalid_fields_before_changes() {
    let (mut wall, _engine) = timed_wall();
    let before = wall.patch.to_file();
    for value in [
        serde_json::json!({"op":"add","kind":"lfo","after_beats":0}),
        serde_json::json!({"op":"remove","module":"vcf1","after_beats":1}),
        serde_json::json!({"op":"unpatch","to":"vcf1.in","after_beats":1}),
        serde_json::json!({"op":"patch","from":"lfo1.out","to":"vcf1.cutoff","curve":"linear"}),
        serde_json::json!({"op":"turn","module":"vcf1","knob":"cutoff","value":300,"after_beats":257}),
    ] {
        let refused = ask(&mut wall, &request(value)).unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadRequest);
        assert_eq!(wall.patch.to_file(), before);
    }
}

#[test]
fn batch_scheduling_uses_the_acceptance_tempo() {
    let (mut wall, mut engine) = timed_wall();
    engine.render(&mut [0.0; 64], 2);
    let result = ask(&mut wall, &request(serde_json::json!({"op":"batch","note":"staggered","changes":[
        {"op":"tempo","bpm":240},
        {"op":"turn","module":"vcf1","knob":"cutoff","value":300,"after_beats":4,"glide_beats":0}
    ]}))).unwrap();
    assert_eq!(result["change"]["what"]["changes"][1]["after_beats"], 4.0);
    let slot = wall.rack.slot_of("vcf1").unwrap();
    let knob = wall
        .patch
        .module("vcf1")
        .unwrap()
        .kind
        .spec()
        .knob_index("cutoff")
        .unwrap();
    let before = wall.engine.knob(slot, knob).unwrap();
    for _ in 0..3000 {
        engine.render(&mut [0.0; 64], 2);
    }
    assert_eq!(
        (wall.engine.knob(slot, knob).unwrap()).to_bits(),
        (before).to_bits()
    );
    for _ in 0..16 {
        engine.render(&mut [0.0; 64], 2);
    }
    assert_eq!(
        (wall.engine.knob(slot, knob).unwrap()).to_bits(),
        (300.0_f32).to_bits()
    );
}

#[test]
fn scheduled_summary_names_delay_duration_and_curve() {
    let what: What = serde_json::from_value(serde_json::json!({"op":"turn","module":"out3","knob":"level","from":0,"to":0.6,"glide_beats":16,"after_beats":8,"curve":"ease_in"})).unwrap();
    let summary = crate::change::describe(&what);
    assert!(
        summary.contains("after 8 beats over 16 beats, easing in"),
        "{summary}"
    );
}

#[test]
fn four_turns_start_at_zero_four_eight_and_twelve_beats() {
    let (mut wall, mut engine) = timed_wall();
    for _ in 0..3 {
        ask(
            &mut wall,
            &request(serde_json::json!({"op":"add","kind":"out"})),
        )
        .unwrap();
    }
    engine.render(&mut [0.0; 64], 2);
    let mut changes = Vec::new();
    let mut slots = Vec::new();
    for index in 0..4 {
        let module = format!("out{}", index + 1);
        slots.push(wall.rack.slot_of(&module).unwrap());
        changes.push(serde_json::json!({"op":"turn","module":module,"knob":"level","value":0.25,"glide_beats":0,"after_beats":index * 4}));
    }
    let before: Vec<_> = slots
        .iter()
        .map(|&slot| wall.engine.knob(slot, 0).unwrap())
        .collect();
    ask(
        &mut wall,
        &request(serde_json::json!({"op":"batch","note":"four entrances","changes":changes})),
    )
    .unwrap();
    for stage in 0..4 {
        for _ in 0..16 {
            engine.render(&mut [0.0; 64], 2);
        }
        for (index, &slot) in slots.iter().enumerate() {
            let expected = if index <= stage { 0.25 } else { before[index] };
            assert_eq!(
                wall.engine.knob(slot, 0).unwrap().to_bits(),
                expected.to_bits(),
                "stage {stage}, out {}",
                index + 1
            );
        }
        if stage < 3 {
            for _ in 0..2984 {
                engine.render(&mut [0.0; 64], 2);
            }
        }
    }
}

#[test]
fn cable_timing_and_invalid_ranges_use_the_acceptance_boundary() {
    let (mut wall, mut engine) = timed_wall();
    engine.render(&mut [0.0; 64], 2);
    let made = ask(&mut wall, &request(serde_json::json!({"op":"patch","from":"lfo1.out","to":"vcf1.cutoff","after_beats":1,"fade_beats":2}))).unwrap();
    let cable = u32::try_from(made["change"]["what"]["cable"]["id"].as_u64().unwrap()).unwrap();
    assert_eq!(wall.cable_motion[&cable], (24_000, 48_000));
    assert_eq!(made["change"]["what"]["fade_beats"], 2.0);
    let before = wall.patch.to_file();
    for value in [
        serde_json::json!({"op":"turn","module":"vcf1","knob":"cutoff","value":300,"after_beats":-1}),
        serde_json::json!({"op":"patch","from":"lfo1.out","to":"vcf1.cutoff","fade_beats":65}),
        serde_json::json!({"op":"tempo","bpm":90,"after_beats":257}),
    ] {
        assert_eq!(
            ask(&mut wall, &request(value)).unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(wall.patch.to_file(), before);
    }
}

#[test]
fn undo_of_a_waiting_turn_is_immediate_and_linear() {
    let (mut wall, mut engine) = timed_wall();
    engine.render(&mut [0.0; 64], 2);
    let made = ask(&mut wall, &request(serde_json::json!({"op":"turn","module":"out1","knob":"level","value":0.25,"after_beats":8,"glide_beats":0,"curve":"ease_in"}))).unwrap();
    engine.render(&mut [0.0; 64], 2);
    let seq = made["change"]["seq"].as_u64().unwrap();
    let undone = ask(&mut wall, &Request::Undo { change: seq }).unwrap();
    let what = &undone["change"]["what"];
    assert!(what.get("after_beats").is_none());
    assert!(what.get("curve").is_none());
    assert_eq!(what["to"], made["change"]["what"]["from"]);
    engine.render(&mut [0.0; 512], 2);
    let slot = wall.rack.slot_of("out1").unwrap();
    assert_eq!(
        wall.engine.knob(slot, 0).unwrap().to_bits(),
        (what["to"].as_f64().unwrap() as f32).to_bits()
    );
}
