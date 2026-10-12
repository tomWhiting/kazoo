use super::*;
use crate::ipc::protocol::FrameBuffer;
use crate::ipc::types::{MSG_REGISTER, MSG_REGISTERED, MSG_SHUTDOWN, RegisteredMsg};
use ringbuf::traits::{Producer, Split};
use std::os::unix::net::UnixListener;
use std::time::Duration;

fn config(path: &std::path::Path) -> LinkConfig {
    LinkConfig {
        address: HubAddress::Socket(path.to_path_buf()),
        ..LinkConfig::new("kazoo-test", 2, 48_000, 256)
    }
}

#[test]
fn connect_with_no_desk_is_no_desk_after_three_tries() {
    let directory = tempfile::tempdir().unwrap();
    let mut sleeps = Vec::new();
    let result = connect_with(config(&directory.path().join("missing.sock")), |delay| {
        sleeps.push(delay);
    });
    assert!(matches!(result, Err(Error::NoDesk)));
    assert_eq!(sleeps, vec![Duration::from_millis(250); 3]);
}

#[test]
fn drain_events_takes_at_most_sixty_four_per_call() {
    let (mut sender, receiver) = ringbuf::HeapRb::new(128).split();
    for _ in 0..100 {
        sender.try_push(HubEvent::Shutdown).unwrap();
    }
    let mut receiver = EventRx::new(receiver);
    let mut count = 0;
    drain_events(&mut receiver, |_| count += 1);
    assert_eq!(count, 64);
    drain_events(&mut receiver, |_| count += 1);
    assert_eq!(count, 100);
}

#[test]
fn drop_sends_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("desk.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let hub = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = FrameBuffer::new();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_REGISTER
        );
        let reply = RegisteredMsg {
            strip_index: 0,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: 0,
            bpm: 120.0,
            position: 0,
        };
        reply.encode(buffer.payload_mut());
        buffer
            .write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
            .unwrap();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_SHUTDOWN
        );
    });
    let linked = connect_with(config(&socket), |_| {
        panic!("a listening desk must connect immediately")
    })
    .unwrap();
    drop(linked);
    hub.join().unwrap();
}

fn accept_within(listener: &UnixListener, limit: Duration) -> std::os::unix::net::UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + limit;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the link stopped looking for the desk"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("accept failed: {err}"),
        }
    }
}

#[test]
fn a_link_that_found_no_desk_joins_a_desk_that_starts_later() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("late.sock");
    let (link, audio) = connect_config_or_direct(config(&socket)).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    let mut stream = accept_within(&listener, Duration::from_secs(5));
    let mut buffer = FrameBuffer::new();
    assert_eq!(
        buffer.read_frame(&mut stream).unwrap().msg_type,
        MSG_REGISTER
    );
    drop(audio);
    drop(link);
}
