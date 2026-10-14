//! The daemon over a real socket: seats, events, undo, persistence, roles.
//!
//! Each test runs a headless daemon (no audio device, no desk) with its
//! socket and state in fresh temporary directories.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use kazoo_wall::daemon::{Daemon, DaemonConfig};
use kazoo_wall::protocol::client::{Subscription, WallClient};
use kazoo_wall::protocol::{ErrorCode, Event, MAX_LINE, Place, Response, What};

/// Socket paths are limited to about 100 bytes: the runtime directory lives
/// directly under /tmp.
fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    let runtime = tempfile::Builder::new()
        .prefix("kw")
        .tempdir_in("/tmp")
        .unwrap();
    let state = tempfile::tempdir().unwrap();
    (runtime, state)
}

/// A test daemon with the load guard off: a debug build renders far slower
/// than real time, so its render load says nothing about the wall.
fn start(runtime: &Path, state: &Path) -> Daemon {
    Daemon::start(DaemonConfig {
        load_guard: false,
        ..DaemonConfig::headless(runtime, state)
    })
    .unwrap()
}

const fn code(err: &kazoo_wall::protocol::client::ClientError) -> ErrorCode {
    err.wall_error().unwrap().code
}

/// The next change, passing over seats coming and going and fingerprints.
fn next_change(events: &mut Subscription) -> Event {
    loop {
        let event = events.next_event().unwrap();
        if matches!(event, Event::Change { .. }) {
            return event;
        }
    }
}

/// The next seat coming or going, passing over everything else.
fn next_seat(events: &mut Subscription) -> Event {
    loop {
        let event = events.next_event().unwrap();
        if matches!(event, Event::Seat { .. }) {
            return event;
        }
    }
}

#[test]
fn two_seats_see_each_others_changes_and_not_their_own() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();

    let (mut tom_events, subscribed) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    assert_eq!(subscribed.seats, vec!["Tom"]);
    tom_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut waffles_events, _) = Subscription::open(&socket, "Waffles", "test", false).unwrap();
    waffles_events
        .set_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    // Tom hears Waffles arrive.
    assert_eq!(
        tom_events.next_event().unwrap(),
        Event::Seat {
            seat: "Waffles".to_string(),
            joined: true,
            seq: Some(0),
        }
    );

    let mut waffles = WallClient::connect(&socket, "Waffles", "test", false).unwrap();
    assert_eq!(waffles.hello().seats, vec!["Tom", "Waffles"]);
    let turned = waffles.turn("vcf1", "cutoff", 800.0, Some(4.0)).unwrap();
    assert_eq!(
        turned.change.summary,
        "Waffles turned vcf1 cutoff 900 Hz → 800 Hz over 4 beats"
    );

    // Tom hears it...
    let Event::Change { change } = next_change(&mut tom_events) else {
        panic!("not a change");
    };
    assert_eq!(change.seq, turned.change.seq);
    assert_eq!(change.seat, "Waffles");
    // ...and Waffles, the actor, does not: only the fingerprints it moved.
    let Event::Fingerprints { seq, .. } = waffles_events.next_event().unwrap() else {
        panic!("the actor heard more than its fingerprints");
    };
    assert_eq!(seq, turned.change.seq);
    let err = waffles_events.next_event().unwrap_err();
    assert!(err.is_timeout(), "{err}");

    // Tom's change reaches Waffles.
    let mut tom = WallClient::connect(&socket, "Tom", "test", true).unwrap();
    let added = tom.add("lfo", Some("slow wobble")).unwrap();
    assert_eq!(added.module.as_deref(), Some("lfo3"));
    waffles_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Event::Change { change } = next_change(&mut waffles_events) else {
        panic!("not a change");
    };
    assert_eq!(change.summary, "Tom added lfo3 \"slow wobble\"");

    // Waffles leaving is news to Tom once its last connection closes.
    drop(waffles);
    drop(waffles_events);
    let left = next_seat(&mut tom_events);
    assert_eq!(
        left,
        Event::Seat {
            seat: "Waffles".to_string(),
            joined: false,
            seq: Some(2),
        }
    );
    drop(tom);
    daemon.stop();
    daemon.wait().unwrap();
    assert!(!socket.exists());
}

#[test]
fn undo_applies_the_inverse_as_a_new_change() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let seeded = wall.look().unwrap().cables.len();
    let patched = wall.patch("lfo1.out", "vcf1.drive", Some(0.5)).unwrap();
    let cable = patched.cable.unwrap();
    assert!(wall.look().unwrap().cables.iter().any(|c| c.id == cable));
    let undone = wall.undo(patched.change.seq).unwrap();
    assert_eq!(undone.change.undoes, Some(patched.change.seq));
    assert!(matches!(undone.change.what, What::Unpatch { .. }));
    assert!(!wall.look().unwrap().cables.iter().any(|c| c.id == cable));
    // Undoing the undo plugs the same cable back.
    wall.undo(undone.change.seq).unwrap();
    assert!(wall.look().unwrap().cables.iter().any(|c| c.id == cable));

    let removed = wall.remove("vcf1").unwrap();
    assert!(!wall.look().unwrap().modules.iter().any(|m| m.id == "vcf1"));
    wall.undo(removed.change.seq).unwrap();
    let look = wall.look().unwrap();
    assert!(look.modules.iter().any(|m| m.id == "vcf1"));
    assert_eq!(look.cables.len(), seeded + 1);

    let log = wall.log(None, Some(3)).unwrap();
    assert_eq!(log.changes.len(), 3);
    assert!(log.more);
    assert_eq!(log.changes.last().unwrap().seq, look.revision);
    assert_eq!(
        code(&wall.undo(9_999).unwrap_err()),
        ErrorCode::UnknownChange
    );
}

