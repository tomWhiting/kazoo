use super::*;
use kazoo_core::ipc::protocol::FrameBuffer;
use kazoo_core::ipc::types::{MSG_TRANSPORT_REQUEST, TransportRequestMsg};
use std::os::unix::net::UnixStream;

fn linked(dir: &std::path::Path) -> (Wall, Engine, UnixStream) {
    let (link, (audio, stream)) = a_desk(dir, 120.0);
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), Some(audio));
    let mut patch = seed().unwrap();
    patch.tempo = 120.0;
    (
        Wall::new(patch, control, Some(link), Vec::new(), None),
        engine,
        stream,
    )
}

fn tempo(bpm: f64, after_beats: f64, batch: bool) -> Request {
    let step = serde_json::json!({"op":"tempo", "bpm":bpm, "after_beats":after_beats});
    serde_json::from_value(if batch {
        serde_json::json!({"op":"batch", "note":"timed tempo", "changes":[step]})
    } else {
        step
    })
    .unwrap()
}

fn requests(wall: &Wall, stream: &mut UnixStream) -> Vec<f32> {
    // The marker uses the same ordered queue, so reading it proves every
    // request submitted before it has reached the desk.
    wall.desk.as_ref().unwrap().request_tempo(299.0).unwrap();
    let mut frame = FrameBuffer::new();
    let mut bpms = Vec::new();
    loop {
        if frame.read_frame(stream).unwrap().msg_type == MSG_TRANSPORT_REQUEST {
            let bpm = TransportRequestMsg::decode(frame.payload()).requested_bpm;
            if bpm.to_bits() == 299.0_f32.to_bits() {
                return bpms;
            }
            bpms.push(bpm);
        }
    }
}

#[test]
fn delayed_desk_tempo_waits_for_the_control_tick() {
    for batch in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut wall, _engine, mut stream) = linked(dir.path());
        let now = Instant::now();
        let outcome = wall.request("Tom", &tempo(150.0, 4.0, batch), now).unwrap();
        assert!(outcome.change.is_some());
        assert_eq!(wall.patch.tempo.to_bits(), 150.0_f64.to_bits());
        assert!(
            requests(&wall, &mut stream).is_empty(),
            "asked before its wait"
        );
        wall.tick(now + Duration::from_millis(1_999));
        assert!(requests(&wall, &mut stream).is_empty(), "asked before due");
        wall.tick(now + Duration::from_secs(2));
        assert_eq!(requests(&wall, &mut stream), vec![150.0]);
    }
}

#[test]
fn delayed_desk_tempo_uses_the_acceptance_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine, mut stream) = linked(dir.path());
    let request = serde_json::from_value(
        serde_json::json!({"op":"batch", "note":"two tempos", "changes":[
            {"op":"tempo", "bpm":240}, {"op":"tempo", "bpm":150, "after_beats":4}
        ]}),
    )
    .unwrap();
    let now = Instant::now();
    wall.request("Tom", &request, now).unwrap();
    assert!(requests(&wall, &mut stream).is_empty());
    wall.tick(now + Duration::from_secs(1));
    assert!(requests(&wall, &mut stream).is_empty());
    wall.tick(now + Duration::from_secs(2));
    assert_eq!(requests(&wall, &mut stream), vec![150.0]);
}

#[test]
fn later_desk_tempo_replaces_a_pending_request() {
    for batch in [false, true] {
        for after in [0.0, 8.0] {
            let dir = tempfile::tempdir().unwrap();
            let (mut wall, _engine, mut stream) = linked(dir.path());
            let now = Instant::now();
            wall.request("Tom", &tempo(150.0, 4.0, !batch), now)
                .unwrap();
            assert!(requests(&wall, &mut stream).is_empty());
            wall.request(
                "Tom",
                &tempo(180.0, after, batch),
                now + Duration::from_secs(1),
            )
            .unwrap();
            let immediate = requests(&wall, &mut stream);
            assert_eq!(immediate, if after == 0.0 { vec![180.0] } else { vec![] });
            wall.tick(now + Duration::from_secs(2));
            assert!(
                requests(&wall, &mut stream).is_empty(),
                "replaced tempo sent"
            );
            wall.tick(now + Duration::from_secs(5));
            assert_eq!(
                requests(&wall, &mut stream),
                if after == 0.0 { vec![] } else { vec![180.0] }
            );
        }
    }
}

