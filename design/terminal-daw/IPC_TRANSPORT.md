# IPC, Audio Transport, And Synchronization Specification

## Purpose

Kazoo's terminal DAW needs multiple independent terminal processes to behave like one low-latency studio. This document defines the best-case communication architecture between `kazoo-mix`, instruments, controllers, and tape.

## Design Summary

Use two planes:

```text
Control plane: Unix domain sockets
Audio plane:   shared memory lock-free ring buffers
```

The control plane is flexible. The audio plane is predictable.

## Studio Server

`kazoo-mix` starts a studio server:

```text
$XDG_RUNTIME_DIR/kazoo/{session-id}/control.sock
```

Fallback on macOS:

```text
/tmp/kazoo-{uid}/{session-id}/control.sock
```

The server owns:

- session id
- sample rate
- block size
- transport state
- client registry
- shared memory allocation descriptors
- channel assignment

## Client Registration

Client opens control socket and sends:

```rust
ClientHello {
    protocol_version: u32,
    client_kind: Instrument | Controller | TapeUi | MeterBridge,
    crate_name: String,
    display_name: String,
    instance_id: Uuid,
    outputs: Vec<AudioPortDescriptor>,
    inputs: Vec<AudioPortDescriptor>,
    capabilities: ClientCapabilities,
}
```

Mixer replies:

```rust
ServerWelcome {
    protocol_version: u32,
    session_id: Uuid,
    client_id: ClientId,
    sample_rate: u32,
    block_size: u32,
    safety_lead_blocks: u32,
    transport: TransportSnapshot,
    assigned_channels: Vec<ChannelId>,
    audio_buffers: Vec<SharedAudioBufferDescriptor>,
}
```

## Audio Buffer Model

Each audio output port gets a shared ring buffer.

```rust
struct SharedAudioBufferDescriptor {
    buffer_id: BufferId,
    shm_name: String,
    channels: u16,
    block_frames: u32,
    capacity_blocks: u32,
    format: AudioFormat, // f32 interleaved initially
}
```

Each block includes metadata:

```rust
struct AudioBlockHeader {
    start_frame: u64,
    frames: u32,
    channels: u16,
    sequence: u64,
    flags: BlockFlags,
}
```

Audio samples are `f32`, interleaved for transport simplicity.

## Render Scheduling

Best-case mode is mixer-scheduled.

### Scheduler Thread

Mixer scheduler sends render requests ahead of time:

```rust
RenderRequest {
    request_id: u64,
    start_frame: u64,
    frames: u32,
    transport: TransportSnapshot,
    deadline_frame: u64,
}
```

Instrument renders exactly that frame range into its shared buffer and replies:

```rust
RenderComplete {
    request_id: u64,
    start_frame: u64,
    frames: u32,
    status: Ok | Silent | Underrun | Error,
}
```

### Audio Callback

The mixer callback does not send requests. It only consumes blocks that are already ready.

If a block is missing:

- output silence for that channel/block
- increment underrun counter
- keep audio callback moving

## Why Scheduled Pull Beats Push

Push model lets each instrument run on its own clock. That creates drift and jitter. Scheduled pull makes the mixer the clock authority.

The actual implementation is not synchronous blocking pull. It is request-ahead scheduling with ring-buffer delivery.

## Transport Snapshot

```rust
struct TransportSnapshot {
    sample_rate: u32,
    block_size: u32,
    session_frame: u64,
    bpm: f64,
    time_signature_numerator: u8,
    time_signature_denominator: u8,
    play_state: PlayState,
    loop_enabled: bool,
    loop_start_frame: u64,
    loop_end_frame: u64,
    bar: u32,
    beat: u32,
    tick: u32,
    swing: SwingState,
    groove_id: Option<GrooveId>,
}
```

## BPM Sync

BPM changes are timestamped:

```rust
TempoChange {
    effective_frame: u64,
    bpm: f64,
    ramp: TempoRamp,
}
```