#[test]
fn the_wall_survives_a_restart() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    wall.turn("vco1", "tune", 5.0, Some(0.0)).unwrap();
    wall.add("noise", Some("hiss")).unwrap();
    wall.patch("noise1.out", "mix1.c", Some(-0.25)).unwrap();
    let tempo = wall.tempo(100.0).unwrap();
    assert!(!tempo.desk);
    wall.arrange("noise1", Place::end_of(0)).unwrap();
    let before = wall.look().unwrap();
    // A console may stop the wall.
    assert!(wall.shutdown().unwrap().stopping);
    daemon.wait().unwrap();

    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let after = wall.look().unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.rack, before.rack, "the rows are where they were");
    assert!(
        after
            .rack
            .as_ref()
            .is_some_and(|rows| rows[0].contains(&"noise1".to_string()))
    );
    assert_eq!(after.cables, before.cables);
    let ids = |look: &kazoo_wall::protocol::Snapshot| {
        look.modules
            .iter()
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&after), ids(&before));
    let tune = after
        .modules
        .iter()
        .find(|m| m.id == "vco1")
        .and_then(|m| m.knobs.iter().find(|k| k.name == "tune"))
        .unwrap();
    assert!((tune.target - 5.0).abs() < 1e-6);
    assert!((after.tempo - 100.0).abs() < 1e-9);
    // The numbering carries on, and the history is still there to undo.
    let next = wall.add("noise", None).unwrap();
    assert_eq!(next.module.as_deref(), Some("noise2"));
    assert_eq!(next.change.seq, before.revision + 1);
    assert_eq!(wall.log(None, None).unwrap().changes.len(), 5);
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn a_corrupt_patch_is_set_aside_and_the_wall_seeds() {
    let (runtime, state) = dirs();
    std::fs::write(
        state.path().join("patch.json"),
        b"{\"version\": 1, \"modules\": [",
    )
    .unwrap();
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let seeded = kazoo_wall::seed::seed().unwrap();
    assert_eq!(wall.look().unwrap().modules.len(), seeded.modules().len());
    let aside: Vec<String> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("patch.broken-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(
        std::fs::read(state.path().join(&aside[0])).unwrap(),
        b"{\"version\": 1, \"modules\": ["
    );
    // Said in the log, as the wall's own change, with why and where.
    let logged = wall.log(None, None).unwrap().changes;
    let note = logged
        .iter()
        .find(|change| matches!(change.what, What::SetAside { .. }))
        .expect("the setting aside, logged");
    assert_eq!(note.seat, "kazoo-wall");
    let What::SetAside { reason, moved_to } = &note.what else {
        panic!("{note:?}");
    };
    assert_eq!(moved_to, &aside[0]);
    assert!(reason.contains("not JSON"), "{reason}");
    assert!(note.summary.contains(&aside[0]), "{}", note.summary);
    assert_eq!(
        code(&wall.undo(note.seq).unwrap_err()),
        ErrorCode::NotAllowed
    );
    // And listed among the broken saves, never out of sight.
    let saves = wall.saves().unwrap();
    assert!(
        saves.broken.iter().any(|broken| broken.name == aside[0]),
        "{:?}",
        saves.broken
    );
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

/// Send raw lines on a raw connection and read the answers.
struct Raw {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Raw {
    fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Self {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }

    fn send(&mut self, line: &[u8]) -> Response {
        self.writer.write_all(line).unwrap();
        self.writer.write_all(b"\n").unwrap();
        self.read()
    }

    fn read(&mut self) -> Response {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn closed(&mut self) -> bool {
        let mut line = String::new();
        matches!(self.reader.read_line(&mut line), Ok(0))
    }
}

const fn error_code(response: &Response) -> ErrorCode {
    response.error.as_ref().unwrap().code
}

#[test]
fn the_protocol_holds_its_rules() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut raw = Raw::connect(daemon.socket());

    let response = raw.send(br#"{"id":1,"op":"look"}"#);
    assert_eq!(
        (response.id, error_code(&response)),
        (1, ErrorCode::NotHello)
    );
    let response = raw.send(b"not json");
    assert_eq!(
        (response.id, error_code(&response)),
        (0, ErrorCode::BadRequest)
    );
    let response = raw.send(br#"{"id":2,"op":"hello","seat":"no/slashes","client":"raw"}"#);
    assert_eq!(
        (response.id, error_code(&response)),
        (2, ErrorCode::BadName)
    );
    let response = raw.send(br#"{"id":3,"op":"hello","seat":"Vesper","client":"raw"}"#);
    assert!(response.ok);
    let response = raw.send(br#"{"id":4,"op":"hello","seat":"Vesper","client":"raw"}"#);
    assert_eq!(
        (response.id, error_code(&response)),
        (4, ErrorCode::NotAllowed)
    );
    let response = raw.send(br#"{"id":5,"op":"shutdown"}"#);
    assert_eq!(
        (response.id, error_code(&response)),
        (5, ErrorCode::NotAllowed)
    );
    let response = raw.send(br#"{"id":6,"op":"twirl"}"#);
    assert_eq!(
        (response.id, error_code(&response)),
        (6, ErrorCode::BadRequest)
    );
    // Every request gets exactly one response, in order.
    raw.writer
        .write_all(b"{\"id\":7,\"op\":\"listen\"}\n{\"id\":8,\"op\":\"catalogue\"}\n")
        .unwrap();
    assert_eq!(raw.read().id, 7);
    assert_eq!(raw.read().id, 8);

    // A line past the cap closes the connection with an error.
    let mut long = vec![b'x'; MAX_LINE + 10];
    long.push(b'\n');
    raw.writer.write_all(&long).unwrap();
    let response = raw.read();
    assert_eq!(error_code(&response), ErrorCode::BadRequest);
    assert!(raw.closed());

    // Flooding seats are told to slow down.
    let mut flood = WallClient::connect(daemon.socket(), "Loop", "test", false).unwrap();
    let mut refused = None;
    for step in 0..40 {
        if let Err(err) = flood.turn("vcf1", "cutoff", 300.0 + f64::from(step), Some(0.0)) {
            refused = Some(err);
            break;
        }
    }
    assert_eq!(code(&refused.unwrap()), ErrorCode::SlowDown);
    drop(flood);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn connections_are_capped_and_a_second_daemon_is_refused() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut held = Vec::new();
    for _ in 0..kazoo_wall::daemon::control::MAX_CONNECTIONS {
        held.push(WallClient::connect(daemon.socket(), "Many", "test", false).unwrap());
    }
    let mut raw = Raw::connect(daemon.socket());
    let response = raw.read();
    assert_eq!(error_code(&response), ErrorCode::Full);
    assert!(raw.closed());
    drop(held);

    let other = tempfile::tempdir().unwrap();
    let refused = Daemon::start(DaemonConfig::headless(runtime.path(), other.path())).unwrap_err();
    assert!(refused.to_string().contains("already playing"), "{refused}");
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn a_patch_json_past_the_limits_with_the_log_ahead_starts_within_them() {
    let (runtime, state) = dirs();
    // A patch.json larger than a wall's file may be (words no speaker
    // could be given), and a log with a change it does not have.
    let mut patch = kazoo_wall::seed::seed().unwrap();
    patch.add("speak", None, None).unwrap();
    let mut file = patch.to_file();
    let limit = usize::try_from(kazoo_wall::store::MAX_WALL_FILE).unwrap();
    file.speech.insert(
        "speak1".to_string(),
        kazoo_wall::patch::Words {
            text: "a".repeat(limit + 1),
            voice: None,
        },
    );
    let bytes = serde_json::to_vec(&file).unwrap();
    assert!(bytes.len() > limit);
    std::fs::write(state.path().join("patch.json"), &bytes).unwrap();
    let mut store = kazoo_wall::store::Store::open(state.path()).unwrap();
    store
        .append(&kazoo_wall::protocol::Change {
            seq: 1,
            at: "2026-10-09T00:00:00Z".to_string(),
            seat: "Tom".to_string(),
            summary: "Tom set the tempo".to_string(),
            what: What::Tempo {
                from: 92.0,
                to: 100.0,
                desk: false,
            },
            undoes: None,
            clock: None,
        })
        .unwrap();
    store.sync_log().unwrap();
    drop(store);
    // It starts: read whole (the wall wrote it), brought within today's
    // limits, said so in the log, and saved within them.
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    assert_eq!(
        wall.look().unwrap().log,
        kazoo_wall::protocol::LogHealth::default()
    );
    let logged = wall.log(None, None).unwrap().changes;
    assert!(
        logged
            .iter()
            .any(|change| matches!(change.what, What::Resumed { .. }))
    );
    assert!(
        logged.iter().any(|change| matches!(
            &change.what,
            What::Migrate { notes, .. }
                if notes.iter().any(|note| note.contains("longer than a speaker may be given"))
        )),
        "{logged:?}"
    );
    let saved = std::fs::metadata(state.path().join("patch.json"))
        .unwrap()
        .len();
    assert!(saved <= kazoo_wall::store::MAX_WALL_FILE, "{saved}");
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn a_wall_stopped_can_be_started_again_at_once() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    // As `serve` does: waiting for the daemon to stop.
    let serving = std::thread::spawn(move || daemon.wait());
    kazoo_wall::daemon::stop_daemon(&socket, Duration::from_secs(10)).unwrap();
    // Stopped means its state is free: a wall starts on it at once.
    let again = start(runtime.path(), state.path());
    serving.join().unwrap().unwrap();
    again.stop();
    again.wait().unwrap();
}

#[test]
fn a_wall_started_while_another_stops_waits_for_its_state() {
    let (runtime, state) = dirs();
    let first = start(runtime.path(), state.path());
    // Asked to stop, but not waited for: it still holds the state a
    // moment, while it saves.
    first.stop();
    let other = tempfile::Builder::new()
        .prefix("kw")
        .tempdir_in("/tmp")
        .unwrap();
    let second = start(other.path(), state.path());
    first.wait().unwrap();
    // A wall on the same state that does not stop is refused, saying so.
    let refused = Daemon::start(DaemonConfig::headless(runtime.path(), state.path())).unwrap_err();
    assert!(
        refused.to_string().contains("another wall is using"),
        "{refused}"
    );
    second.stop();
    second.wait().unwrap();
}

#[test]
fn the_wall_keeps_time_and_listens() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let first = wall.look().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut heard = None;
    while heard.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        heard = wall.listen().unwrap().listen;
    }
    let heard = heard.expect("the wall was never heard");
    assert!(!heard.words.is_empty());
    let later = wall.look().unwrap();
    assert!(
        later.beat > first.beat,
        "{} then {}",
        first.beat,
        later.beat
    );
    assert!(later.listen.is_some());
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn scopes_and_readouts_are_probed_over_the_socket() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Waffles", "test", false).unwrap();
    assert!(wall.probe(None).unwrap().probes.is_empty());
    let scope = wall.add("scope", None).unwrap().module.unwrap();
    let readout = wall.add("readout", None).unwrap().module.unwrap();
    wall.patch("vco1.out", &format!("{scope}.a"), None).unwrap();
    wall.patch("seq1.pitch", &format!("{readout}.in"), None)
        .unwrap();
    // A debug build on a busy machine renders when it can.
    let deadline = Instant::now() + Duration::from_secs(30);
    let probed = loop {
        let probed = wall.probe(None).unwrap();
        // Measured (what was measured before the cables reached the
        // engine is let go, never shown).
        let measured = probed
            .probes
            .iter()
            .all(|probe| probe.scope.is_some() || probe.readout.is_some());
        if measured || Instant::now() > deadline {
            break probed;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(probed.probes.len(), 2);
    let trace = probed.probes[0].scope.as_ref().expect("the scope measured");
    assert_eq!(trace.a.len(), 96);
    assert!(trace.input_a.connected && !trace.input_b.connected);
    let shown = probed.probes[1]
        .readout
        .as_ref()
        .expect("the readout measured");
    assert_eq!(shown.mode, "pitch", "a sequencer's pitch is shown as pitch");
    assert!(shown.note.is_some(), "{shown:?}");
    assert_eq!(wall.look().unwrap().probes.len(), 2);
    assert_eq!(wall.probe(Some(&readout)).unwrap().probes.len(), 1);
    assert_eq!(
        code(&wall.probe(Some("vco1")).unwrap_err()),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(&wall.probe(Some("scope9")).unwrap_err()),
        ErrorCode::UnknownModule
    );
    // A watcher may probe: it is a read.
    let mut watcher = WallClient::watch(daemon.socket(), "meridian", "test").unwrap();
    assert_eq!(watcher.probe(Some(&scope)).unwrap().probes.len(), 1);
    drop(watcher);
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn a_watcher_sees_everything_and_changes_nothing() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (mut tom_events, _) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    tom_events
        .set_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    // A watcher named like a player still hears that player's changes.
    let (mut watcher, subscribed) = Subscription::watch(&socket, "Tom", "a visualiser").unwrap();
    assert_eq!(subscribed.seats, vec!["Tom"]);
    watcher.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut looking = WallClient::watch(&socket, "meridian", "a visualiser").unwrap();
    assert!(looking.hello().watcher);
    assert_eq!(looking.hello().seats, vec!["Tom"]);
    // Nobody was told a watcher arrived, and it is not among the seats.
    assert!(tom_events.next_event().unwrap_err().is_timeout());
    assert_eq!(looking.look().unwrap().seats, vec!["Tom"]);

    for refused in [
        looking.turn("vcf1", "cutoff", 500.0, None).map(|_| ()),
        looking.patch("lfo1.out", "vcf1.drive", None).map(|_| ()),
        looking.unpatch_cable(1).map(|_| ()),
        looking.add("lfo", None).map(|_| ()),
        looking.remove("lfo1").map(|_| ()),
        looking.arrange("lfo1", Place::end_of(0)).map(|_| ()),
        looking.undo(1).map(|_| ()),
        looking.tempo(100.0).map(|_| ()),
        looking.record(true).map(|_| ()),
        looking.shutdown().map(|_| ()),
    ] {
        assert_eq!(code(&refused.unwrap_err()), ErrorCode::NotAllowed);
    }
    assert!(
        looking.catalogue().is_ok() && looking.listen().is_ok() && looking.log(None, None).is_ok()
    );

    let mut tom = WallClient::connect(&socket, "Tom", "test", true).unwrap();
    let turned = tom.turn("lfo1", "depth", 0.0, Some(0.0)).unwrap();
    let Event::Change { change } = watcher.next_event().unwrap() else {
        panic!("the watcher missed the change");
    };
    assert_eq!(change.seq, turned.change.seq);
    drop(looking);
    drop(watcher);
    // Nor was anyone told the watchers left.
    let quiet = tom_events.next_event();
    assert!(!matches!(quiet, Ok(Event::Seat { .. })), "{quiet:?}");
    drop(tom);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn fingerprints_follow_their_change_and_seat_events_carry_seq() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (mut watcher, _) = Subscription::watch(&socket, "meridian", "test").unwrap();
    watcher.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let (mut tom_events, _) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    tom_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let Event::Seat { seat, joined, seq } = watcher.next_event().unwrap() else {
        panic!("no seat event");
    };
    assert_eq!((seat.as_str(), joined, seq), ("Tom", true, Some(0)));

    let mut tom = WallClient::connect(&socket, "Tom", "test", true).unwrap();
    let turned = tom.turn("lfo1", "depth", 0.0, Some(0.0)).unwrap();
    let seq = turned.change.seq;
    // The watcher: the change, then the fingerprints it moved, same seq.
    let Event::Change { change } = watcher.next_event().unwrap() else {
        panic!("not the change first");
    };
    assert_eq!(change.seq, seq);
    let Event::Fingerprints {
        seq: moved,
        modules,
        cables,
    } = watcher.next_event().unwrap()
    else {
        panic!("not the fingerprints second");
    };
    assert_eq!(moved, seq);
    assert!(modules["lfo1"]["Tom"] > 0.999);
    assert!(!cables.is_empty());
    // The actor gets no change event, but does get the fingerprints.
    let Event::Fingerprints { seq: moved, .. } = tom_events.next_event().unwrap() else {
        panic!("the actor did not get the fingerprints");
    };
    assert_eq!(moved, seq);
    let look = tom.look().unwrap();
    assert_eq!(look.fingerprints.modules["lfo1"], modules["lfo1"]);

    // A seat leaving is placed after the latest change.
    let (waffles, _) = Subscription::open(&socket, "Waffles", "test", false).unwrap();
    let Event::Seat { seq: joined_at, .. } = watcher.next_event().unwrap() else {
        panic!("no join");
    };
    assert_eq!(joined_at, Some(seq));
    drop(waffles);
    let Event::Seat {
        joined,
        seq: left_at,
        ..
    } = watcher.next_event().unwrap()
    else {
        panic!("no leave");
    };
    assert!(!joined);
    assert_eq!(left_at, Some(seq));

    // Fingerprints survive a restart.
    drop(tom);
    drop(tom_events);
    drop(watcher);
    daemon.stop();
    daemon.wait().unwrap();
    let daemon = start(runtime.path(), state.path());
    let mut again = WallClient::watch(daemon.socket(), "meridian", "test").unwrap();
    assert_eq!(again.look().unwrap().fingerprints, look.fingerprints);
    drop(again);
    daemon.stop();
    daemon.wait().unwrap();
}

/// Tom's "Airports" wall as version 1 saved it.
const AIRPORTS: &[u8] = include_bytes!("fixtures/airports-v1.json");

#[test]
fn a_version_1_patch_is_brought_up_to_date_on_start() {
    let (runtime, state) = dirs();
    std::fs::write(state.path().join("patch.json"), AIRPORTS).unwrap();
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let look = wall.look().unwrap();
    assert_eq!(look.modules.len(), 47);
    let kind = |id: &str| {
        look.modules
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .kind
            .clone()
    };
    assert_eq!(kind("delay2"), "digital");
    assert_eq!(kind("reverb2"), "plate");
    // Every cable Tom plugged is there under its own number.
    let original: serde_json::Value = serde_json::from_slice(AIRPORTS).unwrap();
    let cables = original["cables"].as_array().unwrap();
    for cable in cables {
        let id = u32::try_from(cable["id"].as_u64().unwrap()).unwrap();
        assert!(look.cables.iter().any(|c| c.id == id), "cable {id}");
    }
    assert_eq!(look.cables.len(), cables.len() + 4);
    // The migration is in the log, as the loader's own change.
    let log = wall.log(None, Some(1)).unwrap();
    let change = &log.changes[0];
    assert_eq!(change.seat, "kazoo-wall");
    assert_eq!(change.seq, 233);
    let What::Migrate {
        from_version,
        to_version,
        notes,
    } = &change.what
    else {
        panic!("not a migration: {:?}", change.what);
    };
    assert_eq!((*from_version, *to_version), (1, 2));
    assert_eq!(notes.len(), 9);
    assert_eq!(code(&wall.undo(233).unwrap_err()), ErrorCode::NotAllowed);
    // The original is kept, byte for byte; the new patch is saved.
    let kept: Vec<_> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("patch.before-")
        })
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read(&kept[0]).unwrap(), AIRPORTS);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.path().join("patch.json")).unwrap()).unwrap();
    assert_eq!(saved["version"], 2);
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();

    // Starting again finds a current patch: nothing more to do.
    let daemon = start(runtime.path(), state.path());
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    assert_eq!(wall.look().unwrap().revision, 233);
    assert_eq!(wall.look().unwrap().modules.len(), 47);
    let before = std::fs::read_dir(state.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("patch.before-")
        })
        .count();
    assert_eq!(before, 1);
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn speak_renders_words_plays_them_and_keeps_them_private() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (mut waffles, _) = Subscription::open(&socket, "Waffles", "test", false).unwrap();
    waffles.set_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut tom = WallClient::connect(&socket, "Tom", "test", true).unwrap();
    let speaker = tom.add("speak", None).unwrap().module.unwrap();
    assert_eq!(
        code(&tom.speak("vcf1", "hello", None).unwrap_err()),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(&tom.speak(&speaker, "   ", None).unwrap_err()),
        ErrorCode::BadRequest
    );
    let mut watcher = WallClient::watch(&socket, "meridian", "test").unwrap();
    assert_eq!(
        code(&watcher.speak(&speaker, "hello", None).unwrap_err()),
        ErrorCode::NotAllowed
    );
    let words = "hello from the wall";
    if !std::path::Path::new("/usr/bin/say").exists() {
        assert_eq!(
            code(&tom.speak(&speaker, words, None).unwrap_err()),
            ErrorCode::Internal
        );
        drop(tom);
        daemon.stop();
        daemon.wait().unwrap();
        return;
    }
    let spoken = tom.speak(&speaker, words, None).unwrap();
    let What::Speak {
        module,
        words: count,
        seconds,
    } = &spoken.change.what
    else {
        panic!("not a speak change: {:?}", spoken.change.what);
    };
    assert_eq!((module.as_str(), *count), (speaker.as_str(), 4));
    assert!(*seconds > 0.3, "{seconds}");
    assert!(
        spoken
            .change
            .summary
            .starts_with(&format!("Tom gave {speaker} 4 words to say"))
    );

    // Another seat hears that words were given, never what they are.
    let change = loop {
        let Event::Change { change } = next_change(&mut waffles) else {
            panic!("no change");
        };
        if change.seq == spoken.change.seq {
            break change;
        }
    };
    let heard = serde_json::to_string(&change).unwrap();
    assert!(!heard.contains("hello"), "{heard}");
    let look = serde_json::to_string(&tom.look().unwrap()).unwrap();
    assert!(!look.contains("hello"));

    // Gate it into its own out, with the seed's out closed: the master
    // hears the words.
    tom.turn("out1", "level", 0.0, Some(0.0)).unwrap();
    let out = tom.add("out", None).unwrap().module.unwrap();
    tom.turn(&out, "level", 1.0, Some(0.0)).unwrap();
    tom.turn(&speaker, "mode", 1.0, Some(0.0)).unwrap();
    tom.patch(&format!("{speaker}.out"), &format!("{out}.left"), None)
        .unwrap();
    tom.patch("clock1.out", &format!("{speaker}.gate"), None)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut loudest = -120.0_f64;
    while Instant::now() < deadline && loudest < -40.0 {
        std::thread::sleep(Duration::from_millis(100));
        loudest = loudest.max(tom.look().unwrap().levels.peak_l);
    }
    assert!(
        loudest > -40.0,
        "the words were never heard: {loudest} dBFS"
    );

    // The words are kept with the patch, privately, and survive a restart.
    drop(tom);
    drop(watcher);
    drop(waffles);
    daemon.stop();
    daemon.wait().unwrap();
    let saved = std::fs::read_to_string(state.path().join("patch.json")).unwrap();
    assert!(saved.contains(words));
    let daemon = start(runtime.path(), state.path());
    let mut tom = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    assert!(tom.look().unwrap().modules.iter().any(|m| m.id == speaker));
    drop(tom);
    daemon.stop();
    daemon.wait().unwrap();
    let saved = std::fs::read_to_string(state.path().join("patch.json")).unwrap();
    assert!(saved.contains(words));
}

#[test]
fn only_a_console_makes_the_wall_heard_and_it_starts_silent() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut console = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let mut seat = WallClient::connect(daemon.socket(), "Cassio", "test", false).unwrap();
    assert!(!console.look().unwrap().heard, "silent until asked");
    let refused = seat.monitor(true).unwrap_err();
    assert_eq!(code(&refused), ErrorCode::NotAllowed);
    assert!(!seat.look().unwrap().heard);
    assert!(console.monitor(true).unwrap().on);
    assert!(seat.look().unwrap().heard);
    assert!(!console.monitor(false).unwrap().on);
    assert!(!seat.look().unwrap().heard);
    // A monitor turn is not a change: nothing in the log.
    assert!(console.log(None, None).unwrap().changes.is_empty());
    drop((console, seat));
    daemon.stop();
    daemon.wait().unwrap();
}

/// Wait until the recording under way holds at least a tenth of a second:
/// a debug build on a busy machine renders when it can, not in real time.
fn a_tenth_recorded(wall: &mut WallClient) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while wall
        .look()
        .unwrap()
        .recording
        .is_none_or(|recording| recording.seconds < 0.1)
    {
        assert!(Instant::now() < deadline, "the recording never grew");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The one recording in `dir`, as (path, frames), after checking it is a
/// 32-bit float stereo WAV at 48 kHz.
fn only_recording(dir: &Path) -> (std::path::PathBuf, u32) {
    let files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let reader = hound::WavReader::open(&files[0]).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 48_000);
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    (files[0].clone(), reader.duration())
}

#[test]
fn a_seat_records_the_wall_and_others_are_told() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (mut tom_events, _) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    tom_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut waffles = WallClient::connect(&socket, "Waffles", "test", false).unwrap();

    let started = waffles.record(true).unwrap();
    assert!(started.on);
    let path = started.path.unwrap();
    assert!(
        path.starts_with(
            &state
                .path()
                .join("recordings")
                .to_string_lossy()
                .into_owned()
        )
    );
    let Event::Change { change } = next_change(&mut tom_events) else {
        unreachable!()
    };
    assert_eq!(change.seat, "Waffles");
    assert!(matches!(change.what, What::Record { on: true, .. }));
    let look = waffles.look().unwrap();
    assert_eq!(
        look.recording.as_ref().map(|r| r.path.as_str()),
        Some(path.as_str())
    );
    a_tenth_recorded(&mut waffles);

    let stopped = waffles.record(false).unwrap();
    assert!(!stopped.on);
    assert!(stopped.seconds >= 0.1, "{}", stopped.seconds);
    let Event::Change { change } = next_change(&mut tom_events) else {
        unreachable!()
    };
    assert!(matches!(change.what, What::Record { on: false, .. }));
    let (file, frames) = only_recording(&state.path().join("recordings"));
    assert_eq!(file.to_string_lossy(), path);
    // Every frame counted in the answer is in the file.
    assert!(
        (f64::from(frames) / 48_000.0 - stopped.seconds).abs() < 1e-6,
        "{frames} frames against {} s",
        stopped.seconds
    );
    assert!(waffles.look().unwrap().recording.is_none());
    drop(waffles);
    drop(tom_events);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn a_recording_is_finished_when_the_wall_stops() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let mut tom = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let path = tom.record(true).unwrap().path.unwrap();
    a_tenth_recorded(&mut tom);
    tom.shutdown().unwrap();
    drop(tom);
    daemon.wait().unwrap();
    let (file, frames) = only_recording(&state.path().join("recordings"));
    assert_eq!(file.to_string_lossy(), path);
    assert!(frames >= 4_800, "{frames} frames");
    // The header holds every sample the file has.
    let bytes = std::fs::metadata(&file).unwrap().len();
    assert!(
        bytes >= u64::from(frames) * 8 + 44,
        "{bytes} bytes for {frames} frames"
    );
    // The stop is in the log, as the wall's own.
    let log = std::fs::read_to_string(state.path().join(kazoo_wall::store::LOG_FILE)).unwrap();
    let last = log.lines().last().unwrap();
    assert!(last.contains("\"seat\":\"kazoo-wall\""), "{last}");
    assert!(last.contains("the wall stopped"), "{last}");
}

#[test]
fn a_move_is_told_to_every_console_the_mover_too_and_logs_nothing() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (mut tom_events, _) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    tom_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut waffles_events, _) = Subscription::open(&socket, "Waffles", "test", false).unwrap();
    waffles_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut tom = WallClient::connect(&socket, "Tom", "test", true).unwrap();
    let before = tom.look().unwrap();
    let rows = before.rack.clone().expect("the rows");
    let moved = tom.arrange("lfo1", Place::end_of(rows.len())).unwrap();
    assert_eq!(moved.rows.last(), Some(&vec!["lfo1".to_string()]));
    // Both hear the rows (Tom, the mover, too); nobody hears a change.
    for events in [&mut tom_events, &mut waffles_events] {
        loop {
            match events.next_event().unwrap() {
                Event::Rack { rows } => {
                    assert_eq!(rows, moved.rows);
                    break;
                }
                Event::Change { change } => panic!("a move was logged: {change:?}"),
                Event::Loaded { .. } | Event::Saved { .. } | Event::SaveDeleted { .. } => {
                    panic!("a move touched the saves")
                }
                Event::Seat { .. }
                | Event::Chains { .. }
                | Event::Groups { .. }
                | Event::Fingerprints { .. }
                | Event::Fault { .. }
                | Event::Log { .. }
                | Event::Marked { .. }
                | Event::Unknown => {}
            }
        }
    }
    let after = tom.look().unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.rack, Some(moved.rows));
    assert!(tom.log(None, None).unwrap().changes.is_empty());
    drop(tom);
    daemon.stop();
    daemon.wait().unwrap();
}

