use super::*;
use crate::host::link::{drain_voice, take_events};
use crate::ipc::link::voice::rings;
use crate::ipc::types::{
    FEATURE_VOICE, FeaturesMsg, MSG_DESK_FEATURES, MSG_VOICE, MSG_VOICE_FOLLOW, VoiceFollowMsg,
};
use crate::ipc::voice::{VOICE_BATCH_MAX, decode_batch, encode_batch};
use crate::voice::VoiceFrame;

struct Rig {
    link: HubLink,
    audio: HubLinkAudio,
    worker: LinkWorker,
    directory: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let logs = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/kz-logs");
        std::fs::create_dir_all(&logs).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("v")
            .tempdir_in(logs)
            .unwrap();
        let mut config = config(&directory.path().join("s"));
        config.real_time = false;
        let (mut link, mut audio) = start_link(config.clone(), None, false).unwrap();
        let (wire, receiver) = HeapRb::new(128 * 1024).split();
        audio.wire = wire;
        let (messages, receiver_messages) = HeapRb::new(MESSAGE_BACKLOG).split();
        audio.events = crate::host::link::EventRx::new(receiver_messages);
        let (requests, ui_requests) = mpsc::sync_channel(REQUEST_BACKLOG);
        link.requests = requests;
        let (requests, callback_requests) = HeapRb::new(REQUEST_BACKLOG).split();
        audio.requests = requests;
        let (out, incoming, voice) = rings();
        audio.voice_out = out;
        audio.voice = incoming;
        let worker = LinkWorker {
            config,
            state: Arc::clone(&link.state),
            wire: receiver,
            audio_frame_left: 0,
            messages,
            callback_requests,
            ui_requests,
            client: None,
            stuck_since: None,
            refusal_delay: RETRY_INTERVAL,
            health_before: DeskHealthMsg::default(),
            health_now: DeskHealthMsg::default(),
            voice,
        };
        Self {
            link,
            audio,
            worker,
            directory,
        }
    }

    fn connect(&mut self, features: Option<u32>) -> (UnixStream, FrameBuffer) {
        let path = self.directory.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let hub = thread::spawn(move || accept_features(&listener, features));
        self.worker.try_connect();
        let result = hub.join().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(self.link.is_connected(), "{:?}", self.link.status());
        self.worker.serve().unwrap();
        // Consume registration bookkeeping before testing ordinary events.
        assert!(
            take_events(&mut self.audio.events)
                .any(|event| matches!(event, HubMessage::TransportSync(_)))
        );
        result
    }
}

fn accept_features(listener: &UnixListener, features: Option<u32>) -> (UnixStream, FrameBuffer) {
    let (mut stream, mut buf) = accept(listener);
    if let Some(features) = features {
        announce(&mut stream, &mut buf, features);
    }
    (stream, buf)
}

fn announce(hub: &mut UnixStream, buf: &mut FrameBuffer, features: u32) {
    FeaturesMsg { features }.encode(buf.payload_mut());
    buf.write_frame(MSG_DESK_FEATURES, 0, FeaturesMsg::WIRE_SIZE, hub)
        .unwrap();
}

fn frame(at: u16) -> VoiceFrame {
    VoiceFrame {
        at: u64::from(at),
        hz: 220.0 + f32::from(at),
        volts: 0.5,
        snapped_hz: 440.0 + f32::from(at),
        snapped_volts: 1.0,
        loudness: 0.5,
        breath: 0.25,
        confidence: 0.75,
        gate: true,
        onset: at % 2 == 0,
        voiced: true,
    }
}

fn forward(hub: &mut UnixStream, buf: &mut FrameBuffer, frames: &[VoiceFrame]) {
    let len = encode_batch(frames, buf.payload_mut());
    assert!(len > 0);
    buf.write_frame(MSG_VOICE, 0, len, hub).unwrap();
}

fn read_voice(hub: &mut UnixStream, buf: &mut FrameBuffer) -> Vec<VoiceFrame> {
    let header = buf.read_frame(hub).unwrap();
    assert_eq!(header.msg_type, MSG_VOICE);
    let len = usize::try_from(header.payload_len).unwrap();
    let mut frames = Vec::new();
    let count = decode_batch(&buf.payload()[..len], |f| frames.push(f)).unwrap();
    assert!((1..=VOICE_BATCH_MAX).contains(&count));
    frames
}

