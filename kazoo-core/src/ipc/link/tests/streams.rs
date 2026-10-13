use super::*;
use crate::ipc::protocol::{AUDIO_STREAM_PAYLOAD_HEADER, decode_audio_stream_id};
use crate::ipc::types::*;

struct Rig {
    link: HubLink,
    audio: HubLinkAudio,
    worker: LinkWorker,
    path: PathBuf,
}

impl Rig {
    fn new(streams: Vec<StreamConfig>) -> Self {
        let path = socket();
        let mut config = config(&path);
        config.streams = streams;
        config.real_time = false;
        let (mut link, mut audio) = start_link(config.clone(), None, false).unwrap();
        let (wire_tx, wire_rx) = HeapRb::new(128 * 1024).split();
        audio.wire = wire_tx;
        let (messages, receiver) = HeapRb::new(MESSAGE_BACKLOG).split();
        audio.events = crate::host::link::EventRx::new(receiver);
        audio.stream_updates = Arc::clone(&audio.events.stream_updates);
        for (id, channels) in audio.stream_channels.iter().enumerate() {
            audio.stream_updates[id].store(u64::from(channels.unwrap_or(0)), Ordering::Release);
        }
        let (requests, ui_requests) = mpsc::sync_channel(REQUEST_BACKLOG);
        link.requests = requests;
        let (callback, callback_requests) = HeapRb::new(REQUEST_BACKLOG).split();
        audio.requests = callback;
        let worker = LinkWorker {
            config,
            state: Arc::clone(&link.state),
            wire: wire_rx,
            audio_frame_left: 0,
            messages,
            callback_requests,
            ui_requests,
            client: None,
            stuck_since: None,
            refusal_delay: RETRY_INTERVAL,
            health_before: DeskHealthMsg::default(),
            health_now: DeskHealthMsg::default(),
        };
        Self {
            link,
            audio,
            worker,
            path,
        }
    }

    fn connect(&mut self, strips: Vec<u8>) -> (UnixStream, FrameBuffer, RegisterMsg) {
        let listener = UnixListener::bind(&self.path).unwrap();
        let hub = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = FrameBuffer::new();
            let header = buf.read_frame(&mut stream).unwrap();
            let register = RegisterMsg::decode(&buf.payload()[..header.payload_len as usize]);
            let reply = RegisteredMsg {
                strip_index: strips[0],
                hub_sample_rate: 48_000,
                hub_buffer_size: 256,
                transport_state: 0,
                bpm: 120.0,
                position: 0,
                strips,
            };
            let len = reply.encode(buf.payload_mut());
            buf.write_frame(MSG_REGISTERED, 0, len, &mut stream)
                .unwrap();
            (stream, buf, register)
        });
        self.worker.try_connect();
        assert!(self.link.is_connected(), "{:?}", self.link.status());
        let result = hub.join().unwrap();
        std::fs::remove_file(&self.path).unwrap();
        result
    }

    fn ready(&mut self, stream: &mut UnixStream, buf: &mut FrameBuffer, strip: u8, reason: &str) {
        StreamReadyMsg { stream: 1, strip }.encode(buf.payload_mut());
        buf.payload_mut()[3..3 + reason.len()].copy_from_slice(reason.as_bytes());
        buf.write_frame(MSG_STREAM_READY, 0, 3 + reason.len(), stream)
            .unwrap();
        self.worker.serve().unwrap();
    }

    fn add(&mut self, stream: &mut UnixStream, buf: &mut FrameBuffer) {
        assert_eq!(self.link.add_stream("lead", 2), Ok(1));
        self.worker.serve().unwrap();
        let header = buf.read_frame(stream).unwrap();
        assert_eq!(header.msg_type, MSG_STREAM_ADD);
        assert_eq!(StreamAddMsg::decode(buf.payload()).spec.name_str(), "lead");
        assert!(matches!(
            self.audio.try_recv(),
            Some(HubMessage::TransportSync(_))
        ));
        assert!(self.audio.try_recv().is_none());
    }
}

fn pair() -> Vec<StreamConfig> {
    vec![
        StreamConfig {
            name: "kick".to_string(),
            channels: 1,
        },
        StreamConfig {
            name: "pad".to_string(),
            channels: 2,
        },
    ]
}

