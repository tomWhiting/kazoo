//! End-to-end hub tests: real sockets, the client instruments use, and the
//! real audio callback path (patchbay, engine, shared state).

mod named_link;
mod tcp;
pub mod tempo;
mod transport;
mod wake;

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kazoo_core::ipc::client::{HubIpcClient, HubMessage};
use kazoo_core::ipc::types::{
    DeskHealthMsg, DeskPaceMsg, NOTE_ON, NoteEventMsg, SYNC_NOW, TRANSPORT_PLAYING,
    TRANSPORT_UNCHANGED,
};

use super::instrument::{HEALTH_INTERVAL, PACE_INTERVAL};

use super::*;
use crate::callback::AudioCallback;
use crate::engine::MixerEngine;
use crate::patchbay::patchbay;
use crate::test_support::floats_equal;

const RATE: u32 = 48_000;

/// Schedule the calling thread as real-time for work every `period`, as the
/// thread it stands in for is in the studio. A test that times one must not
/// run it at normal priority: it would measure whatever else the machine is
/// doing. Any process may ask on macOS; on Linux the process needs
/// `RLIMIT_RTPRIO` above 0 or `CAP_SYS_NICE`, and without it the test says so
/// and stops rather than measure the scheduler.
fn real_time(period: Duration, stands_for: &str) -> kazoo_core::realtime::RealTime {
    kazoo_core::realtime::RealTime::promote(period).unwrap_or_else(|why| {
        panic!(
            "{stands_for} runs real-time in the studio, so it must here: {why} \
             (on Linux, give the process RLIMIT_RTPRIO above 0 or CAP_SYS_NICE)"
        )
    })
}

/// A hub on a private socket, with the desk's callback run by hand.
struct Rig {
    hub: Hub,
    callback: AudioCallback,
    shared: Arc<SharedState>,
    socket: PathBuf,
}

fn unique_socket() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("kzm-hub-{}-{n}.sock", std::process::id()))
}

impl Rig {
    fn new(reserved: [bool; DESK_CHANNELS]) -> Self {
        Self::with_shared(reserved, SharedState::new())
    }

    fn with_shared(reserved: [bool; DESK_CHANNELS], shared: SharedState) -> Self {
        Self::with_network(reserved, shared, None, RATE)
    }

    fn with_network(
        reserved: [bool; DESK_CHANNELS],
        shared: SharedState,
        listen: Option<SocketAddr>,
        rate: u32,
    ) -> Self {
        let shared = Arc::new(shared);
        let (hub_end, callback_end) = patchbay(PATCH_CAPACITY);
        let socket = unique_socket();
        let hub = Hub::start(
            HubConfig {
                socket: socket.clone(),
                advertise: false,
                listen,
                sample_rate: rate,
                reserved,
            },
            Arc::clone(&shared),
            hub_end,
        )
        .unwrap();
        let engine = MixerEngine::with_tempo(DESK_CHANNELS, rate, shared.transport().bpm).unwrap();
        let callback =
            AudioCallback::new(engine, Arc::clone(&shared), 2).with_patchbay(callback_end);
        Self {
            hub,
            callback,
            shared,
            socket,
        }
    }

    fn connect(&self, name: &str, rate: u32) -> std::io::Result<HubIpcClient> {
        HubIpcClient::connect_to(&self.socket, name, 2, rate, 256)
    }

    /// Run one 256-frame desk callback; returns its left channel.
    fn render(&mut self) -> Vec<f32> {
        let mut data = vec![0.0_f32; 512];
        self.render_into(&mut data);
        data.into_iter().step_by(2).collect()
    }

    /// Run one desk callback into `data`, interleaved stereo, allocating
    /// nothing: for a loop that stands in for the real callback.
    fn render_into(&mut self, data: &mut [f32]) {
        self.callback.process(data);
    }

    /// Run the desk until `done` holds, or fail after five seconds: longer
    /// than the hub's own two-second registration timeout.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.render();
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn notice(&mut self) -> HubNotice {
        let mut found = None;
        self.until("a hub notice", |rig| {
            found = rig.hub.next_notice().unwrap();
            found.is_some()
        });
        found.unwrap()
    }
}

