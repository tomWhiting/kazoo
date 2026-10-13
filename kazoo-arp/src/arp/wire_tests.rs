use crate::ClockDivision;
use crate::audio::{
    ArpCommand, AudioCommand, AudioEngine, COMMAND_CAPACITY, DISPLAY_CAPACITY, EngineShared,
    MAX_CALLBACK_FRAMES,
};
use kazoo_core::host::link::connect_config_or_direct;
use kazoo_core::ipc::link::{HubAddress, LinkConfig};
use kazoo_core::ipc::protocol::FrameBuffer;
use kazoo_core::ipc::types::{
    MSG_NOTE_EVENT, MSG_REGISTER, MSG_REGISTERED, NOTE_OFF, NOTE_ON, NoteEventMsg, RegisterMsg,
    RegisteredMsg, TRANSPORT_PLAYING, TRANSPORT_STOPPED, TransportSyncMsg,
};
use ringbuf::traits::Split;
use std::os::unix::net::UnixListener;
use std::sync::Arc;

#[test]
fn scheduled_notes_reach_the_real_link_with_the_selected_destination() {
    let logs = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kz-logs"));
    std::fs::create_dir_all(logs).unwrap();
    let logs = logs.canonicalize().unwrap();
    let dir = tempfile::tempdir_in(logs).unwrap();
    let path = dir.path().join("w.sock");
    assert!(
        path.as_os_str().len() < 104,
        "test socket path is {} bytes; macOS sun_path holds 104 including the closing NUL: {}",
        path.as_os_str().len(),
        path.display()
    );
    let listener = UnixListener::bind(&path).unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = FrameBuffer::new();
        assert_eq!(
            buffer.read_frame(&mut stream).unwrap().msg_type,
            MSG_REGISTER
        );
        let source = RegisterMsg::decode(buffer.payload()).instrument_id;
        RegisteredMsg {
            strips: Vec::new(),
            strip_index: 1,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: TRANSPORT_STOPPED,
            bpm: 120.0,
            position: 0,
        }
        .encode(buffer.payload_mut());
        buffer
            .write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
            .unwrap();
        let mut notes = Vec::new();
        while notes.len() < 2 {
            let header = buffer.read_frame(&mut stream).unwrap();
            if header.msg_type == MSG_NOTE_EVENT {
                notes.push(NoteEventMsg::decode(buffer.payload()));
            }
        }
        (source, notes)
    });
    let mut config = LinkConfig::new("kazoo-arp-test", 2, 48_000, MAX_CALLBACK_FRAMES as u32);
    config.address = HubAddress::Socket(path.clone());
    let (hub, audio) = connect_config_or_direct(config).unwrap();
    let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
    let (display, consumer) = ringbuf::HeapRb::new(DISPLAY_CAPACITY).split();
    drop(consumer);
    let shared = Arc::new(EngineShared::default());
    let mut engine = AudioEngine::new(48_000, 2, rx, audio, display, Arc::clone(&shared));
    while !hub.is_connected() {
        std::thread::yield_now();
    }
    engine.render(&mut [0.0; 2]);
    let args = crate::parse_args(
        ["--target", "0102030405060708090a0b0c0d0e0f10", "--silent"].map(String::from),
    )
    .unwrap()
    .unwrap();
    engine.configure_output(args);
    for command in [
        ArpCommand::SetDivision(ClockDivision::Eighth),
        ArpCommand::SetGate(0.5),
        ArpCommand::NoteOn {
            midi_note: 60,
            velocity: 90,
        },
    ] {
        tx.send(AudioCommand::Arp(command)).unwrap();
    }
    engine.schedule_transport(&TransportSyncMsg {
        state: TRANSPORT_PLAYING,
        bpm: 120.0,
        at_frame: 500,
        beat: 0.0,
    });
    let mut output = vec![f32::NAN; 13_000];
    engine.render(&mut output);
    let (source, notes) = peer.join().unwrap();
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
    assert_eq!(notes.len(), 2);
    for event in &notes {
        assert_eq!(event.source, source);
        assert_eq!(event.target, args.target);
        assert_eq!(event.note, 60);
        assert_eq!(event.channel, 0);
    }
    assert_eq!(notes[0].event_type, NOTE_ON);
    assert_eq!(notes[0].velocity, 90);
    assert_eq!(notes[1].event_type, NOTE_OFF);
    assert_eq!(notes[1].velocity, 0);
    assert_eq!(
        shared.notes_lost.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    drop(engine);
    drop(hub);
    dir.close().unwrap();
    // The brief's evidence that no socket is left behind.
    assert!(!path.exists(), "test socket removed with its folder");
}