#[test]
fn a_link_registers_its_streams_and_learns_their_strips() {
    let mut rig = Rig::new(pair());
    let (_hub, _buf, register) = rig.connect(vec![3, 4]);
    assert_eq!(
        register.streams,
        vec![StreamSpec::new("kick", 1), StreamSpec::new("pad", 2)]
    );
    assert_eq!(rig.link.stream_strip(0), Some(3));
    assert_eq!(rig.link.stream_strip(1), Some(4));
}

#[test]
fn stream_audio_carries_its_stream_id_and_stream_zero_is_plain_audio() {
    let mut rig = Rig::new(pair());
    let (mut hub, mut buf, _register) = rig.connect(vec![3, 4]);
    let before = assert_no_alloc::violation_count();
    let sent =
        assert_no_alloc::assert_no_alloc(|| rig.audio.send_stream_audio(1, 256, &[f32::NAN; 512]));
    assert!(sent);
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert!(rig.audio.send_audio(256, &[f32::INFINITY; 256]));
    rig.worker.serve().unwrap();
    let header = buf.read_frame(&mut hub).unwrap();
    assert_eq!(header.msg_type, MSG_STREAM_AUDIO);
    assert_eq!(
        header.payload_len as usize,
        AUDIO_STREAM_PAYLOAD_HEADER + 512 * 4
    );
    assert_eq!(decode_audio_stream_id(buf.payload()), 1);
    assert!(
        buf.payload()[16..16 + 512 * 4]
            .iter()
            .all(|byte| *byte == 0)
    );
    assert_eq!(buf.read_frame(&mut hub).unwrap().msg_type, MSG_AUDIO);
    assert_eq!(rig.audio.stream_frame(), 256);
    assert_eq!(rig.audio.stream_frame_of(1), 256);
}

#[test]
fn a_stream_added_live_is_announced_then_usable() {
    let mut rig = Rig::new(vec![]);
    let (mut hub, mut buf, _register) = rig.connect(vec![3]);
    assert!(!rig.audio.send_stream_audio(1, 256, &[0.0; 512]));
    assert_eq!(rig.audio.stream_frame_of(1), 0);
    rig.add(&mut hub, &mut buf);
    rig.ready(&mut hub, &mut buf, 6, "");
    assert_eq!(rig.link.stream_strip(1), Some(6));
    assert!(rig.audio.send_stream_audio(1, 256, &[0.0; 512]));
    rig.worker.serve().unwrap();
    assert_eq!(buf.read_frame(&mut hub).unwrap().msg_type, MSG_STREAM_AUDIO);
}

#[test]
fn a_stream_removed_live_is_withdrawn() {
    let mut rig = Rig::new(vec![]);
    let (mut hub, mut buf, _register) = rig.connect(vec![3]);
    rig.add(&mut hub, &mut buf);
    rig.ready(&mut hub, &mut buf, 6, "");
    assert!(rig.audio.send_stream_audio(1, 256, &[0.0; 512]));
    rig.worker.serve().unwrap();
    assert_eq!(buf.read_frame(&mut hub).unwrap().msg_type, MSG_STREAM_AUDIO);
    assert_eq!(rig.link.remove_stream(1), Ok(()));
    assert_eq!(rig.link.stream_strip(1), None);
    rig.worker.serve().unwrap();
    assert_eq!(
        buf.read_frame(&mut hub).unwrap().msg_type,
        MSG_STREAM_REMOVE
    );
    while rig.audio.try_recv().is_some() {}
    assert!(!rig.audio.send_stream_audio(1, 256, &[0.0; 512]));
    assert_eq!(rig.audio.stream_frame_of(1), 0);
    assert_eq!(rig.link.remove_stream(0), Err(RequestError::Invalid));
}

#[test]
fn a_refused_stream_has_no_strip_and_the_reason_is_readable() {
    let mut rig = Rig::new(vec![]);
    let (mut hub, mut buf, _register) = rig.connect(vec![3]);
    rig.add(&mut hub, &mut buf);
    rig.ready(&mut hub, &mut buf, NO_STRIP, "every strip is taken");
    assert_eq!(rig.link.stream_strip(1), None);
    assert!(rig.link.is_connected());
    assert_eq!(rig.link.status().last_refusal, None);
    assert_eq!(
        rig.link.last_stream_refusal(),
        Some((1, "every strip is taken".to_string()))
    );
}