/// The next event that is not a seat coming or going, nor fingerprints.
fn next_news(events: &mut Subscription) -> Event {
    loop {
        let event = events.next_event().unwrap();
        if !matches!(event, Event::Seat { .. } | Event::Fingerprints { .. }) {
            return event;
        }
    }
}

/// What a wall is, as `look` shows it: every module with its knob targets,
/// every cable, and the rows.
fn shape(look: &kazoo_wall::protocol::Snapshot) -> (Vec<(String, Vec<f64>)>, String, String) {
    let modules = look
        .modules
        .iter()
        .map(|module| {
            let targets = module.knobs.iter().map(|knob| knob.target).collect();
            (module.id.clone(), targets)
        })
        .collect();
    (
        modules,
        serde_json::to_string(&look.cables).unwrap(),
        serde_json::to_string(&look.rack).unwrap(),
    )
}

/// A daemon started on a state directory holding a copy of the patch Tom
/// made by hand beside it (before the wall had saves), which comes in as a
/// save.
fn a_wall_with_a_copy_by_hand(runtime: &Path, state: &Path) -> Daemon {
    let seeded = {
        let daemon = start(runtime, state);
        drop(daemon);
        std::fs::read(state.join("patch.json")).unwrap()
    };
    // As a wall from before the saves left it.
    std::fs::remove_dir_all(state.join("saves")).unwrap();
    std::fs::write(state.join("patch.by-hand-2026-09-29.json"), &seeded).unwrap();
    let daemon = start(runtime, state);
    let mut tom = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let listed = tom.saves().unwrap();
    let names: Vec<&str> = listed.saves.iter().map(|save| save.name.as_str()).collect();
    assert_eq!(names, ["by-hand-2026-09-29"]);
    assert!(
        state.join("patch.by-hand-2026-09-29.json").exists(),
        "copied, not moved"
    );
    daemon
}

