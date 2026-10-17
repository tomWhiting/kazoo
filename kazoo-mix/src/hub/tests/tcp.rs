use std::net::{SocketAddr, TcpStream};

use kazoo_core::ipc::protocol::encode_audio_payload;
use kazoo_core::ipc::types::MSG_AUDIO;

use super::*;
use crate::hub::instrument::{NETWORK_LEAD_MOST, NETWORK_LEAD_START};

fn rig() -> Rig {
    Rig::with_network(
        [false; DESK_CHANNELS],
        SharedState::new(),
        Some("127.0.0.1:0".parse().unwrap()),
    )
}

struct Peer {
    stream: TcpStream,
    buffer: FrameBuffer,
    registered: RegisteredMsg,
}

impl Peer {
    fn connect(address: SocketAddr) -> Self {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.set_nodelay(true).unwrap();
        // Readiness drives progress; these bounds only turn a missing reply into failure.
        stream.set_read_timeout(Some(REGISTRATION_TIMEOUT)).unwrap();
        stream
            .set_write_timeout(Some(REGISTRATION_TIMEOUT))
            .unwrap();
        let mut buffer = FrameBuffer::new();
        let register = RegisterMsg::new("kazoo-network", 2, RATE, 256);
        let len = register.encode(buffer.payload_mut());
        buffer
            .write_frame(MSG_REGISTER, 0, len, &mut stream)
            .unwrap();
        let header = buffer.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_REGISTERED);
        let registered = RegisteredMsg::decode(buffer.payload());
        Self {
            stream,
            buffer,
            registered,
        }
    }

    fn audio(&mut self, frame: u64, frames: u32, samples: &[f32]) {
        let len = encode_audio_payload(frame, frames, samples, self.buffer.payload_mut());
        self.buffer
            .write_frame(MSG_AUDIO, 0, len, &mut self.stream)
            .unwrap();
    }

    fn anchor(&mut self, state: u8) -> TransportSyncMsg {
        let budget = Duration::from_millis(50);
        self.stream.set_read_timeout(Some(budget)).unwrap();
        let started = Instant::now();
        loop {
            assert!(started.elapsed() < budget, "anchor not forwarded");
            let header = self.buffer.read_frame(&mut self.stream).unwrap();
            if header.msg_type == MSG_TRANSPORT_SYNC {
                let sync = TransportSyncMsg::decode(self.buffer.payload());
                if sync.state == state {
                    return sync;
                }
            }
        }
    }
}

#[test]
fn tcp_registration_seats_audio_and_receives_a_transport_anchor() {
    let mut rig = rig();
    let address = rig.hub.listen_addr().expect("TCP listener was requested");
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);
    let mut peer = Peer::connect(address);
    assert_eq!(peer.registered.strip_index, 0);
    assert_eq!(peer.registered.hub_sample_rate, RATE);
    assert!(matches!(rig.notice(), HubNotice::Joined { slot: 0, .. }));
    rig.until("the TCP strip", |rig| {
        rig.shared.channel_readout(0).connected
    });
    peer.audio(0, 256, &[0.25; 512]);
    let sent = Instant::now();
    while rig.hub.snapshot().blocks < 1 {
        assert!(sent.elapsed() < Duration::from_secs(3), "audio not counted");
        std::hint::spin_loop();
    }
    let mut heard = false;
    for _ in 0..16 {
        heard |= rig
            .render()
            .iter()
            .any(|sample| (*sample - 0.25).abs() < 0.02);
    }
    assert!(heard, "TCP audio did not reach the output");
    rig.shared.set_playing(true);
    rig.render();
    let anchor = peer.anchor(TRANSPORT_PLAYING);
    assert_ne!(anchor.at_frame, SYNC_NOW);
    assert_eq!(anchor.bpm.to_bits(), 120.0_f32.to_bits());
    assert_eq!(rig.hub.snapshot().instruments, 1);
}

#[test]
fn without_listen_the_hub_has_no_tcp_listener() {
    let rig = Rig::new([false; DESK_CHANNELS]);
    assert_eq!(rig.hub.listen_addr(), None);
    assert_eq!(
        HubConfig::standard(RATE, [false; DESK_CHANNELS]).listen,
        None
    );
}

