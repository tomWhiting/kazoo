//! Studio protocol domain types for the future `kazoo-mix` architecture.
//!
//! The existing [`crate::ipc`] module implements today's `kazoo-tui` hub
//! protocol. This module is intentionally separate: it names the stable studio
//! concepts described in `design/terminal-daw/IPC_TRANSPORT.md` without forcing
//! the current hub to migrate all at once.
//!
//! These types are plain, allocation-light Rust data structures. Wire encoding,
//! socket framing, and shared-memory descriptors can be layered on top as the
//! dedicated `kazoo-mix` server comes online.

use crate::DEFAULT_BUFFER_SIZE;

/// Current studio protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Oldest studio protocol version this crate can currently negotiate.
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// Default number of audio blocks by which clients should render ahead of the
/// hardware callback.
pub const DEFAULT_SAFETY_LEAD_BLOCKS: u32 = 3;

/// MIDI-style ticks per quarter note used for bar/beat display and event timing.
pub const TICKS_PER_QUARTER: u32 = 480;

/// Version negotiation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionMismatch {
    /// Lowest protocol version supported by the server.
    pub server_min: u32,
    /// Highest protocol version supported by the server.
    pub server_max: u32,
    /// Protocol version requested by the client.
    pub client_version: u32,
}

/// Negotiate a studio protocol version.
///
/// The initial protocol is intentionally strict: a client advertises one version
/// and the server accepts it only if it falls inside the supported range. This
/// keeps the first `kazoo-mix` control-plane implementation simple while still
/// making version checks explicit at registration time.
pub const fn negotiate_version(client_version: u32) -> Result<u32, VersionMismatch> {
    negotiate_version_in_range(client_version, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION)
}

/// Negotiate a protocol version against an explicit server range.
pub const fn negotiate_version_in_range(
    client_version: u32,
    server_min: u32,
    server_max: u32,
) -> Result<u32, VersionMismatch> {
    if client_version >= server_min && client_version <= server_max {
        Ok(client_version)
    } else {
        Err(VersionMismatch {
            server_min,
            server_max,
            client_version,
        })
    }
}

/// Unique identifier assigned by the mixer to a connected client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u32);

/// Unique mixer channel identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelId(pub u16);

/// Unique shared audio buffer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub u32);

/// Unique render request identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RenderRequestId(pub u64);

/// Stable identity for a studio session.
///
/// This is represented as raw UUID bytes without depending on a UUID crate in
/// `kazoo-core` yet. Callers may format or parse it however their control-plane
/// layer chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub [u8; 16]);

/// Stable identity for a client instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(pub [u8; 16]);

/// The role a connected process plays in the studio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// A sound-generating instrument such as Juno, 303, 808, CS-80, or mouth.
    Instrument,
    /// A controller/sequencer that primarily emits events.
    Controller,
    /// A dedicated tape UI/controller process.
    TapeUi,
    /// A display-only meter/status bridge.
    MeterBridge,
}

/// Supported transport sample format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// 32-bit floating point samples, interleaved by channel.
    F32Interleaved,
}

/// A named audio port advertised by a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPortDescriptor {
    /// Human-readable port name, for example `main`, `sidechain`, or `click`.
    pub name: String,
    /// Number of channels on this port.
    pub channels: u16,
}

impl AudioPortDescriptor {
    /// Create a new audio port descriptor.
    #[must_use]
    pub fn new(name: impl Into<String>, channels: u16) -> Self {
        Self {
            name: name.into(),
            channels: channels.max(1),
        }
    }

    /// Convenience constructor for a mono port.
    #[must_use]
    pub fn mono(name: impl Into<String>) -> Self {
        Self::new(name, 1)
    }

    /// Convenience constructor for a stereo port.
    #[must_use]
    pub fn stereo(name: impl Into<String>) -> Self {
        Self::new(name, 2)
    }
}

/// Capability flags advertised by a studio client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientCapabilities {
    bits: u32,
}

