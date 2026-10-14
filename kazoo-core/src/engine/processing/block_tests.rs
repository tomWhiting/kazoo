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
            strips: Vec::new(),
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
    let mut phase = 0.0_f32;
    let mut voice_blocks = 0;
    let mut run = |rig: &mut Rig, blocks: usize| {
        for _ in 0..blocks {
            let hz = [220.0, 330.0, 440.0][(voice_blocks / 32) % 3];
            voice_blocks += 1;
            let mut mic = [0.0; BLOCK];
            for sample in &mut mic {
                phase = (phase + std::f32::consts::TAU * hz / RATE as f32) % std::f32::consts::TAU;
                *sample = 0.5 * phase.sin();
            }
            assert_eq!(rig.mic.push_slice(&mic), BLOCK);
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
    // A voice-following synth, so the sung pitch reaches a synth that uses it.
    handle
        .set_track_synthesis_mode(ids[5], SynthesisMode::PitchTracked)
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

fn pitch(rig: &Rig, id: TrackId) -> f32 {
    rig.state
        .mixer
        .track(id)
        .unwrap()
        .layer(0)
        .unwrap()
        .synth()
        .target_frequency()
        .unwrap()
}

fn close_pitch(actual: f32, expected: f32) -> bool {
    actual > 0.0 && (1200.0 * (actual / expected).log2()).abs() <= 5.0
}

fn sing(rig: &mut Rig, hz_curve: &[f32]) -> Vec<f32> {
    let mut phase = 0.0_f32;
    let mut targets = Vec::new();
    let mut out = [0.0; 256];
    for &hz in hz_curve {
        let mut mic = [0.0; 128];
        for sample in &mut mic {
            phase = (phase + std::f32::consts::TAU * hz / RATE as f32) % std::f32::consts::TAU;
            *sample = 0.5 * phase.sin();
        }
        assert_eq!(rig.mic.push_slice(&mic), mic.len());
        render(rig, &mut out);
        targets.push(pitch(rig, TrackId(0)));
        while let Some(frame) = rig.display.try_pop() {
            assert!(rig.recycle.try_push(frame).is_ok());
        }
    }
    targets
}

#[test]
fn an_armed_track_follows_a_note_change_within_two_periods() {
    let mut rig = Rig::build(RATE, 128, OUTBOUND_CAPACITY, None);
    let id = rig.add_track("Voice", SynthesisMode::PitchTracked);
    rig.apply(EngineCommand::SetTrackArm(id, true));
    let mut curve = vec![220.0; 113];
    curve.extend_from_slice(&[330.0; 4]);
    let targets = sing(&mut rig, &curve);
    assert!(
        close_pitch(targets[112], 220.0),
        "initial target: {}",
        targets[112]
    );
    let blocks = targets[113..].iter().position(|&hz| close_pitch(hz, 330.0));
    assert!(
        blocks.is_some(),
        "targets after change: {:?}",
        &targets[113..]
    );
    let elapsed = (blocks.unwrap() + 1) as f32 * 128.0 / RATE as f32;
    assert!(elapsed <= 2.0 / 330.0 + 0.001 + 128.0 / RATE as f32);
}

#[test]
fn the_display_shows_the_trackers_pitch() {
    let mut rig = Rig::build(RATE, 128, OUTBOUND_CAPACITY, None);
    let mut reference = crate::voice::VoiceTracker::new(RATE);
    let mut last = None;
    let mut locked_at = None;
    let mut display_at = None;
    let mut phase = 0.0_f32;
    let mut out = [0.0; 256];
    // Request a snapshot every block to isolate pitch publication from UI pacing.
    rig.state.display_interval = 128;
    for block in 0..75 {
        let mut mic = [0.0; 128];
        for sample in &mut mic {
            phase = (phase + std::f32::consts::TAU * 440.0 / RATE as f32) % std::f32::consts::TAU;
            *sample = phase.sin() * 0.5;
        }
        reference.process(&mic, |frame| last = Some(frame));
        if last.is_some_and(|frame| frame.voiced && close_pitch(frame.hz, 440.0)) {
            locked_at.get_or_insert(block);
        }
        assert_eq!(rig.mic.push_slice(&mic), mic.len());
        render(&mut rig, &mut out);
        while let Some(frame) = rig.display.try_pop() {
            if frame
                .pitch
                .frequency
                .is_some_and(|hz| close_pitch(hz, 440.0))
                && frame.pitch.voiced_probability >= 0.9
            {
                display_at.get_or_insert(block);
            }
            if block == 74 {
                assert!(
                    frame
                        .pitch
                        .frequency
                        .is_some_and(|hz| close_pitch(hz, 440.0)),
                    "display pitch: {:?}",
                    frame.pitch
                );
                assert!(frame.pitch.voiced_probability >= 0.9);
            }
            assert!(rig.recycle.try_push(frame).is_ok());
        }
    }
    assert!(
        display_at.is_some(),
        "tracker locked at {locked_at:?}, display never locked"
    );
    assert!(display_at.unwrap() <= locked_at.unwrap() + 2);
}

#[test]
fn playback_still_drives_synths_from_the_clip() {
    let mut rig = Rig::build(RATE, 128, OUTBOUND_CAPACITY, None);
    let id = rig.add_track("Clip", SynthesisMode::PitchTracked);
    let audio = (0..4_800)
        .map(|i| (i as f32 * 330.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5)
        .collect();
    rig.apply(EngineCommand::AddClip {
        track_id: id,
        clip_data: ClipData::new(audio, "Sine".into(), None, RATE),
        position: 0,
    });
    rig.apply(EngineCommand::Transport(TransportCommand::Play));
    let mut out = [0.0; 256];
    for _ in 0..18 {
        render(&mut rig, &mut out);
        if close_pitch(pitch(&rig, id), 330.0) {
            return;
        }
    }
    panic!("clip target after 48 ms: {}", pitch(&rig, id));
}

/// A synth that logs, for every render, how many samples it made and the
/// pitch it held. The log is preallocated and written with atomics, so
/// rendering never allocates.
struct PitchLog {
    pitch: f32,
    renders: Arc<std::sync::atomic::AtomicUsize>,
    log: Arc<[std::sync::atomic::AtomicU64]>,
}

impl PitchLog {
    fn boxed(capacity: usize) -> (Box<dyn Processor>, Self) {
        let log: Arc<[std::sync::atomic::AtomicU64]> = (0..capacity)
            .map(|_| std::sync::atomic::AtomicU64::new(0))
            .collect();
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = Self {
            pitch: 0.0,
            renders: Arc::clone(&renders),
            log: Arc::clone(&log),
        };
        let synth = Self {
            pitch: 0.0,
            renders,
            log,
        };
        (Box::new(synth), reader)
    }

    /// Each render's length and the pitch it held, in order.
    fn renders(&self) -> Vec<(usize, f32)> {
        let count = self.renders.load(std::sync::atomic::Ordering::Acquire);
        self.log[..count.min(self.log.len())]
            .iter()
            .map(|entry| {
                let packed = entry.load(std::sync::atomic::Ordering::Acquire);
                ((packed >> 32) as usize, f32::from_bits(packed as u32))
            })
            .collect()
    }
}

impl Processor for PitchLog {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let index = self
            .renders
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let entry = &self.log[index];
        entry.store(
            ((input.len() as u64) << 32) | u64::from(self.pitch.to_bits()),
            std::sync::atomic::Ordering::Release,
        );
        output.fill(0.0);
    }

    fn reset(&mut self) {}

    fn name(&self) -> &'static str {
        "Pitch log"
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn set_pitch(&mut self, frequency: f32) {
        self.pitch = frequency;
    }
}

/// The brief's cadence: a new pitch every 64 samples, whatever the
/// callback size. A 512-frame callback holds eight tracker frames; the
/// synth must render each 64 samples with the frame made from them, not
/// all 512 with the last.
#[test]
fn a_long_callback_gives_the_synth_a_new_pitch_every_hop() {
    use crate::voice::{VOICE_HOP, VoiceTracker};
    const LONG: usize = 512;
    const CALLBACKS: usize = 30;
    let mut rig = Rig::build(RATE, LONG, OUTBOUND_CAPACITY, None);
    let (synth, log) = PitchLog::boxed(4 * CALLBACKS * LONG / VOICE_HOP);
    let id = rig.add_track_with("Voice", synth, SynthesisMode::PitchTracked);
    rig.apply(EngineCommand::SetTrackArm(id, true));

    // An octave glide across the whole run: about five cents a hop, so a
    // pitch one hop stale is plainly wrong.
    let length = (CALLBACKS * LONG) as f32 / RATE as f32;
    let mut reference = VoiceTracker::new(RATE);
    let mut expected = Vec::new();
    let mut held = 0.0_f32;
    let mut phase = 0.0_f32;
    let mut out = [0.0_f32; LONG * 2];
    for callback in 0..CALLBACKS {
        let mut mic = [0.0_f32; LONG];
        for (i, sample) in mic.iter_mut().enumerate() {
            let t = (callback * LONG + i) as f32 / RATE as f32;
            let hz = 220.0 * (t / length).exp2();
            phase = (phase + std::f32::consts::TAU * hz / RATE as f32) % std::f32::consts::TAU;
            *sample = 0.5 * phase.sin();
        }
        reference.process(&mic, |frame| {
            if frame.voiced {
                held = frame.hz;
            }
            expected.push(held);
        });
        assert_eq!(rig.mic.push_slice(&mic), LONG);
        render(&mut rig, &mut out);
        while let Some(frame) = rig.display.try_pop() {
            assert!(rig.recycle.try_push(frame).is_ok());
        }
    }

    let renders = log.renders();
    let lengths: Vec<usize> = renders.iter().map(|&(len, _)| len).collect();
    assert!(
        lengths.iter().all(|&len| len == VOICE_HOP),
        "render lengths: {lengths:?}"
    );
    assert_eq!(renders.len(), expected.len());
    assert!(expected.iter().filter(|&&hz| hz > 0.0).count() > expected.len() / 2);
    for (hop, (&(_, pitch), &want)) in renders.iter().zip(&expected).enumerate() {
        let off = if want > 0.0 {
            (1200.0 * (pitch / want).log2()).abs()
        } else {
            pitch.abs()
        };
        assert!(
            off <= 0.5,
            "hop {hop}: the synth held {pitch} Hz, the tracker's frame was {want} Hz"
        );
    }
}

/// A callback rendered in several pieces still hears its whole mic block:
/// the stale-sample skip is made once per block, never per piece.
#[test]
fn a_long_callback_hears_its_whole_mic_block() {
    const LONG: usize = 512;
    let mut rig = Rig::build(RATE, LONG, OUTBOUND_CAPACITY, None);
    let id = rig.add_track("Mic", SynthesisMode::Passthrough);
    rig.apply(EngineCommand::SetTrackArm(id, true));
    let mut out = [0.0_f32; LONG * 2];
    for _ in 0..2 {
        assert_eq!(rig.mic.push_slice(&[0.25; LONG]), LONG);
        render(&mut rig, &mut out);
    }
    let silent = out.iter().filter(|s| s.abs() < 0.01).count();
    assert_eq!(silent, 0, "{silent} of {} samples silent", out.len());
}