fn receive(client: &mut HubIpcClient, what: &str) -> HubMessage {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(message) = client.try_recv().unwrap() {
            return message;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn an_instrument_plugs_into_the_first_free_strip_and_is_heard() {
    let mut reserved = [false; DESK_CHANNELS];
    reserved[0] = true;
    let mut rig = Rig::new(reserved);
    let mut client = rig.connect("kazoo-808", RATE).unwrap();
    assert_eq!(client.strip_index(), 1);
    assert_eq!(client.hub_sample_rate(), RATE);
    assert_eq!(
        rig.notice(),
        HubNotice::Joined {
            slot: 1,
            name: "kazoo-808".to_string()
        }
    );
    rig.until("the strip to be patched in", |rig| {
        rig.shared.channel_readout(1).connected
    });
    assert_eq!(
        crate::engine::name_str(&rig.shared.channel_readout(1).name),
        "808"
    );

    // Stream a steady level and wait for it to come out of the desk.
    let block = vec![0.25_f32; 256 * 2];
    let mut sent = 0;
    let mut heard = false;
    for _ in 0..400 {
        client.send_audio(sent, 256, &block).unwrap();
        sent += 256;
        thread::sleep(Duration::from_millis(1));
        let left = rig.render();
        if left.iter().any(|s| (s - 0.25).abs() < 0.02) {
            heard = true;
            break;
        }
    }
    assert!(
        heard,
        "the instrument's audio never reached the desk output"
    );
    assert!(rig.hub.snapshot().blocks > 0);
    assert_eq!(rig.hub.snapshot().instruments, 1);
}

#[test]
fn an_instrument_at_another_sample_rate_is_refused_and_told_why() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let refused = rig.connect("kazoo-mini", 44_100).unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(refused.to_string().contains("44100 Hz"), "{refused}");
    let notice = rig.notice();
    assert_eq!(
        notice,
        HubNotice::Refused {
            name: "kazoo-mini".to_string(),
            reason: RefuseReason::SampleRate {
                instrument: 44_100,
                desk: RATE
            },
            told: true,
        }
    );
    assert!(notice.to_string().contains("44100 Hz"));
    assert!(!rig.shared.channel_readout(0).connected);
}

#[test]
fn a_retrying_instrument_is_reported_once_until_something_else_happens() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    assert!(rig.connect("kazoo-mini", 44_100).is_err());
    assert!(matches!(rig.notice(), HubNotice::Refused { .. }));
    assert!(rig.connect("kazoo-mini", 44_100).is_err());
    let _joined = rig.connect("kazoo-808", RATE).unwrap();
    // The repeat refusal was not reported; the join was.
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    assert!(rig.connect("kazoo-mini", 44_100).is_err());
    assert!(matches!(rig.notice(), HubNotice::Refused { .. }));
}

#[test]
fn a_probe_that_connects_and_closes_is_not_reported() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    assert!(kazoo_core::ipc::discovery::hub_listening(&rig.socket).unwrap());
    let _joined = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
}

#[test]
fn a_second_hub_on_the_same_socket_is_refused() {
    let rig = Rig::new([false; DESK_CHANNELS]);
    let (hub_end, _callback_end) = patchbay(PATCH_CAPACITY);
    let second = Hub::start(
        HubConfig {
            socket: rig.socket.clone(),
            advertise: false,
            listen: None,
            sample_rate: RATE,
            reserved: [false; DESK_CHANNELS],
        },
        Arc::new(SharedState::new()),
        hub_end,
    );
    assert!(
        matches!(&second, Err(HubStartError::AnotherHub { socket, .. }) if *socket == rig.socket),
        "{second:?}"
    );
    // The first hub is untouched.
    assert!(rig.hub.is_running());
    assert!(kazoo_core::ipc::discovery::hub_listening(&rig.socket).unwrap());
}

#[test]
fn a_full_desk_refuses_the_next_instrument() {
    let mut reserved = [true; DESK_CHANNELS];
    reserved[3] = false;
    let mut rig = Rig::new(reserved);
    let _first = rig.connect("kazoo-cs80", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { slot: 3, .. }));
    assert!(rig.connect("kazoo-dx", RATE).is_err());
    assert!(matches!(
        rig.notice(),
        HubNotice::Refused {
            reason: RefuseReason::DeskFull,
            ..
        }
    ));
}

