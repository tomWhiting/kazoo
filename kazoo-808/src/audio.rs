//! The 808's audio engine: everything the cpal output callback owns.
//!
//! The callback drains UI commands and hub messages, runs the sequencer and
//! drum machine, and hands each rendered block to the hub link. All state is
//! allocated here, before the stream starts; rendering never allocates,
//! locks or blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use kazoo_808::sequencer::Sequencer;
use kazoo_808::synth::{DrumMachine, VoiceIndex, VoiceParam};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportChange, TransportFollower};
use kazoo_core::ipc::link::{HubLinkAudio, RequestError};
use kazoo_core::ipc::types::{NOTE_ON, TRANSPORT_PLAYING, TRANSPORT_STOPPED};

/// Largest block rendered in one go, in frames. Longer device buffers are
/// rendered in chunks of this size, and it is the block size the hub is
/// told about.
pub const MAX_CALLBACK_FRAMES: usize = 4096;

/// Capacity of the UI -> audio command channel.
pub const COMMAND_CAPACITY: usize = 256;

/// Commands sent from the UI thread to the audio thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCommand {
    Play,
    Stop,
    SetBpm(f64),
    SetSwing(f64),
    ToggleStep {
        voice: usize,
        step: usize,
    },
    ToggleAccent {
        voice: usize,
        step: usize,
    },
    TriggerVoice {
        voice: usize,
        velocity: f32,
    },
    SetVoiceParam {
        voice: VoiceIndex,
        param: VoiceParam,
        value: f32,
    },
    SelectPattern(usize),
    /// Add an empty pattern to the bank and select it.
    AddPattern,
}

/// State the audio side publishes for the UI.
#[derive(Debug, Default)]
pub struct EngineShared {
    /// Step the sequencer fired most recently.
    pub playback_step: AtomicUsize,
    /// Whether the sequencer is running.
    pub playing: AtomicBool,
    /// Errors cpal reported for the output stream.
    pub stream_errors: AtomicU64,
    /// cpal reported the device gone or the stream invalid: no more audio.
    pub stream_lost: AtomicBool,
    /// The sequencer's tempo, as `f64` bits: the desk can change it.
    pub bpm: AtomicU64,
    /// Transport messages lost between the 808 and the desk: play/stop and
    /// tempo requests the link could not take, and desk changes the 808
    /// could not schedule.
    pub desk_lost: AtomicU64,
}

impl EngineShared {
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
}

/// Why the audio engine did not receive a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendFailure {
    /// The command queue is full: the audio callback is not keeping up.
    QueueFull,
    /// The audio callback is gone (stream stopped or device lost).
    EngineGone,
}

/// UI-side sender for audio commands that records every failed delivery,
/// so the UI can show it instead of silently diverging from the engine.
#[derive(Debug)]
pub struct CommandSender {
    tx: Sender<AudioCommand>,
    failed: u64,
    last_failure: Option<SendFailure>,
}

impl CommandSender {
    #[must_use]
    pub const fn new(tx: Sender<AudioCommand>) -> Self {
        Self {
            tx,
            failed: 0,
            last_failure: None,
        }
    }

    /// Queue a command for the audio thread. Returns whether it was queued,
    /// so the caller can mirror the change only once the engine will see
    /// it; failures are counted and remembered for display.
    pub fn send(&mut self, cmd: AudioCommand) -> bool {
        match self.deliver(cmd) {
            Ok(()) => true,
            Err(failure) => {
                self.record(failure);
                false
            }
        }
    }

    /// Queue a command that has no UI-side state to keep in step (an
    /// audition, or an absolute value the UI already shows). A failure is
    /// counted and remembered for display.
    pub fn post(&mut self, cmd: AudioCommand) {
        if let Err(failure) = self.deliver(cmd) {
            self.record(failure);
        }
    }

    fn deliver(&self, cmd: AudioCommand) -> Result<(), SendFailure> {
        self.tx.try_send(cmd).map_err(|err| match err {
            TrySendError::Full(_) => SendFailure::QueueFull,
            TrySendError::Disconnected(_) => SendFailure::EngineGone,
        })
    }

    const fn record(&mut self, failure: SendFailure) {
        self.failed = self.failed.saturating_add(1);
        self.last_failure = Some(failure);
    }