/// Only a console replaces or deletes a save; a watcher saves, loads and
/// deletes nothing, but may list them.
fn only_a_console_destroys_a_save(socket: &Path, waffles: &mut WallClient) {
    assert_eq!(
        code(&waffles.save("waffles jam", true).unwrap_err()),
        ErrorCode::NotAllowed
    );
    assert_eq!(
        code(&waffles.save("Waffles Jam", false).unwrap_err()),
        ErrorCode::SaveExists
    );
    assert_eq!(
        code(&waffles.delete_save("waffles jam").unwrap_err()),
        ErrorCode::NotAllowed
    );
    let mut looking = WallClient::watch(socket, "meridian", "a visualiser").unwrap();
    for refused in [
        looking.save("watched", false).map(|_| ()),
        looking.load("waffles jam").map(|_| ()),
        looking.new_wall().map(|_| ()),
        looking.delete_save("waffles jam").map(|_| ()),
    ] {
        assert_eq!(code(&refused.unwrap_err()), ErrorCode::NotAllowed);
    }
    assert_eq!(looking.saves().unwrap().saves.len(), 2);
}

/// A turn Tom aimed at the wall he last saw (wall 0) is refused once wall
/// `up` has replaced it: it would land on the new wall's `vcf1`. The wall
/// is still `shape` after it; Tom then sends for wall `up`.
fn a_turn_for_the_old_wall_is_refused(
    tom: &mut WallClient,
    up: u64,
    shape_now: &(Vec<(String, Vec<f64>)>, String, String),
) {
    tom.set_wall(Some(0));
    assert_eq!(
        code(&tom.turn("vcf1", "cutoff", 500.0, Some(0.0)).unwrap_err()),
        ErrorCode::WallReplaced
    );
    assert_eq!(
        &shape(&tom.look().unwrap()),
        shape_now,
        "nothing was turned"
    );
    tom.set_wall(Some(up));
}