#[test]
fn leaving_frees_the_strip_for_the_next_instrument() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut first = rig.connect("kazoo-mini", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { slot: 0, .. }));
    rig.until("strip 1 patched in", |rig| {
        rig.shared.channel_readout(0).connected
    });

    first.send_shutdown().unwrap();
    assert_eq!(
        rig.notice(),
        HubNotice::Left {
            slot: 0,
            name: "kazoo-mini".to_string(),
            reason: LeaveReason::Goodbye
        }
    );
    rig.until("strip 1 unplugged", |rig| {
        !rig.shared.channel_readout(0).connected
    });

    let second = rig.connect("kazoo-dx", RATE).unwrap();
    assert_eq!(second.strip_index(), 0);
}

#[test]
fn a_dropped_connection_is_noticed_and_unplugged() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let client = rig.connect("kazoo-arp", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    drop(client);
    assert!(matches!(
        rig.notice(),
        HubNotice::Left {
            reason: LeaveReason::Disconnected(_),
            ..
        }
    ));
    rig.until("the strip unplugged", |rig| {
        rig.hub.snapshot().instruments == 0
    });
}

#[test]
fn tempo_and_play_state_reach_every_instrument_both_ways() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));

    // The desk changes tempo and plays: the instrument hears about it once
    // the desk's callback has scheduled it.
    rig.shared.set_tempo(96.0);
    rig.shared.set_playing(true);
    let mut heard = None;
    rig.until("a transport sync", |_| {
        while let Some(message) = client.try_recv().unwrap() {
            if let HubMessage::TransportSync(sync) = message {
                if floats_equal(sync.bpm, 96.0) && sync.state == TRANSPORT_PLAYING {
                    heard = Some(sync);
                }
            }
        }
        heard.is_some()
    });

    // The instrument asks for a change: the desk follows.
    client
        .send_transport_request(TRANSPORT_STOPPED, Some(140.0))
        .unwrap();
    rig.until("the desk to follow the instrument", |rig| {
        let transport = rig.shared.transport();
        !transport.playing && (transport.bpm - 140.0).abs() < 1e-3
    });

    // A tempo-only request changes the tempo and nothing else.
    rig.shared.set_playing(true);
    client
        .send_transport_request(TRANSPORT_UNCHANGED, Some(110.0))
        .unwrap();
    rig.until("the desk to take the new tempo", |rig| {
        (rig.shared.transport().bpm - 110.0).abs() < 1e-3
    });
    assert!(
        rig.shared.transport().playing,
        "a tempo change must not stop the desk"
    );
}

/// Stream one 256-frame block from `client`, wait the block's own length
/// (the desk runs in real time, as a device would drive it), and render one
/// desk buffer. Returns the desk's left channel with the studio frame of its
/// first sample.
fn step(
    rig: &mut Rig,
    client: &mut HubIpcClient,
    sent: &mut u64,
    block: &[f32],
) -> (u64, Vec<f32>) {
    client.send_audio(*sent, 256, block).unwrap();
    *sent += 256;
    thread::sleep(Duration::from_secs_f64(256.0 / f64::from(RATE)));
    let frame = rig.callback.engine().next_frame();
    (frame, rig.render())
}