    /// Commands the audio thread never received.
    #[must_use]
    pub const fn failed(&self) -> u64 {
        self.failed
    }

    /// The most recent reason a command was not delivered.
    #[must_use]
    pub const fn last_failure(&self) -> Option<SendFailure> {
        self.last_failure
    }
}

/// Everything the output callback owns.
#[derive(Debug)]
pub struct AudioEngine {
    drum_machine: DrumMachine,
    sequencer: Sequencer,
    commands: Receiver<AudioCommand>,
    hub: HubLinkAudio,
    /// The desk's transport changes, waiting for their frame.
    follower: TransportFollower,
    shared: Arc<EngineShared>,
    channels: usize,
    /// Interleaved stereo scratch block for the hub.
    stereo: Vec<f32>,
}

impl AudioEngine {
    /// Build the engine. `channels` is the device's channel count and must
    /// be at least 1.
    #[must_use]
    pub fn new(
        sample_rate: f32,
        channels: usize,
        commands: Receiver<AudioCommand>,
        hub: HubLinkAudio,
        shared: Arc<EngineShared>,
    ) -> Self {
        let engine = Self {
            drum_machine: DrumMachine::new(sample_rate),
            sequencer: Sequencer::new(sample_rate),
            commands,
            hub,
            // Device rates are whole numbers of Hz.
            follower: TransportFollower::new(sample_rate.round() as u32),
            shared,
            channels: channels.max(1),
            stereo: vec![0.0; MAX_CALLBACK_FRAMES * 2],
        };
        engine.publish_bpm();
        engine
    }

