mod shutdown;
mod streams;
mod transport;
mod voice;
mod wake;

use super::*;
use crate::ipc::protocol::AUDIO_PAYLOAD_HEADER;
use crate::ipc::protocol::FrameBuffer;
use crate::ipc::types::{
    MSG_AUDIO, MSG_DESK_HEALTH, MSG_DESK_PACE, MSG_NOTE_EVENT, MSG_REFUSED, MSG_REGISTER,
    MSG_REGISTERED, MSG_SHUTDOWN, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC, RegisterMsg,
    RegisteredMsg, TransportRequestMsg,
};
use std::os::unix::net::{UnixListener, UnixStream};

fn socket() -> PathBuf {
    use std::sync::atomic::AtomicU32;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "kzl-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// Accept one instrument on `listener` and answer its registration.
fn accept(listener: &UnixListener) -> (UnixStream, FrameBuffer) {
    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = FrameBuffer::new();
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_REGISTER);
    let register = RegisterMsg::decode(&buf.payload()[..header.payload_len as usize]);
    assert_eq!(register.name_str(), "kazoo-test");
    let reply = RegisteredMsg {
        strips: vec![3],
        strip_index: 3,
        hub_sample_rate: 48_000,
        hub_buffer_size: 256,
        transport_state: 0,
        bpm: 120.0,
        position: 0,
    };
    let len = reply.encode(buf.payload_mut());
    buf.write_frame(MSG_REGISTERED, 0, len, &mut stream)
        .unwrap();
    (stream, buf)
}

fn config(path: &std::path::Path) -> LinkConfig {
    LinkConfig {
        address: HubAddress::Socket(path.to_path_buf()),
        ..LinkConfig::new("kazoo-test", 2, 48_000, 256)
    }
}

#[test]
fn a_link_is_real_time_only_when_asked_and_only_once_plugged_in() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, _audio) = hub_link(LinkConfig {
        real_time: true,
        ..config(&path)
    })
    .unwrap();
    // Looking for the desk happens at normal priority.
    assert_eq!(link.real_time(), None);
    let (_stream, _buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    wait("the link to ask for real-time", || {
        link.real_time().is_some()
    });
    #[cfg(target_os = "macos")]
    assert_eq!(link.real_time(), Some(&Ok(())));
    drop(link);

    // Not asked: never promoted.
    let mut plain_config = config(&path);
    plain_config.real_time = false;
    let (plain, _audio) = hub_link(plain_config).unwrap();
    let (_stream, _buf) = accept(&listener);
    wait("the plain link to connect", || plain.is_connected());
    assert_eq!(plain.real_time(), None);
    drop(plain);
    std::fs::remove_file(&path).unwrap();
}

/// Tell the link how the desk is taking its audio.
fn health(stream: &mut UnixStream, buf: &mut FrameBuffer, placed_afresh: u64, dropped: u64) {
    let health = DeskHealthMsg {
        placed_afresh,
        dropped,
    };
    health.encode(buf.payload_mut());
    buf.write_frame(MSG_DESK_HEALTH, 0, DeskHealthMsg::WIRE_SIZE, stream)
        .unwrap();
}