#[test]
fn an_instrument_starts_on_the_very_frame_the_desk_does() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let silence = vec![0.0_f32; 512];
    let mut sent = 0_u64;

    // Once its first audio is placed on the studio clock, the hub tells the
    // instrument where the song is in the instrument's own frames.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut placed = false;
    while !placed {
        assert!(Instant::now() < deadline, "never placed");
        step(&mut rig, &mut client, &mut sent, &silence);
        while let Some(message) = client.try_recv().unwrap() {
            if let HubMessage::TransportSync(sync) = message {
                placed |= sync.at_frame != SYNC_NOW;
            }
        }
    }

    // Play. The instrument puts an impulse on the frame it is told the song
    // starts on; it must come out of the desk on the frame the desk's own
    // song starts on.
    rig.shared.set_playing(true);
    let mut impulse_at: Option<u64> = None;
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "the song never started");
        let mut block = vec![0.0_f32; 512];
        if let Some(at) = impulse_at.filter(|at| (sent..sent + 256).contains(at)) {
            let index = usize::try_from(at - sent).unwrap() * 2;
            block[index] = 0.5;
            block[index + 1] = 0.5;
        }
        let (frame, left) = step(&mut rig, &mut client, &mut sent, &block);
        output.extend(
            left.into_iter()
                .enumerate()
                .map(|(i, s)| (frame + i as u64, s)),
        );
        while let Some(message) = client.try_recv().unwrap() {
            if let HubMessage::TransportSync(sync) = message {
                if sync.state == TRANSPORT_PLAYING {
                    assert!(
                        sync.at_frame >= sent,
                        "the change reached the instrument after its frame: {} < {sent}",
                        sync.at_frame
                    );
                    assert!(floats_equal(sync.beat as f32, 0.0), "{sync:?}");
                    impulse_at = Some(sync.at_frame);
                }
            }
        }
        let song = rig.callback.engine().song();
        let played_past = output
            .last()
            .is_some_and(|(frame, _)| song.playing && *frame > song.frame + 8_192);
        if played_past {
            break;
        }
    }
    let start = rig.callback.engine().song().frame;
    let heard = output
        .iter()
        .find(|(_, sample)| sample.abs() > 0.05)
        .map(|(frame, _)| *frame);
    assert_eq!(
        heard,
        Some(start),
        "the instrument's downbeat and the desk's"
    );
}

#[test]
fn untargeted_notes_reach_every_other_instrument() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut arp = rig.connect("kazoo-arp", RATE).unwrap();
    let mut mini = rig.connect("kazoo-mini", RATE).unwrap();
    for _ in 0..2 {
        assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    }
    let note = NoteEventMsg {
        source: *arp.instrument_id(),
        target: [0; 16],
        event_type: NOTE_ON,
        channel: 0,
        note: 60,
        velocity: 100,
    };
    arp.send_note_event(&note).unwrap();
    let received = loop {
        if let HubMessage::NoteEvent(event) = receive(&mut mini, "the routed note") {
            break event;
        }
    };
    assert_eq!((received.note, received.velocity), (60, 100));
    // The sender does not hear its own note back.
    thread::sleep(Duration::from_millis(20));
    while let Some(message) = arp.try_recv().unwrap() {
        assert!(!matches!(message, HubMessage::NoteEvent(_)));
    }
}

#[test]
fn a_connection_that_never_registers_is_refused() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let stream = UnixStream::connect(&rig.socket).unwrap();
    let notice = rig.notice();
    assert!(
        matches!(
            &notice,
            HubNotice::Refused {
                reason: RefuseReason::Handshake(_),
                ..
            }
        ),
        "{notice:?}"
    );
    drop(stream);
}

#[test]
fn a_malformed_message_disconnects_only_that_instrument() {
    use std::io::Write as _;
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let _good = rig.connect("kazoo-cs80", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));

    // Register by hand, then send an audio frame claiming more samples than
    // it carries.
    let mut raw = UnixStream::connect(&rig.socket).unwrap();
    let register = RegisterMsg::new("rogue", 2, RATE, 256);
    let mut buf = FrameBuffer::new();
    register.encode(buf.payload_mut());
    buf.write_frame(MSG_REGISTER, 0, RegisterMsg::WIRE_SIZE, &mut raw)
        .unwrap();
    buf.read_frame(&mut raw).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { slot: 1, .. }));
    let mut bad = Vec::new();
    bad.extend_from_slice(&[kazoo_core::ipc::types::MSG_AUDIO]);
    bad.extend_from_slice(&8_u32.to_le_bytes()); // payload length
    bad.extend_from_slice(&0_u32.to_le_bytes()); // sequence
    bad.extend_from_slice(&100_u32.to_le_bytes()); // 100 frames claimed
    bad.extend_from_slice(&[0; 4]);
    raw.write_all(&bad).unwrap();

    let notice = rig.notice();
    assert!(
        matches!(
            &notice,
            HubNotice::Left {
                slot: 1,
                reason: LeaveReason::Protocol(_),
                ..
            }
        ),
        "{notice:?}"
    );
    assert_eq!(rig.hub.snapshot().instruments, 1);
}

