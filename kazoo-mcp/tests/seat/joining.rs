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

use super::{Client, Ears, call, dirs, start, stop, wait_for_arrival, works};

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

/// Wait until `watcher` hears `seat` go.
fn wait_for_leaving(watcher: &mut Subscription, seat: &str) {
    loop {
        if let Event::Seat {
            seat: who,
            joined: false,
            ..
        } = watcher.next_event().unwrap()
            && who == seat
        {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bare_seat_finds_joins_plays_and_leaves() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut watcher = Subscription::open(daemon.socket(), "Watch", "test", false)
        .unwrap()
        .0;
    let client = bare(runtime.path()).await;

    // Nobody is on it yet, and looking announces nobody.
    let walls = works(&client, "wall_walls", serde_json::json!({})).await;
    assert!(walls.starts_with("here (this Mac): playing"), "{walls}");
    assert!(
        !walls.contains("kazoo-mcp"),
        "the look is never a seat: {walls}"
    );
    assert!(!walls.contains("you are on it"), "{walls}");

    // Every tool but these three waits for a join.
    let (failed, why) = call(&client, "wall_look", serde_json::json!({})).await;
    assert!(failed);
    assert_eq!(
        why,
        "join a wall first: wall_walls lists them, and wall_join {as} joins one"
    );

    let (failed, why) = call(&client, "wall_join", serde_json::json!({"as": "no/slash"})).await;
    assert!(
        failed && why.contains("is not a name the wall takes"),
        "{why}"
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
    assert_eq!(joined, "On here as Nova. Call wall_look to see it.");
    wait_for_arrival(&mut watcher, "Nova");
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

    // Off again: the wall sees the seat go, and the tools wait once more.
    let left = works(&client, "wall_leave", serde_json::json!({})).await;
    assert_eq!(left, "Off here, where this seat was Nova.");
    wait_for_leaving(&mut watcher, "Nova");
    let (failed, _) = call(&client, "wall_look", serde_json::json!({})).await;
    assert!(failed);

    // And back on as another name.
    works(&client, "wall_join", serde_json::json!({"as": "Nova Two"})).await;
    wait_for_arrival(&mut watcher, "Nova Two");
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
    works(&client, "wall_join", serde_json::json!({"as": "Early"})).await;

    let daemon = start(runtime.path(), state.path());
    let mut watcher = Subscription::open(daemon.socket(), "Watch", "test", false)
        .unwrap()
        .0;
    // The event line tries every two seconds: the seat arrives on its own.
    let arrived = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            wait_for_arrival(&mut watcher, "Early");
            watcher
        }),
    )
    .await
    .expect("the seat arrived as the wall started")
    .unwrap();
    drop(arrived);
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
