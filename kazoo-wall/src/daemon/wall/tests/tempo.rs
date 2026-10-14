use super::*;

#[test]
fn the_wall_logs_the_desks_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, mut stream)) = a_desk(dir.path(), 120.0);
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    let mut wall = Wall::new(patch, control, Some(link), Vec::new(), None);
    let tree = dir.path().join("tree");
    crate::tree::git::plant(&tree).unwrap();
    wall.find_tree(&tree);
    while wall.snapshot().tree.unwrap().through != Some(0) {
        wall.wait_for_tree();
    }
    let before = commits(&tree).len();
    wall.tick(Instant::now());
    sync(&mut stream, 96.0);
    hear(&wall, &mut engine, 96.0);
    let now = Instant::now();
    assert!(wall.tick(now).is_empty());
    let events = wall.tick(now + Duration::from_secs(2));
    let changes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Change { change } => Some(change),
            _ => None,
        })
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].seat, "desk");
    assert_eq!(
        changes[0].what,
        What::Tempo {
            from: 120.0,
            to: 96.0,
            desk: true
        }
    );
    assert_eq!(changes[0].summary, "the desk moved the tempo to 96 BPM");
    assert!(wall.tick(Instant::now()).is_empty());
    while wall.snapshot().tree.unwrap().through != Some(1) {
        wall.wait_for_tree();
    }
    let after = commits(&tree);
    assert_eq!(after.len(), before + 1);
    assert!(
        after
            .first()
            .unwrap()
            .contains("the desk moved the tempo to 96 BPM")
    );
    let output = crate::tree::git::git_command("show", Some(&tree))
        .arg("HEAD:wall.toml")
        .output()
        .unwrap();
    assert!(output.status.success());
    let saved: toml::Value = toml::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert_eq!(
        saved["tempo"].as_float().unwrap().to_bits(),
        96.0_f64.to_bits()
    );
}

#[test]
fn a_desk_drag_is_one_change_with_the_first_before_and_last_after() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 120.0;
    let now = Instant::now();
    for step in 1_u32..=30 {
        let bpm = (f64::from(step) / 30.0).mul_add(-20.0, 120.0);
        wall.sync_desk_tempo_at(bpm, now + Duration::from_millis(u64::from(step) * 50));
        assert!(wall.log.is_empty(), "a drag was committed before it ended");
    }
    wall.settle_desk_tempo(now + Duration::from_millis(3_500));
    assert_eq!(wall.log.len(), 1);
    assert_eq!(wall.log[0].seat, "desk");
    assert_eq!(
        wall.log[0].what,
        What::Tempo {
            from: 120.0,
            to: 100.0,
            desk: true
        }
    );
}

#[test]
fn a_desk_move_of_less_than_point_zero_five_is_not_logged() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 120.0;
    wall.sync_desk_tempo(120.02);
    wall.tick(Instant::now() + Duration::from_secs(2));
    assert!(wall.log.is_empty());
    assert_eq!(wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
}

#[test]
fn a_linked_go_asks_at_its_landing_bar_and_reports_a_refusal() {
    linked_go(false);
    linked_go(true);
}

fn go_wall(dir: &std::path::Path) -> (Wall, Engine, std::os::unix::net::UnixStream, String) {
    let (link, (audio, stream)) = a_desk(dir, 120.0);
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 3.4), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 96.0;
    let mut wall = Wall::new(
        patch,
        control,
        Some(link),
        Vec::new(),
        Some(Store::peek(dir).unwrap()),
    );
    let tree = dir.join("tree");
    crate::tree::git::plant(&tree).unwrap();
    wall.find_tree(&tree);
    while wall.snapshot().tree.unwrap().through != Some(0) {
        wall.wait_for_tree();
    }
    let output = crate::tree::git::git_command("rev-parse", Some(&tree))
        .arg("HEAD")
        .output()
        .unwrap();
    assert!(output.status.success());
    let moment = String::from_utf8(output.stdout).unwrap().trim().to_string();
    (wall, engine, stream, moment)
}

