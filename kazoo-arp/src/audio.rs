//! The standalone arpeggiator's audio engine: everything the cpal output
//! callback owns, plus the command and display plumbing around it.
//!
//! The callback drains UI commands and hub messages, runs the arpeggiator
//! clock, plays each note on a sine audition voice and hands every rendered
//! block to the hub link. Plugged into the kazoo-mix desk, the clock follows
//! the desk's transport to the frame: play, stop and tempo changes land on
//! the frame the desk scheduled them for, at the song position it gives, and
//! play/stop and tempo from the arp's own keys go to the desk. Unplugged,
//! they act on the arp alone. All state is allocated before the stream
//! starts; rendering never allocates, locks or blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use kazoo_arp::{ArpClock, ArpCursor, ArpMode, Arpeggiator, ClockDivision, NoteEvent};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportChange, TransportFollower};
use kazoo_core::ipc::link::{HubLinkAudio, RequestError};
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON, TRANSPORT_PLAYING, TRANSPORT_STOPPED};
use ringbuf::HeapProd;
use ringbuf::traits::Producer;

/// Largest block rendered in one go, in frames. Longer device buffers are
/// rendered in chunks of this size, and it is the block size the hub is
/// told about.
pub const MAX_CALLBACK_FRAMES: usize = 4096;

/// Capacity of the UI -> audio command channel.
pub const COMMAND_CAPACITY: usize = 256;

/// Capacity of the audio -> UI display ring.
pub const DISPLAY_CAPACITY: usize = 256;

/// Tempo the arpeggiator starts at.
pub const DEFAULT_BPM: f64 = 120.0;

/// A change to the arpeggiator itself: its notes and pattern settings.
/// Applied the same way to the engine and to the UI's display copy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArpCommand {
    NoteOn { midi_note: u8, velocity: u8 },
    NoteOff { midi_note: u8 },
    SetSwing(f64),
    SetDivision(ClockDivision),
    SetMode(ArpMode),
    SetGate(f32),
    SetOctaveRange(u8),
    SetLatch(bool),
}

/// Commands from the UI thread to the audio thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCommand {
    /// Change the arpeggiator.
    Arp(ArpCommand),
    /// Start playing: the studio when plugged into the desk, the arp alone
    /// when not.
    Play,
    /// Stop: the studio when plugged in, the arp alone when not.
    Stop,
    /// Change tempo: the studio's when plugged in, the arp's when not.
    SetBpm(f64),
}

/// Apply an arpeggiator change to an arpeggiator and its clock.
///
/// The audio engine and the UI's display copy both go through this one
/// function, so the copy changes exactly as the engine does. Transport
/// commands are not here: the desk may decide them, so the engine resolves
/// them and publishes the outcome in [`EngineShared`].
pub fn apply_command(cmd: ArpCommand, arp: &mut Arpeggiator, clock: &mut ArpClock) {
    match cmd {
        ArpCommand::NoteOn {
            midi_note,
            velocity,
        } => arp.note_on(midi_note, velocity),
        ArpCommand::NoteOff { midi_note } => arp.note_off(midi_note),
        ArpCommand::SetSwing(swing) => clock.set_swing(swing),
        ArpCommand::SetDivision(div) => clock.set_division(div),
        ArpCommand::SetMode(mode) => arp.set_mode(mode),
        ArpCommand::SetGate(gate) => arp.set_gate_pct(gate),
        ArpCommand::SetOctaveRange(range) => arp.set_octave_range(range),
        ArpCommand::SetLatch(enabled) => {
            if arp.latch != enabled {
                arp.toggle_latch();
            }
        }
    }
}

/// What the audio thread tells the UI, in the order it happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayEvent {
    /// The arpeggiator played a note; `cursor` is where it now stands.
    Played { midi_note: u8, cursor: ArpCursor },
    /// The hub pressed a note (the UI's copy of the pool must follow).
    HubNoteOn { midi_note: u8, velocity: u8 },
    /// The hub released a note.
    HubNoteOff { midi_note: u8 },
}

