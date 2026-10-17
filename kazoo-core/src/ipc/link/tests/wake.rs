use super::*;
use crate::ipc::protocol::FrameHeader;

const BLOCKS: usize = 10_000;

/// Schedule the calling thread as real-time for work every `period`, as the
/// instrument thread it stands in for is. A test that times the link from a
/// thread at normal priority measures whatever else the machine is doing.
/// Any process may ask on macOS; on Linux the process needs `RLIMIT_RTPRIO`
/// above 0 or `CAP_SYS_NICE`, and without it the test says so and stops.
fn real_time(period: Duration, stands_for: &str) -> crate::realtime::RealTime {
    crate::realtime::RealTime::promote(period).unwrap_or_else(|why| {
        panic!(
            "{stands_for} runs real-time in the studio, so it must here: {why} \
             (on Linux, give the process RLIMIT_RTPRIO above 0 or CAP_SYS_NICE)"
        )
    })
}

struct Connection {
    link: HubLink,
    audio: HubLinkAudio,
    stream: UnixStream,
    buffer: FrameBuffer,
    path: PathBuf,
}

impl Connection {
    fn new() -> Self {
        Self::with_config(LinkConfig::new("kazoo-test", 2, 48_000, 256))
    }

    fn with_config(mut config: LinkConfig) -> Self {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        config.address = HubAddress::Socket(path.clone());
        // Measure the same scheduling request every instrument makes.
        let (link, mut audio) = hub_link(config).unwrap();
        let (mut stream, mut buffer) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        // A round trip establishes that registration has finished serving.
        assert!(audio.send_audio(256, &[0.0; 512]));
        assert_eq!(buffer.read_frame(&mut stream).unwrap().msg_type, MSG_AUDIO);
        match link.real_time() {
            Some(Ok(())) => {}
            Some(Err(why)) => panic!("production link was refused real-time scheduling: {why}"),
            None => panic!("production link did not ask for real-time scheduling"),
        }
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

/// Whether at least 99% of the sends were followed by a woken park.
///
/// `ticked` is reported, never bounded: between one frame arriving and the
/// next send, the link thread rightly times out on its tick whenever the
/// sending thread is off the processor for a tick, so its count measures the
/// machine's load, not the wake.
fn check_wake_counts(blocks: u64, woken: u64, ticked: u64) -> Result<(), String> {
    let minimum_woken = blocks * 99 / 100;
    if woken < minimum_woken {
        return Err(format!(
            "blocks={blocks}, woken={woken} (minimum {minimum_woken}), ticked={ticked}"
        ));
    }
    Ok(())
}

#[test]
fn a_descheduled_sender_is_not_a_lost_wake() {
    // The counts of a run at load 141 to 312 on Tom's Mac (KZ-B01c): every
    // send was served by a woken park, and the link also timed out on its
    // tick whenever the sending thread was off the processor for a tick.
    check_wake_counts(10_000, 9_949, 2_225).unwrap();
}

#[test]
fn a_lost_wake_is_seen() {
    const LOST_BLOCKS: u64 = 200;
    let mut connection = Connection::with_config(LinkConfig::for_test_without_wake(
        "kazoo-test",
        2,
        48_000,
        256,
    ));
    let (woken_before, ticked_before) = connection.link.wake_counts();
    for _ in 0..LOST_BLOCKS {
        assert!(connection.audio.send_audio(256, &[0.25; 512]));
        assert_eq!(connection.read_frame().msg_type, MSG_AUDIO);
    }
    let (woken_after, ticked_after) = connection.link.wake_counts();
    let woken = woken_after - woken_before;
    let ticked = ticked_after - ticked_before;
    assert!(woken <= LOST_BLOCKS / 100, "woken={woken}, ticked={ticked}");
    let failure = check_wake_counts(LOST_BLOCKS, woken, ticked).unwrap_err();
    eprintln!("lost wake rejected: {failure}");
}

#[test]
fn a_queued_block_wakes_the_link_thread_at_once() {
    let mut connection = Connection::new();
    let block = [0.25; 512];
    let mut elapsed = Vec::with_capacity(BLOCKS);
    let (woken_before, ticked_before) = connection.link.wake_counts();
    for _ in 0..BLOCKS {
        let start = Instant::now();
        assert!(connection.audio.send_audio(256, &block));
        let header = connection.read_frame();
        elapsed.push(start.elapsed());
        assert_eq!(header.msg_type, MSG_AUDIO);
        assert_eq!(header.payload_len as usize, AUDIO_PAYLOAD_HEADER + 512 * 4);
    }
    let (woken_after, ticked_after) = connection.link.wake_counts();
    let woken = woken_after - woken_before;
    let ticked = ticked_after - ticked_before;
    elapsed.sort_unstable();
    let p50 = elapsed[BLOCKS / 2];
    let p99 = elapsed[BLOCKS * 99 / 100];
    eprintln!(
        "BENCH instrument blocks={BLOCKS} p50_us={:.3} p99_us={:.3} woken={woken} ticked={ticked} real_time={:?}",
        p50.as_secs_f64() * 1_000_000.0,
        p99.as_secs_f64() * 1_000_000.0,
        connection.link.real_time()
    );
    if let Err(why) = check_wake_counts(BLOCKS as u64, woken, ticked) {
        panic!("{why}; p50={p50:?}, p99={p99:?}");
    }
}

#[test]
fn a_ui_request_is_carried_without_waiting_for_the_tick() {
    let mut connection = Connection::new();
    // The side timing it stands in for the hub, which runs real-time.
    let promoted = real_time(Duration::from_millis(2), "the hub thread");
    let mut slowest = Duration::ZERO;
    for _ in 0..10 {
        let start = Instant::now();
        connection.link.request_tempo(100.0).unwrap();
        let header = connection.read_frame();
        slowest = slowest.max(start.elapsed());
        assert_eq!(header.msg_type, MSG_TRANSPORT_REQUEST);
    }
    promoted.demote().expect("the timing side leaves real time");
    assert!(slowest < Duration::from_millis(1), "slowest={slowest:?}");
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
    // The instrument's side, timing it, runs real-time as an instrument's
    // audio thread does; it waits in short sleeps, not a spin.
    let promoted = real_time(LINK_TICK, "an instrument's audio thread");
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
        std::thread::sleep(Duration::from_micros(100));
    }
    let elapsed = start.elapsed();
    promoted
        .demote()
        .expect("the instrument's side leaves real time");
    assert!(
        elapsed < 2 * LINK_TICK,
        "sync exceeded two link ticks: {elapsed:?}"
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
