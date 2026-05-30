# Hub IPC Protocol — Instrument-to-Hub Communication

How instrument crates connect to the kazoo-tui hub.

## Transport Layer

**Primary:** Unix domain sockets. Low latency, zero network overhead, kernel-managed buffering. Each instrument opens a connection to the hub's socket at a well-known path (e.g., `/tmp/kazoo-hub.sock` or `$XDG_RUNTIME_DIR/kazoo/hub.sock`).

**Alternative (benchmark both):** Shared-memory ring buffers (`ringbuf` crate, same as the internal mic buffer). Even lower latency — no kernel copy. But requires coordinating memory-mapped regions. Start with Unix domain sockets; upgrade to shared memory if latency profiling shows it matters.

## Wire Protocol

Binary framed protocol. Little-endian. No handshake beyond the initial registration message.

### Frame Format

```
+--------+--------+--------+---------------------------+
| Type   | Length | Seq    | Payload                   |
| 1 byte | 4 byte | 4 byte | variable                  |
+--------+--------+--------+---------------------------+
```

- **Type:** message type (see below)
- **Length:** total payload length in bytes (not including header)
- **Seq:** monotonic sequence number for ordering

### Message Types

#### 0x01: Register

Instrument -> Hub. Sent once on connection.

```
{
  instrument_id: [u8; 16],   // UUID
  name: String,               // "kazoo-808", "kazoo-cs80", etc.
  channel_count: u8,          // 1 (mono) or 2 (stereo)
  sample_rate: u32,           // must match hub's rate
  buffer_size: u32,           // agreed block size
}
```

Hub responds with 0x02 (Registered) containing the assigned channel strip index.

#### 0x02: Registered

Hub -> Instrument. Confirms registration.

```
{
  strip_index: u8,            // mixer channel strip assigned
  hub_sample_rate: u32,       // authoritative sample rate
  hub_buffer_size: u32,       // authoritative buffer size
  transport_state: u8,        // current transport state
  bpm: f32,                   // current tempo
  position: u64,              // current position in samples
}
```

#### 0x10: Audio

Instrument -> Hub. Sent every buffer cycle.

```
{
  frame_count: u32,           // number of frames in this block
  samples: [f32; frame_count * channel_count]  // interleaved
}
```

This is the hot path. Must be as lean as possible. No JSON, no allocations. Raw f32 slice with a 4-byte frame count header.

#### 0x20: Transport Sync

Hub -> Instrument. Sent on every transport state change.

```
{
  state: u8,                  // 0=Stopped, 1=Playing, 2=Recording, 3=Paused
  bpm: f32,
  position: u64,              // sample position
  timestamp: u64,             // monotonic clock (nanos) for drift correction
}
```

#### 0x21: Transport Request

Instrument -> Hub. Instrument requests transport change (e.g., drum machine hits play).

```
{
  requested_state: u8,
  requested_bpm: Option<f32>,
}
```

Hub is authoritative — it may accept or ignore.

#### 0x30: Note Event

Between instruments (routed through hub, or direct if instruments discover each other).

```
{
  source: [u8; 16],          // instrument UUID
  target: [u8; 16],          // target instrument UUID (or broadcast)
  event_type: u8,            // 0=NoteOn, 1=NoteOff, 2=CC, 3=PitchBend
  channel: u8,
  note: u8,
  velocity: u8,
  // for CC: controller_number in note field, value in velocity field
}
```

Used by kazoo-arp to drive kazoo-mini or kazoo-cs80.

#### 0x40: Parameter Change

Hub -> Instrument or Instrument -> Hub. Mixer parameter updates.

```
{
  strip_index: u8,
  param: u8,                  // 0=Volume, 1=Pan, 2=Mute, 3=Solo, 4=Arm
  value: f32,
}
```

#### 0xFF: Shutdown

Either direction. Clean disconnect.

## Timing and Synchronization

The hub's output callback runs at a fixed buffer size (128 samples at native sample rate). Every output callback cycle, the hub:

1. Reads audio frames from all connected instruments' sockets (non-blocking)
2. Mixes into per-channel buffers
3. Applies master bus effects
4. Writes to output

Instruments must deliver audio blocks in sync with the hub's buffer cycle. The transport sync message includes a monotonic timestamp for drift correction. If an instrument's audio arrives late, the hub uses the previous block (hold-last-value). If it arrives early, it's queued.

## Latency Budget

```
Instrument synthesis:     ~2.9ms  (128 samples at 44100)
Socket write + read:      ~0.1ms  (Unix domain, kernel buffer)
Hub mix + master effects: ~0.0ms  (within hub's output callback)
Hub output buffer:        ~2.9ms  (128 samples)
------------------------------------------------------------
Total instrument->speaker: ~6ms
```

Compare to internal (mic->speaker): ~5.8ms. The socket adds about 0.1-0.2ms. Imperceptible.

## Discovery

Hub listens on a well-known socket path. Instruments connect on startup. If the hub isn't running, instruments can run standalone (they contain their own audio output via kazoo-core's `build_streams`). When the hub comes up, instruments reconnect.

The hub broadcasts its presence via a pidfile at `$XDG_RUNTIME_DIR/kazoo/hub.pid` containing the socket path and PID.