impl ClientCapabilities {
    /// Client can render specific frame ranges requested by the mixer.
    pub const SCHEDULED_RENDER: u32 = 1 << 0;
    /// Client can follow mixer transport snapshots.
    pub const TRANSPORT_SYNC: u32 = 1 << 1;
    /// Client can receive timestamped note events.
    pub const RECEIVES_NOTE_EVENTS: u32 = 1 << 2;
    /// Client can emit timestamped note events.
    pub const SENDS_NOTE_EVENTS: u32 = 1 << 3;
    /// Client can expose parameter summaries/updates to the mixer.
    pub const PARAMETERS: u32 = 1 << 4;

    /// Create capabilities from raw bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self { bits }
    }

    /// Raw capability bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.bits
    }

    /// Return `true` when all `flag` bits are set.
    #[must_use]
    pub const fn contains(self, flag: u32) -> bool {
        (self.bits & flag) == flag
    }

    /// Return a copy with `flag` bits enabled.
    #[must_use]
    pub const fn with(self, flag: u32) -> Self {
        Self {
            bits: self.bits | flag,
        }
    }

    /// Capabilities expected from a normal sound-generating instrument.
    #[must_use]
    pub const fn instrument() -> Self {
        Self::from_bits(Self::SCHEDULED_RENDER | Self::TRANSPORT_SYNC | Self::PARAMETERS)
    }
}

/// How a terminal app should attach to audio/studio infrastructure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunMode {
    /// Always open local audio devices and run without a studio server.
    Standalone,
    /// Require connection to a studio server; fail if it cannot be reached.
    Connect,
    /// Prefer a studio connection, but fall back to standalone mode.
    #[default]
    Auto,
}

/// Audio configuration advertised by a client before registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAudioConfig {
    /// Number of output channels the client renders.
    pub output_channels: u16,
    /// Preferred or current sample rate.
    pub sample_rate: u32,
    /// Maximum block size the client can render without reallocating.
    pub max_block_frames: u32,
}

impl ClientAudioConfig {
    /// Create a new client audio configuration.
    #[must_use]
    pub const fn new(output_channels: u16, sample_rate: u32, max_block_frames: u32) -> Self {
        Self {
            output_channels: if output_channels == 0 {
                1
            } else {
                output_channels
            },
            sample_rate: if sample_rate == 0 {
                crate::DEFAULT_SAMPLE_RATE
            } else {
                sample_rate
            },
            max_block_frames: if max_block_frames == 0 {
                DEFAULT_BUFFER_SIZE as u32
            } else {
                max_block_frames
            },
        }
    }

    /// Stereo client using Kazoo's default sample rate and buffer size.
    #[must_use]
    pub const fn stereo_default() -> Self {
        Self::new(2, crate::DEFAULT_SAMPLE_RATE, DEFAULT_BUFFER_SIZE as u32)
    }
}

/// Registration request sent by a client on the studio control socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    /// Protocol version supported by the client.
    pub protocol_version: u32,
    /// Client role.
    pub client_kind: ClientKind,
    /// Crate/package name, for example `kazoo-808`.
    pub crate_name: String,
    /// Human-readable name shown in mixer UI.
    pub display_name: String,
    /// Stable instance identity for reconnects/session restore.
    pub instance_id: InstanceId,
    /// Audio outputs provided by the client.
    pub outputs: Vec<AudioPortDescriptor>,
    /// Audio inputs consumed by the client, if any.
    pub inputs: Vec<AudioPortDescriptor>,
    /// Behavioral capabilities.
    pub capabilities: ClientCapabilities,
}

/// Registration response sent by the mixer.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerWelcome {
    /// Negotiated protocol version.
    pub protocol_version: u32,
    /// Studio session identity.
    pub session_id: SessionId,
    /// Mixer-assigned client identity.
    pub client_id: ClientId,
    /// Authoritative studio sample rate.
    pub sample_rate: u32,
    /// Authoritative studio render block size.
    pub block_size: u32,
    /// Number of blocks clients should render ahead.
    pub safety_lead_blocks: u32,
    /// Current transport state at registration time.
    pub transport: TransportSnapshot,
    /// Mixer channels assigned to this client.
    pub assigned_channels: Vec<ChannelId>,
    /// Shared audio buffers assigned to this client's outputs.
    pub audio_buffers: Vec<SharedAudioBufferDescriptor>,
}

