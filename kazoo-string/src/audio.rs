//! kazoo-string's audio engine: everything the cpal output callback owns.
//!
//! The callback drains UI commands and hub messages, follows the desk's
//! transport to the frame, advances the `--phrase` loop, runs the string synth
//! and hands every rendered block to the hub link, plugged in or not, so the
//! stream position the desk schedules on stays true. All state is allocated
//! here, before the stream starts; rendering never allocates, locks or
//! blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportChange, TransportFollower};
use kazoo_core::ipc::link::{HubLinkAudio, RequestError};
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON, TRANSPORT_PLAYING, TRANSPORT_STOPPED};

use crate::patch::Patch;
use crate::phrase::{MAX_BPM, MIN_BPM, Phrase};
use crate::synth::{SCOPE_LEN, Source, StringSynth, VOICES};

/// Largest block rendered in one pass, in frames. Device buffers larger than
/// this are rendered in several passes, and it is the block size the hub is
/// told about.
pub const MAX_BLOCK_FRAMES: usize = 1_024;
/// Phrase tempo until the desk or the player changes it.
pub const DEFAULT_BPM: f64 = 112.0;
/// Display snapshots per second pushed from the audio thread.
const DISPLAY_HZ: f32 = 60.0;

/// UI to audio-thread messages. Every variant is `Copy`, so sending never
/// allocates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCommand {
    NoteOn {
        note: u8,
        velocity: u8,
    },
    NoteOff {
        note: u8,
    },
    AllNotesOff,
    SetPatch(Patch),
    SetMaster(f32),
    /// Start (`true`) or stop the phrase: the whole studio when plugged into
    /// the desk, just this synth when not.
    PlayPhrase(bool),
    /// Change the phrase tempo: the studio's when plugged in.
    SetBpm(f64),
}

/// Audio-thread to UI snapshot.
#[derive(Debug, Clone, Copy)]
pub struct DisplaySnapshot {
    pub scope: [f32; SCOPE_LEN],
    pub voices: [Option<(u8, bool)>; VOICES],
    pub peak: f32,
}

impl DisplaySnapshot {
    pub const EMPTY: Self = Self {
        scope: [0.0; SCOPE_LEN],
        voices: [None; VOICES],
        peak: 0.0,
    };
}

/// State the audio side publishes for the UI, so nothing it could not do
/// is silently lost.
#[derive(Debug, Default)]
pub struct AudioStats {
    /// Display snapshots the audio thread could not hand to the UI because
    /// the UI had fallen behind (or was gone).
    pub display_dropped: AtomicU64,
    /// Whether the phrase is looping.
    pub phrase_playing: AtomicBool,
    /// The phrase tempo, as `f64` bits: the desk can change it.
    pub bpm: AtomicU64,
    /// Transport messages lost between this synth and the desk: play/stop
    /// and tempo requests the link could not take, and desk changes that
    /// could not be scheduled.
    pub desk_lost: AtomicU64,
    /// Errors cpal reported for the output stream.
    pub stream_errors: AtomicU64,
    /// cpal reported the device gone or the stream invalid: no more audio.
    pub stream_lost: AtomicBool,
}

impl AudioStats {
    /// Record an error cpal reported for the output stream.
    pub fn record_stream_error(&self, err: &cpal::StreamError) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
        match err {
            cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated => {
                self.stream_lost.store(true, Ordering::Release);
            }
            cpal::StreamError::BufferUnderrun | cpal::StreamError::BackendSpecific { .. } => {}
        }
    }

    /// The phrase tempo the engine last published.
    #[must_use]
    pub fn bpm(&self) -> f64 {
        f64::from_bits(self.bpm.load(Ordering::Acquire))
    }
}

/// Write one stereo frame to a device frame of any width: a mono device gets
/// the centre, a stereo one left and right, and extra channels the centre.
fn write_frame(frame: &mut [f32], left: f32, right: f32) {
    let centre = 0.5 * (left + right);
    frame.fill(centre);
    if frame.len() >= 2 {
        frame[0] = left;
        frame[1] = right;
    }
}

