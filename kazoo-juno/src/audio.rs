//! The audio engine: everything the cpal output callback owns.
//!
//! The callback drains UI commands and hub note events, renders the synth in
//! chunks of at most [`MAX_CALLBACK_FRAMES`], hands every chunk to the hub
//! link (plugged in or not, so the stream position stays true), and silences
//! its own output while the desk is playing it. All state is allocated
//! before the stream starts; rendering never allocates, locks or blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::link::HubLinkAudio;
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON};
use kazoo_juno::{JunoSynth, NUM_VOICES, SynthParams, VoiceStatus};

use crate::app::WAVEFORM_BUF_SIZE;

/// Largest block rendered in one go, in frames. Longer device buffers are
/// rendered in chunks of this size, and it is the block size the hub is
/// told about.
pub const MAX_CALLBACK_FRAMES: usize = 4096;

/// Capacity of the UI -> audio command channel.
pub const COMMAND_CAPACITY: usize = 256;

/// Capacity of the audio -> UI display channel.
pub const DISPLAY_CAPACITY: usize = 2;

/// Push a display snapshot every this many callbacks.
const DISPLAY_INTERVAL: u32 = 3;

/// UI to audio-thread messages. None of them owns heap memory, so the audio
/// thread never frees anything when it drops one.
#[derive(Debug)]
pub enum AudioCommand {
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    UpdateParams(SynthParams),
    AllNotesOff,
}

/// What the UI shows of the engine, sent a few times a second.
#[derive(Debug, Clone, Copy)]
pub struct DisplaySnapshot {
    pub voice_status: [VoiceStatus; NUM_VOICES],
    pub waveform: [f32; WAVEFORM_BUF_SIZE],
}

/// Counters the audio side publishes for the UI, so nothing it fails to do
/// goes unseen.
#[derive(Debug, Default)]
pub struct EngineShared {
    /// Display snapshots the UI was too far behind to take.
    pub display_dropped: AtomicU64,
    /// Errors cpal reported for the output stream.
    pub stream_errors: AtomicU64,
    /// cpal reported the device gone or the stream invalid: no more audio.
    pub stream_lost: AtomicBool,
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

/// Everything the output callback owns.
#[derive(Debug)]
pub struct AudioEngine {
    synth: JunoSynth,
    commands: Receiver<AudioCommand>,
    display: Sender<DisplaySnapshot>,
    hub: HubLinkAudio,
    shared: Arc<EngineShared>,
    /// Interleaved device channels; never zero.
    channels: usize,
    /// Interleaved stereo scratch block for the hub.
    stereo: Vec<f32>,
    waveform: [f32; WAVEFORM_BUF_SIZE],
    waveform_pos: usize,
    display_counter: u32,
}

impl AudioEngine {
    /// Build the engine. A `channels` of zero is treated as mono.
    #[must_use]
    pub fn new(
        sample_rate: f32,
        channels: usize,
        commands: Receiver<AudioCommand>,
        display: Sender<DisplaySnapshot>,
        hub: HubLinkAudio,
        shared: Arc<EngineShared>,
    ) -> Self {
        Self {
            synth: JunoSynth::new(sample_rate),
            commands,
            display,
            hub,
            shared,
            channels: channels.max(1),
            stereo: vec![0.0; MAX_CALLBACK_FRAMES * 2],
            waveform: [0.0; WAVEFORM_BUF_SIZE],
            waveform_pos: 0,
            display_counter: 0,
        }
    }

    /// Render one device buffer. Real-time safe.
    pub fn render(&mut self, data: &mut [f32]) {
        while let Ok(cmd) = self.commands.try_recv() {
            self.apply(cmd);
        }
        while let Some(msg) = self.hub.try_recv() {
            self.apply_hub_message(&msg);
        }
        let chunk_len = MAX_CALLBACK_FRAMES * self.channels;
        for chunk in data.chunks_mut(chunk_len) {
            self.render_chunk(chunk);
        }
        self.publish_display();
    }

    fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::NoteOn { note, velocity } => self.synth.note_on(note, velocity),
            AudioCommand::NoteOff { note } => self.synth.note_off(note),
            AudioCommand::UpdateParams(params) => {
                self.synth.params = params;
                self.synth.apply_params();
            }
            AudioCommand::AllNotesOff => self.synth.all_notes_off(),
        }
    }

    /// Play the notes the desk routes here. A note-on with velocity 0 is a
    /// note-off, as in MIDI.
    fn apply_hub_message(&mut self, msg: &HubMessage) {
        match msg {
            HubMessage::NoteEvent(event) => match event.event_type {
                NOTE_ON if event.velocity == 0 => self.synth.note_off(event.note),
                NOTE_ON => self
                    .synth
                    .note_on(event.note, f32::from(event.velocity.min(127)) / 127.0),
                NOTE_OFF => self.synth.note_off(event.note),
                // Controllers and pitch bend carry nothing this synth plays.
                _ => {}
            },
            // A keyboard synth has no transport and no remote parameters;
            // the link itself handles the desk shutting down.
            HubMessage::TransportSync(_)
            | HubMessage::ParameterChange(_)
            | HubMessage::Shutdown => {}
        }
    }

    /// Render at most [`MAX_CALLBACK_FRAMES`] frames into `chunk`.
    fn render_chunk(&mut self, chunk: &mut [f32]) {
        let mut frames = 0;
        for frame in chunk.chunks_exact_mut(self.channels) {
            let sample = kazoo_core::sanitize_sample(self.synth.process_sample());
            frame.fill(sample);
            self.waveform[self.waveform_pos] = sample;
            self.waveform_pos = (self.waveform_pos + 1) % WAVEFORM_BUF_SIZE;
            self.stereo[frames * 2] = sample;
            self.stereo[frames * 2 + 1] = sample;
            frames += 1;
        }
        // A trailing partial frame (never expected) is silenced.
        let whole = frames * self.channels;
        chunk[whole..].fill(0.0);

        // `frames` <= MAX_CALLBACK_FRAMES, so the cast is lossless.
        if self
            .hub
            .send_audio(frames as u32, &self.stereo[..frames * 2])
        {
            // The desk is playing this instrument: don't play it twice.
            chunk.fill(0.0);
        }
    }

    fn publish_display(&mut self) {
        self.display_counter = self.display_counter.wrapping_add(1);
        if self.display_counter % DISPLAY_INTERVAL != 0 {
            return;
        }
        let snapshot = DisplaySnapshot {
            voice_status: self.synth.voice_status(),
            waveform: self.waveform,
        };
        // Full: the UI is behind. Disconnected: the UI has gone. Either way
        // the snapshot is counted, never waited on.
        if self.display.try_send(snapshot).is_err() {
            self.shared.display_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_core::ipc::link::{HubAddress, HubLink, LinkConfig, hub_link};
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{
        MSG_NOTE_EVENT, MSG_REGISTER, MSG_REGISTERED, NOTE_CC, NoteEventMsg, RegisteredMsg,
        TRANSPORT_STOPPED,
    };
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const NAME: &str = "kazoo-juno";

    struct Rig {
        engine: AudioEngine,
        tx: Sender<AudioCommand>,
        display: Receiver<DisplaySnapshot>,
        shared: Arc<EngineShared>,
    }

    fn build(channels: usize, hub_audio: HubLinkAudio) -> Rig {
        let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
        let (display_tx, display) = crossbeam_channel::bounded(DISPLAY_CAPACITY);
        let shared = Arc::new(EngineShared::default());
        let engine = AudioEngine::new(
            48_000.0,
            channels,
            rx,
            display_tx,
            hub_audio,
            Arc::clone(&shared),
        );
        Rig {
            engine,
            tx,
            display,
            shared,
        }
    }

    /// An engine whose link looks for a desk on a private socket nobody
    /// listens on.
    fn unplugged(channels: usize, test: &str) -> Rig {
        let mut config = LinkConfig::new(NAME, 2, 48_000, MAX_CALLBACK_FRAMES as u32);
        config.address = HubAddress::Socket(
            std::env::temp_dir().join(format!("{NAME}-no-hub-{test}-{}.sock", std::process::id())),
        );
        let (hub, hub_audio) = hub_link(config).unwrap();
        // The UI half says goodbye on drop; the audio half keeps working
        // unconnected, which is what these tests exercise.
        drop(hub);
        build(channels, hub_audio)
    }

    fn note(event_type: u8, note: u8, velocity: u8) -> HubMessage {
        HubMessage::NoteEvent(NoteEventMsg {
            source: [0; 16],
            target: [0; 16],
            event_type,
            channel: 0,
            note,
            velocity,
        })
    }

    fn energy(data: &[f32]) -> f32 {
        data.iter().map(|s| s.abs()).sum()
    }

    fn voice_on(engine: &AudioEngine, midi: u8) -> bool {
        engine
            .synth
            .voice_status()
            .iter()
            .any(|v| v.active && !v.releasing && v.note == Some(midi))
    }

    #[test]
    fn hub_notes_play_and_release_the_synth() {
        let mut rig = unplugged(2, "notes");
        rig.engine.apply_hub_message(&note(NOTE_ON, 60, 127));
        assert!(voice_on(&rig.engine, 60));
        let mut data = vec![0.0_f32; 4096];
        rig.engine.render(&mut data);
        assert!(energy(&data) > 1.0, "a hub note must be audible");

        rig.engine.apply_hub_message(&note(NOTE_OFF, 60, 0));
        assert!(!voice_on(&rig.engine, 60));

        // Velocity 0 is a release; controllers play nothing.
        rig.engine.apply_hub_message(&note(NOTE_ON, 62, 90));
        assert!(voice_on(&rig.engine, 62));
        rig.engine.apply_hub_message(&note(NOTE_ON, 62, 0));
        assert!(!voice_on(&rig.engine, 62));
        rig.engine.apply_hub_message(&note(NOTE_CC, 64, 127));
        assert!(!voice_on(&rig.engine, 64));
    }

    #[test]
    fn unplugged_the_stream_position_still_counts_every_frame() {
        let mut rig = unplugged(2, "position");
        assert!(
            rig.tx
                .try_send(AudioCommand::NoteOn {
                    note: 57,
                    velocity: 1.0,
                })
                .is_ok()
        );
        let frames = MAX_CALLBACK_FRAMES * 2 + 100;
        let mut data = vec![0.0_f32; frames * 2];
        rig.engine.render(&mut data);
        assert_eq!(rig.engine.hub.stream_frame(), frames as u64);
        // Not plugged in: the synth plays locally, all the way to the end.
        let tail = &data[MAX_CALLBACK_FRAMES * 4..];
        assert!(energy(tail) > 0.1, "the tail must be rendered");
        for frame in data.chunks(2) {
            assert!((frame[0] - frame[1]).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn every_device_channel_is_written() {
        let mut rig = unplugged(3, "channels");
        rig.engine.apply_hub_message(&note(NOTE_ON, 69, 127));
        let mut data = vec![f32::NAN; 3 * 1000 + 1];
        rig.engine.render(&mut data);
        assert!(
            data.iter().all(|s| s.is_finite()),
            "no sample left unwritten"
        );
        assert!(data[3000].abs() < f32::EPSILON, "a partial frame is silent");
        assert_eq!(rig.engine.hub.stream_frame(), 1000);
    }

    #[test]
    fn display_snapshots_are_sent_and_overflow_is_counted() {
        let mut rig = unplugged(2, "display");
        rig.engine.apply_hub_message(&note(NOTE_ON, 60, 100));
        for _ in 0..DISPLAY_INTERVAL * 4 {
            rig.engine.render(&mut [0.0; 128]);
        }
        let snapshot = rig.display.try_recv().unwrap();
        assert!(
            snapshot
                .voice_status
                .iter()
                .any(|v| v.active && v.note == Some(60)),
            "hub notes show in the voice display"
        );
        assert!(rig.shared.display_dropped.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn stream_errors_are_recorded() {
        let shared = EngineShared::default();
        shared.record_stream_error(&cpal::StreamError::BufferUnderrun);
        assert!(!shared.stream_lost.load(Ordering::Acquire));
        shared.record_stream_error(&cpal::StreamError::DeviceNotAvailable);
        assert_eq!(shared.stream_errors.load(Ordering::Relaxed), 2);
        assert!(shared.stream_lost.load(Ordering::Acquire));
    }

    /// A desk on a private socket that has registered one instrument.
    struct Desk {
        stream: UnixStream,
        buf: FrameBuffer,
        path: PathBuf,
    }

    impl Drop for Desk {
        fn drop(&mut self) {
            if let Err(err) = std::fs::remove_file(&self.path) {
                assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
            }
        }
    }

    fn plug_in() -> (Desk, Rig, HubLink) {
        let path = std::env::temp_dir().join(format!("{NAME}-desk-{}.sock", std::process::id()));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let listener = UnixListener::bind(&path).unwrap();
        let mut config = LinkConfig::new(NAME, 2, 48_000, MAX_CALLBACK_FRAMES as u32);
        config.address = HubAddress::Socket(path.clone());
        let (hub, hub_audio) = hub_link(config).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = FrameBuffer::new();
        assert_eq!(buf.read_frame(&mut stream).unwrap().msg_type, MSG_REGISTER);
        RegisteredMsg {
            strip_index: 2,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: TRANSPORT_STOPPED,
            bpm: 120.0,
            position: 0,
        }
        .encode(buf.payload_mut());
        buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !hub.is_connected() {
            assert!(Instant::now() < deadline, "never plugged in");
            std::thread::sleep(Duration::from_millis(1));
        }
        (Desk { stream, buf, path }, build(2, hub_audio), hub)
    }

    #[test]
    fn plugged_in_a_desk_note_sounds_on_the_desk_not_locally() {
        let (mut desk, mut rig, hub) = plug_in();
        assert_eq!(hub.status().strip, Some(2));

        NoteEventMsg {
            source: [0; 16],
            target: [0; 16],
            event_type: NOTE_ON,
            channel: 0,
            note: 60,
            velocity: 127,
        }
        .encode(desk.buf.payload_mut());
        desk.buf
            .write_frame(MSG_NOTE_EVENT, 1, NoteEventMsg::WIRE_SIZE, &mut desk.stream)
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !voice_on(&rig.engine, 60) {
            assert!(Instant::now() < deadline, "the desk's note never arrived");
            rig.engine.render(&mut [0.0; 64]);
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut data = vec![1.0_f32; 2048];
        rig.engine.render(&mut data);
        assert!(
            data.iter().all(|s| s.abs() < f32::EPSILON),
            "the desk plays it: local output is silent"
        );
        assert!(
            energy(&rig.engine.stereo[..2048]) > 1.0,
            "the note went to the desk"
        );
        drop(hub);
    }
}
