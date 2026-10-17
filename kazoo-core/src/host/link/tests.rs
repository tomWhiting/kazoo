use super::*;
use crate::ipc::link::HubAddress;
use crate::ipc::protocol::FrameBuffer;
use crate::ipc::types::{MSG_REFUSED, MSG_REGISTER, MSG_REGISTERED, MSG_SHUTDOWN, RegisteredMsg};
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
fn no_desk_sleeps_only_between_tries() {
    let directory = tempfile::tempdir().unwrap();
    let mut sleeps = Vec::new();
    let result = connect_with(config(&directory.path().join("missing.sock")), |delay| {
        sleeps.push(delay);
    });
    assert!(matches!(result, Err(Error::NoDesk)));
    assert_eq!(sleeps, vec![Duration::from_millis(250); 2]);
}

#[test]
fn drain_events_takes_at_most_sixty_four_per_call() {
    let (mut sender, receiver) = ringbuf::HeapRb::new(128).split();
    for _ in 0..100 {
        sender.try_push(ToAudio::Hub(HubEvent::Shutdown)).unwrap();
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
            strips: Vec::new(),
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

#[test]
fn a_desk_that_refuses_the_name_leaves_the_instrument_playing_direct() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("refusing.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let hub = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = FrameBuffer::new();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_REGISTER
        );
        let reason = b"kazoo-test is already on the desk";
        buffer.payload_mut()[..reason.len()].copy_from_slice(reason);
        buffer
            .write_frame(MSG_REFUSED, 0, reason.len(), &mut stream)
            .unwrap();
        listener
    });
    let parts = connect_config_or_direct(config(&socket));
    let listener = hub.join().unwrap();
    assert!(parts.is_ok(), "a refusal must not stop the instrument");
    drop(parts);
    drop(listener);
}

#[test]
fn connecting_builds_one_link_not_two() {
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
            strips: Vec::new(),
            strip_index: 0,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: 0,
            bpm: 120.0,
            position: 0,
        };
        let size = reply.encode(buffer.payload_mut());
        buffer
            .write_frame(MSG_REGISTERED, 0, size, &mut stream)
            .unwrap();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_SHUTDOWN
        );
    });
    let before = crate::ipc::link::LINK_CONSTRUCTIONS.with(std::cell::Cell::get);
    let (link, audio) = connect_config_or_direct(config(&socket)).unwrap();
    let built = crate::ipc::link::LINK_CONSTRUCTIONS.with(std::cell::Cell::get) - before;
    drop(audio);
    drop(link);
    hub.join().unwrap();
    assert_eq!(built, 1);
}

#[test]
fn exit_report_is_silent_when_every_link_stopped() {
    assert_eq!(exit_report_from(0, 0), None);
    let directory = tempfile::tempdir().unwrap();
    let stuck = crate::ipc::link::stuck_shutdowns();
    let late = crate::ipc::link::undelivered_completions();
    let (link, audio) =
        start_link(config(&directory.path().join("missing.sock")), None, true).unwrap();
    drop(link);
    drop(audio);
    assert_eq!(
        exit_report_from(
            crate::ipc::link::stuck_shutdowns() - stuck,
            crate::ipc::link::undelivered_completions() - late,
        ),
        None,
    );
}

#[test]
fn exit_report_counts_stuck_and_late_links() {
    for (stuck, late, expected) in [
        (1, 0, "kazoo: 1 desk link did not stop within 500 ms"),
        (2, 0, "kazoo: 2 desk links did not stop within 500 ms"),
        (
            1,
            1,
            "kazoo: 1 desk link did not stop within 500 ms; 1 finished later",
        ),
        (
            2,
            2,
            "kazoo: 2 desk links did not stop within 500 ms; 2 finished later",
        ),
        (
            0,
            1,
            "kazoo: 0 desk links did not stop within 500 ms; 1 finished later",
        ),
    ] {
        assert_eq!(exit_report_from(stuck, late).as_deref(), Some(expected));
    }
}
