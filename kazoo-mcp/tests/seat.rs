//! kazoo-mcp as a real child process, driven by rmcp's own client over the
//! child-process transport, against a real headless wall daemon.
//!
//! Each test runs its own daemon with its socket and state in fresh
//! temporary directories (the runtime directory directly under /tmp,
//! because macOS caps socket paths at 104 bytes), and a short notification
//! window so the channel can be watched quickly.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use kazoo_wall::daemon::{Daemon, DaemonConfig};
use kazoo_wall::protocol::Event;
use kazoo_wall::protocol::SOCKET_NAME;
use kazoo_wall::protocol::client::{Subscription, WallClient};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, ContentBlock, CustomNotification,
    ProtocolVersion,
};
use rmcp::service::{MaybeSendFuture, NotificationContext, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, RoleClient, ServiceExt as _};
use tokio::sync::mpsc;

/// What every tool answers while the daemon is down.
const NOT_RUNNING: &str = "the wall is not running (start it with `kazoo-wall`)";

/// Every tool.
const TOOLS: [&str; 13] = [
    "wall_add",
    "wall_catalogue",
    "wall_listen",
    "wall_log",
    "wall_look",
    "wall_patch",
    "wall_record",
    "wall_remove",
    "wall_speak",
    "wall_tempo",
    "wall_turn",
    "wall_undo",
    "wall_unpatch",
];

/// A client that keeps every custom notification the server sends, and
/// asks for a chosen protocol revision.
struct Ears {
    heard: mpsc::UnboundedSender<CustomNotification>,
    version: Option<ProtocolVersion>,
}

impl ClientHandler for Ears {
    fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        assert!(
            self.heard.send(notification).is_ok(),
            "the test stopped listening while the server still spoke"
        );
        std::future::ready(())
    }

    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        if let Some(version) = &self.version {
            info.protocol_version = version.clone();
        }
        info
    }
}

type Client = RunningService<RoleClient, Ears>;

/// A socket directory under /tmp and a state directory.
fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    let runtime = tempfile::Builder::new()
        .prefix("km")
        .tempdir_in("/tmp")
        .unwrap();
    let state = tempfile::tempdir().unwrap();
    (runtime, state)
}

fn start(runtime: &Path, state: &Path) -> Daemon {
    Daemon::start(DaemonConfig::headless(runtime, state)).unwrap()
}

fn stop(daemon: Daemon) {
    daemon.stop();
    daemon.wait().unwrap();
}

/// A kazoo-mcp seat on `socket`, telling at most every `every` seconds.
async fn seat(
    socket: &Path,
    name: &str,
    every: u64,
    version: Option<ProtocolVersion>,
) -> (Client, mpsc::UnboundedReceiver<CustomNotification>) {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_kazoo-mcp"));
    command
        .arg("--seat")
        .arg(name)
        .arg("--notify-every")
        .arg(every.to_string())
        .arg("--socket")
        .arg(socket);
    command.env_remove("KAZOO_SEAT");
    let (transport, _) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .expect("kazoo-mcp spawns");
    let (heard, notifications) = mpsc::unbounded_channel();
    let client = Ears { heard, version }
        .serve(transport)
        .await
        .expect("initialize succeeds");
    (client, notifications)
}

/// Every text block of a result, joined.
fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Call `tool` with `arguments`: whether it failed, and its text.
async fn call(client: &Client, tool: &'static str, arguments: serde_json::Value) -> (bool, String) {
    let arguments = arguments
        .as_object()
        .expect("arguments are an object")
        .clone();
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .unwrap_or_else(|err| panic!("{tool} returns a result: {err}"));
    (result.is_error == Some(true), text_of(&result))
}

/// Call `tool` and insist it worked.
async fn works(client: &Client, tool: &'static str, arguments: serde_json::Value) -> String {
    let (failed, text) = call(client, tool, arguments).await;
    assert!(!failed, "{tool} failed: {text}");
    text
}

/// The number after `marker` in `text`, e.g. the 12 in `Cable #12.`.
fn number_after(text: &str, marker: &str) -> u64 {
    let digits = text.split(marker).nth(1).map_or_else(
        || panic!("no '{marker}' in: {text}"),
        |rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        },
    );
    digits
        .parse()
        .unwrap_or_else(|err| panic!("no number after '{marker}' in {text}: {err}"))
}

