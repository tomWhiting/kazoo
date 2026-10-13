use super::*;
use crate::ipc::protocol::{decode_audio_frame_count, decode_audio_samples};

struct WireRig {
    link: HubLink,
    audio: HubLinkAudio,
    stream: UnixStream,
    buffer: FrameBuffer,
    path: PathBuf,
}

impl WireRig {
    fn new(block_frames: u32) -> Self {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, audio) = hub_link(LinkConfig {
            max_block_frames: block_frames,
            ..config(&path)
        })
        .unwrap();
        let (stream, buffer) = accept(&listener);
        wait("registration", || link.is_connected());
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        Self {
            link,
            audio,
            stream,
            buffer,
            path,
        }
    }
}

impl Drop for WireRig {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}

#[test]
fn a_block_bigger_than_sixty_four_kib_reaches_the_hub_whole() {
    let mut rig = WireRig::new(16_384);
    let mut samples: Vec<f32> = (0..32_768).map(|n| (n % 64) as f32 / 64.0).collect();
    samples[0] = f32::NAN;
    samples[1] = f32::INFINITY;
    assert!(rig.audio.send_audio(16_384, &samples));
    let header = rig.buffer.read_frame(&mut rig.stream).unwrap();
    assert_eq!(header.msg_type, MSG_AUDIO);
    assert_eq!(header.payload_len, 12 + 16_384 * 8);
    let mut decoded = vec![0.0; samples.len()];
    decode_audio_samples(rig.buffer.payload(), &mut decoded, samples.len());
    assert_eq!(
        decoded.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
        samples
            .iter()
            .map(|s| sanitize_sample(*s).to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_backlog_holds_a_quarter_second_then_counts_drops() {
    // Hold the consumer still to measure the ring independently of kernel buffering.
    let path = socket();
    let (link, mut audio) = start_link(
        LinkConfig {
            max_block_frames: 512,
            ..config(&path)
        },
        None,
        false,
    )
    .unwrap();
    link.state.connected.store(true, Ordering::Release);
    let mut queued = 0;
    loop {
        audio.send_audio(512, &[0.25; 1024]);
        if link.status().blocks_dropped > 0 {
            break;
        }
        queued += 1;
        assert!(queued < 1_000);
    }
    let seconds = f64::from(queued * 512) / 48_000.0;
    assert!(seconds >= 0.25, "queued={queued}, seconds={seconds}");
    assert!(seconds <= 0.29, "queued={queued}, seconds={seconds}");
    assert_eq!(link.status().blocks_dropped, 1);

    let mut live = WireRig::new(512);
    let observer = live.audio.wire.observe();
    let mut accepted_blocks = 0_u64;
    loop {
        live.audio.send_audio(512, &[0.25; 1024]);
        if live.link.status().blocks_dropped > 0 {
            break;
        }
        accepted_blocks += 1;
        assert!(accepted_blocks < 10_000);
    }
    let total_bytes = accepted_blocks as usize * (HEADER_SIZE + AUDIO_PAYLOAD_HEADER + 4_096);
    let waiting = observer.occupied_len();
    let socket_accepted = total_bytes - waiting;
    let ring_bytes = wire::link_ring_bytes(48_000, 2, 512);
    assert!(
        accepted_blocks * 512 >= 12_000,
        "only {accepted_blocks} blocks before dropping"
    );
    assert!(total_bytes <= ring_bytes + socket_accepted);
    eprintln!(
        "BACKLOG blocks={accepted_blocks} socket_accepted={socket_accepted} ring_bytes={ring_bytes}"
    );
}

#[test]
fn a_stream_added_live_gets_its_own_quarter_second() {
    let path = socket();
    let (link, mut audio) = start_link(
        LinkConfig {
            max_block_frames: 512,
            ..config(&path)
        },
        None,
        false,
    )
    .unwrap();
    link.state.connected.store(true, Ordering::Release);
    link.state.strips[1].store(4, Ordering::Release);
    // What the link thread hands the callback when a stereo stream is added.
    audio.stream_updates[1].store(2, Ordering::Release);
    let mut queued = 0;
    loop {
        audio.send_audio(512, &[0.25; 1024]);
        assert!(audio.send_stream_audio(1, 512, &[0.25; 1024]));
        if link.status().blocks_dropped > 0 {
            break;
        }
        queued += 1;
        assert!(queued < 1_000);
    }
    let seconds = f64::from(queued * 512) / 48_000.0;
    assert!(seconds >= 0.25, "queued={queued}, seconds={seconds}");
    assert!(seconds <= 0.29, "queued={queued}, seconds={seconds}");
}

#[test]
fn a_hub_that_stops_reading_for_half_a_second_is_let_go_by_time() {
    let mut stopped = WireRig::new(512);
    let start = Instant::now();
    while stopped.link.is_connected() && start.elapsed() < Duration::from_millis(1_500) {
        stopped.audio.send_audio(512, &[0.25; 1024]);
        thread::park_timeout(Duration::from_millis(5));
    }
    assert!(
        !stopped.link.is_connected(),
        "unread socket stayed connected"
    );
    assert!(
        start.elapsed() >= STUCK_LIMIT,
        "disconnected before the time limit"
    );
    assert!(
        stopped
            .link
            .status()
            .last_refusal
            .unwrap()
            .contains("stopped taking audio")
    );
    let mut trickle = WireRig::new(16_384);
    trickle.stream.set_nonblocking(true).unwrap();
    let samples = vec![0.25; 32_768];
    let start = Instant::now();
    let mut next_read = start;
    while start.elapsed() < Duration::from_secs(2) {
        trickle.audio.send_audio(16_384, &samples);
        if Instant::now() >= next_read {
            let header = loop {
                if let Some(header) = trickle.buffer.try_read_frame(&mut trickle.stream).unwrap() {
                    break header;
                }
                assert!(start.elapsed() < Duration::from_secs(3));
                thread::yield_now();
            };
            assert_eq!(header.payload_len, 12 + 16_384 * 8);
            next_read += Duration::from_millis(100);
        }
        assert!(
            trickle.link.is_connected(),
            "a reader making progress was disconnected: {:?}",
            trickle.link.status()
        );
        thread::park_timeout(Duration::from_millis(5));
    }
}

#[test]
fn request_frames_and_audio_frames_never_interleave_on_the_wire() {
    let mut rig = WireRig::new(8_192);
    let mut reader = rig.stream.try_clone().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let read = thread::spawn(move || {
        let mut buffer = FrameBuffer::new();
        for _ in 0..500 {
            let header = buffer.read_frame(&mut reader).unwrap();
            match header.msg_type {
                MSG_AUDIO => {
                    assert_eq!(header.payload_len, 12 + 8_192 * 8);
                    assert_eq!(decode_audio_frame_count(buffer.payload()), 8_192);
                    let mut samples = vec![0.0; 16_384];
                    decode_audio_samples(buffer.payload(), &mut samples, 16_384);
                    assert!(samples.iter().all(|s| s.to_bits() == 0.25_f32.to_bits()));
                }
                MSG_NOTE_EVENT => assert_eq!(header.payload_len, 36),
                other => panic!("unexpected frame {other}"),
            }
            tx.send(()).unwrap();
            thread::park_timeout(Duration::from_millis(1));
        }
    });
    let samples = vec![0.25; 16_384];
    for _ in 0..500 {
        rig.audio
            .send_note(NoteEventMsg {
                event_type: NOTE_ON,
                channel: 0,
                note: 60,
                velocity: 100,
                source: [0; 16],
                target: [0; 16],
            })
            .unwrap();
        assert!(rig.audio.send_audio(8_192, &samples));
        rx.recv().unwrap();
    }
    read.join().unwrap();
}

#[test]
fn blocks_dropped_counts_a_discarded_backlog_once() {
    let path = socket();
    let (producer, consumer) = HeapRb::<u8>::new(16_384).split();
    let (link, mut audio) = start_link(config(&path), None, false).unwrap();
    audio.wire = producer;
    link.state.connected.store(true, Ordering::Release);
    for _ in 0..3 {
        audio.send_audio(256, &[0.25; 512]);
    }
    let before = link.status().blocks_dropped;
    let mut worker = LinkWorker {
        config: config(&path),
        state: Arc::clone(&link.state),
        wire: consumer,
        audio_frame_left: 0,
        messages: HeapRb::new(1).split().0,
        callback_requests: HeapRb::new(1).split().1,
        ui_requests: mpsc::sync_channel(1).1,
        client: None,
        stuck_since: None,
        refusal_delay: RETRY_INTERVAL,
        health_before: DeskHealthMsg::default(),
        health_now: DeskHealthMsg::default(),
    };
    worker.discard_backlog();
    assert_eq!(link.status().blocks_dropped - before, 1);
}
