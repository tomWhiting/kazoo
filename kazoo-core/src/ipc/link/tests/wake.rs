use super::*;

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
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        let (mut stream, mut buffer) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        // A round trip establishes that registration has finished serving.
        assert!(audio.send_audio(256, &[0.0; 512]));
        assert_eq!(buffer.read_frame(&mut stream).unwrap().msg_type, MSG_AUDIO);
        Self {
            link,
            audio,
            stream,
            buffer,
            path,
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
        let header = connection
            .buffer
            .read_frame(&mut connection.stream)
            .unwrap();
        elapsed.push(start.elapsed());
        assert_eq!(header.msg_type, MSG_AUDIO);
        assert_eq!(header.payload_len as usize, AUDIO_PAYLOAD_HEADER + 512 * 4);
    }
    elapsed.sort_unstable();
    let p50 = elapsed[BLOCKS / 2];
    let p99 = elapsed[BLOCKS * 99 / 100];
    eprintln!(
        "BENCH instrument blocks={BLOCKS} p50_us={:.3} p99_us={:.3}",
        p50.as_secs_f64() * 1_000_000.0,
        p99.as_secs_f64() * 1_000_000.0
    );
    assert!(p50 < Duration::from_micros(200), "p50={p50:?}, p99={p99:?}");
}

#[test]
fn a_ui_request_is_carried_without_waiting_for_the_tick() {
    let mut connection = Connection::new();
    for _ in 0..10 {
        let start = Instant::now();
        connection.link.request_tempo(100.0).unwrap();
        let header = connection
            .buffer
            .read_frame(&mut connection.stream)
            .unwrap();
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
        thread::yield_now();
    }
    assert!(
        start.elapsed() < 2 * LINK_TICK,
        "sync exceeded two link ticks"
    );
}