#[test]
fn streams_are_registered_again_on_reconnect() {
    let mut rig = Rig::new(vec![]);
    let (mut hub, mut buf, _register) = rig.connect(vec![3]);
    rig.add(&mut hub, &mut buf);
    drop(hub);
    rig.worker.disconnect("desk restarted".to_string());
    let (_hub, _buf, register) = rig.connect(vec![3, 7]);
    assert_eq!(register.streams.len(), 2);
    assert_eq!(register.streams[1].name_str(), "lead");
    assert_eq!(rig.link.stream_strip(1), Some(7));
}

#[test]
fn add_stream_refuses_a_bad_name_a_used_name_and_the_seventeenth() {
    let rig = Rig::new(vec![]);
    for (name, channels) in [
        ("", 1),
        ("main", 2),
        ("lead", 3),
        ("lead", 0),
        ("a name longer than twenty four bytes", 1),
        ("bad\0name", 1),
    ] {
        assert_eq!(
            rig.link.add_stream(name, channels),
            Err(RequestError::Invalid)
        );
    }
    for id in 1..16 {
        assert_eq!(rig.link.add_stream(&format!("stream{id}"), 2), Ok(id));
    }
    assert_eq!(rig.link.add_stream("overflow", 2), Err(RequestError::Full));
    assert_eq!(rig.link.remove_stream(16), Err(RequestError::Invalid));
}

#[test]
fn a_sparse_stream_keeps_its_id_after_reconnect() {
    let mut streams = pair();
    streams.push(StreamConfig {
        name: "lead".to_string(),
        channels: 2,
    });
    let mut rig = Rig::new(streams);
    assert_eq!(rig.link.remove_stream(1), Ok(()));
    let (mut hub, mut buf, register) = rig.connect(vec![3]);
    assert_eq!(register.streams.len(), 1);
    let header = buf.read_frame(&mut hub).unwrap();
    assert_eq!(header.msg_type, MSG_STREAM_ADD);
    let add = StreamAddMsg::decode(buf.payload());
    assert_eq!(add.stream, 2);
    assert_eq!(add.spec.name_str(), "lead");
    while rig.audio.try_recv().is_some() {}
    assert!(!rig.audio.send_stream_audio(1, 1, &[0.0]));
    assert!(rig.audio.send_stream_audio(2, 1, &[0.0; 2]));
    rig.worker.serve().unwrap();
    assert_eq!(buf.read_frame(&mut hub).unwrap().msg_type, MSG_STREAM_AUDIO);
    assert_eq!(decode_audio_stream_id(buf.payload()), 2);
}

#[test]
fn detached_host_events_apply_stream_changes_without_exposing_them() {
    let mut rig = Rig::new(vec![]);
    let (mut hub, mut buf, _register) = rig.connect(vec![3]);
    rig.add(&mut hub, &mut buf);
    let (sender, receiver) = HeapRb::new(1).split();
    let mut events = std::mem::replace(
        &mut rig.audio.events,
        crate::host::link::EventRx::new(receiver),
    );
    drop(sender);
    assert!(rig.audio.send_stream_audio(1, 1, &[0.0; 2]));
    rig.worker.serve().unwrap();
    assert_eq!(buf.read_frame(&mut hub).unwrap().msg_type, MSG_STREAM_AUDIO);
    assert_eq!(rig.link.remove_stream(1), Ok(()));
    rig.worker.serve().unwrap();
    assert_eq!(
        buf.read_frame(&mut hub).unwrap().msg_type,
        MSG_STREAM_REMOVE
    );
    let mut events_seen = 0;
    crate::host::link::drain_events(&mut events, |_| events_seen += 1);
    assert_eq!(events_seen, 0);
    assert!(!rig.audio.send_stream_audio(1, 1, &[0.0; 2]));
    assert_eq!(rig.audio.stream_frame_of(1), 0);
}

#[test]
fn a_full_request_queue_does_not_record_an_unannounced_stream() {
    let mut rig = Rig::new(vec![]);
    let (_hub, _buf, _register) = rig.connect(vec![3]);
    for _ in 0..REQUEST_BACKLOG {
        rig.link.request_tempo(120.0).unwrap();
    }
    assert_eq!(rig.link.add_stream("lead", 2), Err(RequestError::Full));
    assert_eq!(rig.link.remove_stream(1), Err(RequestError::Invalid));
}