/// State the audio side publishes for the UI.
#[derive(Debug, Default)]
pub struct EngineShared {
    /// Whether the arp's clock is running.
    pub playing: AtomicBool,
    /// The clock's tempo, as `f64` bits: the desk can change it.
    pub bpm: AtomicU64,
    /// Song position at the end of the last rendered block, in beats, as
    /// `f64` bits.
    pub beat: AtomicU64,
    /// Transport messages lost between the arp and the desk: play/stop and
    /// tempo requests the link could not take, and desk changes the arp
    /// could not schedule.
    pub desk_lost: AtomicU64,
    /// Display events lost because the UI had not drained the ring.
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

    /// Queue a command without counting a failure: for retrying a command
    /// whose first failure was already counted.
    pub fn deliver(&self, cmd: AudioCommand) -> Result<(), SendFailure> {
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

/// Simple sine audition voice for hearing arpeggiated notes.
#[derive(Debug)]
struct AuditionVoice {
    sample_rate: f32,
    phase: f32,
    frequency: f32,
    /// Per-sample gain envelope for click-free note on/off.
    gain: f32,
    target_gain: f32,
    /// Smoothing coefficient for envelope.
    smooth: f32,
}

impl AuditionVoice {
    fn new(sample_rate: f32) -> Self {
        // ~5ms attack/release for click-free transitions.
        let smooth = 1.0 - (-1.0 / (sample_rate * 0.005)).exp();
        Self {
            sample_rate,
            phase: 0.0,
            frequency: 0.0,
            gain: 0.0,
            target_gain: 0.0,
            smooth,
        }
    }

    fn note_on(&mut self, midi_note: u8, velocity: u8) {
        self.frequency = 440.0 * ((f32::from(midi_note) - 69.0) / 12.0).exp2();
        self.target_gain = f32::from(velocity) / 127.0;
    }

    const fn note_off(&mut self) {
        self.target_gain = 0.0;
    }

    fn tick(&mut self) -> f32 {
        self.gain = self.smooth.mul_add(self.target_gain - self.gain, self.gain);

        if self.gain < 0.0001 {
            self.gain = 0.0;
            return 0.0;
        }

        let sample = (self.phase * std::f32::consts::TAU).sin();
        self.phase += self.frequency / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        kazoo_core::sanitize_sample(sample * self.gain * 0.3)
    }
}

/// Everything the output callback owns.
pub struct AudioEngine {
    arp: Arpeggiator,
    clock: ArpClock,
    voice: AuditionVoice,
    commands: Receiver<AudioCommand>,
    hub: HubLinkAudio,
    /// The desk's transport changes, waiting for their frame.
    follower: TransportFollower,
    display: HeapProd<DisplayEvent>,
    shared: Arc<EngineShared>,
    channels: usize,
    /// Interleaved stereo scratch block for the hub.
    stereo: Vec<f32>,
}

// HeapProd is !Debug; implement manually.
impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("arp", &self.arp)
            .field("clock", &self.clock)
            .field("voice", &self.voice)
            .field("hub", &self.hub)
            .field("follower", &self.follower)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl AudioEngine {
    /// Build the engine, playing from song beat 0 until the desk says
    /// otherwise. `sample_rate` is the device's rate in whole Hz;
    /// `channels` is its channel count and must be at least 1.
    #[must_use]
    pub fn new(
        sample_rate: u32,
        channels: usize,
        commands: Receiver<AudioCommand>,
        hub: HubLinkAudio,
        display: HeapProd<DisplayEvent>,
        shared: Arc<EngineShared>,
    ) -> Self {
        let mut clock = ArpClock::new(f64::from(sample_rate), DEFAULT_BPM);
        clock.start();
        let engine = Self {
            arp: Arpeggiator::new(),
            clock,
            voice: AuditionVoice::new(sample_rate as f32),
            commands,
            hub,
            follower: TransportFollower::new(sample_rate),
            display,
            shared,
            channels: channels.max(1),
            stereo: vec![0.0; MAX_CALLBACK_FRAMES * 2],
        };
        engine.publish_transport();
        engine
    }

    /// Render one device buffer. Real-time safe.
    pub fn render(&mut self, data: &mut [f32]) {
        while let Ok(cmd) = self.commands.try_recv() {
            self.apply(cmd);
        }
        self.drain_hub();
        let chunk_len = MAX_CALLBACK_FRAMES * self.channels;
        for chunk in data.chunks_mut(chunk_len) {
            self.render_chunk(chunk);
        }
        self.shared
            .beat
            .store(self.clock.position().to_bits(), Ordering::Release);
    }

    fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Arp(cmd) => apply_command(cmd, &mut self.arp, &mut self.clock),
            AudioCommand::Play => self.request_playing(true),
            AudioCommand::Stop => self.request_playing(false),
            AudioCommand::SetBpm(bpm) => self.request_bpm(bpm),
        }
    }