/// The next notification within `within`.
async fn next_notification(
    notifications: &mut mpsc::UnboundedReceiver<CustomNotification>,
    within: Duration,
) -> Option<CustomNotification> {
    tokio::time::timeout(within, notifications.recv())
        .await
        .unwrap_or_default()
}

/// A notification's content and meta, checked for the channel's shape.
fn channel_shape(
    notification: &CustomNotification,
) -> (String, serde_json::Map<String, serde_json::Value>) {
    assert_eq!(notification.method, "notifications/claude/channel");
    let params = notification.params.as_ref().expect("params");
    let content = params["content"].as_str().expect("content is a string");
    let meta = params["meta"]
        .as_object()
        .expect("meta is an object")
        .clone();
    assert!(
        meta.values().all(serde_json::Value::is_string),
        "every meta value is a string: {meta:?}"
    );
    assert_eq!(meta["source"], "kazoo-wall");
    assert!(content.starts_with("News from the wall:\n"), "{content}");
    assert!(
        content
            .ends_with("call wall_look to see the wall as it is now before acting on any of it."),
        "{content}"
    );
    (content.to_string(), meta)
}

/// Wait until `seat`'s event line is subscribed: `watcher` (a subscription
/// opened before the seat started) hears it arrive, and the subscribe that
/// follows its hello is given a moment.
fn wait_for_arrival(watcher: &mut Subscription, seat: &str) {
    loop {
        match watcher.next_event().unwrap() {
            Event::Seat {
                seat: who,
                joined: true,
                ..
            } if who == seat => break,
            _ => {}
        }
    }
    std::thread::sleep(Duration::from_millis(500));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_tool_plays_a_real_wall() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let (client, _notifications) = seat(daemon.socket(), "Waffles", 1, None).await;
    the_handshake_declares_the_channel(&client).await;
    looking_and_the_catalogue(&client).await;
    turning_and_refusals(&client).await;
    adding_patching_removing_and_undoing(&client).await;
    the_log_listening_and_tempo(&client).await;
    speaking(&client).await;
    recording(&client, &state.path().join("recordings")).await;
    client.cancel().await.unwrap();
    stop(daemon);
}

/// The server's handshake and its tool list.
async fn the_handshake_declares_the_channel(client: &Client) {
    let info = client.peer_info().expect("the server describes itself");
    assert_eq!(
        info.server_info.as_ref().map(|server| server.name.as_str()),
        Some("kazoo-mcp")
    );
    assert!(info.capabilities.tools.is_some());
    assert!(
        info.capabilities
            .experimental
            .as_ref()
            .is_some_and(|experimental| experimental.contains_key("claude/channel")),
        "the Claude channel is declared: {:?}",
        info.capabilities
    );
    assert!(
        info.instructions
            .as_deref()
            .is_some_and(|words| words.contains("module.port")),
        "the instructions teach how to play"
    );
    let mut names = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, TOOLS);
}

/// `wall_look` and `wall_catalogue`.
async fn looking_and_the_catalogue(client: &Client) {
    let look = works(client, "wall_look", serde_json::json!({})).await;
    assert!(look.starts_with("The wall at revision "), "{look}");
    assert!(look.contains("Seats here: Waffles."), "{look}");
    assert!(
        look.contains("vcf1 (vcf) \"filter\"\n  knobs: cutoff=900"),
        "{look}"
    );
    assert!(
        look.contains("lfo1.out → vcf1.cutoff amount 0.35"),
        "{look}"
    );

    let catalogue = works(client, "wall_catalogue", serde_json::json!({})).await;
    assert!(catalogue.contains("\nvcf ("), "{catalogue}");
    let vcf = works(client, "wall_catalogue", serde_json::json!({"kind": "vcf"})).await;
    assert!(vcf.contains("knob cutoff: 20..18000 Hz"), "{vcf}");
    assert!(!vcf.contains("\nlfo ("), "{vcf}");
    let (failed, why) = call(client, "wall_catalogue", serde_json::json!({"kind": "vcx"})).await;
    assert!(failed && why.contains("kinds: "), "{why}");
}

