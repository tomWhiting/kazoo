use super::tests::connected;
use super::*;
use crate::ipc::types::*;

#[test]
fn register_at_sends_its_streams_and_reads_their_strips() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!(".s{}.sock", std::process::id()));
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let hub = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = FrameBuffer::new();
        let head = buf.read_frame(&mut socket).unwrap();
        let register = RegisterMsg::decode(&buf.payload()[..head.payload_len as usize]);
        assert_eq!(
            register.streams,
            vec![
                StreamSpec::new("kick", 1),
                StreamSpec::new("pad", 2),
                StreamSpec::new("lead", 2)
            ]
        );
        let reply = RegisteredMsg {
            strip_index: 2,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: 0,
            bpm: 120.0,
            position: 0,
            strips: vec![2, NO_STRIP, 5],
        };
        let len = reply.encode(buf.payload_mut());
        buf.write_frame(MSG_REGISTERED, 0, len, &mut socket)
            .unwrap();
    });
    let mut register = RegisterMsg::new("test", 1, 48_000, 256);
    register.streams = vec![
        StreamSpec::new("kick", 1),
        StreamSpec::new("pad", 2),
        StreamSpec::new("lead", 2),
    ];
    let client = HubIpcClient::register_at(&path, &register).unwrap();
    assert_eq!(client.registration().strips, vec![2, NO_STRIP, 5]);
    hub.join().unwrap();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_stream_ready_with_a_reason_is_kept_for_the_link() {
    let (mut client, mut hub, mut buf) = connected();
    let ready = StreamReadyMsg {
        stream: 1,
        strip: NO_STRIP,
    };
    ready.encode(buf.payload_mut());
    let reason = b"every strip is taken";
    buf.payload_mut()[3..3 + reason.len()].copy_from_slice(reason);
    buf.write_frame(MSG_STREAM_READY, 0, 3 + reason.len(), &mut hub)
        .unwrap();
    assert!(matches!(
        client.try_recv().unwrap(),
        Some(HubMessage::StreamReady {
            stream: 1,
            strip: None
        })
    ));
    assert_eq!(
        client.take_stream_refusal(),
        Some((1, "every strip is taken".to_string()))
    );
    assert_eq!(client.take_stream_refusal(), None);
}
