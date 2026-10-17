use super::*;
use crate::ipc::protocol::{decode_audio_frame_count, decode_audio_samples};
use std::net::TcpListener;

fn exercise_tcp_link(synchronous: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let bound = listener.local_addr().unwrap();
    let address = if synchronous {
        bound.to_string()
    } else {
        format!("localhost:{}", bound.port())
    };
    let hub = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nodelay(true).unwrap();
        let mut buffer = FrameBuffer::new();
        let header = buffer.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_REGISTER);
        let register = RegisterMsg::decode(buffer.payload());
        assert_eq!(register.name_str(), "kazoo-test");
        let reply = RegisteredMsg {
            strips: vec![3],
            strip_index: 3,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: TRANSPORT_STOPPED,
            bpm: 120.0,
            position: 0,
        };
        let size = reply.encode(buffer.payload_mut());
        buffer
            .write_frame(MSG_REGISTERED, 0, size, &mut stream)
            .unwrap();
        let header = buffer.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_AUDIO);
        assert_eq!(decode_audio_frame_count(buffer.payload()), 2);
        let mut samples = [0.0; 4];
        decode_audio_samples(buffer.payload(), &mut samples, 4);
        assert_eq!(
            samples.map(f32::to_bits),
            [0.25_f32, -0.25, 0.0, 0.0].map(f32::to_bits)
        );
        let sync = TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: 96.0,
            at_frame: 123_456,
            beat: 8.0,
        };
        sync.encode(buffer.payload_mut());
        buffer
            .write_frame(
                MSG_TRANSPORT_SYNC,
                1,
                TransportSyncMsg::WIRE_SIZE,
                &mut stream,
            )
            .unwrap();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_SHUTDOWN
        );
    });
    let config = LinkConfig {
        address: HubAddress::parse(&address).unwrap(),
        real_time: false,
        ..LinkConfig::new("kazoo-test", 2, 48_000, 256)
    };
    let (link, mut audio) = if synchronous {
        crate::host::link::connect_config(config)
            .unwrap()
            .into_parts()
    } else {
        hub_link(config).unwrap()
    };
    while !link.is_connected() {
        thread::yield_now();
    }
    assert_eq!(link.status().strip, Some(3));
    assert_no_alloc::assert_no_alloc(|| {
        assert!(audio.send_audio(2, &[0.25, -0.25, f32::NAN, f32::INFINITY]));
    });
    loop {
        if let Some(HubMessage::TransportSync(sync)) = audio.try_recv()
            && sync.at_frame == 123_456
        {
            assert_eq!(sync.state, TRANSPORT_PLAYING);
            assert_eq!(sync.bpm.to_bits(), 96.0_f32.to_bits());
            break;
        }
        thread::yield_now();
    }
    drop(link);
    hub.join().unwrap();
}

#[test]
fn tcp_worker_registers_sends_audio_and_receives_transport() {
    exercise_tcp_link(false);
}

#[test]
fn tcp_host_registers_sends_audio_and_receives_transport() {
    exercise_tcp_link(true);
}

#[test]
fn tcp_registration_refusal_names_the_desk_and_retains_its_rate() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let reason = rate_refusal(44_100, 48_000);
    let refused = reason.clone();
    let hub = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = FrameBuffer::new();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_REGISTER
        );
        buffer.payload_mut()[..refused.len()].copy_from_slice(refused.as_bytes());
        buffer
            .write_frame(MSG_REFUSED, 0, refused.len(), &mut stream)
            .unwrap();
    });
    let (link, audio) = hub_link(LinkConfig {
        address: HubAddress::parse(&address).unwrap(),
        real_time: false,
        ..LinkConfig::new("kazoo-test", 2, 44_100, 256)
    })
    .unwrap();
    hub.join().unwrap();
    let status = loop {
        let status = link.status();
        if status.last_refusal.is_some() {
            break status;
        }
        thread::yield_now();
    };
    assert!(status.last_refusal.as_deref().unwrap().contains(&address));
    assert_eq!(status.desk_refusal(), Some(reason.as_str()));
    assert_eq!(status.refused_for_rate(), Some(48_000));
    assert!(!audio.is_connected());
}