#[test]
fn tcp_audio_is_taken_the_moment_it_arrives() {
    const BLOCKS: usize = 10_000;
    let rig = rig();
    let mut peer = Peer::connect(rig.hub.listen_addr().unwrap());
    let mut elapsed = Vec::with_capacity(BLOCKS);
    for count in 1..=BLOCKS {
        let start = Instant::now();
        peer.audio(0, 0, &[]);
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
        "BENCH TCP hub blocks={BLOCKS} p50_us={:.3} p99_us={:.3}",
        p50.as_secs_f64() * 1_000_000.0,
        p99.as_secs_f64() * 1_000_000.0
    );
    assert!(p50 < Duration::from_micros(200), "p50={p50:?}, p99={p99:?}");
    assert_eq!(rig.hub.snapshot().blocks, BLOCKS as u64);
}

#[test]
fn tcp_transport_anchor_arrives_within_the_tick_budget() {
    let mut rig = rig();
    let mut peer = Peer::connect(rig.hub.listen_addr().unwrap());
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    rig.until("the TCP strip", |rig| {
        rig.shared.channel_readout(0).connected
    });
    let mut data = vec![0.0; 512];
    let promoted = real_time(Duration::from_millis(1), "an instrument's audio thread");
    let start = Instant::now();
    rig.shared.set_playing(true);
    rig.render_into(&mut data);
    peer.anchor(TRANSPORT_PLAYING);
    let elapsed = start.elapsed();
    promoted.demote().unwrap();
    eprintln!(
        "BENCH TCP transport anchor_us={:.3}",
        elapsed.as_secs_f64() * 1_000_000.0
    );
    assert!(
        elapsed < Duration::from_millis(50),
        "anchor arrived late: {elapsed:?}"
    );
}

#[test]
fn tcp_jittery_timer_is_placed_once_and_stays_placed() {
    use kazoo_core::host::link::connect_config_or_direct;
    use kazoo_core::ipc::link::{HubAddress, LinkConfig};
    use std::sync::atomic::AtomicBool;

    let mut rig = rig();
    let (link, mut audio) = connect_config_or_direct(LinkConfig {
        address: HubAddress::Tcp(rig.hub.listen_addr().unwrap().to_string()),
        pace_lead_frames: 1_920,
        ..LinkConfig::new("kazoo-wall", 2, RATE, 256)
    })
    .unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let source_running = Arc::clone(&running);
    // Whatever way the test ends, a panic included, the source stops.
    let stop = Stop(Arc::clone(&running));
    // A source like the headless wall: render what the desk says is owed,
    // on its own timer until the desk says; and every tenth of a second,
    // oversleep by 20 to 30 ms, as a busy machine does. It runs real-time,
    // as the wall's render thread does, with the wall's period (four
    // 256-frame blocks a wake); the desk below runs real-time as its audio
    // callback does. The hub and the link's worker ask for real time
    // already. So the jitter is the source's own, not the machine's.
    let source = thread::spawn(move || {
        let block = vec![0.1_f32; 512];
        let promoted = real_time(
            Duration::from_secs_f64(4.0 * 256.0 / f64::from(RATE)),
            "the wall's render thread",
        );
        let started = Instant::now();
        let mut sent = 0_u64;
        let mut next_hiccup = started + Duration::from_millis(100);
        let mut hiccup = 20_u64;
        while source_running.load(Ordering::Acquire) {
            let now = Instant::now();
            if now >= next_hiccup {
                thread::sleep(Duration::from_millis(hiccup));
                hiccup = if hiccup >= 30 { 20 } else { hiccup + 5 };
                next_hiccup = now + Duration::from_millis(100);
                continue;
            }
            let due = audio.desk_owes(now).map_or_else(
                || sent as f64 <= now.duration_since(started).as_secs_f64() * f64::from(RATE),
                |owes| owes.frames > 0,
            );
            if due {
                audio.send_audio(256, &block);
                sent += 256;
            } else {
                thread::sleep(Duration::from_micros(500));
            }
        }
        promoted.demote().expect("the source leaves real time");
    });
    // The desk plays in real time: a 256-frame buffer every 5⅓ ms.
    let buffer = Duration::from_secs_f64(256.0 / f64::from(RATE));
    let mut data = vec![0.0_f32; 512];
    let desk = real_time(buffer, "the desk's audio callback");
    let started = Instant::now();
    let mut buffers = 0_u32;
    while started.elapsed() < Duration::from_millis(1_500) {
        let due = started + buffer * buffers;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        rig.render_into(&mut data);
        buffers += 1;
    }
    desk.demote().expect("the desk leaves real time");
    drop(stop);
    source.join().unwrap();
    let snapshot = rig.hub.snapshot();
    eprintln!("BENCH TCP jitter {snapshot:?}");
    assert!(snapshot.blocks > 200, "{snapshot:?}");
    assert_eq!(snapshot.resyncs, 1, "{snapshot:?}");
    drop(link);
}