#[test]
fn saves_load_live_over_the_socket_and_a_load_undoes_after_a_restart() {
    let (runtime, state) = dirs();
    let daemon = a_wall_with_a_copy_by_hand(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let mut tom = WallClient::connect(&socket, "Tom", "test", true).unwrap();
    let (mut tom_events, _) = Subscription::open(&socket, "Tom", "test", true).unwrap();
    tom_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut waffles_events, _) = Subscription::open(&socket, "Waffles", "test", false).unwrap();
    waffles_events
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut waffles = WallClient::connect(&socket, "Waffles", "test", false).unwrap();

    // Waffles saves the wall with a knob turned; Tom is told.
    waffles.turn("vcf1", "cutoff", 1_234.0, Some(0.0)).unwrap();
    let jam_saved = waffles.save("waffles jam", false).unwrap();
    assert!(!jam_saved.replaced);
    let jam = shape(&tom.look().unwrap());
    // Tom hears the turn, then the save; Waffles hears its own save too.
    assert!(matches!(next_news(&mut tom_events), Event::Change { .. }));
    let told = Event::Saved {
        seat: "Waffles".to_string(),
        name: "waffles jam".to_string(),
        replaced: false,
    };
    assert_eq!(next_news(&mut tom_events), told);
    assert_eq!(next_news(&mut waffles_events), told);
    only_a_console_destroys_a_save(&socket, &mut waffles);

    // The wall moves on; Waffles loads the jam back while it plays.
    tom.turn("vcf1", "cutoff", 400.0, Some(0.0)).unwrap();
    tom.add("noise", None).unwrap();
    assert!(matches!(
        next_news(&mut waffles_events),
        Event::Change { .. }
    ));
    assert!(matches!(
        next_news(&mut waffles_events),
        Event::Change { .. }
    ));
    let moved_on = shape(&tom.look().unwrap());
    let loaded = waffles.load("waffles jam").unwrap();
    assert!(loaded.backup.starts_with("before-load-"));
    let look = tom.look().unwrap();
    assert_eq!(shape(&look), jam);
    assert_eq!(look.revision, loaded.change.seq);
    assert_eq!(
        look.save.as_ref().map(|mark| mark.name.as_str()),
        Some("waffles jam")
    );
    // Tom hears the change, then that the wall was replaced; Waffles, who
    // loaded it, hears only that.
    let Event::Change { change } = next_news(&mut tom_events) else {
        panic!("Tom did not hear the load");
    };
    assert_eq!(*change, loaded.change);
    assert!(matches!(&change.what, What::Load { .. }));
    assert_eq!(look.wall, loaded.change.seq);
    a_turn_for_the_old_wall_is_refused(&mut tom, look.wall, &jam);
    let replaced = Event::Loaded {
        seq: loaded.change.seq,
        seat: "Waffles".to_string(),
        save: Some("waffles jam".to_string()),
        backup: loaded.backup.clone(),
    };
    assert_eq!(next_news(&mut tom_events), replaced);
    assert_eq!(next_news(&mut waffles_events), replaced);
    // The wall as it was is among the saves, marked as the wall's own.
    let saves = tom.saves().unwrap();
    let backup = saves
        .saves
        .iter()
        .find(|save| save.name == loaded.backup)
        .unwrap();
    assert!(backup.auto);
    drop((waffles, waffles_events, tom_events));
    drop(tom);
    daemon.stop();
    daemon.wait().unwrap();

    // After a restart the load is still in the log, with the wall it
    // replaced: undoing it brings that wall back exactly.
    let daemon = start(runtime.path(), state.path());
    let mut tom = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    assert_eq!(shape(&tom.look().unwrap()), jam);
    let log = tom.log(None, Some(1)).unwrap();
    assert_eq!(
        log.changes[0], loaded.change,
        "seats see it without the wall"
    );
    let undone = tom.undo(loaded.change.seq).unwrap();
    assert_eq!(undone.change.undoes, Some(loaded.change.seq));
    assert_eq!(shape(&tom.look().unwrap()), moved_on);
    // Tom deletes the hand-made copy's save; the copy itself stays.
    tom.delete_save("BY-HAND-2026-09-29").unwrap();
    assert!(state.path().join("patch.by-hand-2026-09-29.json").exists());
    assert_eq!(
        code(&tom.load("by-hand-2026-09-29").unwrap_err()),
        ErrorCode::UnknownSave
    );
    // An empty wall, and back.
    let cleared = tom.new_wall().unwrap();
    assert!(tom.look().unwrap().modules.is_empty());
    tom.undo(cleared.change.seq).unwrap();
    assert_eq!(shape(&tom.look().unwrap()), moved_on);
    drop(tom);
    daemon.stop();
    daemon.wait().unwrap();
}