#[test]
fn stopping_the_hub_tells_instruments_and_removes_the_socket() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-dx", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let socket = rig.socket.clone();
    drop(rig);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match client.try_recv() {
            Ok(Some(HubMessage::Shutdown)) | Err(_) => break,
            Ok(_) => {
                assert!(Instant::now() < deadline, "never told the hub closed");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert!(!socket.exists());
}

/// A wrapped frame distance, signed.
fn signed(distance: u64) -> i64 {
    i64::from_ne_bytes(distance.to_ne_bytes())
}

/// Send `client` audio and run the desk until it has told the client where
/// it is playing its stream, or fail after two seconds.
fn paced(rig: &mut Rig, client: &mut HubIpcClient, sent: &mut u64) -> Option<DeskPaceMsg> {
    let block = vec![0.0_f32; 256 * 2];
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        client.send_audio(*sent, 256, &block).unwrap();
        *sent += 256;
        rig.render();
        thread::sleep(Duration::from_millis(1));
        while client.try_recv().unwrap().is_some() {}
        if let Some(pace) = client.take_desk_pace() {
            return Some(pace);
        }
    }
    None
}

#[test]
fn a_timer_paced_instrument_is_told_where_the_desk_plays_it_and_gets_its_lead() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut register = RegisterMsg::new("kazoo-wall", 2, RATE, 256);
    // The tenth of a second the headless wall asks for at 48 kHz.
    register.pace_lead_frames = 4_800;
    let mut client = HubIpcClient::register_at(&rig.socket, &register).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let mut sent = 0;
    let first = paced(&mut rig, &mut client, &mut sent).expect("a pace");
    // Its lead is the desk's usual (a device buffer, a block, a margin)
    // and the 4 800 frames it asked for.
    assert!(first.lead_frames >= 4_800 + 256 + 256, "{first:?}");
    assert!(first.lead_frames < 4_800 + 4 * 1_024, "{first:?}");
    // The desk plays behind what it was sent, by about that lead (at
    // first before the stream's frame 0: the frame wraps, as stream
    // frames do, and distances are signed).
    let behind = signed(sent.wrapping_sub(first.playing_stream));
    assert!(
        behind > 0 && behind <= i64::from(first.lead_frames) + 2_048,
        "{first:?} with {sent} sent"
    );
    // And the stream moves on as the desk plays.
    thread::sleep(PACE_INTERVAL);
    let later = paced(&mut rig, &mut client, &mut sent).expect("another pace");
    assert!(
        signed(later.playing_stream.wrapping_sub(first.playing_stream)) > 0,
        "{later:?} after {first:?}"
    );
}

#[test]
fn a_device_clocked_instrument_is_never_paced() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let mut sent = 0;
    let deadline = Instant::now() + Duration::from_millis(200);
    let block = vec![0.0_f32; 256 * 2];
    while Instant::now() < deadline {
        client.send_audio(sent, 256, &block).unwrap();
        sent += 256;
        rig.render();
        thread::sleep(Duration::from_millis(1));
        while client.try_recv().unwrap().is_some() {}
        assert_eq!(client.take_desk_pace(), None);
    }
}

#[test]
fn an_extra_lead_is_held_to_what_the_strip_ring_holds() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut register = RegisterMsg::new("kazoo-wall", 2, RATE, 256);
    register.pace_lead_frames = u32::MAX;
    let mut client = HubIpcClient::register_at(&rig.socket, &register).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let mut sent = 0;
    let pace = paced(&mut rig, &mut client, &mut sent).expect("a pace");
    // The whole lead is at most half the stereo ring, and the extra lead
    // is what is left of that half after a device buffer, the largest
    // block and the margin.
    let strip = strip_ring(0, 2);
    let half = strip.capacity_blocks * strip.block_frames / 2;
    assert!(pace.lead_frames <= half, "{pace:?} with half a ring {half}");
    let most_extra = half - (256 + 4_096 + 144);
    assert!(pace.lead_frames >= most_extra + 256 + 256, "{pace:?}");
    // The wall's tenth of a second is well inside it.
    assert!(4_800 < most_extra);
}