/// Shared-memory audio buffer advertised by the mixer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedAudioBufferDescriptor {
    /// Mixer-assigned buffer id.
    pub buffer_id: BufferId,
    /// Platform-specific shared memory name/path.
    pub shm_name: String,
    /// Number of interleaved channels.
    pub channels: u16,
    /// Frames per block.
    pub block_frames: u32,
    /// Ring capacity measured in whole blocks.
    pub capacity_blocks: u32,
    /// Sample storage format.
    pub format: AudioFormat,
}

impl SharedAudioBufferDescriptor {
    /// Total samples in one block.
    #[must_use]
    pub const fn samples_per_block(&self) -> usize {
        self.block_frames as usize * self.channels as usize
    }
}

/// Header attached to an audio block in a shared transport buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioBlockHeader {
    /// Absolute studio frame where this block begins.
    pub start_frame: u64,
    /// Number of frames in this block.
    pub frames: u32,
    /// Number of interleaved channels.
    pub channels: u16,
    /// Monotonic sequence number for gap/drift detection.
    pub sequence: u64,
    /// Block status flags.
    pub flags: BlockFlags,
}

/// Flags describing an audio block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockFlags {
    /// Block intentionally contains silence.
    pub silent: bool,
    /// Block was rendered across a loop boundary segment.
    pub loop_wrapped: bool,
    /// Block is the final segment for a render request.
    pub final_segment: bool,
}

/// Mixer-owned transport play state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// Transport stopped, usually at a stable position.
    Stopped,
    /// Transport playing.
    Playing,
    /// Transport recording.
    Recording,
    /// Count-in is active before recording starts.
    CountIn,
}

/// Swing/groove state carried with transport snapshots.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwingState {
    /// Swing amount in the normalized range `[0.0, 1.0]`.
    pub amount: f32,
    /// Subdivision receiving swing, expressed as notes per whole note
    /// (`8` = eighth-note swing, `16` = sixteenth-note swing).
    pub subdivision: u8,
}

impl Default for SwingState {
    fn default() -> Self {
        Self {
            amount: 0.0,
            subdivision: 8,
        }
    }
}

/// Optional future groove-template identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GrooveId(pub u32);

/// Immutable mixer transport snapshot sent to clients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportSnapshot {
    /// Authoritative sample rate.
    pub sample_rate: u32,
    /// Authoritative block size in frames.
    pub block_size: u32,
    /// Absolute session frame at which this snapshot is effective.
    pub session_frame: u64,
    /// Tempo in beats per minute.
    pub bpm: f64,
    /// Time-signature numerator.
    pub time_signature_numerator: u8,
    /// Time-signature denominator.
    pub time_signature_denominator: u8,
    /// Current play state.
    pub play_state: PlayState,
    /// Whether loop playback is enabled.
    pub loop_enabled: bool,
    /// Loop start frame.
    pub loop_start_frame: u64,
    /// Loop end frame.
    pub loop_end_frame: u64,
    /// One-indexed bar number for display.
    pub bar: u32,
    /// One-indexed beat number for display.
    pub beat: u32,
    /// Tick within the beat.
    pub tick: u32,
    /// Swing state for event scheduling.
    pub swing: SwingState,
    /// Optional groove template.
    pub groove_id: Option<GrooveId>,
}

impl Default for TransportSnapshot {
    fn default() -> Self {
        Self {
            sample_rate: crate::DEFAULT_SAMPLE_RATE,
            block_size: DEFAULT_BUFFER_SIZE as u32,
            session_frame: 0,
            bpm: 120.0,
            time_signature_numerator: 4,
            time_signature_denominator: 4,
            play_state: PlayState::Stopped,
            loop_enabled: false,
            loop_start_frame: 0,
            loop_end_frame: 0,
            bar: 1,
            beat: 1,
            tick: 0,
            swing: SwingState::default(),
            groove_id: None,
        }
    }
}

