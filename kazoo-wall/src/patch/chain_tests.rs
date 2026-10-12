use crate::daemon::wall::Wall;
use crate::engine::{Engine, EngineConfig, engine};
use crate::store::Store;
use std::time::Instant;

fn chain_wall(dir: &std::path::Path) -> (Wall, Engine) {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    (
        Wall::new(
            small(),
            control,
            None,
            Vec::new(),
            Some(Store::open(dir).unwrap()),
        ),
        engine,
    )
}

fn chain_request(wall: &mut Wall, value: serde_json::Value) -> serde_json::Value {
    let request: crate::protocol::Request = serde_json::from_value(value).unwrap();
    wall.request("Tom", &request, Instant::now())
        .unwrap()
        .result
}

fn save_chain(wall: &mut Wall) -> serde_json::Value {
    chain_request(
        wall,
        serde_json::json!({"op":"chain_save", "name":"voice", "modules":["vco1","vcf1"]}),
    )
}

#[test]
fn chain_save_keeps_only_inside_cables() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    let result = save_chain(&mut wall);
    assert_eq!(result["modules"], 2);
    assert_eq!(result["cables"], 1);
    assert_eq!(result["outside"], 1);
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("chains/voice.json")).unwrap())
            .unwrap();
    assert_eq!(value["cables"][0]["from"], serde_json::json!([0, "out"]));
    assert_eq!(value["cables"][0]["to"], serde_json::json!([1, "in"]));
    drop(engine);
}

#[test]
fn chain_add_is_one_undo_step_and_lands_in_a_new_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    save_chain(&mut wall);
    let before = chain_request(&mut wall, serde_json::json!({"op":"look"}));
    let added = chain_request(
        &mut wall,
        serde_json::json!({"op":"chain_add", "name":"voice"}),
    );
    let after = chain_request(&mut wall, serde_json::json!({"op":"look"}));
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(after["modules"].as_array().unwrap().len(), 5);
    assert_eq!(after["cables"].as_array().unwrap().len(), 3);
    assert_eq!(
        after["rack"].as_array().unwrap().last().unwrap(),
        &serde_json::json!(["vco2", "vcf2"])
    );
    chain_request(
        &mut wall,
        serde_json::json!({"op":"undo","change":added["change"]["seq"]}),
    );
    let undone = chain_request(&mut wall, serde_json::json!({"op":"look"}));
    assert_eq!(undone["modules"], before["modules"]);
    assert_eq!(undone["cables"], before["cables"]);
    assert_eq!(undone["rack"], before["rack"]);
    drop(engine);
}

#[test]
fn chain_round_trips_through_json() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    save_chain(&mut wall);
    let bytes = std::fs::read(dir.path().join("chains/voice.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["name"], "voice");
    assert_eq!(value["modules"][0]["kind"], "vco");
    let chain: Chain = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(chain.modules[1].knobs[1].to_bits(), 0.2_f32.to_bits());
    let encoded = serde_json::to_vec_pretty(&chain).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded).unwrap(),
        value
    );
    drop(wall);
    drop(engine);
    let (mut wall, engine) = chain_wall(dir.path());
    chain_request(
        &mut wall,
        serde_json::json!({"op":"chain_add", "name":"voice"}),
    );
    drop(engine);
}

#[test]
fn chain_add_log_failure_leaves_the_patch_whole() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    save_chain(&mut wall);
    let before = wall.patch().to_file();
    wall.store_mut()
        .unwrap()
        .fail_once(crate::store::FailAt::AppendLog);
    let request = crate::protocol::Request::ChainAdd {
        name: "voice".to_string(),
    };
    let refused = wall.request("Tom", &request, Instant::now()).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    let after = wall.patch().to_file();
    assert_eq!(after.modules, before.modules);
    assert_eq!(after.cables, before.cables);
    assert_eq!(after.rack, before.rack);
    assert_eq!(after.next_ids, before.next_ids);
    drop(engine);
}

#[test]
fn broken_chain_is_refused_before_any_module_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    save_chain(&mut wall);
    let file = dir.path().join("chains/voice.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["cables"][0]["to"][0] = serde_json::json!(99);
    std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = wall.patch().clone();
    let request = crate::protocol::Request::ChainAdd {
        name: "voice".to_string(),
    };
    assert_eq!(
        wall.request("Tom", &request, Instant::now())
            .unwrap_err()
            .code,
        ErrorCode::BrokenSave
    );
    assert_eq!(wall.patch(), &before);
    drop(engine);
}

#[test]
fn chain_knobs_labels_and_amounts_survive_add_and_redo() {
    let dir = tempfile::tempdir().unwrap();
    let mut patch = small();
    patch.module_mut("vco1").unwrap().name = Some("lead".to_string());
    patch.turn("vcf1", "cutoff", 2345.0, Some(0.0)).unwrap();
    patch.plug("vco1.out", "vcf1.in", Some(-0.5), None).unwrap();
    let original = patch.clone();
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(
        patch,
        control,
        None,
        Vec::new(),
        Some(Store::open(dir.path()).unwrap()),
    );
    save_chain(&mut wall);
    let added = chain_request(
        &mut wall,
        serde_json::json!({"op":"chain_add", "name":"VOICE"}),
    );
    assert_eq!(
        wall.patch().module("vco2").unwrap().name.as_deref(),
        Some("lead")
    );
    assert_eq!(
        wall.patch().module("vcf2").unwrap().knobs,
        original.module("vcf1").unwrap().knobs
    );
    assert_eq!(
        wall.patch().cables().last().unwrap().amount.to_bits(),
        (-0.5_f32).to_bits()
    );
    let after = wall.patch().to_file();
    let undone = chain_request(
        &mut wall,
        serde_json::json!({"op":"undo", "change":added["change"]["seq"]}),
    );
    chain_request(
        &mut wall,
        serde_json::json!({"op":"undo", "change":undone["change"]["seq"]}),
    );
    let redone = wall.patch().to_file();
    assert_eq!(redone.modules, after.modules);
    assert_eq!(redone.cables, after.cables);
    assert_eq!(redone.rack, after.rack);
    drop(engine);
}

#[test]
fn chain_save_refuses_an_mcp_overwrite_with_save_exists() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    save_chain(&mut wall);
    let request = crate::protocol::Request::ChainSave {
        name: "voice".to_string(),
        modules: vec!["vco1".to_string()],
    };
    let refused = wall
        .request_as("Seat", false, &request, Instant::now())
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::SaveExists);
    drop(engine);
}

#[test]
fn chain_add_reports_unknown_save_for_a_missing_chain() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, engine) = chain_wall(dir.path());
    let request = crate::protocol::Request::ChainAdd {
        name: "missing".to_string(),
    };
    let refused = wall.request("Tom", &request, Instant::now()).unwrap_err();
    assert_eq!(refused.code, ErrorCode::UnknownSave);
    drop(engine);
}
