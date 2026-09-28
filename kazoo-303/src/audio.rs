//! The 303's audio engine: everything the cpal output callback owns.
//!
//! The callback drains UI commands and hub messages, runs the sequencer and
//! synth, and hands each rendered block to the hub link. All state is
//! allocated before the stream starts; rendering never allocates, locks or
//! blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportChange, TransportFollower};
use kazoo_core::ipc::link::{HubLinkAudio, RequestError};
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON, TRANSPORT_PLAYING, TRANSPORT_STOPPED};

use crate::sequencer::{Sequencer, SequencerClock};
use crate::synth::{AcidSynth, AcidSynthParam, Waveform};

/// Largest block rendered in one go, in frames. Longer device buffers are
/// rendered in chunks of this size, and it is the block size the hub is
/// told about.
pub const MAX_BLOCK: usize = 4096;

/// Capacity of the UI -> audio command channel.
pub const COMMAND_CAPACITY: usize = 256;

/// Output gain ahead of the soft limiter.
const OUTPUT_GAIN: f32 = 0.8;

/// Hub velocity at or above which a note is accented.
const ACCENT_VELOCITY: u8 = 100;

/// [`EngineShared::hub_note`] value meaning no hub note is held.
pub const NO_HUB_NOTE: u8 = u8::MAX;

/// UI to audio-thread messages. Every variant is `Copy`, so sending never
/// allocates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCommand {
    Play,
    Stop,
    SetBpm(f64),
    SetSwing(f64),
    ToggleStep(usize),
    ToggleAccent(usize),
    ToggleSlide(usize),
    TransposeStep { step: usize, semitones: i8 },
    SetParam { param: AcidSynthParam, value: f32 },
    SetWaveform(Waveform),
    RandomizePattern,
}

/// Apply an edit to a sequencer and synth. The engine and the UI's mirror
/// both go through this one function, so the mirror changes exactly as the
/// engine does. Transport commands (play, stop, tempo) are not edits: the
/// engine asks the desk, and the UI follows what the engine publishes.
pub fn apply_edit(cmd: AudioCommand, sequencer: &mut Sequencer, synth: &mut AcidSynth) {
    match cmd {
        AudioCommand::Play | AudioCommand::Stop | AudioCommand::SetBpm(_) => {}
        AudioCommand::SetSwing(swing) => sequencer.clock.set_swing(swing),
        AudioCommand::ToggleStep(step) => sequencer.toggle_step(step),
        AudioCommand::ToggleAccent(step) => sequencer.toggle_accent(step),
        AudioCommand::ToggleSlide(step) => sequencer.toggle_slide(step),
        AudioCommand::TransposeStep { step, semitones } => {
            sequencer.transpose_step(step, semitones);
        }
        AudioCommand::SetParam { param, value } => synth.set_param(param, value),
        AudioCommand::SetWaveform(waveform) => synth.set_waveform(waveform),
        AudioCommand::RandomizePattern => sequencer.randomize_acid(),
    }
}

/// State the audio side publishes for the UI.
#[derive(Debug)]
pub struct EngineShared {
    /// Pattern step the sequencer played most recently.
    pub playback_step: AtomicUsize,
    /// Whether the sequencer is running.
    pub playing: AtomicBool,
    /// The sequencer's tempo, as `f64` bits: the desk can change it.
    pub bpm: AtomicU64,
    /// Note the desk is holding on the 303, or [`NO_HUB_NOTE`].
    pub hub_note: AtomicU8,
    /// Messages lost between the 303 and the desk: play/stop and tempo
    /// requests the link could not take, desk transport changes the 303
    /// could not schedule, and desk notes out of MIDI range.
    pub desk_lost: AtomicU64,
    /// Errors cpal reported for the output stream.
    pub stream_errors: AtomicU64,
    /// cpal reported the device gone or the stream invalid: no more audio.
    pub stream_lost: AtomicBool,
}

impl Default for EngineShared {
    fn default() -> Self {
        Self {
            playback_step: AtomicUsize::new(0),
            playing: AtomicBool::new(false),
            bpm: AtomicU64::new(SequencerClock::DEFAULT_BPM.to_bits()),
            hub_note: AtomicU8::new(NO_HUB_NOTE),
            desk_lost: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            stream_lost: AtomicBool::new(false),
        }
    }
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

