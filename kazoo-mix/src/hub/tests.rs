//! End-to-end hub tests: real sockets, the client instruments use, and the
//! real audio callback path (patchbay, engine, shared state).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kazoo_core::ipc::client::{HubIpcClient, HubMessage};
use kazoo_core::ipc::types::{
    DeskPaceMsg, NOTE_ON, NoteEventMsg, SYNC_NOW, TRANSPORT_PLAYING, TRANSPORT_UNCHANGED,
};

use super::instrument::PACE_INTERVAL;

use super::*;
use crate::callback::AudioCallback;
use crate::engine::MixerEngine;
use crate::patchbay::patchbay;
use crate::test_support::floats_equal;

const RATE: u32 = 48_000;

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
        let shared = Arc::new(SharedState::new());
        let (hub_end, callback_end) = patchbay(PATCH_CAPACITY);
        let socket = unique_socket();
        let hub = Hub::start(
            HubConfig {
                socket: socket.clone(),
                advertise: false,
                sample_rate: RATE,
                reserved,
            },
            Arc::clone(&shared),
            hub_end,
        )
        .unwrap();
        let engine = MixerEngine::new(DESK_CHANNELS, RATE).unwrap();
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
        self.callback.process(&mut data);
        data.into_iter().step_by(2).collect()
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
    register.pace_lead_frames = 1_920;
    let mut client = HubIpcClient::register_at(&rig.socket, &register).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let mut sent = 0;
    let first = paced(&mut rig, &mut client, &mut sent).expect("a pace");
    // Its lead is the desk's usual (a device buffer, a block, a margin)
    // and the 1 920 frames it asked for.
    assert!(first.lead_frames >= 1_920 + 256 + 256, "{first:?}");
    assert!(first.lead_frames < 1_920 + 4 * 1_024, "{first:?}");
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
fn an_extra_lead_past_a_second_is_held_to_a_second() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut register = RegisterMsg::new("kazoo-wall", 2, RATE, 256);
    register.pace_lead_frames = u32::MAX;
    let mut client = HubIpcClient::register_at(&rig.socket, &register).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    let mut sent = 0;
    let pace = paced(&mut rig, &mut client, &mut sent).expect("a pace");
    assert!(pace.lead_frames >= RATE, "{pace:?}");
    assert!(pace.lead_frames < RATE + 4 * 1_024, "{pace:?}");
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

#[test]
fn a_paced_source_with_a_jittery_timer_is_placed_once_and_stays_placed() {
    use kazoo_core::ipc::link::{HubAddress, LinkConfig, hub_link};
    use std::sync::atomic::AtomicBool;

    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let (link, mut audio) = hub_link(LinkConfig {
        address: HubAddress::Socket(rig.socket.clone()),
        pace_lead_frames: 1_920,
        ..LinkConfig::new("kazoo-wall", 2, RATE, 256)
    })
    .unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let source_running = Arc::clone(&running);
    // A source like the headless wall: render what the desk says is owed,
    // on its own timer until the desk says; and every tenth of a second,
    // oversleep by 20 to 30 ms, as a busy machine does.
    let source = thread::spawn(move || {
        let block = vec![0.1_f32; 512];
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
    });
    // The desk plays in real time: a 256-frame buffer every 5⅓ ms.
    let started = Instant::now();
    let buffer = Duration::from_secs_f64(256.0 / f64::from(RATE));
    let mut buffers = 0_u32;
    while started.elapsed() < Duration::from_millis(1_500) {
        let due = started + buffer * buffers;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        rig.render();
        buffers += 1;
    }
    running.store(false, Ordering::Release);
    source.join().unwrap();
    let snapshot = rig.hub.snapshot();
    assert!(snapshot.blocks > 200, "{snapshot:?}");
    assert_eq!(snapshot.resyncs, 1, "{snapshot:?}");
    drop(link);
}