#[test]
fn a_timer_paced_instruments_lead_is_never_over_a_second() {
    let ring = AudioRingConfig::new(BufferId(0), 1, 128, 1 << 20);
    let clock = DeskClock {
        studio_now: 0,
        elapsed_frames: 0,
        device_frames: 256,
        margin_frames: 144,
        slack_frames: 960,
    };
    assert_eq!(most_extra_lead(ring_frames(ring), clock, RATE, 4_096), RATE);
    // And a ring too small for any extra lead gives none.
    let tiny = AudioRingConfig::new(BufferId(0), 2, 128, 8);
    assert_eq!(most_extra_lead(ring_frames(tiny), clock, RATE, 4_096), 0);
}

#[test]
fn an_extra_lead_follows_the_device_buffer_and_never_passes_half_the_ring() {
    // Worked out again whenever the clock is read: a device buffer known
    // only once the desk plays, or one rebuilt larger, never takes the
    // whole lead past half the strip's ring.
    let ring = ring_frames(strip_ring(0, 2));
    for device_frames in [64, 256, 512, 2_048, 4_096] {
        let clock = DeskClock {
            studio_now: 0,
            elapsed_frames: 0,
            device_frames,
            margin_frames: 144,
            slack_frames: 960,
        };
        let extra = most_extra_lead(ring, clock, RATE, 4_096);
        let mut stamp = crate::studio_clock::StudioStamp::with_extra_lead(0);
        stamp.set_extra_lead(extra);
        assert_eq!(stamp.extra_lead(), extra);
        // The largest block an instrument may send, counted in the lead.
        let whole = stamp.lead(device_frames, 144) - 256 + 4_096;
        assert!(whole <= ring / 2, "{device_frames}: {whole} of {ring}");
    }
}

#[test]
fn a_registration_cut_short_inside_its_pace_is_refused() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut raw = UnixStream::connect(&rig.socket).unwrap();
    let mut register = RegisterMsg::new("kazoo-wall", 2, RATE, 256);
    register.pace_lead_frames = 1_920;
    let mut buf = FrameBuffer::new();
    register.encode(buf.payload_mut());
    buf.write_frame(MSG_REGISTER, 0, RegisterMsg::WIRE_SIZE + 2, &mut raw)
        .unwrap();
    let header = buf.read_frame(&mut raw).unwrap();
    assert_eq!(header.msg_type, MSG_REFUSED);
    let notice = rig.notice();
    assert!(
        matches!(&notice, HubNotice::Refused { reason: RefuseReason::Handshake(why), .. } if why.contains("61 bytes")),
        "{notice:?}"
    );
    assert_eq!(rig.hub.snapshot().instruments, 0);
}