#[test]
fn undo_drops_its_pending_desk_tempo() {
    for batch in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut wall, _engine, mut stream) = linked(dir.path());
        let now = Instant::now();
        let seq = wall
            .request("Tom", &tempo(150.0, 4.0, batch), now)
            .unwrap()
            .change
            .unwrap()
            .seq;
        assert!(requests(&wall, &mut stream).is_empty());
        wall.request(
            "Tom",
            &Request::Undo { change: seq },
            now + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(requests(&wall, &mut stream), vec![120.0]);
        wall.tick(now + Duration::from_secs(10));
        assert!(requests(&wall, &mut stream).is_empty());
    }
}

#[test]
fn an_unrelated_undo_keeps_the_pending_desk_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine, mut stream) = linked(dir.path());
    let now = Instant::now();
    let seq = wall
        .request("Tom", &turn("vcf1", "cutoff", 500.0), now)
        .unwrap()
        .change
        .unwrap()
        .seq;
    wall.request("Tom", &tempo(150.0, 4.0, true), now).unwrap();
    assert!(requests(&wall, &mut stream).is_empty());
    wall.request("Tom", &Request::Undo { change: seq }, now)
        .unwrap();
    wall.tick(now + Duration::from_secs(2));
    assert_eq!(requests(&wall, &mut stream), vec![150.0]);
}

#[test]
fn delayed_desk_tempo_is_never_sent_twice() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine, mut stream) = linked(dir.path());
    let now = Instant::now();
    wall.request("Tom", &tempo(150.0, 4.0, false), now).unwrap();
    drop(requests(&wall, &mut stream));
    wall.tick(now + Duration::from_secs(3));
    assert_eq!(requests(&wall, &mut stream), vec![150.0]);
    for seconds in [3, 4, 100] {
        wall.tick(now + Duration::from_secs(seconds));
        assert!(requests(&wall, &mut stream).is_empty());
    }
}

#[test]
fn an_unlogged_later_tempo_keeps_the_pending_desk_tempo() {
    for batch in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut wall, _engine, mut stream) = linked(dir.path());
        wall.store = Some(Store::peek(dir.path()).unwrap());
        let now = Instant::now();
        wall.request("Tom", &tempo(150.0, 4.0, !batch), now)
            .unwrap();
        assert!(requests(&wall, &mut stream).is_empty());
        fail_once(&mut wall, FailAt::AppendLog);
        let refused = wall
            .request("Tom", &tempo(180.0, 0.0, batch), now)
            .unwrap_err();
        assert_eq!(refused.code, ErrorCode::Internal);
        assert_eq!(wall.patch.tempo.to_bits(), 150.0_f64.to_bits());
        assert!(requests(&wall, &mut stream).is_empty());
        wall.tick(now + Duration::from_secs(2));
        assert_eq!(requests(&wall, &mut stream), vec![150.0]);
    }
}

#[test]
fn unchanged_desk_reports_keep_the_pending_desk_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine, mut stream) = linked(dir.path());
    let now = Instant::now();
    wall.request("Tom", &tempo(150.0, 4.0, true), now).unwrap();
    wall.sync_desk_tempo_at(120.0, now + Duration::from_secs(1));
    wall.finish_desk_tempo();
    wall.tick(now + Duration::from_secs(1));
    assert!(requests(&wall, &mut stream).is_empty());
    assert_eq!(wall.log.len(), 1);
    wall.tick(now + Duration::from_secs(2));
    assert_eq!(requests(&wall, &mut stream), vec![150.0]);
}

#[test]
fn a_scheduled_desk_tempo_refusal_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let (mut wall, _engine, mut stream) = linked(dir.path());
    let now = Instant::now();
    wall.request("Tom", &tempo(150.0, 4.0, false), now).unwrap();
    assert!(requests(&wall, &mut stream).is_empty());
    drop(wall.desk.take());
    let events = wall.tick(now + Duration::from_secs(2));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Notice { summary }
        if summary.contains("scheduled tempo") && summary.contains("NotConnected")))
    );
    let events = wall.tick(now + Duration::from_secs(3));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Notice { summary }
        if summary.contains("scheduled tempo")))
    );
}