fn linked_go(accept: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, mut engine, mut stream, moment) = go_wall(dir.path());
    wall.request("Tom", &turn("vcf1", "cutoff", 1000.0), Instant::now())
        .unwrap();
    let now = Instant::now();
    let ticket = wall
        .request("Tom", &Request::Go { moment }, now)
        .unwrap()
        .pending
        .unwrap();
    while wall.swaps_sent == 0 {
        wall.tree_test_wait();
        let answers = wall.tree_answers(now);
        assert!(
            answers.is_empty(),
            "the go answered before the desk was asked: {answers:?}"
        );
    }
    assert_eq!(wall.engine.shared().unhushed(), 0);
    let desk_changes = wall
        .log
        .iter()
        .filter(|change| change.seat == "desk")
        .count();
    let mut reader = stream.try_clone().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let reading = std::thread::spawn(move || {
        use kazoo_core::ipc::protocol::FrameBuffer;
        use kazoo_core::ipc::types::{MSG_TRANSPORT_REQUEST, TransportRequestMsg};
        let mut frame = FrameBuffer::new();
        loop {
            let header = frame.read_frame(&mut reader).unwrap();
            if header.msg_type == MSG_TRANSPORT_REQUEST {
                let request = TransportRequestMsg::decode(frame.payload());
                sent.send(request.requested_bpm).unwrap();
                break;
            }
        }
    });
    assert!(matches!(
        received.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| land(&wall, &mut engine));
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert_eq!(
        wall.engine.shared().desk_tempo_reply(wall.patch.last_seq),
        Some(Ok(()))
    );
    assert_eq!(received.recv().unwrap().to_bits(), 96.0_f32.to_bits());
    reading.join().unwrap();
    assert_eq!(wall.engine.shared().origin().to_bits(), 4.0_f64.to_bits());
    let bpm = if accept { 96.0 } else { 120.0 };
    sync(&mut stream, bpm);
    if accept {
        hear(&wall, &mut engine, f64::from(bpm));
    }
    let first = wall.tree_answers(now);
    let answers = if accept {
        first
    } else {
        assert!(first.is_empty());
        wall.tree_answers(now + Duration::from_secs(1))
    };
    let result = answers
        .into_iter()
        .find(|(id, _)| *id == ticket)
        .unwrap()
        .1
        .unwrap();
    let loaded: LoadResult = serde_json::from_value(result.result).unwrap();
    if accept {
        assert!(
            !loaded
                .notes
                .iter()
                .any(|note| note.contains("the desk kept"))
        );
        assert_eq!(
            wall.log
                .iter()
                .filter(|change| change.seat == "desk")
                .count(),
            desk_changes
        );
        assert_eq!(wall.patch.tempo.to_bits(), 96.0_f64.to_bits());
    } else {
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note == "the desk kept 120 BPM; this moment was made at 96 BPM"),
            "{:?}",
            loaded.notes
        );
        assert_eq!(wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
    }
    wall.close(now);
}

fn commits(tree: &std::path::Path) -> Vec<String> {
    let output = crate::tree::git::git_command("log", Some(tree))
        .args(["--format=%s"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_wall_asked_tempo_is_logged_once() {
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::MSG_TRANSPORT_REQUEST;
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, mut stream)) = a_desk(dir.path(), 120.0);
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    let mut wall = Wall::new(patch, control, Some(link), Vec::new(), None);
    wall.tick(Instant::now());
    let change = wall
        .request("Tom", &Request::Tempo { bpm: 110.0 }, Instant::now())
        .unwrap()
        .change
        .unwrap();
    assert_eq!(change.summary, "Tom asked the desk for 110 BPM");
    let mut frame = FrameBuffer::new();
    assert_eq!(
        frame.read_frame(&mut stream).unwrap().msg_type,
        MSG_TRANSPORT_REQUEST
    );
    sync(&mut stream, 110.0);
    hear(&wall, &mut engine, 110.0);
    let events = wall.tick(Instant::now());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Change { .. }))
    );
    assert_eq!(wall.log.len(), 1);
    assert_eq!(wall.patch.tempo.to_bits(), 110.0_f64.to_bits());
}