    /// Play or stop: the whole studio when plugged into the desk (the desk
    /// answers every instrument, this one included, with a transport sync
    /// scheduled on its frame), or just this arp when not.
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

    /// Change tempo: the studio's when plugged into the desk, this arp's
    /// when not.
    fn request_bpm(&mut self, bpm: f64) {
        // The desk's range and the clock's are the same; a NaN stays NaN
        // and is refused by the link, or ignored by the clock.
        let bpm = bpm.clamp(kazoo_arp::MIN_BPM, kazoo_arp::MAX_BPM);
        // Only the tempo: the desk keeps its own play state, which this
        // instrument may not have heard yet.
        // Tempos are tens to hundreds of BPM: f32 holds them closely enough.
        match self.hub.request_tempo(bpm as f32) {
            Ok(()) => {}
            Err(RequestError::NotConnected) => {
                self.clock.set_bpm(bpm);
                self.publish_transport();
            }
            Err(RequestError::Full | RequestError::Invalid) => self.desk_lost(),
        }
    }

    fn desk_lost(&self) {
        self.shared.desk_lost.fetch_add(1, Ordering::Relaxed);
    }

    /// Start from song beat 0, or stop; alone, without the desk.
    fn set_playing(&mut self, playing: bool) {
        if playing == self.clock.running() {
            return;
        }
        if playing {
            self.clock.start();
        } else {
            self.clock.stop();
        }
        self.publish_transport();
    }

    fn publish_transport(&self) {
        self.shared
            .bpm
            .store(self.clock.bpm().to_bits(), Ordering::Release);
        self.shared
            .playing
            .store(self.clock.running(), Ordering::Release);
    }