#[test]
fn a_second_hub_with_the_same_tcp_listener_reports_the_existing_hub() {
    let rig = rig();
    let (patches, callback) = patchbay(PATCH_CAPACITY);
    let result = Hub::start(
        HubConfig {
            socket: rig.socket.clone(),
            listen: rig.hub.listen_addr(),
            advertise: false,
            sample_rate: RATE,
            reserved: [false; DESK_CHANNELS],
        },
        Arc::clone(&rig.shared),
        patches,
    );
    assert!(
        matches!(result, Err(HubStartError::AnotherHub { socket, .. }) if socket == rig.socket)
    );
    drop(callback);
}

#[test]
fn a_failed_tcp_bind_leaves_no_new_unix_socket() {
    let rig = rig();
    let socket = unique_socket();
    let (patches, callback) = patchbay(PATCH_CAPACITY);
    let result = Hub::start(
        HubConfig {
            socket: socket.clone(),
            listen: rig.hub.listen_addr(),
            advertise: false,
            sample_rate: RATE,
            reserved: [false; DESK_CHANNELS],
        },
        Arc::clone(&rig.shared),
        patches,
    );
    assert!(matches!(result, Err(HubStartError::Io(error))
        if error.kind() == std::io::ErrorKind::AddrInUse));
    assert!(!socket.exists());
    drop(callback);
}

/// The next notice, which must be strip 0's network lead.
fn network_lead(rig: &mut Rig) -> Duration {
    match rig.notice() {
        HubNotice::NetworkLead { slot: 0, lead, .. } => lead,
        other => panic!("expected strip 0's network lead, got {other:?}"),
    }
}

/// Wait until the hub has counted `blocks` blocks.
fn counted(rig: &Rig, blocks: u64) {
    let sent = Instant::now();
    while rig.hub.snapshot().blocks < blocks {
        assert!(sent.elapsed() < Duration::from_secs(3), "audio not counted");
        std::hint::spin_loop();
    }
}

#[test]
fn a_network_stream_has_its_own_lead_doubled_each_time_it_comes_late_up_to_the_cap() {
    let mut rig = rig();
    let mut peer = Peer::connect(rig.hub.listen_addr().unwrap());
    assert!(matches!(rig.notice(), HubNotice::Joined { slot: 0, .. }));
    assert_eq!(network_lead(&mut rig), NETWORK_LEAD_START);
    rig.until("the TCP strip", |rig| {
        rig.shared.channel_readout(0).connected
    });
    // The first block places the stream: that is not coming late.
    peer.audio(0, 256, &[0.0; 512]);
    counted(&rig, 1);
    // Each block that goes back to frame 0 is placed afresh, as a late one.
    let mut expected = NETWORK_LEAD_START;
    let mut blocks = 1;
    while expected < NETWORK_LEAD_MOST {
        peer.audio(0, 256, &[0.0; 512]);
        blocks += 1;
        expected = (expected * 2).min(NETWORK_LEAD_MOST);
        assert_eq!(network_lead(&mut rig), expected);
    }
    assert_eq!(expected, Duration::from_millis(160));
    // At the cap, coming late again grows nothing and says nothing.
    peer.audio(0, 256, &[0.0; 512]);
    counted(&rig, blocks + 1);
    assert_eq!(rig.hub.snapshot().resyncs, blocks + 1);
    assert_eq!(rig.hub.next_notice().unwrap(), None);
}

#[test]
fn a_local_stream_has_no_network_lead() {
    let mut rig = rig();
    let mut client = rig.connect("kazoo-local", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { slot: 0, .. }));
    rig.until("the strip", |rig| rig.shared.channel_readout(0).connected);
    for _ in 0..3 {
        client.send_audio(0, 256, &[0.0; 512]).unwrap();
    }
    counted(&rig, 3);
    assert_eq!(rig.hub.snapshot().resyncs, 3);
    assert_eq!(rig.hub.next_notice().unwrap(), None);
}