/// Stops a test's own thread when dropped, however the test ends.
struct Stop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[test]
fn a_paced_source_with_a_jittery_timer_is_placed_once_and_stays_placed() {
    use kazoo_core::host::link::connect_config_or_direct;
    use kazoo_core::ipc::link::{HubAddress, LinkConfig};
    use std::sync::atomic::AtomicBool;

    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let (link, mut audio) = connect_config_or_direct(LinkConfig {
        address: HubAddress::Socket(rig.socket.clone()),
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
    assert!(snapshot.blocks > 200, "{snapshot:?}");
    assert_eq!(snapshot.resyncs, 1, "{snapshot:?}");
    drop(link);
}

#[test]
fn the_hub_thread_says_whether_it_is_real_time() {
    // The hub asks once as it starts, says how it went, and serves either
    // way. Any process may have the time-constraint policy on macOS.
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    rig.until("the hub thread to ask for real-time", |rig| {
        rig.hub.real_time().is_some()
    });
    #[cfg(target_os = "macos")]
    assert_eq!(rig.hub.real_time(), Some(&Ok(())));
    #[cfg(not(target_os = "macos"))]
    if let Some(Err(why)) = rig.hub.real_time() {
        assert!(!why.to_string().is_empty());
    }
    let _client = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
}

#[test]
fn an_instrument_is_told_when_its_stream_is_placed_afresh() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let block = vec![0.0_f32; 256 * 2];
    // The first placement is not news.
    client.send_audio(0, 256, &block).unwrap();
    client.send_audio(256, 256, &block).unwrap();
    rig.until("both blocks to be read", |rig| {
        rig.hub.snapshot().blocks == 2
    });
    let deadline = Instant::now() + Duration::from_millis(50);
    while Instant::now() < deadline {
        rig.render();
        while client.try_recv().unwrap().is_some() {}
    }
    assert_eq!(client.take_desk_health(), None);
    // A stream that goes backwards (as one that restarted does) is placed
    // afresh, and the instrument hears of it.
    client.send_audio(0, 256, &block).unwrap();
    let mut health = None;
    rig.until("the desk's health report", |rig| {
        rig.render();
        while client.try_recv().unwrap().is_some() {}
        health = client.take_desk_health();
        health.is_some()
    });
    assert_eq!(
        health,
        Some(DeskHealthMsg {
            placed_afresh: 1,
            dropped: 0,
        })
    );
}

#[test]
fn a_stream_dropped_for_a_full_ring_is_not_counted_as_placed_afresh_too() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut client = rig.connect("kazoo-808", RATE).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let block = vec![0.0_f32; 256 * 2];
    // The desk does not take the strip's audio: the ring fills, and the
    // blocks past it are dropped.
    // One at a time, each read before the next, so the socket never
    // fills, until the ring is full and blocks are dropped. (Not with
    // `until`, which runs the desk.)
    let mut blocks = 0;
    while rig.hub.snapshot().dropped_blocks < 4 {
        assert!(blocks < 4_096, "the ring never filled");
        client.send_audio(blocks * 256, 256, &block).unwrap();
        blocks += 1;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let read = rig.hub.snapshot();
            if read.blocks + read.dropped_blocks >= blocks {
                break;
            }
            assert!(Instant::now() < deadline, "block {blocks} was never read");
            thread::sleep(Duration::from_millis(1));
        }
    }
    let mut health = DeskHealthMsg::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while health.dropped == 0 {
        assert!(Instant::now() < deadline, "the drops were never told");
        thread::sleep(Duration::from_millis(1));
        while client.try_recv().unwrap().is_some() {}
        if let Some(told) = client.take_desk_health() {
            health = told;
        }
    }
    // (With the desk's clock standing still, the stream crept ahead of it
    // and was placed afresh as it filled: those count.)
    let placed = health.placed_afresh;
    // The desk takes the strip again, and the stream is placed anew: that
    // is the drop already counted, not a block that came late. Step by
    // step (the desk drains the ring, then each block is read before the
    // desk plays on), so the stream is never late whatever the scheduling.
    for _ in 0..1_024 {
        rig.render();
    }
    let mut next = blocks * 256;
    for _ in 0..32 {
        let read = rig.hub.snapshot().blocks;
        client.send_audio(next, 256, &block).unwrap();
        next += 256;
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.hub.snapshot().blocks <= read {
            assert!(Instant::now() < deadline, "a block was never read");
            thread::sleep(Duration::from_millis(1));
        }
        rig.render();
    }
    let deadline = Instant::now() + HEALTH_INTERVAL * 3;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
        while client.try_recv().unwrap().is_some() {}
        if let Some(told) = client.take_desk_health() {
            health = told;
        }
    }
    assert_eq!(health.placed_afresh, placed, "{health:?}");
}

#[test]
fn an_instrument_at_another_rate_reads_the_desks_rate_from_its_refusal() {
    use kazoo_core::host::link::connect_config_or_direct;
    use kazoo_core::ipc::link::{HubAddress, LinkConfig};
    let rig = Rig::new([false; DESK_CHANNELS]);
    let mut config = LinkConfig::new("kazoo-wall", 2, 44_100, 512);
    config.address = HubAddress::Socket(rig.socket.clone());
    // As the wall links to the desk.
    let (link, _audio) = connect_config_or_direct(config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while link.status().refused_for_rate().is_none() {
        assert!(
            Instant::now() < deadline,
            "never refused for its rate: {:?}",
            link.status().last_refusal
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(link.status().refused_for_rate(), Some(RATE));
    assert!(!link.is_connected());
    // The desk goes: its refusal does not outlive it (the link tries again
    // within two seconds of a refusal for its rate, not after a backoff).
    drop(rig);
    let deadline = Instant::now() + Duration::from_secs(5);
    while link.status().refused_for_rate().is_some() {
        assert!(
            Instant::now() < deadline,
            "the gone desk's refusal stayed: {:?}",
            link.status().last_refusal
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(link.status().desk_refusal(), None);
}
