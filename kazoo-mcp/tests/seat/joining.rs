//! KZ-N02: the bare `kazoo-mcp`, which names nobody, finds this Mac's wall
//! and goes on and off it under a name the model chooses.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use kazoo_wall::protocol::Event;
use kazoo_wall::protocol::client::Subscription;
use rmcp::ServiceExt as _;
use rmcp::transport::TokioChildProcess;
use tokio::sync::mpsc;

use super::{Client, Ears, call, dirs, seat, start, stop, works};

/// How long a watcher waits for news before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// What every tool says before a join.
const JOIN_FIRST: &str = "join a wall first: wall_walls lists them, and wall_join {as} joins one";

/// The bare command, finding this Mac's wall in `runtime` as anyone's
/// would: through `KAZOO_WALL_RUNTIME_DIR`, with no flag naming a seat or
/// a socket.
async fn bare(runtime: &Path) -> Client {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_kazoo-mcp"));
    command
        .arg("--notify-every")
        .arg("1")
        .env("KAZOO_WALL_RUNTIME_DIR", runtime)
        .env_remove("KAZOO_SEAT");
    let (transport, _) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .expect("kazoo-mcp spawns");
    let (heard, _notifications) = mpsc::unbounded_channel();
    Ears {
        heard,
        version: None,
    }
    .serve(transport)
    .await
    .expect("initialize succeeds")
}

/// A watcher on the wall at `socket` that fails, rather than hangs, when
/// news does not come.
fn watching(socket: &Path) -> (Subscription, Vec<String>) {
    let (watcher, subscribed) = Subscription::open(socket, "Watch", "test", false).unwrap();
    watcher.set_timeout(Some(PATIENCE)).unwrap();
    (watcher, subscribed.seats)
}

/// The next seat to come or go, passing over every other event.
fn next_seat(watcher: &mut Subscription) -> (String, bool) {
    loop {
        if let Event::Seat { seat, joined, .. } =
            watcher.next_event().expect("news of a seat came in time")
        {
            return (seat, joined);
        }
    }
}

