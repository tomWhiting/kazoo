use super::tests::connected;
use super::*;
use crate::ipc::types::*;

#[test]
fn register_at_sends_its_streams_and_reads_their_strips() {
    let path = std::env::temp_dir().join(format!("kzc-streams-{}.sock", std::process::id()));
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

/// Register three streams with a stand-in desk that answers `reply`, the
/// Registered payload as raw bytes.
fn register_against(reply: Vec<u8>) -> std::io::Result<HubIpcClient> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "kzc-tail-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let hub = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = FrameBuffer::new();
        buf.read_frame(&mut socket).unwrap();
        buf.payload_mut()[..reply.len()].copy_from_slice(&reply);
        buf.write_frame(MSG_REGISTERED, 0, reply.len(), &mut socket)
            .unwrap();
    });
    let mut register = RegisterMsg::new("test", 1, 48_000, 256);
    register.streams = vec![
        StreamSpec::new("kick", 1),
        StreamSpec::new("pad", 2),
        StreamSpec::new("lead", 2),
    ];
    let client = HubIpcClient::register_at(&path, &register);
    hub.join().unwrap();
    std::fs::remove_file(path).unwrap();
    client
}

/// A Registered payload with byte 0 `strip_index` and the given tail.
fn registered(strip_index: u8, strips: &[u8]) -> Vec<u8> {
    let mut buf = vec![0; 64];
    let len = RegisteredMsg {
        strip_index,
        hub_sample_rate: 48_000,
        hub_buffer_size: 256,
        transport_state: 0,
        bpm: 120.0,
        position: 0,
        strips: strips.to_vec(),
    }
    .encode(&mut buf);
    buf.truncate(len);
    buf[0] = strip_index;
    buf
}

#[test]
fn a_registered_tail_that_does_not_answer_every_stream_is_refused() {
    // Two strips for three streams, four for three, and a tail whose
    // stream 0 strip is not the strip the head names.
    for (strip_index, strips) in [(2, &[2, 5][..]), (2, &[2, 5, 6, 7]), (2, &[3, 5, 6])] {
        let err = register_against(registered(strip_index, strips)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{strips:?}");
    }
    // An old desk's 22-byte answer is one strip and is taken.
    let client = register_against(registered(4, &[])).unwrap();
    assert_eq!(client.registration().strips, vec![4]);
    // A full answer is taken.
    let client = register_against(registered(2, &[2, NO_STRIP, 5])).unwrap();
    assert_eq!(client.registration().strips, vec![2, NO_STRIP, 5]);
}