fn read_follow(hub: &mut UnixStream, buf: &mut FrameBuffer) -> bool {
    let header = buf.read_frame(hub).unwrap();
    assert_eq!(header.msg_type, MSG_VOICE_FOLLOW);
    assert_eq!(header.payload_len, 1);
    VoiceFollowMsg::decode(buf.payload()).on
}

#[test]
fn voice_waits_for_the_desk_to_say_it_carries_it() {
    let mut rig = Rig::new();
    assert!(!rig.audio.send_voice(&frame(0)));
    let (mut hub, mut buf) = rig.connect(None);
    assert!(!rig.link.desk_carries_voice());
    assert!(!rig.audio.send_voice(&frame(1)));
    assert_eq!(rig.link.voice_stats().dropped, 2);
    assert_eq!(rig.link.follow_voice(true), Err(RequestError::Unsupported));
    assert_eq!(
        RequestError::Unsupported.to_string(),
        "this desk is too old to carry the voice"
    );
    assert!(rig.link.status().voice.follows);
    announce(&mut hub, &mut buf, FEATURE_VOICE);
    rig.worker.serve().unwrap();
    assert!(rig.link.desk_carries_voice());
    assert!(rig.link.status().voice.desk_carries);
    assert!(read_follow(&mut hub, &mut buf));
    assert!(rig.audio.send_voice(&frame(2)));
    rig.worker.serve().unwrap();
    assert_eq!(read_voice(&mut hub, &mut buf), vec![frame(2)]);
    assert_eq!(rig.link.voice_stats().sent, 1);
}

#[test]
fn frames_go_out_in_batches_in_order() {
    let mut rig = Rig::new();
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    for at in 0..100 {
        assert!(rig.audio.send_voice(&frame(at)));
    }
    rig.worker.serve().unwrap();
    let mut received = Vec::new();
    for expected in [32, 32, 32, 4] {
        let frames = read_voice(&mut hub, &mut buf);
        assert_eq!(frames.len(), expected);
        received.extend(frames);
    }
    assert_eq!(received, (0..100).map(frame).collect::<Vec<_>>());
    assert!(received.windows(2).all(|pair| pair[0].at < pair[1].at));
    assert_eq!(
        rig.link.voice_stats(),
        VoiceLinkStats {
            sent: 100,
            ..VoiceLinkStats::default()
        }
    );
}

#[test]
fn the_follow_wish_survives_a_desk_restart() {
    let mut rig = Rig::new();
    assert_eq!(rig.link.follow_voice(true), Ok(()));
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    assert!(read_follow(&mut hub, &mut buf));
    drop(hub);
    let why = rig.worker.serve().unwrap_err();
    rig.worker.disconnect(why);
    assert!(!rig.link.desk_carries_voice());
    assert!(rig.link.status().voice.follows);
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    assert!(read_follow(&mut hub, &mut buf));
    assert_eq!(rig.link.follow_voice(false), Ok(()));
    rig.worker.serve().unwrap();
    assert!(!read_follow(&mut hub, &mut buf));
    assert!(!rig.link.status().voice.follows);
    assert_eq!(rig.link.status().connections, 2);
}

#[test]
fn forwarded_frames_reach_drain_voice_in_order() {
    let mut rig = Rig::new();
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    let frames: Vec<_> = (0..40).map(frame).collect();
    for batch in frames.chunks(16) {
        forward(&mut hub, &mut buf, batch);
    }
    rig.worker.serve().unwrap();
    let mut received = Vec::new();
    drain_voice(&mut rig.audio.voice, |f| received.push(f));
    assert_eq!(received, frames);
    assert_eq!(rig.link.voice_stats().received, 40);
    assert_eq!(rig.link.voice_stats().overflow, 0);
    drain_voice(&mut rig.audio.voice, |_| panic!("the ring was drained"));
}

