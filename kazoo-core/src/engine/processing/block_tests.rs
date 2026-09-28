//! Whole-callback tests: `process_block` with every ring and channel,
//! standalone and plugged into a stand-in desk. No audio device is used.

use super::tests::Rig;
use super::*;
use crate::engine::reclaim::OUTBOUND_CAPACITY;
use crate::ipc::link::{HubAddress, LinkConfig, hub_link};
use crate::ipc::protocol::{FrameBuffer, decode_audio_frame_count};
use crate::ipc::types::{
    MSG_AUDIO, MSG_REGISTER, MSG_REGISTERED, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC,
    RegisteredMsg, TransportRequestMsg, TransportSyncMsg,
};
use crate::transport::TransportState;
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;
const BLOCK: usize = 64;

fn engine(desk_audio: Option<HubLinkAudio>) -> Rig {
    Rig::build(RATE, BLOCK, OUTBOUND_CAPACITY, desk_audio)
}

/// Arm a track that plays the mic straight through, and feed the mic.
fn monitor_mic(rig: &mut Rig, level: f32) {
    let id = rig.add_track("Mic", SynthesisMode::Passthrough);
    rig.apply(EngineCommand::SetTrackArm(id, true));
    let samples = [level; BLOCK];
    assert_eq!(rig.mic.push_slice(&samples), BLOCK);
}

/// Render one callback of `out`, checking it neither allocates nor frees.
fn render(rig: &mut Rig, out: &mut [f32]) {
    rig.render(out);
}

/// The stream frame the desk link has reached.
fn desk_frame(rig: &Rig) -> u64 {
    rig.io.desk.as_ref().map_or(0, HubLinkAudio::stream_frame)
}

#[test]
fn a_long_callback_is_rendered_in_blocks() {
    let mut rig = engine(None);
    rig.apply(EngineCommand::Transport(TransportCommand::Play));
    let mut out = vec![1.0_f32; BLOCK * 4 * 2 + 1];
    render(&mut rig, &mut out);
    // Every frame advanced the transport, not just the first block's.
    assert_eq!(rig.state.transport.position_samples(), (BLOCK * 4) as u64);
    // The trailing half frame is silenced.
    assert!(out[BLOCK * 8].abs() < f32::EPSILON);
}

#[test]
fn display_snapshots_are_paced_not_pushed_every_block() {
    let mut rig = engine(None);
    let mut out = [0.0_f32; BLOCK * 2];
    let mut snapshots = 0;
    // One second of audio, the UI handing each frame back once read.
    for _ in 0..(RATE as usize / BLOCK) {
        render(&mut rig, &mut out);
        while let Some(frame) = rig.display.try_pop() {
            snapshots += 1;
            assert!(rig.recycle.try_push(frame).is_ok());
        }
    }
    assert!(
        (59..=61).contains(&snapshots),
        "{snapshots} snapshots in one second"
    );
    assert_eq!(rig.state.stats.snapshot().display_frames_dropped, 0);
}

#[test]
fn standalone_the_mic_reaches_the_speakers() {
    let mut rig = engine(None);
    monitor_mic(&mut rig, 0.25);
    let mut out = [0.0_f32; BLOCK * 2];
    render(&mut rig, &mut out);
    assert!(out.iter().any(|s| s.abs() > 0.01), "silent standalone");
}

// -- Plugged into a stand-in desk ----------------------------------------

/// The desk's end of the socket.
struct Desk {
    stream: std::os::unix::net::UnixStream,
    buf: FrameBuffer,
    path: std::path::PathBuf,
}