/// `wall_turn`, and the wall's refusal carried as a tool error.
async fn turning_and_refusals(client: &Client) {
    let turned = works(
        client,
        "wall_turn",
        serde_json::json!({"module": "vcf1", "knob": "cutoff", "value": 800.0, "glide_beats": 4.0}),
    )
    .await;
    assert!(
        turned.contains("Waffles turned vcf1 cutoff 900 Hz → 800 Hz over 4 beats"),
        "{turned}"
    );
    let (failed, why) = call(
        client,
        "wall_turn",
        serde_json::json!({"module": "vcf1", "knob": "cutof", "value": 800.0}),
    )
    .await;
    assert!(failed, "{why}");
    assert!(
        why.starts_with("the wall said no (unknown_knob): "),
        "{why}"
    );
    assert!(
        why.contains("cutoff"),
        "the wall's message lists the knobs: {why}"
    );
}

/// `wall_speak`: words rendered with `say` (or a plain refusal on a machine
/// without it), and a refusal for a module that cannot speak.
async fn speaking(client: &Client) {
    let added = works(client, "wall_add", serde_json::json!({"kind": "speak"})).await;
    assert!(added.contains("Module id: speak1."), "{added}");
    let (failed, why) = call(
        client,
        "wall_speak",
        serde_json::json!({"module": "vcf1", "text": "hello"}),
    )
    .await;
    assert!(failed && why.contains("bad_request"), "{why}");
    let (failed, said) = call(
        client,
        "wall_speak",
        serde_json::json!({"module": "speak1", "text": "hello from here"}),
    )
    .await;
    if std::path::Path::new("/usr/bin/say").exists() {
        assert!(!failed, "{said}");
        assert!(
            said.contains("Waffles gave speak1 3 words to say"),
            "{said}"
        );
        assert!(!said.contains("hello"), "the words stay private: {said}");
    } else {
        assert!(failed && said.contains("internal"), "{said}");
    }
}

/// `wall_add`, `wall_patch`, `wall_unpatch`, `wall_remove` and `wall_undo`.
async fn adding_patching_removing_and_undoing(client: &Client) {
    let added = works(
        client,
        "wall_add",
        serde_json::json!({"kind": "lfo", "name": "slow wobble"}),
    )
    .await;
    assert!(added.contains("Module id: lfo3."), "{added}");
    let patched = works(
        client,
        "wall_patch",
        serde_json::json!({"from": "lfo3.out", "to": "vcf1.drive", "amount": 0.2}),
    )
    .await;
    let cable = number_after(&patched, "Cable #");
    let unpatched = works(client, "wall_unpatch", serde_json::json!({"cable": cable})).await;
    assert!(unpatched.contains("Change #"), "{unpatched}");
    for both_or_neither in [
        serde_json::json!({}),
        serde_json::json!({"cable": cable, "to": "vcf1.drive"}),
    ] {
        let (failed, why) = call(client, "wall_unpatch", both_or_neither).await;
        assert!(failed && why.contains("exactly one"), "{why}");
    }
    works(
        client,
        "wall_patch",
        serde_json::json!({"from": "lfo3.out", "to": "vcf1.drive"}),
    )
    .await;
    works(
        client,
        "wall_unpatch",
        serde_json::json!({"to": "vcf1.drive"}),
    )
    .await;

    let removed = works(client, "wall_remove", serde_json::json!({"module": "lfo3"})).await;
    let removal = number_after(&removed, "Change #");
    let undone = works(client, "wall_undo", serde_json::json!({"change": removal})).await;
    assert!(undone.contains("lfo3"), "{undone}");
    let (failed, why) = call(client, "wall_undo", serde_json::json!({"change": 99_999})).await;
    assert!(failed && why.contains("unknown_change"), "{why}");
}

/// The recording's file name in `wall_record`'s answer.
fn file_name(answer: &str) -> String {
    answer
        .split("/wall-")
        .nth(1)
        .and_then(|rest| rest.split(".wav").next())
        .map_or_else(
            || panic!("no file in: {answer}"),
            |stem| format!("wall-{stem}.wav"),
        )
}