#[test]
fn a_flood_of_voice_never_crowds_out_notes() {
    let mut rig = Rig::new();
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    let frames: Vec<_> = (0..1_000).map(frame).collect();
    for batch in frames.chunks(VOICE_BATCH_MAX) {
        forward(&mut hub, &mut buf, batch);
        rig.worker.serve().unwrap();
    }
    NoteEventMsg {
        source: [1; 16],
        target: [0; 16],
        event_type: NOTE_ON,
        channel: 0,
        note: 60,
        velocity: 100,
    }
    .encode(buf.payload_mut());
    buf.write_frame(MSG_NOTE_EVENT, 0, NoteEventMsg::WIRE_SIZE, &mut hub)
        .unwrap();
    rig.worker.serve().unwrap();
    let events: Vec<_> = take_events(&mut rig.audio.events).collect();
    assert!(matches!(events.as_slice(), [HubMessage::NoteEvent(note)] if note.note == 60));
    assert_eq!(rig.link.voice_stats().received, 1_000);
    assert_eq!(rig.link.voice_stats().overflow, 744);
    assert_eq!(rig.link.status().messages_dropped, 0);
    let mut received = Vec::new();
    drain_voice(&mut rig.audio.voice, |f| received.push(f));
    assert_eq!(received, frames[..VOICE_RING]);
}

#[test]
fn voice_on_the_callback_never_allocates() {
    let mut rig = Rig::new();
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    let frames: Vec<_> = (0..256).map(frame).collect();
    for batch in frames.chunks(VOICE_BATCH_MAX) {
        forward(&mut hub, &mut buf, batch);
        rig.worker.serve().unwrap();
    }
    let before = assert_no_alloc::violation_count();
    let (sent, drained) = assert_no_alloc::assert_no_alloc(|| {
        let mut sent = 0;
        for at in 0..1_000 {
            sent += usize::from(rig.audio.send_voice(&frame(at)));
        }
        let mut drained = 0;
        drain_voice(&mut rig.audio.voice, |f| {
            assert_eq!(f.at, drained);
            drained += 1;
        });
        (sent, drained)
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert_eq!((sent, drained), (256, 256));
    assert_eq!(rig.link.voice_stats().dropped, 744);
}

#[test]
fn a_disconnected_voice_backlog_is_counted_and_never_replayed() {
    let mut rig = Rig::new();
    let (hub, buf) = rig.connect(Some(FEATURE_VOICE));
    for at in 0..100 {
        assert!(rig.audio.send_voice(&frame(at)));
    }
    assert_eq!(rig.link.follow_voice(true), Ok(()));
    drop(hub);
    drop(buf);
    rig.worker.disconnect("desk restarted".to_string());
    assert_eq!(rig.link.voice_stats().dropped, 100);
    assert_eq!(rig.link.voice_stats().sent, 0);
    let (mut hub, mut buf) = rig.connect(None);
    rig.worker.serve().unwrap();
    hub.set_nonblocking(true).unwrap();
    assert!(buf.try_read_frame(&mut hub).unwrap().is_none());
    assert!(rig.link.status().voice.follows);
    assert!(!rig.link.status().voice.desk_carries);
}

#[test]
fn a_malformed_voice_batch_is_a_link_protocol_fault() {
    let mut rig = Rig::new();
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    buf.payload_mut()[..4].fill(0);
    buf.write_frame(MSG_VOICE, 0, 4, &mut hub).unwrap();
    let why = rig.worker.serve().unwrap_err();
    assert!(why.contains("malformed voice batch"), "{why}");
    rig.worker.disconnect(why);
    assert!(!rig.link.is_connected());
    assert_eq!(rig.link.voice_stats().received, 0);
    drain_voice(&mut rig.audio.voice, |_| {
        panic!("a malformed batch reached the callback")
    });
}

#[test]
fn a_full_follow_queue_keeps_the_wish_for_reconnection() {
    let mut rig = Rig::new();
    let (hub, buf) = rig.connect(Some(FEATURE_VOICE));
    for _ in 0..REQUEST_BACKLOG {
        rig.link.request_tempo(120.0).unwrap();
    }
    assert_eq!(rig.link.follow_voice(true), Err(RequestError::Full));
    assert!(rig.link.status().voice.follows);
    drop(hub);
    drop(buf);
    rig.worker.disconnect("desk restarted".to_string());
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    assert!(read_follow(&mut hub, &mut buf));
}

#[test]
fn voice_batches_are_sent_without_serving_allocations() {
    let mut rig = Rig::new();
    let (mut hub, mut buf) = rig.connect(Some(FEATURE_VOICE));
    for at in 0..100 {
        assert!(rig.audio.send_voice(&frame(at)));
    }
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| rig.worker.serve().unwrap());
    assert_eq!(assert_no_alloc::violation_count(), before);
    let mut count = 0;
    for _ in 0..4 {
        count += read_voice(&mut hub, &mut buf).len();
    }
    assert_eq!(count, 100);
}