    /// Tell the UI something happened; a full ring is counted, never waited
    /// on.
    fn publish(&mut self, event: DisplayEvent) {
        if self.display.try_push(event).is_err() {
            self.shared.display_dropped.fetch_add(1, Ordering::Relaxed);
        }
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
                    // A note-on with velocity 0 is a release, as in MIDI.
                    NOTE_ON if event.velocity > 0 => {
                        self.arp.note_on(event.note, event.velocity);
                        self.publish(DisplayEvent::HubNoteOn {
                            midi_note: event.note,
                            velocity: event.velocity,
                        });
                    }
                    NOTE_ON | NOTE_OFF => {
                        self.arp.note_off(event.note);
                        self.publish(DisplayEvent::HubNoteOff {
                            midi_note: event.note,
                        });
                    }
                    // Controllers and pitch bend do not change which notes
                    // the arp plays.
                    _ => {}
                },
                // The arp exposes no hub-controllable parameters, and the
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
        self.clock.set_bpm(change.bpm);
        match change.beat {
            Some(beat) => self.clock.play_from(beat),
            None => self.clock.stop(),
        }
        self.publish_transport();
    }

    /// Render at most [`MAX_CALLBACK_FRAMES`] frames into `chunk`.
    fn render_chunk(&mut self, chunk: &mut [f32]) {
        let mut frames = 0;
        let first = self.hub.stream_frame();
        for frame in chunk.chunks_mut(self.channels) {
            if let Some(change) = self.follower.due(first + frames as u64) {
                self.follow(change);
            }
            let events = self.clock.tick(&mut self.arp);
            if let Some(NoteEvent::NoteOff { .. }) = events.note_off {
                self.voice.note_off();
            }
            if let Some(NoteEvent::NoteOn {
                midi_note,
                velocity,
            }) = events.note_on
            {
                self.voice.note_on(midi_note, velocity);
                let cursor = self.arp.cursor();
                self.publish(DisplayEvent::Played { midi_note, cursor });
            }

            let sample = kazoo_core::soft_limit(self.voice.tick());
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
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{
        MSG_NOTE_EVENT, MSG_REGISTER, MSG_REGISTERED, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC,
        NoteEventMsg, RegisteredMsg, TransportRequestMsg, TransportSyncMsg,
    };
    use ringbuf::HeapCons;
    use ringbuf::traits::{Consumer, Split};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const RATE: u32 = 48_000;

    struct Rig {
        engine: AudioEngine,
        tx: CommandSender,
        display: HeapCons<DisplayEvent>,
        shared: Arc<EngineShared>,
    }

    fn engine_with(hub_audio: HubLinkAudio, display_capacity: usize, channels: usize) -> Rig {
        let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
        let (prod, cons) = ringbuf::HeapRb::new(display_capacity).split();
        let shared = Arc::new(EngineShared::default());
        let engine = AudioEngine::new(RATE, channels, rx, hub_audio, prod, Arc::clone(&shared));
        Rig {
            engine,
            tx: CommandSender::new(tx),
            display: cons,
            shared,
        }
    }

    fn rig(channels: usize, display_capacity: usize) -> Rig {
        let mut config = LinkConfig::new("kazoo-arp-test", 2, RATE, MAX_CALLBACK_FRAMES as u32);
        config.address = HubAddress::Socket(
            std::env::temp_dir().join(format!("kazoo-arp-no-hub-{}.sock", std::process::id())),
        );
        let (hub, hub_audio) = hub_link(config).unwrap();
        // The UI half says goodbye on drop; the audio half keeps working
        // unconnected, which is what these tests exercise.
        drop(hub);
        engine_with(hub_audio, display_capacity, channels)
    }

    fn note_on(midi_note: u8) -> AudioCommand {
        AudioCommand::Arp(ArpCommand::NoteOn {
            midi_note,
            velocity: 127,
        })
    }

    fn bpm(shared: &EngineShared) -> f64 {
        f64::from_bits(shared.bpm.load(Ordering::Acquire))
    }

    /// Stream frames on which the arp started a note over the next
    /// `frames` frames, rendered one frame at a time.
    fn played_frames(rig: &mut Rig, frames: usize) -> Vec<u64> {
        let mut played = Vec::new();
        let mut frame = vec![0.0_f32; rig.engine.channels];
        for _ in 0..frames {
            let at = rig.engine.hub.stream_frame();
            rig.engine.render(&mut frame);
            while let Some(event) = rig.display.try_pop() {
                if let DisplayEvent::Played { .. } = event {
                    played.push(at);
                }
            }
        }
        played
    }

    #[test]
    fn held_note_is_audible_and_reported() {
        let mut rig = rig(2, DISPLAY_CAPACITY);
        assert!(rig.tx.send(note_on(69)));
        let mut data = vec![0.0_f32; 4096];
        rig.engine.render(&mut data);
        assert!(data.iter().any(|s| s.abs() > 0.01), "note must be audible");
        match rig.display.try_pop() {
            Some(DisplayEvent::Played { midi_note, cursor }) => {
                assert_eq!(midi_note, 69);
                assert_eq!(cursor, rig.engine.arp.cursor());
            }
            other => panic!("expected a played note, got {other:?}"),
        }
    }

    #[test]
    fn buffers_longer_than_the_block_size_are_fully_rendered() {
        let mut rig = rig(1, DISPLAY_CAPACITY);
        assert!(rig.tx.send(note_on(69)));
        assert!(rig.tx.send(AudioCommand::Arp(ArpCommand::SetGate(1.0))));
        let mut data = vec![0.0_f32; MAX_CALLBACK_FRAMES * 3];
        rig.engine.render(&mut data);
        let tail = &data[MAX_CALLBACK_FRAMES * 2..];
        assert!(tail.iter().any(|s| s.abs() > 0.01), "tail must be rendered");
        // Every frame counts on the stream, plugged in or not.
        assert_eq!(
            rig.engine.hub.stream_frame(),
            MAX_CALLBACK_FRAMES as u64 * 3
        );
    }

    #[test]
    fn full_display_ring_is_counted() {
        let mut rig = rig(1, 1);
        assert!(rig.tx.send(note_on(60)));
        assert!(rig.tx.send(AudioCommand::Arp(ArpCommand::SetDivision(
            ClockDivision::ThirtySecond
        ))));
        // Several steps fire; only one fits in the ring.
        let mut data = vec![0.0_f32; 48_000];
        rig.engine.render(&mut data);
        let dropped = rig.shared.display_dropped.load(Ordering::Relaxed);
        assert!(dropped > 0, "overflow must be counted");
        assert!(rig.display.try_pop().is_some());
    }

    #[test]
    fn unplugged_play_stop_and_tempo_are_local_and_published() {
        let mut rig = rig(2, DISPLAY_CAPACITY);
        assert!(
            rig.shared.playing.load(Ordering::Acquire),
            "plays from the start"
        );
        assert!((bpm(&rig.shared) - DEFAULT_BPM).abs() < 1e-9);

        assert!(rig.tx.send(AudioCommand::Stop));
        rig.engine.render(&mut [0.0; 64]);
        assert!(!rig.shared.playing.load(Ordering::Acquire));

        assert!(rig.tx.send(AudioCommand::SetBpm(90.0)));
        rig.engine.render(&mut [0.0; 64]);
        assert!((bpm(&rig.shared) - 90.0).abs() < 1e-9);
        assert!(rig.tx.send(AudioCommand::SetBpm(1_000.0)));
        rig.engine.render(&mut [0.0; 64]);
        assert!((bpm(&rig.shared) - kazoo_arp::MAX_BPM).abs() < 1e-9);

        // Playing again starts from the top: the downbeat is the next frame.
        assert!(rig.tx.send(note_on(60)));
        assert!(rig.tx.send(AudioCommand::Play));
        let start = rig.engine.hub.stream_frame();
        assert_eq!(played_frames(&mut rig, 1), [start]);
        assert!(rig.shared.playing.load(Ordering::Acquire));
        assert_eq!(rig.shared.desk_lost.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stopping_silences_the_held_note() {
        let mut rig = rig(1, DISPLAY_CAPACITY);
        assert!(rig.tx.send(note_on(69)));
        assert!(rig.tx.send(AudioCommand::Arp(ArpCommand::SetGate(1.0))));
        rig.engine.render(&mut [0.0; 512]);
        assert!(rig.tx.send(AudioCommand::Stop));
        // The release fades within a few milliseconds.
        rig.engine.render(&mut vec![0.0; 4_800]);
        let mut tail = [0.0_f32; 256];
        rig.engine.render(&mut tail);
        assert!(tail.iter().all(|s| s.abs() < 1e-3), "{:?}", &tail[..8]);
    }

    #[test]
    fn apply_command_matches_engine_semantics() {
        let mut arp = Arpeggiator::new();
        let mut clock = ArpClock::new(48_000.0, DEFAULT_BPM);
        apply_command(ArpCommand::SetLatch(true), &mut arp, &mut clock);
        assert!(arp.latch);
        apply_command(ArpCommand::SetLatch(true), &mut arp, &mut clock);
        assert!(arp.latch, "setting latch twice must not toggle it off");
        apply_command(
            ArpCommand::NoteOn {
                midi_note: 60,
                velocity: 100,
            },
            &mut arp,
            &mut clock,
        );
        apply_command(ArpCommand::NoteOff { midi_note: 60 }, &mut arp, &mut clock);
        assert!(arp.has_notes(), "latched note survives release");
        apply_command(ArpCommand::SetLatch(false), &mut arp, &mut clock);
        assert!(!arp.has_notes(), "unlatching clears the pool");
        apply_command(ArpCommand::SetSwing(0.6), &mut arp, &mut clock);
        assert!((clock.swing() - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn command_failures_are_counted() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut sender = CommandSender::new(tx);
        let latch = |on| AudioCommand::Arp(ArpCommand::SetLatch(on));
        assert!(sender.send(latch(true)));
        assert!(!sender.send(latch(false)));
        assert_eq!(sender.last_failure(), Some(SendFailure::QueueFull));
        assert_eq!(sender.failed(), 1);
        // A quiet retry is not counted again.
        assert_eq!(sender.deliver(latch(false)), Err(SendFailure::QueueFull));
        assert_eq!(sender.failed(), 1);
        drop(rx);
        assert!(!sender.send(latch(false)));
        assert_eq!(sender.failed(), 2);
        assert_eq!(sender.last_failure(), Some(SendFailure::EngineGone));
    }

    #[test]
    fn stream_errors_are_recorded() {
        let shared = EngineShared::default();
        shared.record_stream_error(&cpal::StreamError::BufferUnderrun);
        assert!(!shared.stream_lost.load(Ordering::Acquire));
        shared.record_stream_error(&cpal::StreamError::StreamInvalidated);
        assert_eq!(shared.stream_errors.load(Ordering::Relaxed), 2);
        assert!(shared.stream_lost.load(Ordering::Acquire));
    }

    /// A desk on a private socket that has registered one arp.
    struct Desk {
        stream: UnixStream,
        buf: FrameBuffer,
        path: PathBuf,
    }

    impl Desk {
        /// Plug a fresh arp into a desk that is stopped at 100 BPM.
        fn plug_in(name: &str) -> (Self, Rig, HubLink) {
            let path = std::env::temp_dir()
                .join(format!("kazoo-arp-desk-{name}-{}.sock", std::process::id()));
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            let listener = UnixListener::bind(&path).unwrap();
            let mut config = LinkConfig::new("kazoo-arp-test", 2, RATE, MAX_CALLBACK_FRAMES as u32);
            config.address = HubAddress::Socket(path.clone());
            let (hub, hub_audio) = hub_link(config).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = FrameBuffer::new();
            assert_eq!(buf.read_frame(&mut stream).unwrap().msg_type, MSG_REGISTER);
            RegisteredMsg {
                strip_index: 3,
                hub_sample_rate: RATE,
                hub_buffer_size: 256,
                transport_state: TRANSPORT_STOPPED,
                bpm: 100.0,
                position: 0,
            }
            .encode(buf.payload_mut());
            buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
                .unwrap();
            let mut rig = engine_with(hub_audio, DISPLAY_CAPACITY, 2);
            let desk = Self { stream, buf, path };
            // Joining hands the arp the desk's transport: stopped, 100 BPM.
            let deadline = Instant::now() + Duration::from_secs(5);
            while rig.shared.playing.load(Ordering::Acquire)
                || (bpm(&rig.shared) - 100.0).abs() > 1e-9
            {
                assert!(Instant::now() < deadline, "never joined");
                rig.engine.render(&mut [0.0; 64]);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(hub.status().strip, Some(3));
            (desk, rig, hub)
        }

        fn sync(&mut self, state: u8, bpm: f32, at_frame: u64, beat: f64) {
            TransportSyncMsg {
                state,
                bpm,
                at_frame,
                beat,
            }
            .encode(self.buf.payload_mut());
            self.buf
                .write_frame(
                    MSG_TRANSPORT_SYNC,
                    1,
                    TransportSyncMsg::WIRE_SIZE,
                    &mut self.stream,
                )
                .unwrap();
        }

        /// The next transport request the arp sent, skipping its audio.
        fn transport_request(&mut self) -> TransportRequestMsg {
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

    /// Render in small blocks until the engine holds a scheduled desk
    /// change, without rendering past `limit`.
    fn wait_for_schedule(rig: &mut Rig, limit: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !rig.engine.follower_waiting() {
            assert!(Instant::now() < deadline, "the sync never arrived");
            assert!(
                rig.engine.hub.stream_frame() < limit,
                "rendered past the change"
            );
            rig.engine.render(&mut []);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn plugged_in_the_arp_asks_the_desk_and_follows_its_answer() {
        let (mut desk, mut rig, _hub) = Desk::plug_in("follow");
        assert!(rig.tx.send(note_on(60)));
        assert!(rig.tx.send(note_on(64)));

        assert!(rig.tx.send(AudioCommand::Play));
        rig.engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.requested_state, TRANSPORT_PLAYING);
        assert_eq!(request.has_bpm, 0);
        // Asking is not playing: the desk decides.
        assert!(!rig.shared.playing.load(Ordering::Acquire));

        assert!(rig.tx.send(AudioCommand::SetBpm(131.0)));
        rig.engine.render(&mut [0.0; 64]);
        let request = desk.transport_request();
        assert_eq!(request.has_bpm, 1);
        assert_eq!(request.requested_bpm.to_bits(), 131.0_f32.to_bits());
        assert!((bpm(&rig.shared) - 100.0).abs() < 1e-9, "the desk decides");

        // Play from beat 0, 5 000 frames into the arp's future, at 120 BPM:
        // sixteenths every 6 000 frames.
        let start = rig.engine.hub.stream_frame() + 5_000;
        desk.sync(TRANSPORT_PLAYING, 120.0, start, 0.0);
        wait_for_schedule(&mut rig, start);
        let frames = (start + 12_001 - rig.engine.hub.stream_frame()) as usize;
        let played = played_frames(&mut rig, frames);
        assert_eq!(played, [start, start + 6_000, start + 12_000]);
        assert!(rig.shared.playing.load(Ordering::Acquire));
        assert!((bpm(&rig.shared) - 120.0).abs() < 1e-9);

        // A stop lands on its frame too, and releases the note.
        let stop = rig.engine.hub.stream_frame() + 1_000;
        desk.sync(TRANSPORT_STOPPED, 120.0, stop, f64::NAN);
        wait_for_schedule(&mut rig, stop);
        while rig.engine.hub.stream_frame() < stop {
            assert!(rig.shared.playing.load(Ordering::Acquire), "stopped early");
            rig.engine.render(&mut [0.0; 2]);
        }
        rig.engine.render(&mut [0.0; 2]);
        assert!(!rig.shared.playing.load(Ordering::Acquire));
        assert_eq!(rig.engine.arp.current_sounding_note(), None);
        assert_eq!(rig.shared.desk_lost.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn joining_mid_song_lands_on_the_desks_grid() {
        let (mut desk, mut rig, _hub) = Desk::plug_in("mid-song");
        assert!(rig.tx.send(note_on(60)));
        rig.engine.render(&mut []);
        // The desk has been playing at 120 BPM: at frame `at` the song is
        // at beat 10.1, so the next sixteenth (beat 10.25) is 0.15 beats,
        // 3 600 frames, later, then every 6 000.
        let at = rig.engine.hub.stream_frame() + 2_000;
        desk.sync(TRANSPORT_PLAYING, 120.0, at, 10.1);
        wait_for_schedule(&mut rig, at);
        let frames = (at + 9_601 - rig.engine.hub.stream_frame()) as usize;
        let played = played_frames(&mut rig, frames);
        assert_eq!(played, [at + 3_600, at + 9_600]);
    }

    #[test]
    fn hub_notes_join_and_leave_the_pool() {
        let (mut desk, mut rig, _hub) = Desk::plug_in("notes");
        for (event_type, velocity) in [(NOTE_ON, 90), (NOTE_ON, 0)] {
            NoteEventMsg {
                source: [1; 16],
                target: [0; 16],
                event_type,
                channel: 0,
                note: 48,
                velocity,
            }
            .encode(desk.buf.payload_mut());
            desk.buf
                .write_frame(MSG_NOTE_EVENT, 2, NoteEventMsg::WIRE_SIZE, &mut desk.stream)
                .unwrap();
            let want_held = velocity > 0;
            let deadline = Instant::now() + Duration::from_secs(5);
            while rig.engine.arp.has_notes() != want_held {
                assert!(Instant::now() < deadline, "the note never arrived");
                rig.engine.render(&mut [0.0; 64]);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let mut seen = Vec::new();
        while let Some(event) = rig.display.try_pop() {
            seen.push(event);
        }
        assert_eq!(
            seen,
            [
                DisplayEvent::HubNoteOn {
                    midi_note: 48,
                    velocity: 90
                },
                DisplayEvent::HubNoteOff { midi_note: 48 },
            ]
        );
    }

    #[test]
    fn a_desk_change_the_arp_cannot_use_is_counted() {
        let (mut desk, mut rig, _hub) = Desk::plug_in("lost");
        let at = rig.engine.hub.stream_frame();
        desk.sync(9, 120.0, at, 0.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.shared.desk_lost.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < deadline, "the bad sync was not counted");
            rig.engine.render(&mut [0.0; 64]);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!rig.shared.playing.load(Ordering::Acquire));
    }
}