/// The least each tool takes, so its arguments are read and it can say it
/// waits for a join.
fn least(tool: &str) -> serde_json::Value {
    match tool {
        "wall_add" => serde_json::json!({"kind": "lfo"}),
        "wall_batch" => serde_json::json!({"note": "a note", "changes": []}),
        "wall_chain" => serde_json::json!({"action": "list"}),
        "wall_go" => serde_json::json!({"moment": "dawn"}),
        "wall_group" => serde_json::json!({"group": "g", "modules": ["vcf1"]}),
        "wall_load" | "wall_mark" | "wall_save" => serde_json::json!({"name": "dawn"}),
        "wall_loop" => serde_json::json!({"module": "loop1", "erase": true}),
        "wall_notes" => serde_json::json!({"seq": 1}),
        "wall_patch" => serde_json::json!({"from": "lfo1.out", "to": "vcf1.cutoff"}),
        "wall_record" => serde_json::json!({"on": true}),
        "wall_remove" => serde_json::json!({"module": "lfo1"}),
        "wall_speak" => serde_json::json!({"module": "speak1", "text": "hello"}),
        "wall_tempo" => serde_json::json!({"bpm": 100.0}),
        "wall_turn" => serde_json::json!({"module": "vcf1", "knob": "cutoff", "value": 800.0}),
        "wall_undo" => serde_json::json!({"change": 1}),
        "wall_unpatch" => serde_json::json!({"cable": 1}),
        _ => serde_json::json!({}),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bare_seat_finds_joins_plays_and_leaves() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let (mut watcher, _) = watching(daemon.socket());
    let client = bare(runtime.path()).await;

    // Nobody is on it yet, and looking announces nobody.
    let walls = works(&client, "wall_walls", serde_json::json!({})).await;
    assert!(walls.starts_with("here (this Mac): playing"), "{walls}");
    assert!(!walls.contains("you are on it"), "{walls}");

    // Every tool but these three waits for a join.
    for name in super::TOOLS {
        if matches!(name, "wall_walls" | "wall_join" | "wall_leave") {
            continue;
        }
        let (failed, why) = call(&client, name, least(name)).await;
        assert!(failed, "{name} played before a join: {why}");
        assert_eq!(why, JOIN_FIRST, "{name}");
    }

    let (failed, why) = call(&client, "wall_join", serde_json::json!({"as": "no/slash"})).await;
    assert!(
        failed && why.contains("is not a name the wall takes"),
        "{why}"
    );
    let (failed, why) = call(&client, "wall_join", serde_json::json!({"as": "Desk"})).await;
    assert!(
        failed && why.contains("kept for the wall's own use"),
        "{why}"
    );
    let (failed, why) = call(&client, "wall_join", serde_json::json!({"as": "watch"})).await;
    assert!(failed, "a name someone on the wall has: {why}");
    assert_eq!(
        why,
        "someone is on here as Watch already: choose another name"
    );
    let (failed, why) = call(
        &client,
        "wall_join",
        serde_json::json!({"wall": "elsewhere", "as": "Nova"}),
    )
    .await;
    assert!(
        failed && why.contains("there is no wall 'elsewhere'"),
        "{why}"
    );

    let joined = works(&client, "wall_join", serde_json::json!({"as": "Nova"})).await;
    assert!(
        joined.starts_with("On here as Nova, with Watch, at change "),
        "{joined}"
    );
    // The first seat the wall announces is Nova: the looks before were
    // never seats.
    assert_eq!(next_seat(&mut watcher), ("Nova".to_string(), true));
    let look = works(&client, "wall_look", serde_json::json!({})).await;
    assert!(look.contains("Nova"), "{look}");
    let walls = works(&client, "wall_walls", serde_json::json!({})).await;
    assert!(walls.ends_with("; you are on it as Nova\n"), "{walls}");
    let (failed, why) = call(&client, "wall_join", serde_json::json!({"as": "Other"})).await;
    assert!(failed && why.contains("on here already, as Nova"), "{why}");
    let (failed, why) = call(
        &client,
        "wall_look",
        serde_json::json!({"wall": "elsewhere"}),
    )
    .await;
    assert!(failed && why.contains("it is on here"), "{why}");
    works(&client, "wall_look", serde_json::json!({"wall": "here"})).await;

    // Off again: the wall has seen the seat go by the time it answers, and
    // the tools wait once more.
    let left = works(&client, "wall_leave", serde_json::json!({})).await;
    assert_eq!(left, "Off here, where this seat was Nova.");
    assert_eq!(next_seat(&mut watcher), ("Nova".to_string(), false));
    let (failed, why) = call(&client, "wall_look", serde_json::json!({})).await;
    assert!(failed);
    assert_eq!(why, JOIN_FIRST);

    // And straight back on as the same name: the wall no longer lists it.
    let joined = works(&client, "wall_join", serde_json::json!({"as": "Nova"})).await;
    assert!(
        joined.starts_with("On here as Nova, with Watch"),
        "{joined}"
    );
    assert_eq!(next_seat(&mut watcher), ("Nova".to_string(), true));
    client.cancel().await.unwrap();
    stop(daemon);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wall_not_yet_playing_is_listed_and_joined_as_it_starts() {
    let (runtime, state) = dirs();
    let client = bare(runtime.path()).await;
    let walls = works(&client, "wall_walls", serde_json::json!({})).await;
    assert!(
        walls.starts_with("here (this Mac): not playing at "),
        "{walls}"
    );
    let joined = works(&client, "wall_join", serde_json::json!({"as": "Early"})).await;
    assert!(
        joined.starts_with("On here as Early, but it is not playing yet (at "),
        "{joined}"
    );
    assert!(
        joined.ends_with("): this seat is on it as it starts."),
        "{joined}"
    );

    let daemon = start(runtime.path(), state.path());
    // The event line tries every two seconds, so it may be on the wall
    // before the watcher is.
    let (mut watcher, seats) = watching(daemon.socket());
    if !seats.iter().any(|seat| seat == "Early") {
        assert_eq!(
            tokio::task::spawn_blocking(move || next_seat(&mut watcher))
                .await
                .unwrap(),
            ("Early".to_string(), true)
        );
    }
    works(&client, "wall_look", serde_json::json!({})).await;
    client.cancel().await.unwrap();
    stop(daemon);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_tool_takes_a_wall() {
    let (runtime, _state) = dirs();
    let client = bare(runtime.path()).await;
    for tool in client.list_all_tools().await.unwrap() {
        if tool.name == "wall_walls" {
            continue;
        }
        let schema = serde_json::Value::Object(tool.input_schema.as_ref().clone());
        let wall = &schema["properties"]["wall"];
        assert!(wall.is_object(), "{} takes wall: {schema}", tool.name);
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(
            !required.iter().any(|name| name == "wall"),
            "{}'s wall is optional",
            tool.name
        );
    }
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_seat_config_still_plays_under_a_name_already_on_the_wall() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let (_watcher, _) = watching(daemon.socket());
    // `--seat Watch`, as two sessions sharing one config would run it.
    let (client, _notifications) = seat(daemon.socket(), "Watch", 1, None).await;
    let look = works(&client, "wall_look", serde_json::json!({})).await;
    assert!(look.starts_with("The wall at revision "), "{look}");
    client.cancel().await.unwrap();
    stop(daemon);
}