Most changes are immediate at a block boundary. Future smooth ramps are possible but not required for first full design.

## Loop Sync

Looping is frame-based, even if displayed as bars/beats.

When transport reaches `loop_end_frame`, next block wraps to `loop_start_frame`. Scheduler must split render requests if a block crosses loop boundary:

```text
request A: frames before loop end
request B: frames from loop start
```

Instruments should receive explicit wrapped frame ranges rather than guessing.

## Swing And Groove

Swing affects event scheduling, not sample clock.

```rust
struct SwingState {
    enabled: bool,
    amount: f32,       // 0.5 straight, >0.5 delayed offbeats
    subdivision: Grid, // 1/8, 1/16, triplet, etc.
}
```

Future groove templates are deterministic lookup tables:

```rust
GrooveTemplate {
    id,
    name,
    grid,
    offsets_samples: Vec<i32>,
    velocity_scale: Vec<f32>,
}
```

303/808/arp apply groove to note event timing while still rendering on exact audio frames.

## Note And Control Events

Control messages are timestamped with frame positions:

```rust
NoteEvent {
    target: ClientId | ChannelId,
    frame: u64,
    kind: NoteOn | NoteOff,
    note: u8,
    velocity: f32,
}

ParameterEvent {
    target: ParameterTarget,
    frame: u64,
    parameter_id: ParameterId,
    value: ParameterValue,
}
```

A UI keypress in an instrument can be local to that instrument. A sequencer/controller can send timestamped events to other clients via the mixer.

## Real-Time Safety

### Allowed In Audio Callback

- atomics
- preallocated ring reads
- bounded DSP
- writing output

### Forbidden In Audio Callback

- socket I/O
- allocation
- locks
- filesystem
- dynamic routing mutation
- waiting for render completion
- JSON/TOML parsing

## Serialization

Control plane can use a compact binary format:

- `postcard`
- `bincode`
- custom fixed headers

Avoid JSON in hot paths. JSON/TOML are fine for session files.

## Shared Memory Implementation Options

macOS-compatible options:

- POSIX `shm_open` + mmap
- memfd-like abstraction where available
- temporary mmap files under runtime dir

Rust crates to evaluate later:

- `memmap2`
- `shared_memory`
- custom libc wrapper

The protocol should hide the backend.

## Connection Health

Each client has heartbeat/status:

```rust
ClientStatus {
    client_id,
    last_seen_millis,
    render_queue_depth,
    underruns,
    cpu_estimate,
    state: Connected | Late | Disconnected | Reclaiming,
}
```

Mixer UI exposes health per channel.

## Reconnect

Clients have stable `instance_id`s. If `kazoo-juno` crashes and restarts with the same session identity, mixer can reclaim its channel.

Rules:

- channel settings remain mixer-owned
- instrument patch state may be restored from session if available
- audio remains silent until client catches up

## Protocol Versioning

Every hello includes protocol version. Mixer may reject unsupported clients with a readable message.

## Standalone Fallback

If no control socket is found:

- instrument opens local audio output
- local transport is used
- UI shows `standalone`

If connected:

- instrument does not open output stream
- it uses mixer sample rate/block size
- UI shows assigned channel and studio latency

## Debugging Tools

Add future diagnostic commands:

```bash
kazoo doctor
kazoo studio status
kazoo ipc sniff
kazoo latency-test
```

Diagnostics should show:

- connected clients
- block size
- safety queue depth
- underruns
- average render time
- worst render time
- clock position

## Implementation Strategy

Even though this is the best-case spec, implementation can land in layers:

1. Define protocol types in `kazoo-core`.
2. Implement socket registration and transport broadcast.
3. Implement socket-carried audio blocks for correctness.
4. Replace audio payloads with shared memory rings without changing high-level protocol.
5. Add scheduled render requests.
6. Add reconnect/session restore.

This avoids designing an MVP while still allowing safe construction of the final architecture.