#[test]
fn the_desks_counts_are_kept_across_connections() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, _audio) = hub_link(config(&path)).unwrap();
    assert_eq!(link.desk_health(), DeskHealthMsg::default());
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    health(&mut stream, &mut buf, 2, 1);
    health(&mut stream, &mut buf, 3, 1);
    wait("the counts to arrive", || {
        link.desk_health().placed_afresh == 3
    });
    assert_eq!(link.desk_health().dropped, 1);

    // The desk goes; a new one counts from zero, and the link adds on.
    buf.write_frame(MSG_SHUTDOWN, 0, 0, &mut stream).unwrap();
    wait("the link to notice", || !link.is_connected());
    assert_eq!(link.desk_health().placed_afresh, 3);
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to reconnect", || link.is_connected());
    health(&mut stream, &mut buf, 1, 0);
    wait("the new counts to arrive", || {
        link.desk_health().placed_afresh == 4
    });
    assert_eq!(link.desk_health().dropped, 1);
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn audio_is_local_until_the_hub_appears_then_goes_to_the_desk() {
    let path = socket();
    let (link, mut audio) = hub_link(config(&path)).unwrap();
    let block = [0.5_f32; 512];
    // No hub yet: the instrument keeps its own output.
    assert!(!audio.send_audio(256, &block));

    let listener = UnixListener::bind(&path).unwrap();
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    assert_eq!(link.status().strip, Some(3));

    assert!(audio.send_audio(256, &block));
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_AUDIO);
    assert_eq!(header.payload_len as usize, AUDIO_PAYLOAD_HEADER + 512 * 4);
    wait("the send to be counted", || link.status().blocks_sent == 1);
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn hub_messages_reach_the_callback_and_a_closing_hub_is_noticed() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, mut audio) = hub_link(config(&path)).unwrap();
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());

    // Joining hands the callback the desk's transport straight away.
    let joined = audio.try_recv();
    assert!(
        matches!(joined, Some(HubMessage::TransportSync(s)) if s.state == 0 && s.bpm.to_bits() == 120.0_f32.to_bits() && s.at_frame == SYNC_NOW),
        "{joined:?}"
    );

    let sync = TransportSyncMsg {
        state: 1,
        bpm: 90.0,
        at_frame: 4_800,
        beat: 2.0,
    };
    sync.encode(buf.payload_mut());
    buf.write_frame(
        MSG_TRANSPORT_SYNC,
        1,
        TransportSyncMsg::WIRE_SIZE,
        &mut stream,
    )
    .unwrap();
    let mut received = None;
    wait("the sync to arrive", || {
        received = audio.try_recv();
        received.is_some()
    });
    assert!(matches!(received, Some(HubMessage::TransportSync(s)) if s.state == 1));

    buf.write_frame(MSG_SHUTDOWN, 2, 0, &mut stream).unwrap();
    wait("the link to notice the hub closed", || !link.is_connected());
    assert!(!audio.send_audio(256, &[0.0; 512]));
    assert!(
        link.status()
            .last_refusal
            .is_some_and(|why| why.contains("closed"))
    );
    // Coming down is not the desk refusing the instrument.
    assert_eq!(link.status().desk_refusal(), None);
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

/// Tell the link where the desk is playing its stream.
fn pace(stream: &mut UnixStream, buf: &mut FrameBuffer, playing_stream: u64, lead: u32) {
    let pace = DeskPaceMsg {
        playing_stream,
        lead_frames: lead,
    };
    pace.encode(buf.payload_mut());
    buf.write_frame(MSG_DESK_PACE, 0, DeskPaceMsg::WIRE_SIZE, stream)
        .unwrap();
}

#[test]
fn a_paced_link_asks_for_its_lead_and_owes_the_desk_what_it_is_behind() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, mut audio) = hub_link(LinkConfig {
        pace_lead_frames: 1_920,
        ..config(&path)
    })
    .unwrap();
    // Not plugged in: nothing is owed, and the instrument keeps time.
    assert_eq!(audio.desk_owes(Instant::now()), None);

    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = FrameBuffer::new();
    let header = buf.read_frame(&mut stream).unwrap();
    let register = RegisterMsg::decode(&buf.payload()[..header.payload_len as usize]);
    assert_eq!(register.pace_lead_frames, 1_920);
    let reply = RegisteredMsg {
        strips: Vec::new(),
        strip_index: 0,
        hub_sample_rate: 48_000,
        hub_buffer_size: 256,
        transport_state: 0,
        bpm: 120.0,
        position: 0,
    };
    reply.encode(buf.payload_mut());
    buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
        .unwrap();
    wait("the link to connect", || link.is_connected());
    // Plugged in, but the desk has not paced it yet.
    assert_eq!(audio.desk_owes(Instant::now()), None);

    // 1 024 frames rendered; the desk plays frame 100 of them with a
    // lead of 2 000: it wants up to 2 100, so 1 076 are owed.
    assert!(audio.send_audio(1_024, &[0.0; 2_048]));
    pace(&mut stream, &mut buf, 100, 2_000);
    let mut owed = None;
    wait("the pace to arrive", || {
        owed = audio.desk_owes(Instant::now()).map(|owes| owes.frames);
        owed.is_some()
    });
    let arrived = Instant::now();
    let now_owed = audio.desk_owes(arrived).unwrap().frames;
    // Plus the frames the desk played since the pace came (well under
    // 50 ms of them, even on a busy machine).
    assert!((1_076..1_076 + 2_400).contains(&owed.unwrap()), "{owed:?}");
    assert!(now_owed >= owed.unwrap(), "{now_owed} after {owed:?}");
    // The desk's clock runs on between paces: 10 ms later, 480 more.
    let later = audio
        .desk_owes(arrived + Duration::from_millis(10))
        .unwrap()
        .frames;
    assert!(
        (later - now_owed).abs_diff(480) <= 1,
        "{later} after {now_owed}"
    );
    // Ahead of the desk: owed goes negative.
    assert!(audio.send_audio(4_096, &vec![0.0; 8_192]));
    let ahead = audio.desk_owes(arrived).unwrap().frames;
    assert_eq!(ahead, now_owed - 4_096);
    let reckoning = audio.desk_owes(arrived).unwrap();
    assert_eq!(reckoning.lead_frames, 2_000);
    assert!(reckoning.paced_at <= arrived);
    // A pace gone stale is not followed.
    assert_eq!(
        audio.desk_owes(arrived + PACE_STALE + Duration::from_millis(50)),
        None
    );

    // Unplugged: forgotten.
    buf.write_frame(MSG_SHUTDOWN, 1, 0, &mut stream).unwrap();
    wait("the link to notice the hub closed", || !link.is_connected());
    assert_eq!(audio.desk_owes(Instant::now()), None);
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn what_is_owed_is_a_signed_distance_across_the_wrap() {
    // The desk plays 1 000 frames before the stream's frame 0, with a
    // lead of 2 000: it wants up to frame 1 000 (plus 48 a millisecond
    // on). Frames wrap, and the distance stays signed either side.
    let pace = Pace {
        playing_stream: 0_u64.wrapping_sub(1_000),
        lead_frames: 2_000,
        arrived: Duration::ZERO,
    };
    assert_eq!(owed(pace, Duration::ZERO, 48_000, 0), 1_000);
    assert_eq!(owed(pace, Duration::ZERO, 48_000, 1_256), -256);
    assert_eq!(owed(pace, Duration::from_millis(1), 48_000, 1_000), 48);
    // Near the top of the counter and past it.
    let pace = Pace {
        playing_stream: u64::MAX - 10,
        lead_frames: 100,
        arrived: Duration::ZERO,
    };
    assert_eq!(owed(pace, Duration::ZERO, 48_000, u64::MAX - 10), 100);
    assert_eq!(owed(pace, Duration::ZERO, 48_000, 200), -111);
}

