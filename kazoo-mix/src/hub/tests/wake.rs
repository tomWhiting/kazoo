use super::*;

const BLOCKS: usize = 10_000;

#[test]
fn an_audio_message_is_taken_the_moment_it_arrives() {
    let rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-test", RATE).unwrap();
    let mut elapsed = Vec::with_capacity(BLOCKS);
    for count in 1..=BLOCKS {
        let start = Instant::now();
        client.send_audio(0, 0, &[]).unwrap();
        while rig.hub.snapshot().blocks < count as u64 {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "audio not counted"
            );
            std::hint::spin_loop();
        }
        elapsed.push(start.elapsed());
    }
    elapsed.sort_unstable();
    let p50 = elapsed[BLOCKS / 2];
    let p99 = elapsed[BLOCKS * 99 / 100];
    eprintln!(
        "BENCH hub blocks={BLOCKS} p50_us={:.3} p99_us={:.3}",
        p50.as_secs_f64() * 1_000_000.0,
        p99.as_secs_f64() * 1_000_000.0
    );
    assert!(p50 < Duration::from_micros(200), "p50={p50:?}, p99={p99:?}");
    assert_eq!(rig.hub.snapshot().blocks, BLOCKS as u64);
}

#[test]
fn a_transport_anchor_still_reaches_an_instrument_within_the_tick_budget() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-test", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    rig.until("the strip to be patched in", |rig| {
        rig.shared.channel_readout(0).connected
    });
    // The instrument's side, timing the anchor, runs real-time as an
    // instrument's audio thread does; it waits in short sleeps, not a spin,
    // and renders into a buffer made before it promotes.
    let mut data = vec![0.0_f32; 512];
    let promoted = real_time(Duration::from_millis(1), "an instrument's audio thread");
    let start = Instant::now();
    rig.shared.set_playing(true);
    rig.render_into(&mut data);
    loop {
        if let Some(HubMessage::TransportSync(sync)) = client.try_recv().unwrap()
            && sync.state == TRANSPORT_PLAYING
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "anchor not forwarded"
        );
        thread::sleep(Duration::from_micros(100));
    }
    let elapsed = start.elapsed();
    promoted
        .demote()
        .expect("the instrument's side leaves real time");
    assert!(
        elapsed < Duration::from_millis(50),
        "anchor arrived late: {elapsed:?}"
    );
}

#[test]
fn a_registering_connection_beyond_the_poll_set_is_still_registered() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    // Occupied strips leave fewer poll entries than the silent connections.
    let mut instruments: Vec<_> = (0..DESK_CHANNELS - 1)
        .map(|_| {
            let client = rig.connect("kazoo-test", RATE).unwrap();
            assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
            client
        })
        .collect();
    let pending: [UnixStream; 20] =
        std::array::from_fn(|_| UnixStream::connect(&rig.socket).unwrap());
    let mut stream = UnixStream::connect(&rig.socket).unwrap();
    stream.set_nonblocking(true).unwrap();
    // Audio is read after accept: its count proves the silent sockets are accepted.
    instruments[0].send_audio(0, 0, &[]).unwrap();
    let accepted = Instant::now();
    while rig.hub.snapshot().blocks == 0 {
        assert!(
            accepted.elapsed() < Duration::from_secs(3),
            "audio not counted"
        );
        std::hint::spin_loop();
    }
    let mut buffer = FrameBuffer::new();
    let register = RegisterMsg::new("kazoo-test", 2, RATE, 256);
    let len = register.encode(buffer.payload_mut());
    let start = Instant::now();
    buffer
        .write_frame(MSG_REGISTER, 0, len, &mut stream)
        .unwrap();
    let header = loop {
        if let Some(header) = buffer.try_read_frame(&mut stream).unwrap() {
            break header;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "registration arrived late"
        );
        std::hint::spin_loop();
    };
    assert_eq!(header.msg_type, MSG_REGISTERED);
    let registered = RegisteredMsg::decode(buffer.payload());
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "registration arrived late"
    );
    assert_eq!(usize::from(registered.strip_index), instruments.len());
    drop(pending);
}