#[test]
fn a_first_sync_and_a_restarted_desk_are_logged() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 96.0;
    wall.sync_desk_tempo(100.0);
    wall.finish_desk_tempo();
    assert_eq!(wall.log.len(), 1);
    wall.desk_tempo.disconnected();
    wall.sync_desk_tempo(120.0);
    wall.finish_desk_tempo();
    assert_eq!(wall.log.len(), 2);
    assert_eq!(
        wall.log.back().unwrap().what,
        What::Tempo {
            from: 100.0,
            to: 120.0,
            desk: true
        }
    );
    wall.sync_desk_tempo(120.005);
    assert_eq!(wall.log.len(), 2);
    assert_eq!(wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
    wall.sync_desk_tempo(120.02);
    wall.finish_desk_tempo();
    assert_eq!(wall.log.len(), 2);
    wall.sync_desk_tempo(120.06);
    wall.finish_desk_tempo();
    assert_eq!(wall.log.len(), 3);
}

fn sync(stream: &mut std::os::unix::net::UnixStream, bpm: f32) {
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{
        MSG_TRANSPORT_SYNC, SYNC_NOW, TRANSPORT_STOPPED, TransportSyncMsg,
    };
    let mut frame = FrameBuffer::new();
    let message = TransportSyncMsg {
        state: TRANSPORT_STOPPED,
        bpm,
        at_frame: SYNC_NOW,
        beat: 0.0,
    };
    let bytes = message.encode(frame.payload_mut());
    frame
        .write_frame(MSG_TRANSPORT_SYNC, 0, bytes, stream)
        .unwrap();
}

fn hear(wall: &Wall, engine: &mut Engine, bpm: f64) {
    while wall.engine.shared().bpm().to_bits() != bpm.to_bits() {
        engine.render(&mut [0.0; 512], 2);
        std::thread::yield_now();
    }
}

#[test]
fn a_desk_change_before_the_asked_tick_starts_from_the_logged_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, _stream)) = a_desk(dir.path(), 120.0);
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    let mut wall = Wall::new(patch, control, Some(link), Vec::new(), None);
    wall.sync_desk_tempo(120.0);
    wall.request("Tom", &Request::Tempo { bpm: 100.0 }, Instant::now())
        .unwrap();
    wall.sync_desk_tempo(105.0);
    wall.finish_desk_tempo();
    assert_eq!(wall.log.len(), 2);
    assert_eq!(
        wall.log.back().unwrap().what,
        What::Tempo {
            from: 100.0,
            to: 105.0,
            desk: true
        }
    );
}

#[test]
fn the_desk_race_log_replays_through_rings_without_a_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, _stream)) = a_desk(dir.path(), 120.0);
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    let initial = patch.to_file();
    let mut wall = Wall::new(patch, control, Some(link), Vec::new(), None);
    wall.sync_desk_tempo(120.0);
    wall.request("Tom", &Request::Tempo { bpm: 100.0 }, Instant::now())
        .unwrap();
    wall.sync_desk_tempo(105.0);
    wall.finish_desk_tempo();
    let state = dir.path().join("state");
    std::fs::create_dir_all(state.join("saves")).unwrap();
    std::fs::write(
        state.join("patch.before-1.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    std::fs::write(
        state.join("patch.json"),
        serde_json::to_vec(&wall.patch.to_file()).unwrap(),
    )
    .unwrap();
    let log = wall
        .log
        .iter()
        .map(|change| serde_json::to_string(change).unwrap() + "\n")
        .collect::<String>();
    std::fs::write(state.join("log.jsonl"), log).unwrap();
    let result =
        crate::tree::rings::grow(&state, &dir.path().join("replayed-tree"), false).unwrap();
    assert_eq!(result.gaps, 0, "{}", result.report);
    assert_eq!(result.settles, 0, "{}", result.report);
    assert_eq!(result.changes, 2, "{}", result.report);
}