/// `git -C <dir> <args>` for the tree tests: its output, trimmed.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// The daemon looks for its tree beside its state directory (`~/.kazoo/wall`
/// gives `~/.kazoo/tree`). With none planted there it says so and writes
/// nothing; once one is planted, its start and every change are commits.
#[test]
fn the_daemon_commits_to_a_tree_planted_beside_its_state_and_plants_none() {
    let runtime = tempfile::Builder::new()
        .prefix("kw")
        .tempdir_in("/tmp")
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("wall");
    let tree = root.path().join("tree");

    let daemon = start(runtime.path(), &state);
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    let health = wall.look().unwrap().tree.unwrap();
    assert!(!health.planted, "{health:?}");
    assert_eq!(health.error, None);
    let first = wall.turn("vcf1", "cutoff", 800.0, Some(0.0)).unwrap();
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();
    assert!(!tree.exists(), "the daemon planted a tree");

    kazoo_wall::tree::git::plant(&tree).unwrap();
    let daemon = start(runtime.path(), &state);
    let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
    assert!(wall.look().unwrap().tree.unwrap().planted);
    let second = wall.turn("vcf1", "cutoff", 700.0, Some(0.0)).unwrap();
    drop(wall);
    daemon.stop();
    daemon.wait().unwrap();

    let log = git(
        &tree,
        &[
            "log",
            "--reverse",
            "--format=%an|%s|%(trailers:key=Kazoo-Kind,key=Kazoo-Seq,key=Kazoo-Gap,valueonly,separator=%x2C)",
        ],
    );
    let lines: Vec<_> = log.lines().collect();
    assert_eq!(
        lines,
        [
            format!(
                "kazoo-wall|the tree holds change 1 as one commit|start,{0},1-{0}",
                first.change.seq
            ),
            format!("Tom|{}|change,{}", second.change.summary, second.change.seq),
        ],
        "{log}"
    );
}