/// A scheduled render job issued by the mixer for a future frame range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderRequest {
    /// Request identity used in the completion message.
    pub request_id: RenderRequestId,
    /// Absolute studio frame where rendering should begin.
    pub start_frame: u64,
    /// Number of frames to render.
    pub frames: u32,
    /// Transport state for this frame range.
    pub transport: TransportSnapshot,
    /// Hardware callback frame by which this block must be available.
    pub deadline_frame: u64,
}

/// Render completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStatus {
    /// Audio was rendered successfully.
    Ok,
    /// Client intentionally supplied silence.
    Silent,
    /// Client missed the requested deadline.
    Underrun,
    /// Client failed to render the request.
    Error,
}

/// Completion notification for a scheduled render request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderComplete {
    /// Request identity being completed.
    pub request_id: RenderRequestId,
    /// Absolute studio frame rendered.
    pub start_frame: u64,
    /// Number of rendered frames.
    pub frames: u32,
    /// Completion status.
    pub status: RenderStatus,
}

/// Timestamped musical/control event routed through the studio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    /// Absolute studio frame at which the event takes effect.
    pub frame: u64,
    /// Originating client, if known.
    pub source: Option<ClientId>,
    /// Destination client, if this is not a broadcast/routed event.
    pub destination: Option<ClientId>,
    /// MIDI channel in the range `0..=15`.
    pub channel: u8,
    /// Event payload.
    pub kind: NoteEventKind,
}

impl NoteEvent {
    /// Create a note-on event.
    #[must_use]
    pub fn note_on(frame: u64, channel: u8, note: u8, velocity: f32) -> Self {
        Self {
            frame,
            source: None,
            destination: None,
            channel: channel.min(15),
            kind: NoteEventKind::NoteOn {
                note: note.min(127),
                velocity: velocity.clamp(0.0, 1.0),
            },
        }
    }
}

/// Payload for a timestamped musical/control event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteEventKind {
    /// Start a note.
    NoteOn {
        /// MIDI note number in the range `0..=127`.
        note: u8,
        /// Normalized velocity in the range `[0.0, 1.0]`.
        velocity: f32,
    },
    /// Release a note.
    NoteOff {
        /// MIDI note number in the range `0..=127`.
        note: u8,
        /// Normalized release velocity in the range `[0.0, 1.0]`.
        velocity: f32,
    },
    /// Continuous controller update.
    ControlChange {
        /// Controller number in the range `0..=127`.
        controller: u8,
        /// Normalized controller value in the range `[0.0, 1.0]`.
        value: f32,
    },
    /// Pitch bend normalized to `[-1.0, 1.0]`.
    PitchBend(f32),
}

/// Stable parameter identity exposed by a client or mixer channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParameterId(pub u32);

/// The target of a parameter event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParameterTarget {
    /// Parameter belongs to a connected client/instrument.
    Client(ClientId),
    /// Parameter belongs to a mixer channel.
    Channel(ChannelId),
    /// Parameter belongs to the master bus.
    Master,
    /// Parameter belongs to the tape processor.
    Tape,
}

/// Timestamped normalized parameter update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterEvent {
    /// Absolute studio frame at which the parameter change takes effect.
    pub frame: u64,
    /// Parameter owner.
    pub target: ParameterTarget,
    /// Parameter identity within the target.
    pub parameter_id: ParameterId,
    /// Normalized value in the range `[0.0, 1.0]`.
    pub value: f32,
}

impl ParameterEvent {
    /// Create a new clamped normalized parameter event.
    #[must_use]
    pub const fn normalized(
        frame: u64,
        target: ParameterTarget,
        parameter_id: ParameterId,
        value: f32,
    ) -> Self {
        let value = if value < 0.0 {
            0.0
        } else if value > 1.0 {
            1.0
        } else {
            value
        };

        Self {
            frame,
            target,
            parameter_id,
            value,
        }
    }
}

