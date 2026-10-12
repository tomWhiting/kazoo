use super::*;
use crate::ipc::protocol::FrameHeader;

const BLOCKS: usize = 10_000;

struct Connection {
    link: HubLink,
    audio: HubLinkAudio,
    stream: UnixStream,
    buffer: FrameBuffer,
    path: PathBuf,
}

impl Connection {
    fn new() -> Self {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        // The instruments' own setting: only the wall asks for real time, so
        // the wake is measured at the priority every other instrument has.
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        let (mut stream, mut buffer) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        // A round trip establishes that registration has finished serving.
        assert!(audio.send_audio(256, &[0.0; 512]));
        assert_eq!(buffer.read_frame(&mut stream).unwrap().msg_type, MSG_AUDIO);
        stream.set_nonblocking(true).unwrap();
        Self {
            link,
            audio,
            stream,
            buffer,
            path,
        }
    }

    fn read_frame(&mut self) -> FrameHeader {
        let start = Instant::now();
        loop {
            if let Some(header) = self.buffer.try_read_frame(&mut self.stream).unwrap() {
                return header;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "no frame received"
            );
            // Measure bytes arriving, without a second thread's scheduling delay.
            std::hint::spin_loop();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}

#[test]
fn a_queued_block_wakes_the_link_thread_at_once() {
    let mut connection = Connection::new();
    let block = [0.25; 512];
    let mut elapsed = Vec::with_capacity(BLOCKS);
    for _ in 0..BLOCKS {
        let start = Instant::now();
        assert!(connection.audio.send_audio(256, &block));
        let header = connection.read_frame();
        elapsed.push(start.elapsed());
        assert_eq!(header.msg_type, MSG_AUDIO);
        assert_eq!(header.payload_len as usize, AUDIO_PAYLOAD_HEADER + 512 * 4);
    }
    elapsed.sort_unstable();
    let p50 = elapsed[BLOCKS / 2];
    let p99 = elapsed[BLOCKS * 99 / 100];
    eprintln!(
        "BENCH instrument blocks={BLOCKS} p50_us={:.3} p99_us={:.3} real_time={:?}",
        p50.as_secs_f64() * 1_000_000.0,
        p99.as_secs_f64() * 1_000_000.0,
        connection.link.real_time()
    );
    assert!(p50 < Duration::from_micros(200), "p50={p50:?}, p99={p99:?}");
}

#[test]
fn a_ui_request_is_carried_without_waiting_for_the_tick() {
    let mut connection = Connection::new();
    for _ in 0..10 {
        let start = Instant::now();
        connection.link.request_tempo(100.0).unwrap();
        let header = connection.read_frame();
        let elapsed = start.elapsed();
        assert_eq!(header.msg_type, MSG_TRANSPORT_REQUEST);
        assert!(elapsed < Duration::from_millis(1), "elapsed={elapsed:?}");
    }
}

#[test]
fn an_idle_link_still_reads_the_hubs_messages_within_a_tick() {
    let mut connection = Connection::new();
    while connection.audio.try_recv().is_some() {}
    let sync = TransportSyncMsg {
        state: TRANSPORT_PLAYING,
        bpm: 90.0,
        at_frame: 4_800,
        beat: 2.0,
    };
    sync.encode(connection.buffer.payload_mut());
    let start = Instant::now();
    connection
        .buffer
        .write_frame(
            MSG_TRANSPORT_SYNC,
            0,
            TransportSyncMsg::WIRE_SIZE,
            &mut connection.stream,
        )
        .unwrap();
    loop {
        if let Some(HubMessage::TransportSync(received)) = connection.audio.try_recv() {
            assert_eq!(received.at_frame, sync.at_frame);
            break;
        }
        assert!(
            start.elapsed() < 2 * LINK_TICK,
            "no sync within two link ticks"
        );
        std::hint::spin_loop();
    }
    assert!(
        start.elapsed() < 2 * LINK_TICK,
        "sync exceeded two link ticks"
    );
}

#[test]
fn closing_a_connected_link_does_not_wait_for_the_tick() {
    const CLOSES: usize = 21;
    let mut elapsed = Vec::with_capacity(CLOSES);
    for _ in 0..CLOSES {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, audio) = hub_link(config(&path)).unwrap();
        let (_stream, _buffer) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        // Long enough for the link thread to finish serving and park.
        thread::sleep(Duration::from_millis(1));
        let start = Instant::now();
        drop(link);
        elapsed.push(start.elapsed());
        drop(audio);
        std::fs::remove_file(&path).unwrap();
    }
    elapsed.sort_unstable();
    let median = elapsed[CLOSES / 2];
    assert!(
        median < LINK_TICK / 4,
        "median close took {median:?}; the link thread was left parked for its tick"
    );
}