    /// Render one device buffer. Real-time safe.
    pub fn render(&mut self, data: &mut [f32]) {
        self.drain_commands();
        self.drain_hub();
        let chunk_len = MAX_CALLBACK_FRAMES * self.channels;
        for chunk in data.chunks_mut(chunk_len) {
            self.render_chunk(chunk);
        }
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            self.apply(cmd);
        }
    }

    fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Play => self.request_playing(true),
            AudioCommand::Stop => self.request_playing(false),
            AudioCommand::SetBpm(bpm) => self.request_bpm(bpm),
            AudioCommand::SetSwing(swing) => self.sequencer.clock.set_swing(swing),
            AudioCommand::ToggleStep { voice, step } => self.sequencer.toggle_step(voice, step),
            AudioCommand::ToggleAccent { voice, step } => {
                self.sequencer.toggle_accent(voice, step);
            }
            AudioCommand::TriggerVoice { voice, velocity } => {
                if let Some(vi) = VoiceIndex::from_index(voice) {
                    self.drum_machine.trigger(vi, velocity);
                }
            }
            AudioCommand::SetVoiceParam {
                voice,
                param,
                value,
            } => self.drum_machine.set_voice_param(voice, param, value),
            AudioCommand::SelectPattern(idx) => self.sequencer.select_pattern(idx),
            AudioCommand::AddPattern => {
                // The UI only asks while the bank has room (it mirrors the
                // same bank), so a full bank here leaves both sides equal.
                if let Some(idx) = self.sequencer.add_pattern() {
                    self.sequencer.select_pattern(idx);
                }
            }
        }
    }

    /// Play or stop: the whole studio when plugged into the desk (the desk
    /// answers every instrument, this one included, with a transport sync),
    /// or just this 808 when not.
    fn request_playing(&mut self, playing: bool) {
        let state = if playing {
            TRANSPORT_PLAYING
        } else {
            TRANSPORT_STOPPED
        };
        match self.hub.request_transport(state, None) {
            Ok(()) => {}
            Err(RequestError::NotConnected) => self.set_playing(playing),
            Err(RequestError::Full | RequestError::Invalid) => self.desk_lost(),
        }
    }

    /// Change tempo: the studio's when plugged into the desk, this 808's
    /// when not.
    fn request_bpm(&mut self, bpm: f64) {
        // The desk's range and the sequencer's are the same; a NaN stays NaN
        // and is refused below.
        let bpm = bpm.clamp(20.0, 300.0);
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
        self.shared.desk_lost.fetch_add(1, Ordering::Relaxed);
    }

    fn set_bpm(&mut self, bpm: f64) {
        self.sequencer.clock.set_bpm(bpm);
        self.publish_bpm();
    }

    fn publish_bpm(&self) {
        self.shared
            .bpm
            .store(self.sequencer.clock.bpm().to_bits(), Ordering::Release);
    }

    fn set_playing(&mut self, playing: bool) {
        if playing == self.sequencer.playing {
            return;
        }
        if playing {
            self.sequencer.play();
        } else {
            self.sequencer.stop();
        }
        self.shared.playing.store(playing, Ordering::Release);
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
                HubMessage::NoteEvent(event) => {
                    if event.event_type == NOTE_ON {
                        if let Some(vi) = VoiceIndex::from_index(usize::from(event.note)) {
                            let velocity = f32::from(event.velocity) / 127.0;
                            self.drum_machine.trigger(vi, velocity);
                        }
                    }
                }
                // The 808 exposes no hub-controllable parameters, and the
                // link itself handles the hub shutting down.
                HubMessage::ParameterChange(_) | HubMessage::Shutdown => {}
            }
        }
    }

    /// Whether a desk transport change is waiting for its frame.
    #[cfg(test)]
    const fn follower_waiting(&self) -> bool {
        !self.follower.is_idle()
    }

    /// Apply a desk transport change on its frame.
    fn follow(&mut self, change: TransportChange) {
        self.set_bpm(change.bpm);
        match change.beat {
            Some(beat) => {
                self.sequencer.play_from(beat);
                self.shared.playing.store(true, Ordering::Release);
            }
            None => self.set_playing(false),
        }
    }

    /// Render at most [`MAX_CALLBACK_FRAMES`] frames into `chunk`.
    fn render_chunk(&mut self, chunk: &mut [f32]) {
        let mut frames = 0;
        let first = self.hub.stream_frame();
        for frame in chunk.chunks_mut(self.channels) {
            if let Some(change) = self.follower.due(first + frames as u64) {
                self.follow(change);
            }
            if let Some(step) = self.sequencer.tick(&mut self.drum_machine) {
                self.shared.playback_step.store(step, Ordering::Release);
            }
            let sample = kazoo_core::soft_limit(self.drum_machine.process());
            frame.fill(sample);
            self.stereo[frames * 2] = sample;
            self.stereo[frames * 2 + 1] = sample;
            frames += 1;
        }

        // `frames` <= MAX_CALLBACK_FRAMES, so the cast is lossless.
        if self
            .hub
            .send_audio(frames as u32, &self.stereo[..frames * 2])
        {
            // The desk is playing this instrument: don't play it twice.
            chunk.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_core::ipc::link::{HubAddress, HubLink, LinkConfig, hub_link};

    fn engine(channels: usize) -> (AudioEngine, CommandSender, Arc<EngineShared>) {
        let mut config = LinkConfig::new("kazoo-808-test", 2, 48_000, MAX_CALLBACK_FRAMES as u32);
        config.address = HubAddress::Socket(
            std::env::temp_dir().join(format!("kazoo-808-no-hub-{}.sock", std::process::id())),
        );
        let (hub, hub_audio) = hub_link(config).unwrap();
        // The UI half says goodbye on drop; the audio half keeps working
        // unconnected, which is what these tests exercise.
        drop(hub);
        let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
        let shared = Arc::new(EngineShared::default());
        let engine = AudioEngine::new(48_000.0, channels, rx, hub_audio, Arc::clone(&shared));
        (engine, CommandSender::new(tx), shared)
    }

    /// A desk on a private socket that has registered one 808.
    struct Desk {
        stream: std::os::unix::net::UnixStream,
        buf: kazoo_core::ipc::protocol::FrameBuffer,
        path: std::path::PathBuf,
    }

    impl Desk {
        fn plug_in() -> (Self, AudioEngine, CommandSender, Arc<EngineShared>, HubLink) {
            use kazoo_core::ipc::types::{MSG_REGISTER, MSG_REGISTERED, RegisteredMsg};
            let path =
                std::env::temp_dir().join(format!("kazoo-808-desk-{}.sock", std::process::id()));
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
            let mut config =
                LinkConfig::new("kazoo-808-test", 2, 48_000, MAX_CALLBACK_FRAMES as u32);
            config.address = HubAddress::Socket(path.clone());
            let (hub, hub_audio) = hub_link(config).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = kazoo_core::ipc::protocol::FrameBuffer::new();
            assert_eq!(buf.read_frame(&mut stream).unwrap().msg_type, MSG_REGISTER);
            RegisteredMsg {
                strip_index: 0,
                hub_sample_rate: 48_000,
                hub_buffer_size: 256,
                transport_state: TRANSPORT_STOPPED,
                bpm: 100.0,
                position: 0,
            }
            .encode(buf.payload_mut());
            buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
                .unwrap();

            let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
            let shared = Arc::new(EngineShared::default());
            let mut engine = AudioEngine::new(48_000.0, 2, rx, hub_audio, Arc::clone(&shared));
            // Joining hands the 808 the desk's tempo.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let joined = |shared: &EngineShared| {
                (f64::from_bits(shared.bpm.load(Ordering::Acquire)) - 100.0).abs() < 1e-9
            };
            while !joined(&shared) {
                assert!(std::time::Instant::now() < deadline, "never joined");
                engine.render(&mut [0.0; 64]);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            (
                Self { stream, buf, path },
                engine,
                CommandSender::new(tx),
                shared,
                hub,
            )
        }

        /// The next transport request the 808 sent, skipping its audio.
        fn transport_request(&mut self) -> kazoo_core::ipc::types::TransportRequestMsg {
            use kazoo_core::ipc::types::{MSG_TRANSPORT_REQUEST, TransportRequestMsg};
            loop {
                let header = self.buf.read_frame(&mut self.stream).unwrap();
                if header.msg_type == MSG_TRANSPORT_REQUEST {
                    return TransportRequestMsg::decode(self.buf.payload());
                }
            }
        }
    }

    impl Drop for Desk {
        fn drop(&mut self) {
            if let Err(err) = std::fs::remove_file(&self.path) {
                assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
            }
        }
    }

    #[test]
    fn plugged_in_the_808_asks_the_desk_and_follows_its_answer() {
        use kazoo_core::ipc::types::{MSG_TRANSPORT_SYNC, TransportSyncMsg};
        let (mut desk, mut engine, mut tx, shared, _hub) = Desk::plug_in();

        assert!(tx.send(AudioCommand::Play));
        engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.requested_state, TRANSPORT_PLAYING);
        assert_eq!(request.has_bpm, 0);
        // Asking is not playing: the desk decides.
        assert!(!shared.playing.load(Ordering::Acquire));

        assert!(tx.send(AudioCommand::SetBpm(131.0)));
        engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.has_bpm, 1);
        assert_eq!(request.requested_bpm.to_bits(), 131.0_f32.to_bits());
        assert!((f64::from_bits(shared.bpm.load(Ordering::Acquire)) - 100.0).abs() < 1e-9);

        // Play from beat 0, 5 000 frames into the 808's future.
        let start = engine.hub.stream_frame() + 5_000;
        TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: 131.0,
            at_frame: start,
            beat: 0.0,
        }
        .encode(desk.buf.payload_mut());
        desk.buf
            .write_frame(
                MSG_TRANSPORT_SYNC,
                1,
                TransportSyncMsg::WIRE_SIZE,
                &mut desk.stream,
            )
            .unwrap();
        // Wait for the sync to reach the callback without rendering past the
        // start frame, then render one frame at a time up to it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !engine.follower_waiting() {
            assert!(
                std::time::Instant::now() < deadline,
                "the sync never arrived"
            );
            engine.render(&mut []);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        while engine.hub.stream_frame() < start {
            assert!(!shared.playing.load(Ordering::Acquire), "started early");
            engine.render(&mut [0.0; 2]);
        }
        // The start frame itself is the downbeat.
        engine.render(&mut [0.0; 2]);
        assert!(shared.playing.load(Ordering::Acquire));
        assert!((f64::from_bits(shared.bpm.load(Ordering::Acquire)) - 131.0).abs() < 1e-9);
        assert_eq!(shared.desk_lost.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unplugged_tempo_changes_are_local_and_published() {
        let (mut engine, mut tx, shared) = engine(2);
        assert!((f64::from_bits(shared.bpm.load(Ordering::Acquire)) - 120.0).abs() < 1e-9);
        assert!(tx.send(AudioCommand::SetBpm(90.0)));
        engine.render(&mut [0.0; 64]);
        assert!((f64::from_bits(shared.bpm.load(Ordering::Acquire)) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn audition_is_audible_on_every_channel() {
        let (mut engine, mut tx, _) = engine(2);
        assert!(tx.send(AudioCommand::TriggerVoice {
            voice: VoiceIndex::Kick as usize,
            velocity: 1.0,
        }));
        let mut data = vec![0.0_f32; 1024];
        engine.render(&mut data);
        assert!(data.iter().any(|s| s.abs() > 0.01), "kick must be audible");
        for frame in data.chunks(2) {
            assert!((frame[0] - frame[1]).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn buffers_longer_than_the_block_size_are_fully_rendered() {
        let (mut engine, mut tx, _) = engine(1);
        let frames = MAX_CALLBACK_FRAMES * 2 + 100;
        let mut data = vec![0.0_f32; frames];
        // Trigger once the first chunk has rendered, so the kick lands in
        // the region beyond MAX_CALLBACK_FRAMES.
        engine.render(&mut data[..MAX_CALLBACK_FRAMES]);
        assert!(tx.send(AudioCommand::TriggerVoice {
            voice: VoiceIndex::Kick as usize,
            velocity: 1.0,
        }));
        data.fill(0.0);
        engine.render(&mut data);
        let tail = &data[MAX_CALLBACK_FRAMES..];
        assert!(tail.iter().any(|s| s.abs() > 0.01), "tail must be rendered");
    }

    #[test]
    fn play_and_stop_are_published() {
        let (mut engine, mut tx, shared) = engine(2);
        assert!(tx.send(AudioCommand::Play));
        engine.render(&mut [0.0; 64]);
        assert!(shared.playing.load(Ordering::Acquire));
        assert!(tx.send(AudioCommand::Stop));
        engine.render(&mut [0.0; 64]);
        assert!(!shared.playing.load(Ordering::Acquire));
    }

    #[test]
    fn add_pattern_adds_and_selects() {
        let (mut engine, mut tx, _) = engine(2);
        assert!(tx.send(AudioCommand::AddPattern));
        engine.render(&mut [0.0; 8]);
        assert_eq!(engine.sequencer.patterns.len(), 2);
        assert_eq!(engine.sequencer.current_pattern, 1);
    }

    #[test]
    fn full_queue_is_counted() {
        let (tx, _rx) = crossbeam_channel::bounded(1);
        let mut sender = CommandSender::new(tx);
        assert!(sender.send(AudioCommand::Play));
        assert!(!sender.send(AudioCommand::Stop));
        assert_eq!(sender.failed(), 1);
        assert_eq!(sender.last_failure(), Some(SendFailure::QueueFull));
    }

    #[test]
    fn failed_posts_are_counted() {
        let (tx, _rx) = crossbeam_channel::bounded(1);
        let mut sender = CommandSender::new(tx);
        sender.post(AudioCommand::Play);
        assert_eq!(sender.failed(), 0);
        sender.post(AudioCommand::Stop);
        assert_eq!(sender.failed(), 1);
        assert_eq!(sender.last_failure(), Some(SendFailure::QueueFull));
    }

    #[test]
    fn missing_engine_is_counted() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        drop(rx);
        let mut sender = CommandSender::new(tx);
        assert!(!sender.send(AudioCommand::Play));
        assert_eq!(sender.failed(), 1);
        assert_eq!(sender.last_failure(), Some(SendFailure::EngineGone));
    }

    #[test]
    fn stream_errors_are_recorded() {
        let shared = EngineShared::default();
        shared.record_stream_error(&cpal::StreamError::BufferUnderrun);
        assert_eq!(shared.stream_errors.load(Ordering::Relaxed), 1);
        assert!(!shared.stream_lost.load(Ordering::Acquire));
        shared.record_stream_error(&cpal::StreamError::DeviceNotAvailable);
        assert_eq!(shared.stream_errors.load(Ordering::Relaxed), 2);
        assert!(shared.stream_lost.load(Ordering::Acquire));
    }
}