#[test]
fn a_pace_or_an_unknown_message_does_not_hold_up_the_next() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, mut audio) = hub_link(config(&path)).unwrap();
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    wait("the joining sync", || audio.try_recv().is_some());
    pace(&mut stream, &mut buf, 0, 1_000);
    buf.write_frame(0x7E, 1, 0, &mut stream).unwrap();
    let sync = TransportSyncMsg {
        state: 1,
        bpm: 100.0,
        at_frame: SYNC_NOW,
        beat: 0.0,
    };
    sync.encode(buf.payload_mut());
    buf.write_frame(
        MSG_TRANSPORT_SYNC,
        2,
        TransportSyncMsg::WIRE_SIZE,
        &mut stream,
    )
    .unwrap();
    let mut received = None;
    wait("the sync behind them", || {
        received = audio.try_recv();
        received.is_some()
    });
    assert!(matches!(received, Some(HubMessage::TransportSync(s)) if s.state == 1));
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn dropping_the_link_says_goodbye() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, _audio) = hub_link(config(&path)).unwrap();
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    drop(link);
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_SHUTDOWN);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn oversized_callbacks_are_split_and_malformed_ones_dropped() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, mut audio) = hub_link(config(&path)).unwrap();
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());

    // 600 frames through a 256-frame link: 256, 256, 88.
    assert!(audio.send_audio(600, &[0.25; 1200]));
    for frames in [256, 256, 88] {
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_AUDIO);
        assert_eq!(
            header.payload_len as usize,
            AUDIO_PAYLOAD_HEADER + frames * 2 * 4
        );
    }
    // Three frames of stereo need six samples, not five.
    assert!(audio.send_audio(3, &[0.0; 5]));
    wait("the counters", || {
        let status = link.status();
        status.blocks_sent == 3 && status.blocks_dropped == 1
    });
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn requests_from_either_half_reach_the_desk() {
    let path = socket();
    let (link, mut audio) = hub_link(config(&path)).unwrap();
    // Not plugged in: the instrument is told to act on its own.
    assert_eq!(
        audio.request_transport(TRANSPORT_PLAYING, None),
        Err(RequestError::NotConnected)
    );
    assert_eq!(
        link.request_transport(TRANSPORT_PLAYING, None),
        Err(RequestError::NotConnected)
    );

    let listener = UnixListener::bind(&path).unwrap();
    let (mut stream, mut buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    assert_eq!(link.request_transport(9, None), Err(RequestError::Invalid));
    assert_eq!(
        audio.request_transport(TRANSPORT_PLAYING, Some(f32::NAN)),
        Err(RequestError::Invalid)
    );

    audio
        .request_transport(TRANSPORT_PLAYING, Some(96.0))
        .unwrap();
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_TRANSPORT_REQUEST);
    let request = TransportRequestMsg::decode(buf.payload());
    assert_eq!(request.requested_state, TRANSPORT_PLAYING);
    assert_eq!(request.has_bpm, 1);
    assert_eq!(request.requested_bpm.to_bits(), 96.0_f32.to_bits());

    // A tempo change leaves the desk's play state alone; leaving it
    // alone with no tempo to set is not a request at all.
    assert_eq!(
        link.request_transport(TRANSPORT_UNCHANGED, None),
        Err(RequestError::Invalid)
    );
    assert_eq!(audio.request_tempo(-3.0), Err(RequestError::Invalid));
    link.request_tempo(132.0).unwrap();
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_TRANSPORT_REQUEST);
    let request = TransportRequestMsg::decode(buf.payload());
    assert_eq!(request.requested_state, TRANSPORT_UNCHANGED);
    assert_eq!(request.has_bpm, 1);
    assert_eq!(request.requested_bpm.to_bits(), 132.0_f32.to_bits());

    let note = NoteEventMsg {
        source: [0; 16],
        target: [0; 16],
        event_type: NOTE_ON,
        channel: 0,
        note: 60,
        velocity: 100,
    };
    assert_eq!(
        link.send_note(NoteEventMsg { note: 200, ..note }),
        Err(RequestError::Invalid)
    );
    link.send_note(note).unwrap();
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_NOTE_EVENT);
    let sent = NoteEventMsg::decode(buf.payload());
    assert_eq!(sent.note, 60);
    assert_ne!(sent.source, [0; 16], "the link signs notes with its id");
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_refusal_is_reported_and_retries_back_off() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, _audio) = hub_link(config(&path)).unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = FrameBuffer::new();
    buf.read_frame(&mut stream).unwrap();
    let reason = b"every strip is taken";
    buf.payload_mut()[..reason.len()].copy_from_slice(reason);
    buf.write_frame(MSG_REFUSED, 0, reason.len(), &mut stream)
        .unwrap();
    drop(stream);
    wait("the refusal to be reported", || {
        link.status()
            .last_refusal
            .is_some_and(|why| why.contains("every strip is taken"))
    });
    assert!(!link.is_connected());
    // The desk's own words, without the link's framing around them.
    assert_eq!(link.status().desk_refusal(), Some("every strip is taken"));

    // The next try waits two seconds, not the usual one.
    listener.set_nonblocking(true).unwrap();
    let refused_at = Instant::now();
    let retried_at = loop {
        match listener.accept() {
            Ok(_) => break Instant::now(),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                assert!(refused_at.elapsed() < Duration::from_secs(5), "no retry");
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("accept failed: {err}"),
        }
    };
    assert!(
        retried_at.duration_since(refused_at) > Duration::from_millis(1_500),
        "retried after {:?}",
        retried_at.duration_since(refused_at)
    );
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_desk_that_stops_reading_is_unplugged() {
    let path = socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (link, mut audio) = hub_link(config(&path)).unwrap();
    // Registered, then never read from again.
    let (_stream, _buf) = accept(&listener);
    wait("the link to connect", || link.is_connected());
    let block = [0.1_f32; 512];
    let deadline = Instant::now() + Duration::from_secs(10);
    while link.is_connected() {
        assert!(Instant::now() < deadline, "the link never gave up");
        audio.send_audio(256, &block);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        link.status()
            .last_refusal
            .is_some_and(|why| why.contains("stopped taking audio")),
        "{:?}",
        link.status()
    );
    drop(link);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn an_instrument_link_asks_for_real_time_by_default() {
    assert!(LinkConfig::new("kazoo-808", 2, 48_000, 256).real_time);
}

#[test]
fn a_refused_promotion_is_reported_not_printed() {
    const PROBE: &str = "KZ_LINK_REFUSAL_PROBE";
    if std::env::var_os(PROBE).is_some() {
        let (link, audio) =
            start_link(LinkConfig::new("refusal-test", 2, 48_000, 256), None, false).unwrap();
        let why = RealTime::promote(Duration::ZERO).unwrap_err();
        let expected = why.to_string();
        assert!(priority::promote(&link.state, || Err(why)).is_none());
        assert_eq!(
            link.real_time().unwrap().as_ref().unwrap_err().to_string(),
            expected
        );
        assert!(link.status().normal_priority);
        drop(audio);
        drop(link);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ipc::link::tests::a_refused_promotion_is_reported_not_printed",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "link wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