/// Render context passed to future studio-capable instruments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderContext<'a> {
    /// Scheduled frame range and transport snapshot.
    pub request: RenderRequest,
    /// Timestamped note/control events relevant to this render block.
    pub note_events: &'a [NoteEvent],
    /// Timestamped parameter changes relevant to this render block.
    pub parameter_events: &'a [ParameterEvent],
}

/// Future-facing render-block trait for instrument crates.
///
/// This trait is deliberately small so existing standalone callbacks can adapt
/// to it incrementally. A future `kazoo-mix` scheduler can call this shape for
/// in-process tests, while out-of-process instruments can use the same method
/// internally when handling `RenderRequest`s.
pub trait InstrumentRenderer {
    /// Render interleaved `f32` audio into `output`.
    ///
    /// Implementations must not allocate, lock, block, or perform I/O. Invalid
    /// output samples must be replaced with silence before returning.
    fn render_block(&mut self, context: RenderContext<'_>, output: &mut [f32]);

    /// Reset internal render state after transport stop or reconnect.
    fn reset(&mut self) {}

    /// Maximum number of frames this renderer can process without reallocating.
    fn max_block_frames(&self) -> u32 {
        DEFAULT_BUFFER_SIZE as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_port_channels_are_at_least_one() {
        let port = AudioPortDescriptor::new("bad", 0);
        assert_eq!(port.channels, 1);
    }

    #[test]
    fn shared_buffer_reports_samples_per_block() {
        let desc = SharedAudioBufferDescriptor {
            buffer_id: BufferId(7),
            shm_name: "kazoo-test".to_string(),
            channels: 2,
            block_frames: 128,
            capacity_blocks: 4,
            format: AudioFormat::F32Interleaved,
        };

        assert_eq!(desc.samples_per_block(), 256);
    }

    #[test]
    fn default_transport_matches_studio_defaults() {
        let transport = TransportSnapshot::default();
        assert_eq!(transport.block_size, DEFAULT_BUFFER_SIZE as u32);
        assert_eq!(transport.play_state, PlayState::Stopped);
        assert_eq!(transport.time_signature_numerator, 4);
        assert_eq!(transport.time_signature_denominator, 4);
        assert_eq!(transport.bar, 1);
        assert_eq!(transport.beat, 1);
    }

    #[test]
    fn client_audio_config_clamps_zero_values_to_safe_defaults() {
        let config = ClientAudioConfig::new(0, 0, 0);

        assert_eq!(config.output_channels, 1);
        assert_eq!(config.sample_rate, crate::DEFAULT_SAMPLE_RATE);
        assert_eq!(config.max_block_frames, DEFAULT_BUFFER_SIZE as u32);
    }

    #[test]
    fn run_mode_defaults_to_auto() {
        assert_eq!(RunMode::default(), RunMode::Auto);
    }

    #[test]
    fn version_negotiation_accepts_supported_version() {
        assert_eq!(negotiate_version(PROTOCOL_VERSION), Ok(PROTOCOL_VERSION));
    }

    #[test]
    fn version_negotiation_rejects_unsupported_version() {
        assert_eq!(
            negotiate_version_in_range(3, 1, 2),
            Err(VersionMismatch {
                server_min: 1,
                server_max: 2,
                client_version: 3,
            })
        );
    }

    #[test]
    fn note_on_constructor_clamps_channel_note_and_velocity() {
        let event = NoteEvent::note_on(64, 42, 200, 2.0);

        assert_eq!(event.channel, 15);
        assert_eq!(
            event.kind,
            NoteEventKind::NoteOn {
                note: 127,
                velocity: 1.0
            }
        );
    }

    #[test]
    fn parameter_event_clamps_normalized_value() {
        let event = ParameterEvent::normalized(
            128,
            ParameterTarget::Channel(ChannelId(2)),
            ParameterId(9),
            -1.0,
        );

        assert_eq!(event.value, 0.0);
    }
}