impl Desk {
    /// Start a link to a desk listening on a private socket, and accept
    /// and register it on strip 1 at 120 BPM, stopped.
    fn plug_in(name: &str) -> (Self, crate::ipc::link::HubLink, HubLinkAudio) {
        let path = std::env::temp_dir().join(format!(
            "kazoo-core-desk-{name}-{}.sock",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let mut config = LinkConfig::new("kazoo-tui-test", 2, RATE, BLOCK as u32);
        config.address = HubAddress::Socket(path.clone());
        let (link, audio) = hub_link(config).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buf = FrameBuffer::new();
        assert_eq!(buf.read_frame(&mut stream).unwrap().msg_type, MSG_REGISTER);
        RegisteredMsg {
            strip_index: 0,
            hub_sample_rate: RATE,
            hub_buffer_size: BLOCK as u32,
            transport_state: TRANSPORT_STOPPED,
            bpm: 120.0,
            position: 0,
        }
        .encode(buf.payload_mut());
        buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
            .unwrap();
        (Self { stream, buf, path }, link, audio)
    }

    /// The next frame of `msg_type` the engine sent, skipping others.
    fn next(&mut self, msg_type: u8) -> &[u8] {
        loop {
            let header = self.buf.read_frame(&mut self.stream).unwrap();
            if header.msg_type == msg_type {
                return self.buf.payload();
            }
        }
    }

    fn sync(&mut self, sync: TransportSyncMsg) {
        sync.encode(self.buf.payload_mut());
        self.buf
            .write_frame(
                MSG_TRANSPORT_SYNC,
                1,
                TransportSyncMsg::WIRE_SIZE,
                &mut self.stream,
            )
            .unwrap();
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path) {
            assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
        }
    }
}

/// Render blocks until `done` holds, failing after five seconds.
fn render_until(rig: &mut Rig, what: &str, mut done: impl FnMut(&ProcessingState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut out = [0.0_f32; BLOCK * 2];
    while !done(&rig.state) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        render(rig, &mut out);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn plugged_in_the_desk_gets_the_audio_and_the_speakers_are_silent() {
    let (mut desk, _link, audio) = Desk::plug_in("audio");
    let mut rig = engine(Some(audio));
    render_until(&mut rig, "the desk", |s| s.desk.connected);

    monitor_mic(&mut rig, 0.25);
    let before = desk_frame(&rig);
    let mut out = [1.0_f32; BLOCK * 2];
    render(&mut rig, &mut out);
    assert!(out.iter().all(|s| s.abs() < f32::EPSILON), "heard twice");
    assert_eq!(desk_frame(&rig), before + BLOCK as u64);
    let frames = decode_audio_frame_count(desk.next(MSG_AUDIO));
    assert!(frames > 0 && frames as usize <= BLOCK);
}

#[test]
fn plugged_in_play_asks_the_desk_instead_of_playing_alone() {
    let (mut desk, _link, audio) = Desk::plug_in("request");
    let mut rig = engine(Some(audio));
    render_until(&mut rig, "the desk", |s| s.desk.connected);

    rig.commands
        .send(EngineCommand::Transport(TransportCommand::Play))
        .unwrap();
    let mut out = [0.0_f32; BLOCK * 2];
    render(&mut rig, &mut out);
    let request = TransportRequestMsg::decode(desk.next(MSG_TRANSPORT_REQUEST));
    assert_eq!(request.requested_state, TRANSPORT_PLAYING);
    // Asking is not playing: the desk decides.
    assert_eq!(rig.state.transport.state(), TransportState::Stopped);
    assert_eq!(rig.state.stats.snapshot().desk_requests_dropped, 0);
}

#[test]
fn plugged_in_the_desk_transport_lands_on_its_frame() {
    let (mut desk, _link, audio) = Desk::plug_in("sync");
    let mut rig = engine(Some(audio));
    render_until(&mut rig, "the desk", |s| s.desk.connected);

    // Play at 90 BPM from beat 4, a little ahead, mid-block.
    let at_frame = desk_frame(&rig) + (BLOCK * 3 + 17) as u64;
    desk.sync(TransportSyncMsg {
        state: TRANSPORT_PLAYING,
        bpm: 90.0,
        at_frame,
        beat: 4.0,
    });
    render_until(&mut rig, "the sync", |s| {
        s.transport.state() == TransportState::Playing
    });
    assert!((rig.state.transport.bpm() - 90.0).abs() < 1e-9);
    // 4 beats at 90 BPM is 128 000 samples, plus the frames since.
    let since = desk_frame(&rig) - at_frame;
    assert_eq!(rig.state.transport.position_samples(), 128_000 + since);

    // Stop, from the next frame on.
    desk.sync(TransportSyncMsg {
        state: TRANSPORT_STOPPED,
        bpm: 90.0,
        at_frame: desk_frame(&rig),
        beat: f64::NAN,
    });
    render_until(&mut rig, "the stop", |s| {
        s.transport.state() == TransportState::Stopped
    });
    assert_eq!(rig.state.stats.snapshot().desk_syncs_rejected, 0);
}

// -- The reclaim thread ----------------------------------------------------

#[test]
fn a_recording_round_trips_through_the_reclaim_thread() {
    let mut rig = engine(None);
    let reclaimer = rig.reclaimer.take().unwrap();
    let worker = std::thread::Builder::new()
        .name("test-reclaim".into())
        .spawn(move || reclaimer.run())
        .unwrap();

    monitor_mic(&mut rig, 0.25);
    let mut out = [0.0_f32; BLOCK * 2];
    rig.commands
        .send(EngineCommand::Transport(TransportCommand::Record))
        .unwrap();
    render(&mut rig, &mut out);
    for _ in 0..3 {
        assert_eq!(rig.mic.push_slice(&[0.25; BLOCK]), BLOCK);
        render(&mut rig, &mut out);
    }
    rig.commands
        .send(EngineCommand::Transport(TransportCommand::Stop))
        .unwrap();

    // The take leaves in chunks, the reclaim thread makes it a clip, and
    // the callback places it and gets every chunk back.
    render_until(&mut rig, "the recorded clip", |s| {
        s.mixer.tracks()[0].clips().len() == 1 && s.takes.pool.len() == s.takes.pool.capacity()
    });
    let clip = &rig.state.mixer.tracks()[0].clips()[0];
    assert_eq!(clip.effective_length(), BLOCK * 4);
    assert!(
        clip.data()
            .samples()
            .iter()
            .all(|&s| (s - 0.25).abs() < 1e-6)
    );

    // The reclaim thread exits once the callback's end of the ring is gone.
    drop(rig);
    worker.join().unwrap();
}

// -- Whole-callback real-time checks -----------------------------------------

#[test]
#[should_panic(expected = "allocated or freed")]
fn the_allocation_check_catches_an_allocation() {
    super::tests::on_audio_thread(|| drop(Vec::<u8>::with_capacity(16)));
}

#[test]
fn a_loop_wrap_while_recording_starts_a_new_take_on_the_loop_start() {
    let mut rig = engine(None);
    monitor_mic(&mut rig, 0.25);
    let loop_start = 1_000_u64;
    let loop_end = loop_start + (BLOCK * 3 + BLOCK / 2) as u64;
    for cmd in [
        TransportCommand::SetLoop(Some((loop_start, loop_end))),
        TransportCommand::Seek(loop_start),
        TransportCommand::Record,
    ] {
        rig.commands.send(EngineCommand::Transport(cmd)).unwrap();
    }
    let mut out = [0.0_f32; BLOCK * 2];
    // Five blocks: three and a half fill the loop, the rest wraps.
    for _ in 0..5 {
        assert_eq!(rig.mic.push_slice(&[0.25; BLOCK]), BLOCK);
        render(&mut rig, &mut out);
        rig.reclaim();
    }
    rig.commands
        .send(EngineCommand::Transport(TransportCommand::Stop))
        .unwrap();
    // Stop, and let the reclaimer turn both takes into clips.
    for _ in 0..3 {
        render(&mut rig, &mut out);
        rig.reclaim();
    }
    render(&mut rig, &mut out);
    let passes: Vec<(u64, usize)> = rig.state.mixer.tracks()[0]
        .clips()
        .iter()
        .map(|clip| (clip.position(), clip.effective_length()))
        .collect();
    // In recording order: the first pass fills the loop exactly; the second starts on its start.
    let first = (loop_end - loop_start) as usize;
    assert_eq!(
        passes,
        vec![(loop_start, first), (loop_start, BLOCK * 5 - first)]
    );
}

/// Clip edits on `ids[2]`'s `clip`, and removals of the effect on `ids[0]`
/// and the extra layer on `ids[1]`.
fn edits(ids: &[TrackId], clip: crate::mixer::clip::ClipId) -> Vec<EngineCommand> {
    use crate::mixer::clip::ClipId;
    vec![
        EngineCommand::MoveClip {
            track_id: ids[2],
            clip_id: clip,
            new_position: 60_000,
        },
        EngineCommand::SplitClip {
            track_id: ids[2],
            clip_id: clip,
            split_position: 61_000,
        },
        EngineCommand::DuplicateClip {
            track_id: ids[2],
            clip_id: clip,
            new_position: 90_000,
        },
        EngineCommand::RemoveClip {
            track_id: ids[2],
            clip_id: ClipId(0),
        },
        EngineCommand::RemoveEffect {
            track_id: ids[0],
            effect_index: 0,
        },
        EngineCommand::RemoveSynthLayer {
            track_id: ids[1],
            layer_index: 1,
        },
    ]
}

#[test]
fn a_busy_session_never_allocates_or_frees_on_the_audio_thread() {
    use crate::engine::EngineHandle;
    use crate::mixer::clip::ClipData;

    let mut rig = engine(None);
    // Commands are built the way a frontend builds them: by an engine
    // handle, off the audio thread.
    let handle = EngineHandle::detached(rig.commands.clone(), RATE, BLOCK);
    let mut out = [0.0_f32; BLOCK * 2];
    let mut run = |rig: &mut Rig, blocks: usize| {
        for _ in 0..blocks {
            assert_eq!(rig.mic.push_slice(&[0.1; BLOCK]), BLOCK);
            render(rig, &mut out);
            rig.reclaim();
            while let Some(frame) = rig.display.try_pop() {
                assert!(rig.recycle.try_push(frame).is_ok());
            }
        }
    };

    let ids: Vec<TrackId> = (0..crate::MAX_TRACKS)
        .map(|i| {
            handle
                .add_track(format!("{i}"), SynthesisMode::Passthrough)
                .unwrap()
        })
        .collect();
    run(&mut rig, 2);
    assert_eq!(rig.state.mixer.track_count(), crate::MAX_TRACKS);
    for &id in &ids {
        rig.commands
            .send(EngineCommand::SetTrackArm(id, true))
            .unwrap();
    }
    handle
        .add_effect(ids[0], Box::new(crate::effects::Delay::new(RATE as f32)))
        .unwrap();
    handle
        .add_synth_layer(ids[1], SynthesisMode::Wavetable, "Pad".into())
        .unwrap();

    // Three takes on every track, long enough to cross chunks.
    for _ in 0..3 {
        handle.record().unwrap();
        run(&mut rig, chunk_len(RATE) / BLOCK + 3);
        handle.stop().unwrap();
        run(&mut rig, 3);
    }
    // Clip edits, synth swaps, removals.
    let data = ClipData::new(vec![0.5; 4_000], "Loaded".into(), None, RATE);
    rig.commands
        .send(EngineCommand::AddClip {
            track_id: ids[2],
            clip_data: data,
            position: 50_000,
        })
        .unwrap();
    run(&mut rig, 1);
    let clip = rig
        .state
        .mixer
        .track(ids[2])
        .unwrap()
        .clips()
        .last()
        .unwrap()
        .id();
    for cmd in edits(&ids, clip) {
        rig.commands.send(cmd).unwrap();
    }
    handle
        .set_track_synthesis_mode(ids[3], SynthesisMode::Vocoder)
        .unwrap();
    handle.remove_track(ids[4]).unwrap();
    handle.play().unwrap();
    run(&mut rig, 20);
    handle.stop().unwrap();
    run(&mut rig, 3);

    let stats = rig.state.stats.snapshot();
    assert_eq!(stats.callback_frees, 0);
    assert_eq!(stats.take_samples_dropped, 0);
    assert_eq!(stats.takes_unavailable, 0);
    assert_eq!(stats.display_frames_dropped, 0);
    assert_eq!(stats.clips_rejected, 0);
    assert_eq!(rig.state.mixer.track_count(), crate::MAX_TRACKS - 1);
    // Every armed track kept all three takes.
    let takes = rig.state.mixer.track(ids[0]).unwrap().clips().len();
    assert_eq!(takes, 3);
    assert_eq!(rig.state.takes.pool.len(), rig.state.takes.pool.capacity());
    // The timeline was published and reaches the UI side.
    assert!(rig.timelines.take().is_some());
}

#[test]
fn a_desk_request_that_could_not_be_queued_is_made_locally_once_the_desk_is_gone() {
    let mut rig = engine(None);
    rig.state.desk.pending = Some(TransportCommand::Play);
    let mut out = [0.0_f32; BLOCK * 2];
    render(&mut rig, &mut out);
    assert!(rig.state.desk.pending.is_none());
    assert_eq!(rig.state.transport.state(), TransportState::Playing);
}