    /// The tempo the engine is running at.
    #[must_use]
    pub fn bpm(&self) -> f64 {
        f64::from_bits(self.bpm.load(Ordering::Acquire))
    }

    /// The note the desk is holding, if any.
    #[must_use]
    pub fn hub_note(&self) -> Option<u8> {
        let note = self.hub_note.load(Ordering::Acquire);
        (note != NO_HUB_NOTE).then_some(note)
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

/// Queue a command for the audio thread without blocking.
///
/// # Errors
///
/// [`SendFailure::QueueFull`] when the callback is behind,
/// [`SendFailure::EngineGone`] when it has stopped.
pub fn send(tx: &Sender<AudioCommand>, cmd: AudioCommand) -> Result<(), SendFailure> {
    tx.try_send(cmd).map_err(|err| match err {
        TrySendError::Full(_) => SendFailure::QueueFull,
        TrySendError::Disconnected(_) => SendFailure::EngineGone,
    })
}

/// Everything the output callback owns.
#[derive(Debug)]
pub struct AudioEngine {
    synth: AcidSynth,
    sequencer: Sequencer,
    commands: Receiver<AudioCommand>,
    hub: HubLinkAudio,
    /// The desk's transport changes, waiting for their frame.
    follower: TransportFollower,
    /// Note the desk is holding, for legato and its release.
    hub_note: Option<u8>,
    shared: Arc<EngineShared>,
    channels: usize,
    /// Interleaved stereo scratch block for the hub.
    stereo: Vec<f32>,
}

impl AudioEngine {
    /// Build the engine. `channels` is the device's channel count; zero is
    /// treated as one.
    #[must_use]
    pub fn new(
        sample_rate: u32,
        channels: usize,
        commands: Receiver<AudioCommand>,
        hub: HubLinkAudio,
        shared: Arc<EngineShared>,
    ) -> Self {
        // Device rates are whole numbers of Hz, well inside f32's exact range.
        let rate = sample_rate.max(1) as f32;
        let engine = Self {
            synth: AcidSynth::new(rate),
            sequencer: Sequencer::new(rate),
            commands,
            hub,
            follower: TransportFollower::new(sample_rate),
            hub_note: None,
            shared,
            channels: channels.max(1),
            stereo: vec![0.0; MAX_BLOCK * 2],
        };
        engine.publish_bpm();
        engine
    }

    /// Render one device buffer. Real-time safe.
    pub fn render(&mut self, data: &mut [f32]) {
        while let Ok(cmd) = self.commands.try_recv() {
            self.apply(cmd);
        }
        self.drain_hub();
        let chunk_len = MAX_BLOCK * self.channels;
        for chunk in data.chunks_mut(chunk_len) {
            self.render_chunk(chunk);
        }
    }

    fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Play => self.request_playing(true),
            AudioCommand::Stop => self.request_playing(false),
            AudioCommand::SetBpm(bpm) => self.request_bpm(bpm),
            AudioCommand::SetSwing(_)
            | AudioCommand::ToggleStep(_)
            | AudioCommand::ToggleAccent(_)
            | AudioCommand::ToggleSlide(_)
            | AudioCommand::TransposeStep { .. }
            | AudioCommand::SetParam { .. }
            | AudioCommand::SetWaveform(_)
            | AudioCommand::RandomizePattern => {
                apply_edit(cmd, &mut self.sequencer, &mut self.synth);
            }
        }
    }

    /// Play or stop: the whole studio when plugged into the desk (the desk
    /// answers every instrument, this one included, with a transport sync),
    /// or just this 303 when not.
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