/// `wall_record`, and the recording in `wall_look`.
async fn recording(client: &Client, recordings: &Path) {
    let look = works(client, "wall_look", serde_json::json!({})).await;
    assert!(
        look.contains("Recording: no (wall_record starts one)."),
        "{look}"
    );
    let started = works(client, "wall_record", serde_json::json!({"on": true})).await;
    assert!(
        started.contains("Waffles started recording wall-"),
        "{started}"
    );
    assert!(
        started.contains(&recordings.display().to_string()),
        "{started}"
    );
    let again = works(client, "wall_record", serde_json::json!({"on": true})).await;
    assert!(again.starts_with("Already recording to "), "{again}");
    let look = works(client, "wall_look", serde_json::json!({})).await;
    assert!(look.contains("Recording: yes, "), "{look}");
    assert!(look.contains("started by Waffles"), "{look}");
    // A tenth of a second in the file: a debug build on a busy machine
    // renders when it can, not in real time.
    let deadline = Instant::now() + Duration::from_secs(30);
    let growing = recordings.join(file_name(&started));
    while std::fs::metadata(&growing).map_or(0, |meta| meta.len()) <= 44 + 4_800 * 8 {
        assert!(Instant::now() < deadline, "the recording never grew");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let stopped = works(client, "wall_record", serde_json::json!({"on": false})).await;
    assert!(
        stopped.contains("Waffles stopped recording wall-"),
        "{stopped}"
    );
    let files: Vec<std::fs::DirEntry> = std::fs::read_dir(recordings)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().to_string_lossy().into_owned();
    assert!(
        name.starts_with("wall-") && files[0].path().extension().is_some_and(|ext| ext == "wav"),
        "{name}"
    );
    assert!(stopped.contains(&name), "{stopped}");
    // A header and at least a tenth of a second of 48 kHz stereo f32.
    let bytes = files[0].metadata().unwrap().len();
    assert!(bytes > 44 + 4_800 * 8, "{bytes} bytes");
    let look = works(client, "wall_look", serde_json::json!({})).await;
    assert!(look.contains("Recording: no"), "{look}");
    let idle = works(client, "wall_record", serde_json::json!({"on": false})).await;
    assert_eq!(idle, "Nothing was recording; nothing changed.");
}

/// `wall_log`, `wall_listen` and `wall_tempo`.
async fn the_log_listening_and_tempo(client: &Client) {
    let log = works(client, "wall_log", serde_json::json!({"limit": 3})).await;
    assert!(log.starts_with("Changes, oldest first:\n"), "{log}");
    assert_eq!(
        log.lines().filter(|line| line.starts_with('#')).count(),
        3,
        "{log}"
    );
    assert!(
        log.contains("Older changes exist: call wall_log with before="),
        "{log}"
    );

    let heard = works(client, "wall_listen", serde_json::json!({})).await;
    assert!(
        heard.starts_with("Heard at ") || heard.contains("not been heard yet"),
        "{heard}"
    );
    let tempo = works(client, "wall_tempo", serde_json::json!({"bpm": 96.0})).await;
    assert!(
        tempo.contains("96 BPM, set on the wall's own clock"),
        "{tempo}"
    );

    // The engine takes the new tempo on its next block; the snapshot
    // follows it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let look = loop {
        let look = works(client, "wall_look", serde_json::json!({})).await;
        if look.contains(": 96 BPM,") || Instant::now() > deadline {
            break look;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(look.contains(": 96 BPM,"), "{look}");
    assert!(look.contains("lfo3 (lfo) \"slow wobble\""), "{look}");
    // Waffles's hands are on what Waffles touched, and nobody else's.
    assert!(
        look.contains("vcf1 (vcf) \"filter\" (hands: Waffles 100%)"),
        "{look}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_seats_are_told_coalesced_and_a_seats_own_changes_are_not() {
    const EVERY: u64 = 2;
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (mut watcher, _) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    watcher.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let (client, mut notifications) = seat(&socket, "Waffles", EVERY, None).await;
    wait_for_arrival(&mut watcher, "Waffles");
    // Tom stays a seat that reads its events, so the daemon keeps it (a
    // subscriber that stops reading is closed, and its leaving is news).
    let closer = watcher.closer().unwrap();
    watcher.set_timeout(None).unwrap();
    let drain = std::thread::spawn(move || while watcher.next_event().is_ok() {});

    // Waffles's own change is never news to Waffles.
    works(
        &client,
        "wall_turn",
        serde_json::json!({"module": "vcf1", "knob": "resonance", "value": 0.4}),
    )
    .await;

    // Vesper arrives and makes twelve changes at once.
    let started = Instant::now();
    let mut vesper = WallClient::connect(&socket, "Vesper", "test", false).unwrap();
    let mut last = 0;
    for step in 0..12 {
        let turned = vesper
            .turn(
                "vcf1",
                "cutoff",
                f64::from(step).mul_add(50.0, 400.0),
                Some(0.0),
            )
            .unwrap();
        last = turned.change.seq;
    }

    let window = Duration::from_secs(EVERY);
    let notification = next_notification(&mut notifications, window * 3)
        .await
        .expect("the others' changes are told");
    assert!(
        started.elapsed() >= window.saturating_sub(Duration::from_millis(200)),
        "held for the window, not sent at once ({:?})",
        started.elapsed()
    );
    let (content, meta) = channel_shape(&notification);
    println!(
        "a real notification:\n{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "method": notification.method,
            "params": notification.params,
        }))
        .unwrap()
    );
    assert!(
        content.starts_with(
            "News from the wall:\n- Vesper came to the wall\n- Vesper turned vcf1 cutoff"
        ),
        "{content}"
    );
    assert_eq!(content.matches("\n- Vesper turned").count(), 9, "{content}");
    assert!(
        content.contains("\n- and 3 more (see wall_log)\n"),
        "{content}"
    );
    assert!(
        !content.contains("Waffles"),
        "own changes are not news: {content}"
    );
    assert_eq!(meta["seq"], last.to_string());
    let seats = meta["seats"]
        .as_str()
        .unwrap()
        .split(',')
        .collect::<Vec<_>>();
    for here in ["Tom", "Waffles", "Vesper"] {
        assert!(seats.contains(&here), "{seats:?}");
    }
    // One notification for all of it.
    let extra = next_notification(&mut notifications, window + Duration::from_secs(1)).await;
    assert!(
        extra.is_none(),
        "the burst was coalesced into one, but then: {:?}",
        extra.map(|told| told.params)
    );

    // Only Waffles's own work: nothing to tell.
    works(
        &client,
        "wall_turn",
        serde_json::json!({"module": "vcf1", "knob": "resonance", "value": 0.5}),
    )
    .await;
    assert!(
        next_notification(&mut notifications, window * 2 + Duration::from_secs(1))
            .await
            .is_none(),
        "a seat's own changes never notify it"
    );

    a_seat_leaving_is_told(vesper, &mut notifications, window).await;

    client.cancel().await.unwrap();
    closer.close().unwrap();
    drain.join().unwrap();
    stop(daemon);
}