/// Everything the output callback owns.
#[derive(Debug)]
pub struct AudioEngine {
    synth: StringSynth,
    phrase: Option<Phrase>,
    commands: Receiver<AudioCommand>,
    display: Sender<DisplaySnapshot>,
    hub: HubLinkAudio,
    /// The desk's transport changes, waiting for their frame.
    follower: TransportFollower,
    stats: Arc<AudioStats>,
    channels: usize,
    /// Tempo, kept even without a phrase so requests carry the right one.
    bpm: f64,
    /// Whether the transport this synth follows is rolling.
    playing: bool,
    /// Interleaved stereo scratch block for the hub.
    stereo: Vec<f32>,
    display_interval: usize,
    since_display: usize,
    peak: f32,
}

impl AudioEngine {
    /// Build the engine. `channels` is the device's channel count and must
    /// be at least 1.
    #[must_use]
    pub fn new(
        sample_rate: u32,
        channels: usize,
        commands: Receiver<AudioCommand>,
        display: Sender<DisplaySnapshot>,
        hub: HubLinkAudio,
        phrase: Option<Phrase>,
        stats: Arc<AudioStats>,
    ) -> Self {
        let rate = sample_rate.max(1);
        let bpm = phrase.as_ref().map_or(DEFAULT_BPM, Phrase::bpm);
        let engine = Self {
            synth: StringSynth::new(rate as f32),
            phrase,
            commands,
            display,
            hub,
            follower: TransportFollower::new(rate),
            stats,
            channels: channels.max(1),
            bpm,
            playing: false,
            stereo: vec![0.0; MAX_BLOCK_FRAMES * 2],
            display_interval: (rate as f32 / DISPLAY_HZ).max(1.0) as usize,
            since_display: 0,
            peak: 0.0,
        };
        engine.publish();
        engine
    }

    /// Render one device buffer. Real-time safe.
    pub fn render(&mut self, data: &mut [f32]) {
        while let Ok(cmd) = self.commands.try_recv() {
            self.apply(cmd);
        }
        self.drain_hub();
        for chunk in data.chunks_mut(MAX_BLOCK_FRAMES * self.channels) {
            self.render_chunk(chunk);
        }
    }

    fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::NoteOn { note, velocity } => self.synth.note_on(note, velocity),
            AudioCommand::NoteOff { note } => self.synth.note_off(note),
            AudioCommand::AllNotesOff => self.synth.all_notes_off(),
            AudioCommand::SetPatch(patch) => self.synth.set_patch(patch),
            AudioCommand::SetMaster(value) => self.synth.set_master(value),
            AudioCommand::PlayPhrase(on) => self.request_playing(on),
            AudioCommand::SetBpm(bpm) => self.request_bpm(bpm),
        }
    }

    /// Play or stop: the whole studio when plugged into the desk (the desk
    /// answers every instrument, this one included, with a transport sync),
    /// or just this phrase when not.
    fn request_playing(&mut self, playing: bool) {
        let state = if playing {
            TRANSPORT_PLAYING
        } else {
            TRANSPORT_STOPPED
        };
        match self.hub.request_transport(state, None) {
            Ok(()) => {}
            Err(RequestError::NotConnected) => {
                if playing {
                    self.play_from(0.0);
                } else {
                    self.stop();
                }
            }
            Err(RequestError::Full | RequestError::Invalid) => self.desk_lost(),
        }
    }

    /// Change tempo: the studio's when plugged into the desk, this phrase's
    /// when not.
    fn request_bpm(&mut self, bpm: f64) {
        // A NaN stays NaN and is refused by the link, or ignored locally.
        let bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        // Only the tempo: the desk keeps its own play state, which this
        // instrument may not have heard yet.
        // Tempos are tens to hundreds of BPM: f32 holds them closely enough.
        match self.hub.request_tempo(bpm as f32) {
            Ok(()) => {}
            Err(RequestError::NotConnected) => self.set_bpm(bpm),
            Err(RequestError::Full | RequestError::Invalid) => self.desk_lost(),
        }
    }

    fn desk_lost(&self) {
        self.stats.desk_lost.fetch_add(1, Ordering::Relaxed);
    }

    fn set_bpm(&mut self, bpm: f64) {
        if !(bpm.is_finite() && bpm > 0.0) {
            return;
        }
        match self.phrase.as_mut() {
            Some(phrase) => {
                phrase.set_bpm(bpm);
                self.bpm = phrase.bpm();
            }
            None => self.bpm = bpm.clamp(MIN_BPM, MAX_BPM),
        }
        self.publish();
    }

    fn play_from(&mut self, beat: f64) {
        self.playing = true;
        if let Some(phrase) = self.phrase.as_mut() {
            if phrase.play_from(beat) {
                // A jump: notes from the old place must not ring on.
                self.synth.release_source(Source::Phrase);
            }
        }
        self.publish();
    }

    fn stop(&mut self) {
        self.playing = false;
        if let Some(phrase) = self.phrase.as_mut() {
            phrase.stop();
        }
        self.synth.release_source(Source::Phrase);
        self.publish();
    }

    fn publish(&self) {
        self.stats.bpm.store(self.bpm.to_bits(), Ordering::Release);
        let looping = self.phrase.as_ref().is_some_and(Phrase::is_playing);
        self.stats.phrase_playing.store(looping, Ordering::Release);
    }

    fn drain_hub(&mut self) {
        while let Some(msg) = self.hub.try_recv() {
            match msg {
                // Lands on its own frame, in `render_chunk`.
                HubMessage::TransportSync(sync) => {
                    if self.follower.schedule(&sync).is_err() {
                        self.desk_lost();
                    }
                }
                HubMessage::NoteEvent(event) => match event.event_type {
                    NOTE_ON => self.synth.note_on(event.note, event.velocity),
                    NOTE_OFF => self.synth.note_off(event.note),
                    // Controllers and pitch bend have no string mapping yet.
                    _ => {}
                },
                // No hub-controllable parameters, and the link itself
                // handles the hub shutting down.
                HubMessage::ParameterChange(_) | HubMessage::Shutdown => {}
            }
        }
    }

    /// Apply a desk transport change on its frame.
    fn follow(&mut self, change: TransportChange) {
        self.set_bpm(change.bpm);
        match change.beat {
            Some(beat) => self.play_from(beat),
            None => self.stop(),
        }
    }

    /// Render at most [`MAX_BLOCK_FRAMES`] frames into `chunk`.
    fn render_chunk(&mut self, chunk: &mut [f32]) {
        let frames = (chunk.len() / self.channels).min(MAX_BLOCK_FRAMES);
        let first = self.hub.stream_frame();
        for i in 0..frames {
            // `i` < MAX_BLOCK_FRAMES: lossless.
            if let Some(change) = self.follower.due(first + i as u64) {
                self.follow(change);
            }
            if let Some(phrase) = self.phrase.as_mut() {
                let synth = &mut self.synth;
                phrase.advance(|note, velocity| match velocity {
                    Some(velocity) => synth.note_on_from(Source::Phrase, note, velocity),
                    None => synth.note_off_from(Source::Phrase, note),
                });
            }
            let [left, right] = self.synth.process();
            self.stereo[i * 2] = left;
            self.stereo[i * 2 + 1] = right;
            self.peak = self.peak.max(left.abs()).max(right.abs());
        }

        // `frames` <= MAX_BLOCK_FRAMES, so the cast is lossless.
        let routed = self
            .hub
            .send_audio(frames as u32, &self.stereo[..frames * 2]);
        for (frame, pair) in chunk
            .chunks_mut(self.channels)
            .zip(self.stereo[..frames * 2].chunks_exact(2))
        {
            // While the desk plays this instrument, it is not played twice.
            if routed {
                frame.fill(0.0);
            } else {
                write_frame(frame, pair[0], pair[1]);
            }
        }
        // A trailing partial frame (never expected) is silenced.
        chunk[frames * self.channels..].fill(0.0);

        self.since_display += frames;
        if self.since_display >= self.display_interval {
            self.since_display = 0;
            self.push_display();
        }
    }

    fn push_display(&mut self) {
        let (ring, pos) = self.synth.scope();
        let mut scope = [0.0_f32; SCOPE_LEN];
        let (older, newer) = ring.split_at(pos);
        scope[..newer.len()].copy_from_slice(newer);
        scope[newer.len()..].copy_from_slice(older);
        let snapshot = DisplaySnapshot {
            scope,
            voices: self.synth.voice_states(),
            peak: self.peak,
        };
        // Full: the UI is behind and still has a frame queued.
        // Disconnected: the UI has gone. Either way the frame is counted,
        // never waited on.
        if self.display.try_send(snapshot).is_err() {
            self.stats.display_dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.peak = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_fit_any_device_width() {
        let mut mono = [9.0];
        write_frame(&mut mono, 0.2, 0.6);
        assert!((mono[0] - 0.4).abs() < 1.0e-6);
        let mut stereo = [9.0; 2];
        write_frame(&mut stereo, 0.2, 0.6);
        assert!((stereo[0] - 0.2).abs() < 1.0e-6 && (stereo[1] - 0.6).abs() < 1.0e-6);
        let mut surround = [9.0; 6];
        write_frame(&mut surround, 0.2, 0.6);
        assert!((surround[0] - 0.2).abs() < 1.0e-6 && (surround[1] - 0.6).abs() < 1.0e-6);
        assert!(surround[2..].iter().all(|s| (s - 0.4).abs() < 1.0e-6));
    }
    use kazoo_core::ipc::link::{HubAddress, HubLink, LinkConfig, hub_link};
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{
        MSG_REGISTER, MSG_REGISTERED, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC, RegisteredMsg,
        TransportRequestMsg, TransportSyncMsg,
    };
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const RATE: u32 = 48_000;

    struct Rig {
        engine: AudioEngine,
        tx: Sender<AudioCommand>,
        stats: Arc<AudioStats>,
        _display: Receiver<DisplaySnapshot>,
    }

    fn rig_with(hub: HubLinkAudio, phrase: Option<&str>) -> Rig {
        let (tx, rx) = crossbeam_channel::bounded(64);
        let (display_tx, display_rx) = crossbeam_channel::bounded(2);
        let stats = Arc::new(AudioStats::default());
        let phrase = phrase.map(|text| Phrase::parse(text, DEFAULT_BPM, RATE).unwrap());
        let engine = AudioEngine::new(RATE, 2, rx, display_tx, hub, phrase, Arc::clone(&stats));
        Rig {
            engine,
            tx,
            stats,
            _display: display_rx,
        }
    }

    /// An engine whose link never finds a desk.
    fn unplugged(phrase: Option<&str>) -> Rig {
        let mut config = LinkConfig::new("kazoo-string-test", 2, RATE, MAX_BLOCK_FRAMES as u32);
        config.address = HubAddress::Socket(
            std::env::temp_dir().join(format!("kazoo-string-no-hub-{}.sock", std::process::id())),
        );
        let (hub, hub_audio) = hub_link(config).unwrap();
        // The UI half says goodbye on drop; the audio half keeps working
        // unconnected, which is what these tests exercise.
        drop(hub);
        rig_with(hub_audio, phrase)
    }

    fn sounding(engine: &AudioEngine, note: u8) -> bool {
        engine
            .synth
            .voice_states()
            .iter()
            .flatten()
            .any(|&(n, held)| n == note && held)
    }

    #[test]
    fn unplugged_the_phrase_plays_and_changes_tempo_locally() {
        let mut rig = unplugged(Some("c4/4 e4/4"));
        assert!((rig.stats.bpm() - 112.0).abs() < 1e-9);
        rig.tx.send(AudioCommand::PlayPhrase(true)).unwrap();
        rig.engine.render(&mut [0.0; 64]);
        assert!(rig.stats.phrase_playing.load(Ordering::Acquire));
        assert!(sounding(&rig.engine, 60), "the downbeat plays at once");
        rig.tx.send(AudioCommand::SetBpm(90.0)).unwrap();
        rig.engine.render(&mut [0.0; 64]);
        assert!((rig.stats.bpm() - 90.0).abs() < 1e-9);
        rig.tx.send(AudioCommand::PlayPhrase(false)).unwrap();
        rig.engine.render(&mut [0.0; 64]);
        assert!(!rig.stats.phrase_playing.load(Ordering::Acquire));
        assert!(!sounding(&rig.engine, 60), "stopping releases the phrase");
        assert_eq!(rig.stats.desk_lost.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn every_block_advances_the_stream_while_unplugged() {
        let mut rig = unplugged(None);
        // Longer than one block: rendered in passes, every frame counted.
        let mut data = vec![0.5_f32; (MAX_BLOCK_FRAMES * 2 + 100) * 2];
        rig.engine.render(&mut data);
        assert_eq!(
            rig.engine.hub.stream_frame(),
            (MAX_BLOCK_FRAMES * 2 + 100) as u64
        );
        assert!(data.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn hub_notes_play_the_synth() {
        let mut rig = unplugged(None);
        rig.tx
            .send(AudioCommand::NoteOn {
                note: 64,
                velocity: 100,
            })
            .unwrap();
        let mut data = vec![0.0_f32; 2_048];
        rig.engine.render(&mut data);
        assert!(sounding(&rig.engine, 64));
        assert!(data.iter().any(|s| s.abs() > 1e-4), "heard locally");
    }

    /// A desk on a private socket that has registered one kazoo-string. A
    /// reader thread keeps taking the synth's audio, as a real desk does, so
    /// the link never gives up on it, and hands over its transport requests.
    struct Desk {
        stream: UnixStream,
        buf: FrameBuffer,
        path: PathBuf,
        requests: std::sync::mpsc::Receiver<TransportRequestMsg>,
        reader: Option<std::thread::JoinHandle<()>>,
    }

    impl Desk {
        fn plug_in(phrase: &str) -> (Self, Rig, HubLink) {
            let path =
                std::env::temp_dir().join(format!("kazoo-string-desk-{}.sock", std::process::id()));
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            let listener = UnixListener::bind(&path).unwrap();
            let mut config = LinkConfig::new("kazoo-string-test", 2, RATE, MAX_BLOCK_FRAMES as u32);
            config.address = HubAddress::Socket(path.clone());
            let (hub, hub_audio) = hub_link(config).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = FrameBuffer::new();
            assert_eq!(buf.read_frame(&mut stream).unwrap().msg_type, MSG_REGISTER);
            RegisteredMsg {
                strip_index: 0,
                hub_sample_rate: RATE,
                hub_buffer_size: 256,
                transport_state: TRANSPORT_STOPPED,
                bpm: 100.0,
                position: 0,
            }
            .encode(buf.payload_mut());
            buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
                .unwrap();

            let (request_tx, requests) = std::sync::mpsc::channel();
            let mut read_half = stream.try_clone().unwrap();
            let reader = std::thread::spawn(move || {
                let mut buf = FrameBuffer::new();
                // Ends when the test shuts the socket down.
                while let Ok(header) = buf.read_frame(&mut read_half) {
                    if header.msg_type == MSG_TRANSPORT_REQUEST
                        && request_tx
                            .send(TransportRequestMsg::decode(buf.payload()))
                            .is_err()
                    {
                        return;
                    }
                }
            });

            let mut rig = rig_with(hub_audio, Some(phrase));
            // Joining hands the synth the desk's tempo.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if (rig.stats.bpm() - 100.0).abs() < 1e-9 {
                    break;
                }
                assert!(Instant::now() < deadline, "never joined");
                rig.engine.render(&mut [0.0; 64]);
                std::thread::sleep(Duration::from_millis(1));
            }
            (
                Self {
                    stream,
                    buf,
                    path,
                    requests,
                    reader: Some(reader),
                },
                rig,
                hub,
            )
        }

        /// The next transport request the synth sent.
        fn transport_request(&self) -> TransportRequestMsg {
            self.requests.recv_timeout(Duration::from_secs(5)).unwrap()
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
            if let Err(err) = self.stream.shutdown(std::net::Shutdown::Both) {
                assert_eq!(err.kind(), std::io::ErrorKind::NotConnected, "{err}");
            }
            if let Some(reader) = self.reader.take() {
                assert!(reader.join().is_ok(), "the desk reader panicked");
            }
            if let Err(err) = std::fs::remove_file(&self.path) {
                assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
            }
        }
    }

    #[test]
    fn plugged_in_the_phrase_asks_the_desk_and_starts_on_its_frame() {
        let (mut desk, mut rig, hub) = Desk::plug_in("c4/4 e4/4");

        rig.tx.send(AudioCommand::PlayPhrase(true)).unwrap();
        rig.engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.requested_state, TRANSPORT_PLAYING);
        assert_eq!(request.has_bpm, 0);
        // Asking is not playing: the desk decides.
        assert!(!rig.stats.phrase_playing.load(Ordering::Acquire));

        rig.tx.send(AudioCommand::SetBpm(131.0)).unwrap();
        rig.engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.has_bpm, 1);
        assert_eq!(request.requested_bpm.to_bits(), 131.0_f32.to_bits());
        assert!((rig.stats.bpm() - 100.0).abs() < 1e-9);

        // Play from beat 1 (the e4), 5 000 frames into the synth's future.
        let start = rig.engine.hub.stream_frame() + 5_000;
        desk.sync(TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: 131.0,
            at_frame: start,
            beat: 1.0,
        });
        // Wait for the sync to reach the callback without rendering past the
        // start frame, then render one frame at a time up to it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.engine.follower.is_idle() {
            assert!(Instant::now() < deadline, "the sync never arrived");
            rig.engine.render(&mut []);
            std::thread::sleep(Duration::from_millis(1));
        }
        while rig.engine.hub.stream_frame() < start {
            assert!(
                !rig.stats.phrase_playing.load(Ordering::Acquire),
                "started early"
            );
            rig.engine.render(&mut [0.0; 2]);
        }
        assert!(!sounding(&rig.engine, 64));
        // The start frame itself plays the note on beat 1.
        rig.engine.render(&mut [0.0; 2]);
        assert!(rig.stats.phrase_playing.load(Ordering::Acquire));
        assert!(sounding(&rig.engine, 64));
        assert!(!sounding(&rig.engine, 60));
        assert!((rig.stats.bpm() - 131.0).abs() < 1e-9);

        // Beat 2 (the c4 of the next loop) lands exactly one beat later.
        let next = start + (60.0 * f64::from(RATE) / 131.0).ceil() as u64;
        while rig.engine.hub.stream_frame() < next {
            assert!(!sounding(&rig.engine, 60), "the next beat came early");
            rig.engine.render(&mut [0.0; 2]);
        }
        rig.engine.render(&mut [0.0; 2]);
        assert!(sounding(&rig.engine, 60));

        // The desk stops on its frame, too.
        let stop = rig.engine.hub.stream_frame() + 1_000;
        desk.sync(TransportSyncMsg {
            state: TRANSPORT_STOPPED,
            bpm: 131.0,
            at_frame: stop,
            beat: f64::NAN,
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.engine.follower.is_idle() {
            assert!(Instant::now() < deadline, "the stop never arrived");
            rig.engine.render(&mut []);
            std::thread::sleep(Duration::from_millis(1));
        }
        while rig.engine.hub.stream_frame() < stop {
            assert!(rig.stats.phrase_playing.load(Ordering::Acquire));
            rig.engine.render(&mut [0.0; 2]);
        }
        rig.engine.render(&mut [0.0; 2]);
        assert!(!rig.stats.phrase_playing.load(Ordering::Acquire));
        assert!(!sounding(&rig.engine, 60));
        assert_eq!(rig.stats.desk_lost.load(Ordering::Relaxed), 0);
        assert!(hub.is_connected(), "the desk kept the synth plugged in");
    }

    #[test]
    fn stream_errors_are_recorded() {
        let stats = AudioStats::default();
        stats.record_stream_error(&cpal::StreamError::BufferUnderrun);
        assert!(!stats.stream_lost.load(Ordering::Acquire));
        stats.record_stream_error(&cpal::StreamError::DeviceNotAvailable);
        assert_eq!(stats.stream_errors.load(Ordering::Relaxed), 2);
        assert!(stats.stream_lost.load(Ordering::Acquire));
    }
}
