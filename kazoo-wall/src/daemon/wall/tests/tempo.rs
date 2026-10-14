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
    sync(&mut stream, 100.0);
    hear(&wall, &mut engine, 100.0);
    let events = wall.tick(Instant::now());
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
            to: 100.0,
            desk: true
        }
    );
    assert_eq!(
        changes[0].summary,
        "the desk set the tempo 120 BPM → 100 BPM"
    );
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
            .contains("the desk set the tempo 120 BPM → 100 BPM")
    );
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
        .request("Tom", &Request::Tempo { bpm: 100.0 }, Instant::now())
        .unwrap()
        .change
        .unwrap();
    assert_eq!(change.summary, "Tom asked the desk for 100 BPM");
    let mut frame = FrameBuffer::new();
    assert_eq!(
        frame.read_frame(&mut stream).unwrap().msg_type,
        MSG_TRANSPORT_REQUEST
    );
    sync(&mut stream, 100.0);
    hear(&wall, &mut engine, 100.0);
    let events = wall.tick(Instant::now());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Change { .. }))
    );
    assert_eq!(wall.log.len(), 1);
    assert_eq!(wall.patch.tempo.to_bits(), 100.0_f64.to_bits());
}

#[test]
fn a_first_sync_and_a_restarted_desk_are_logged() {
    let (mut wall, _engine) = wall();
    wall.patch.tempo = 96.0;
    wall.sync_desk_tempo(100.0);
    assert_eq!(wall.log.len(), 1);
    wall.desk_tempo.disconnected();
    wall.sync_desk_tempo(120.0);
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
    wall.sync_desk_tempo(120.02);
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