#[test]
fn the_patch_tempo_waits_for_the_desk_change_to_be_logged() {
    let (mut wall, _engine, _dir) = saving_wall();
    wall.patch.tempo = 120.0;
    fail_once(&mut wall, FailAt::AppendLog);
    wall.sync_desk_tempo(100.0);
    wall.finish_desk_tempo();
    assert_eq!(wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
    assert_eq!(wall.owed.len(), 1);
    wall.pay_owed().unwrap();
    assert_eq!(wall.patch.tempo.to_bits(), 100.0_f64.to_bits());
}

#[test]
fn a_matched_desk_report_does_not_rewind_the_latest_logged_ask() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 110.0;
    wall.desk_tempo.asked(100.0);
    wall.desk_tempo.asked(110.0);
    wall.sync_desk_tempo(100.0);
    wall.sync_desk_tempo(105.0);
    wall.finish_desk_tempo();
    assert_eq!(
        wall.log.back().unwrap().what,
        What::Tempo {
            from: 110.0,
            to: 105.0,
            desk: true
        }
    );
}

#[test]
fn a_matched_report_with_rounding_does_not_change_the_patch() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 100.0;
    wall.desk_tempo.asked(100.0);
    wall.sync_desk_tempo(100.005);
    assert_eq!(wall.patch.tempo.to_bits(), 100.0_f64.to_bits());
    assert!(wall.log.is_empty());
}

#[test]
fn nonfinite_desk_reports_are_refused_before_clamping() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 96.0;
    for bpm in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        wall.sync_desk_tempo(bpm);
        assert_eq!(wall.patch.tempo.to_bits(), 96.0_f64.to_bits());
        assert!(wall.log.is_empty());
    }
}

#[test]
fn a_subthreshold_first_sync_keeps_the_next_log_continuous() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 120.0;
    wall.sync_desk_tempo(120.005);
    assert!(wall.log.is_empty());
    wall.sync_desk_tempo(125.0);
    wall.finish_desk_tempo();
    assert_eq!(
        wall.log.back().unwrap().what,
        What::Tempo {
            from: 120.0,
            to: 125.0,
            desk: true
        }
    );
}

#[test]
fn a_tick_before_the_desk_hears_an_ask_logs_nothing() {
    // The desk takes a few blocks to apply an ask and report it back; a
    // tick in between still hears the old tempo, which is not a desk move.
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, mut stream)) = a_desk(dir.path(), 120.0);
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    let mut wall = Wall::new(patch, control, Some(link), Vec::new(), None);
    wall.request("Tom", &Request::Tempo { bpm: 100.0 }, Instant::now())
        .unwrap();
    for _ in 0..3 {
        let events = wall.tick(Instant::now());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Change { .. })),
            "{:?}",
            wall.log
        );
    }
    sync(&mut stream, 100.0);
    hear(&wall, &mut engine, 100.0);
    let events = wall.tick(Instant::now());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Change { .. })),
        "{:?}",
        wall.log
    );
    assert_eq!(wall.log.len(), 1, "{:?}", wall.log);
    assert_eq!(wall.patch.tempo.to_bits(), 100.0_f64.to_bits());
}

/// What the wall's modules last read as the song position.
#[derive(Debug)]
struct BeatSeen(std::sync::Arc<std::sync::atomic::AtomicU64>);

impl DspModule for BeatSeen {
    fn process(&mut self, tick: &Tick, _io: Io<'_>) {
        self.0.store(tick.beat.to_bits(), Ordering::Relaxed);
    }

    fn reset(&mut self) {}
}

fn desk_plays_from(stream: &mut std::os::unix::net::UnixStream, beat: f64) {
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{
        MSG_TRANSPORT_SYNC, SYNC_NOW, TRANSPORT_PLAYING, TransportSyncMsg,
    };
    let mut frame = FrameBuffer::new();
    let message = TransportSyncMsg {
        state: TRANSPORT_PLAYING,
        bpm: 120.0,
        at_frame: SYNC_NOW,
        beat,
    };
    let bytes = message.encode(frame.payload_mut());
    frame
        .write_frame(MSG_TRANSPORT_SYNC, 0, bytes, stream)
        .unwrap();
}