    /// Change tempo: the studio's when plugged into the desk, this 303's
    /// when not.
    fn request_bpm(&mut self, bpm: f64) {
        // A NaN stays NaN and is refused below (and ignored locally).
        let bpm = bpm.clamp(SequencerClock::MIN_BPM, SequencerClock::MAX_BPM);
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
        if playing == self.sequencer.is_playing() {
            return;
        }
        if playing {
            self.sequencer.play();
        } else {
            self.sequencer.stop();
            self.synth.release();
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
                HubMessage::NoteEvent(event) => match event.event_type {
                    // MIDI's note-on at velocity 0 is a release.
                    NOTE_ON if event.velocity > 0 => self.hub_note_on(event.note, event.velocity),
                    NOTE_ON | NOTE_OFF => self.hub_note_off(event.note),
                    // Controllers and bends have nothing to drive on the 303.
                    _ => {}
                },
                // The 303 exposes no hub-controllable parameters, and the
                // link itself handles the hub shutting down.
                HubMessage::ParameterChange(_) | HubMessage::Shutdown => {}
            }
        }
    }

    /// Play a note from the desk (kazoo-arp, another keyboard) on the 303's
    /// voice, as a CV/gate input would: loud notes are accented, and a note
    /// pressed while another is held slides into it, the 303's legato.
    fn hub_note_on(&mut self, note: u8, velocity: u8) {
        // MIDI notes are 0..=127: they fit an i8.
        let Ok(pitch) = i8::try_from(note) else {
            self.desk_lost();
            return;
        };
        self.synth
            .note_on(pitch, velocity >= ACCENT_VELOCITY, self.hub_note.is_some());
        self.hub_note = Some(note);
        self.shared.hub_note.store(note, Ordering::Release);
    }

    fn hub_note_off(&mut self, note: u8) {
        if self.hub_note == Some(note) {
            self.synth.release();
            self.hub_note = None;
            self.shared.hub_note.store(NO_HUB_NOTE, Ordering::Release);
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

    /// Render at most [`MAX_BLOCK`] frames into `chunk`.
    fn render_chunk(&mut self, chunk: &mut [f32]) {
        let mut frames = 0;
        let first = self.hub.stream_frame();
        for frame in chunk.chunks_mut(self.channels) {
            if let Some(change) = self.follower.due(first + frames as u64) {
                self.follow(change);
            }
            let tick = self.sequencer.tick();
            if tick.gate_off {
                self.synth.release();
            }
            if let Some(event) = tick.trigger {
                self.shared
                    .playback_step
                    .store(event.step_index, Ordering::Release);
                self.synth.note_on(event.note, event.accent, event.slide);
            }
            let sample = kazoo_core::soft_limit(self.synth.process() * OUTPUT_GAIN);
            frame.fill(sample);
            self.stereo[frames * 2] = sample;
            self.stereo[frames * 2 + 1] = sample;
            frames += 1;
        }

        // `frames` <= MAX_BLOCK, so the cast is lossless.
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
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{
        MSG_NOTE_EVENT, MSG_REGISTER, MSG_REGISTERED, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC,
        NoteEventMsg, RegisteredMsg, TransportRequestMsg, TransportSyncMsg,
    };
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const RATE: u32 = 48_000;

    fn engine(channels: usize) -> (AudioEngine, Sender<AudioCommand>, Arc<EngineShared>) {
        let mut config = LinkConfig::new("kazoo-303-test", 2, RATE, MAX_BLOCK as u32);
        config.address = HubAddress::Socket(
            std::env::temp_dir().join(format!("kazoo-303-no-hub-{}.sock", std::process::id())),
        );
        let (hub, hub_audio) = hub_link(config).unwrap();
        // The UI half says goodbye on drop; the audio half keeps working
        // unconnected, which is what these tests exercise.
        drop(hub);
        let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
        let shared = Arc::new(EngineShared::default());
        let engine = AudioEngine::new(RATE, channels, rx, hub_audio, Arc::clone(&shared));
        (engine, tx, shared)
    }

    /// A desk on a private socket that has registered one 303.
    struct Desk {
        stream: UnixStream,
        buf: FrameBuffer,
        path: PathBuf,
    }

    impl Desk {
        fn plug_in(
            name: &str,
        ) -> (
            Self,
            AudioEngine,
            Sender<AudioCommand>,
            Arc<EngineShared>,
            HubLink,
        ) {
            let path = std::env::temp_dir()
                .join(format!("kazoo-303-desk-{name}-{}.sock", std::process::id()));
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            let listener = UnixListener::bind(&path).unwrap();
            let mut config = LinkConfig::new("kazoo-303-test", 2, RATE, MAX_BLOCK as u32);
            config.address = HubAddress::Socket(path.clone());
            let (hub, hub_audio) = hub_link(config).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = FrameBuffer::new();
            assert_eq!(buf.read_frame(&mut stream).unwrap().msg_type, MSG_REGISTER);
            RegisteredMsg {
                strip_index: 2,
                hub_sample_rate: RATE,
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
            let mut engine = AudioEngine::new(RATE, 2, rx, hub_audio, Arc::clone(&shared));
            // Joining hands the 303 the desk's tempo.
            let deadline = Instant::now() + Duration::from_secs(5);
            while shared.bpm().to_bits() != 100.0_f64.to_bits() {
                assert!(Instant::now() < deadline, "never joined");
                engine.render(&mut [0.0; 64]);
                std::thread::sleep(Duration::from_millis(1));
            }
            (Self { stream, buf, path }, engine, tx, shared, hub)
        }

        /// The next transport request the 303 sent, skipping its audio.
        fn transport_request(&mut self) -> TransportRequestMsg {
            loop {
                let header = self.buf.read_frame(&mut self.stream).unwrap();
                if header.msg_type == MSG_TRANSPORT_REQUEST {
                    return TransportRequestMsg::decode(self.buf.payload());
                }
            }
        }

        fn send_sync(&mut self, sync: TransportSyncMsg) {
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

        fn send_note(&mut self, event_type: u8, note: u8, velocity: u8) {
            NoteEventMsg {
                source: [1; 16],
                target: [0; 16],
                event_type,
                channel: 0,
                note,
                velocity,
            }
            .encode(self.buf.payload_mut());
            self.buf
                .write_frame(MSG_NOTE_EVENT, 2, NoteEventMsg::WIRE_SIZE, &mut self.stream)
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

    /// Render until the desk's sync is waiting, without rendering past it.
    fn wait_for_sync(engine: &mut AudioEngine) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !engine.follower_waiting() {
            assert!(Instant::now() < deadline, "the sync never arrived");
            engine.render(&mut []);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn plugged_in_the_303_asks_the_desk_and_starts_on_its_frame() {
        let (mut desk, mut engine, tx, shared, _hub) = Desk::plug_in("start");

        send(&tx, AudioCommand::Play).unwrap();
        engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.requested_state, TRANSPORT_PLAYING);
        assert_eq!(request.has_bpm, 0);
        // Asking is not playing: the desk decides.
        assert!(!shared.playing.load(Ordering::Acquire));

        send(&tx, AudioCommand::SetBpm(131.0)).unwrap();
        engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.has_bpm, 1);
        assert_eq!(request.requested_bpm.to_bits(), 131.0_f32.to_bits());
        assert!((shared.bpm() - 100.0).abs() < 1e-9, "the desk decides");

        // Play from beat 0, 5 000 frames into the 303's future.
        let start = engine.hub.stream_frame() + 5_000;
        desk.send_sync(TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: 131.0,
            at_frame: start,
            beat: 0.0,
        });
        wait_for_sync(&mut engine);
        while engine.hub.stream_frame() < start {
            assert!(!shared.playing.load(Ordering::Acquire), "started early");
            engine.render(&mut [0.0; 2]);
        }
        // The start frame itself is the downbeat: step 0 plays on it.
        shared.playback_step.store(99, Ordering::Release);
        engine.render(&mut [0.0; 2]);
        assert!(shared.playing.load(Ordering::Acquire));
        assert_eq!(shared.playback_step.load(Ordering::Acquire), 0);
        assert!((shared.bpm() - 131.0).abs() < 1e-9);
        assert_eq!(shared.desk_lost.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn joining_mid_song_lands_on_the_desks_step() {
        let (mut desk, mut engine, _tx, shared, _hub) = Desk::plug_in("join");
        // The desk places this 303 at beat 2.25 (song step 9) 1 000 frames
        // ahead.
        let at = engine.hub.stream_frame() + 1_000;
        desk.send_sync(TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: 120.0,
            at_frame: at,
            beat: 2.25,
        });
        wait_for_sync(&mut engine);
        while engine.hub.stream_frame() < at {
            engine.render(&mut [0.0; 2]);
        }
        shared.playback_step.store(99, Ordering::Release);
        engine.render(&mut [0.0; 2]);
        assert_eq!(shared.playback_step.load(Ordering::Acquire), 9);

        // Then the desk stops it on a frame of its own.
        let stop = engine.hub.stream_frame() + 300;
        desk.send_sync(TransportSyncMsg {
            state: TRANSPORT_STOPPED,
            bpm: 120.0,
            at_frame: stop,
            beat: f64::NAN,
        });
        wait_for_sync(&mut engine);
        while engine.hub.stream_frame() < stop {
            assert!(shared.playing.load(Ordering::Acquire), "stopped early");
            engine.render(&mut [0.0; 2]);
        }
        engine.render(&mut [0.0; 2]);
        assert!(!shared.playing.load(Ordering::Acquire));
    }

    #[test]
    fn desk_notes_play_the_voice_and_release_it() {
        let (mut desk, mut engine, _tx, shared, _hub) = Desk::plug_in("notes");
        desk.send_note(NOTE_ON, 40, 110);
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.hub_note().is_none() {
            assert!(Instant::now() < deadline, "the note never arrived");
            engine.render(&mut [0.0; 64]);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(shared.hub_note(), Some(40));
        // Another note's release leaves it alone; its own releases it.
        desk.send_note(NOTE_OFF, 41, 0);
        desk.send_note(NOTE_ON, 40, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.hub_note().is_some() {
            assert!(Instant::now() < deadline, "the release never arrived");
            engine.render(&mut [0.0; 64]);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_desk_note_is_audible_unplugged_output_off_while_plugged() {
        // Unplugged: local output carries the sound.
        let (mut engine, _tx, _) = engine(2);
        engine.hub_note_on(45, 127);
        let mut data = vec![0.0_f32; 2048];
        engine.render(&mut data);
        assert!(data.iter().any(|s| s.abs() > 0.01), "must be audible");
        for frame in data.chunks(2) {
            assert!((frame[0] - frame[1]).abs() < f32::EPSILON);
        }
        // Plugged in: the desk plays it, so local output is silent.
        let (_desk, mut engine, _tx, _, _hub) = Desk::plug_in("silent");
        engine.hub_note_on(45, 127);
        let mut data = vec![0.0_f32; 2048];
        engine.render(&mut data);
        assert!(data.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn unplugged_transport_is_local_and_published() {
        let (mut engine, tx, shared) = engine(2);
        send(&tx, AudioCommand::SetBpm(90.0)).unwrap();
        send(&tx, AudioCommand::Play).unwrap();
        engine.render(&mut [0.0; 64]);
        assert!((shared.bpm() - 90.0).abs() < 1e-9);
        assert!(shared.playing.load(Ordering::Acquire));
        send(&tx, AudioCommand::Stop).unwrap();
        engine.render(&mut [0.0; 64]);
        assert!(!shared.playing.load(Ordering::Acquire));
        // Out of range and nonsense tempos never reach the clock.
        send(&tx, AudioCommand::SetBpm(f64::NAN)).unwrap();
        send(&tx, AudioCommand::SetBpm(1_000.0)).unwrap();
        engine.render(&mut [0.0; 64]);
        assert!((shared.bpm() - SequencerClock::MAX_BPM).abs() < 1e-9);
    }

    #[test]
    fn nonsense_from_the_desk_is_counted() {
        let (mut desk, mut engine, _tx, shared, _hub) = Desk::plug_in("nonsense");
        desk.send_sync(TransportSyncMsg {
            state: 42,
            bpm: 120.0,
            at_frame: 0,
            beat: 0.0,
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.desk_lost.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < deadline, "never counted");
            engine.render(&mut [0.0; 64]);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!shared.playing.load(Ordering::Acquire));
    }

    #[test]
    fn buffers_longer_than_the_block_size_are_fully_rendered() {
        let (mut engine, tx, _) = engine(1);
        send(&tx, AudioCommand::Play).unwrap();
        let mut data = vec![0.0_f32; MAX_BLOCK * 3 + 100];
        engine.render(&mut data);
        let tail = &data[MAX_BLOCK * 2..];
        assert!(tail.iter().any(|s| s.abs() > 0.01), "tail must be rendered");
        assert_eq!(engine.hub.stream_frame(), (MAX_BLOCK * 3 + 100) as u64);
    }

    #[test]
    fn stream_errors_are_recorded() {
        let shared = EngineShared::default();
        shared.record_stream_error(&cpal::StreamError::BufferUnderrun);
        assert_eq!(shared.stream_errors.load(Ordering::Relaxed), 1);
        assert!(!shared.stream_lost.load(Ordering::Acquire));
        shared.record_stream_error(&cpal::StreamError::DeviceNotAvailable);
        assert!(shared.stream_lost.load(Ordering::Acquire));
    }

    #[test]
    fn full_and_missing_engines_are_told_apart() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        send(&tx, AudioCommand::Play).unwrap();
        assert_eq!(send(&tx, AudioCommand::Stop), Err(SendFailure::QueueFull));
        drop(rx);
        assert_eq!(send(&tx, AudioCommand::Stop), Err(SendFailure::EngineGone));
    }
}