/// `vesper` leaves: the seat is told, and the seats in the meta follow.
async fn a_seat_leaving_is_told(
    vesper: WallClient,
    notifications: &mut mpsc::UnboundedReceiver<CustomNotification>,
    window: Duration,
) {
    drop(vesper);
    let notification = next_notification(notifications, window * 3)
        .await
        .expect("a seat leaving is told");
    let (content, meta) = channel_shape(&notification);
    assert!(content.contains("\n- Vesper left the wall\n"), "{content}");
    assert!(
        !meta["seats"].as_str().unwrap().contains("Vesper"),
        "{meta:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wall_that_is_down_is_said_plainly_and_rejoined_when_it_is_back() {
    let (runtime, state) = dirs();
    let socket: PathBuf = runtime.path().join(SOCKET_NAME);
    let (client, mut notifications) = seat(&socket, "Waffles", 1, None).await;

    for tool in ["wall_look", "wall_listen", "wall_log"] {
        let (failed, why) = call(&client, tool, serde_json::json!({})).await;
        assert!(failed, "{tool}: {why}");
        assert_eq!(why, NOT_RUNNING, "{tool}");
    }
    let (failed, why) = call(
        &client,
        "wall_turn",
        serde_json::json!({"module": "vcf1", "knob": "cutoff", "value": 800.0}),
    )
    .await;
    assert!(failed);
    assert_eq!(why, NOT_RUNNING);

    // The wall starts: the tools work at once, and the event line rejoins
    // within its two-second retry.
    let daemon = start(runtime.path(), state.path());
    works(&client, "wall_look", serde_json::json!({})).await;
    let mut vesper = WallClient::connect(&socket, "Vesper", "test", false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut drive = 0.0;
    let told = loop {
        assert!(Instant::now() < deadline, "the event line never rejoined");
        drive += 0.05;
        vesper.turn("vcf1", "drive", drive, Some(0.0)).unwrap();
        if let Some(told) = next_notification(&mut notifications, Duration::from_secs(1)).await {
            break told;
        }
    };
    let (content, _) = channel_shape(&told);
    assert!(content.contains("Vesper"), "{content}");
    drop(vesper);

    // The wall stops: the tools say so, and the channel says the line
    // dropped.
    stop(daemon);
    let (failed, why) = call(&client, "wall_look", serde_json::json!({})).await;
    assert!(failed);
    assert_eq!(why, NOT_RUNNING);
    let mut dropped = false;
    while let Some(told) = next_notification(&mut notifications, Duration::from_secs(3)).await {
        let (content, _) = channel_shape(&told);
        if content.contains("the line to the wall dropped") {
            dropped = true;
            break;
        }
    }
    assert!(dropped, "the channel says the line dropped");

    // And back again, from the same saved state.
    let daemon = start(runtime.path(), state.path());
    let look = works(&client, "wall_look", serde_json::json!({})).await;
    assert!(look.contains("vcf1 (vcf)"), "{look}");
    let mut back = false;
    while let Some(told) = next_notification(&mut notifications, Duration::from_secs(5)).await {
        let (content, _) = channel_shape(&told);
        if content.contains("the line to the wall is back") {
            back = true;
            break;
        }
    }
    assert!(back, "the channel says the line is back");

    client.cancel().await.unwrap();
    stop(daemon);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_legacy_revisions_are_offered_so_the_channel_can_travel() {
    let (runtime, _state) = dirs();
    let socket = runtime.path().join(SOCKET_NAME);
    for (asked, answered) in [
        (ProtocolVersion::V_2024_11_05, ProtocolVersion::V_2024_11_05),
        (ProtocolVersion::V_2025_03_26, ProtocolVersion::V_2025_03_26),
        (ProtocolVersion::V_2025_06_18, ProtocolVersion::V_2025_06_18),
        (ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2025_11_25),
        (ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25),
    ] {
        let (client, _) = seat(&socket, "Waffles", 1, Some(asked.clone())).await;
        let info = client.peer_info().expect("the server describes itself");
        assert_eq!(info.protocol_version, answered, "asked for {asked:?}");
        client.cancel().await.unwrap();
    }
}

#[test]
fn a_missing_or_bad_seat_stops_with_a_message() {
    // Never near the default socket or state, though these runs stop
    // before connecting.
    let (runtime, state) = dirs();
    let run = |args: &[&str], env: Option<&str>| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_kazoo-mcp"));
        command
            .args(args)
            .stdin(Stdio::null())
            .env("KAZOO_WALL_RUNTIME_DIR", runtime.path())
            .env("KAZOO_WALL_STATE_DIR", state.path());
        match env {
            Some(seat) => command.env("KAZOO_SEAT", seat),
            None => command.env_remove("KAZOO_SEAT"),
        };
        let output = command.output().expect("kazoo-mcp runs");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let (code, said) = run(&[], None);
    assert_eq!(code, Some(2), "{said}");
    assert!(
        said.contains("--seat <Name>") && said.contains("KAZOO_SEAT"),
        "{said}"
    );
    let (code, said) = run(&["--seat", "bad/name"], None);
    assert_eq!(code, Some(2), "{said}");
    assert!(said.contains("not a valid seat name"), "{said}");
    let (code, said) = run(&[], Some("Tom!"));
    assert_eq!(code, Some(2), "{said}");
    assert!(said.contains("not a valid seat name"), "{said}");
    let (code, said) = run(&["--help"], None);
    assert_eq!(code, Some(0), "{said}");
}