/// Render until the engine's song position is past `beat`.
fn until_past(engine: &mut Engine, shared: &crate::engine::Shared, beat: f64) {
    loop {
        if shared.beat() >= beat {
            break;
        }
        engine.render(&mut [0.0; 64], 2);
        std::thread::yield_now();
    }
}

#[test]
fn a_desk_jump_moves_a_held_go_to_the_bar_after_it_and_keeps_the_origin() {
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, mut stream)) = a_desk(dir.path(), 120.0);
    // The desk hears the wall's audio, so the link never backs up.
    let mut heard = stream.try_clone().unwrap();
    let hearing = std::thread::spawn(move || {
        let mut sink = vec![0_u8; 1 << 16];
        while matches!(std::io::Read::read(&mut heard, &mut sink), Ok(read) if read > 0) {}
    });
    let (mut engine, mut control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let shared = std::sync::Arc::clone(control.shared());
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(f64::NAN.to_bits()));
    control.begin_swap();
    control.send(Command::Hush);
    control.send(Command::Insert {
        slot: 0,
        tag: 1,
        kind: Kind::CLOCK,
        module: Box::new(BeatSeen(std::sync::Arc::clone(&seen))),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Order(Box::new(crate::engine::Order::new(
        &[0],
        &[],
    ))));
    control.send(Command::Unhush);
    control.end_swap();
    assert_eq!(control.landing_beat().to_bits(), 4.0_f64.to_bits());
    // The desk starts playing from beat 30.5, past the bar the go waited
    // for: it lands on the next bar of the desk's song, beat 32.
    desk_plays_from(&mut stream, 30.5);
    until_past(&mut engine, &shared, 30.0);
    assert_eq!(
        shared.unhushed(),
        0,
        "landed off the bar, as the desk jumped"
    );
    until_past(&mut engine, &shared, 32.0);
    // The sub-block that starts on beat 32 takes the go in.
    engine.render(&mut [0.0; 2 * 2 * SUB_BLOCK], 2);
    assert_eq!(shared.unhushed(), 1);
    assert_eq!(shared.origin().to_bits(), 32.0_f64.to_bits());
    // The desk moves on to beat 40: the modules read it from the origin.
    desk_plays_from(&mut stream, 40.0);
    until_past(&mut engine, &shared, 39.0);
    engine.render(&mut [0.0; 64], 2);
    let read = f64::from_bits(seen.load(Ordering::Relaxed));
    assert!((8.0..8.01).contains(&read), "the modules read beat {read}");
    assert_eq!(shared.origin().to_bits(), 32.0_f64.to_bits());
    // The wall leaving closes the link: the desk hears the end.
    drop((engine, control, link));
    hearing.join().unwrap();
}

#[test]
fn a_batch_tempo_on_the_desk_starts_from_what_plays_as_a_single_tempo_does() {
    let dir = tempfile::tempdir().unwrap();
    let (link, (audio, mut stream)) = a_desk(dir.path(), 120.0);
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    let mut wall = Wall::new(patch, control, Some(link), Vec::new(), None);
    wall.tick(Instant::now());
    // The desk moves; the wall's tick has not heard it yet.
    sync(&mut stream, 105.0);
    hear(&wall, &mut engine, 105.0);
    let request: Request = serde_json::from_value(serde_json::json!({
        "op": "batch",
        "note": "faster",
        "changes": [{"op": "tempo", "bpm": 100.0}]
    }))
    .unwrap();
    wall.request("Tom", &request, Instant::now()).unwrap();
    let logged: Vec<What> = wall.log.iter().map(|change| change.what.clone()).collect();
    assert_eq!(
        logged,
        vec![
            What::Tempo {
                from: 120.0,
                to: 105.0,
                desk: true
            },
            What::Batch {
                note: "faster".to_string(),
                changes: vec![What::Tempo {
                    from: 105.0,
                    to: 100.0,
                    desk: true
                }],
            },
        ]
    );
}
