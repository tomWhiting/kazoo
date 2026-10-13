# KZ-V07a: the voice on the wire: voice frames in batches, a follow request, and the desk saying it carries them; the instrument side

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core (`ipc/`, `host/link.rs`). Time: about 30 minutes. Depends: KZ-V04 (`VoiceFrame`), KZ-B03a landed (it edits `ipc/types.rs`, `protocol.rs`, `client.rs` and `link.rs`, the same files; this lane builds on its `RegisteredMsg` and request plumbing). If KZ-A03b is in flight (it edits `link.rs` and `host/link.rs`), run after it.

Read by Cassio, 13 Oct 20:4x, at `96051a2`. KZ-B03a has landed (review head 1b81a1c): it added `MSG_STREAM_READY` (0x14), `RegisteredMsg::strips` and `HubIpcClient::desk_takes_streams()`; build `desk_features` beside that, in the same style. `link.rs` is now 1126 lines. 0x24, 0x31 and 0x32 are free on main. This lane opens only after KZ-V04 and KZ-A03b have both landed on main.

Split: the voice stream across the desk is two builders' work, as B03 was. This file is the wire format and `kazoo-core`'s link (V07a); `KZ-V07b-desk-carries-the-voice.md` is the hub (V07b). V07b depends on V07a.

Why: Tom wants his voice "to drive all the engines we have". The mouth (V08a) makes a `VoiceFrame` every 64 samples; every following instrument (V09) and the wall (V10) need those frames, live, through the desk, the way notes travel today. The choice of carrier, decided:

- **Not B03's named streams.** Those are audio: each takes a desk strip, is summed into the mix and (B05) resampled. Seven control values at 750 Hz are not audio.
- **Not notes plus `ParameterChange`.** `NoteEventMsg` is 7-bit with no timestamp (`ipc/types.rs:482-540`) and pitch bend is unmapped everywhere; continuous Hz through `ParameterChange` needs every instrument's parameter table (B06) and 750 messages a second would fill the 64-entry request ring notes share (`ipc/link.rs:88`) and the desk's event recorder (C05a).
- **A message of its own**, batched per wake of the link thread, carried to followers only, on rings of its own so it can never crowd out notes or transport syncs. `PROTOCOL_VERSION` stays 3: a desk built before this lane would disconnect an instrument that sent an unknown type (`kazoo-mix/src/hub/instrument.rs:260`), so an instrument sends voice messages only after the desk has said it carries them; an instrument built before this lane ignores the desk's new message (unknown hub-to-instrument types are skipped, `ipc/client.rs:359`).

## Outcome

An instrument's audio callback can hand voice frames to its link (`send_voice`), which sends them to the desk in batches; an instrument can ask the desk to follow the voice (`follow_voice(true)`), and its audio callback reads every forwarded frame in order (`drain_voice`). All of it is allocation-free on the callback, survives a desk restart (the follow wish is sent again), and is refused cleanly by a desk that does not carry the voice.

## The design (decided; build exactly this)

### Types and wire (`kazoo-core/src/ipc/voice.rs`, new; `pub mod voice;` in `ipc/mod.rs`; constants re-exported from `types.rs` beside the other `MSG_` constants)

- `pub const MSG_DESK_FEATURES: u8 = 0x24;` hub → instrument, sent once right after `MSG_REGISTERED`. Payload `features: u32 LE`. `pub const FEATURE_VOICE: u32 = 1;` (bit 0). Why 0x24: the 0x2x range is the desk's own messages (pace 0x22, health 0x23).
- `pub const MSG_VOICE: u8 = 0x31;` both directions (instrument → hub from the voice's source; hub → instrument to followers). Why 0x31: beside notes (0x30), the other performance data.
- `pub const MSG_VOICE_FOLLOW: u8 = 0x32;` instrument → hub. Payload 1 byte: 1 = follow the voice, 0 = stop.
- `pub const VOICE_FRAME_WIRE: usize = 40;` frame layout, little-endian:

| Bytes | Field |
|---|---|
| 0..8 | `at: u64` |
| 8..12 | `hz: f32` |
| 12..16 | `volts: f32` |
| 16..20 | `snapped_hz: f32` |
| 20..24 | `snapped_volts: f32` |
| 24..28 | `loudness: f32` |
| 28..32 | `breath: f32` |
| 32..36 | `confidence: f32` |
| 36 | flags: bit 0 gate, bit 1 onset, bit 2 voiced; bits 3..7 zero |
| 37 | reserved, 0 |
| 38..40 | reserved `u16`, 0 |

  Reserved bytes are written 0 and ignored on read, so a later field can take them without a version change.
- `pub const VOICE_BATCH_MAX: usize = 32;` `MSG_VOICE` payload: `count: u8` (1..=32), 3 reserved bytes (0), then `count` frames: at most `4 + 32 × 40 = 1284` bytes. Why 32: a 2048-sample callback makes 32 frames; one wake sends at most one batch per 32 frames.
- `pub fn encode_frame(f: &VoiceFrame, out: &mut [u8; VOICE_FRAME_WIRE])`; `pub fn decode_frame(b: &[u8; VOICE_FRAME_WIRE]) -> VoiceFrame` which **sanitises**: every float not finite becomes 0.0; `loudness`, `breath`, `confidence` clamped to 0..=1; `hz`/`snapped_hz` clamped to 0..=20,000; `volts`/`snapped_volts` clamped to ±8; a frame whose `hz` was not finite has `voiced` and `gate` cleared. `pub fn encode_batch(frames: &[VoiceFrame], out: &mut [u8]) -> usize` and `pub fn decode_batch(payload: &[u8], visit: impl FnMut(VoiceFrame)) -> Result<usize, VoiceWireError>` (`Truncated`, `BadCount` for 0 or over 32, `LengthMismatch` when `len != 4 + count·40`). `FeaturesMsg` and `VoiceFollowMsg` with `WIRE_SIZE`, `encode`, `decode` in `types.rs`'s style.

### The client (`ipc/client.rs`)

- `MSG_DESK_FEATURES` is decoded and kept (`desk_features: u32`, `take_desk_features()` like `take_desk_health`, `client.rs:337-345`); it is not a `HubMessage`.
- `MSG_VOICE` never becomes a `HubMessage` (that enum's slots stay small and the 256-slot message ring, `link.rs:85`, stays for notes and transport). The client exposes the raw payload of a voice message for the link thread to decode straight into the voice ring (e.g. a `HubRead::Voice(&[u8])` return from the read path, or a `take_voice_payload()`; follow the read path's existing shape).
- `send_voice_batch(&mut self, payload: &[u8])` and `send_voice_follow(&mut self, on: bool)` through the outbox, like `send_note_event`.

### The link (`ipc/link.rs` plus `ipc/link/voice.rs`, new, because `link.rs` is 1163 lines)

- `LinkState` gains `desk_features: AtomicU32` (0 until the desk says otherwise; 0 again on disconnect), `follow_voice: AtomicBool` (the instrument's wish), `voice_sent: AtomicU64`, `voice_dropped: AtomicU64` (frames the audio half could not queue or the link could not deliver), `voice_received: AtomicU64`, `voice_overflow: AtomicU64` (forwarded frames that did not fit the receive ring).
- Two new SPSC rings, built in `start_link` (`:415-421`), each `HeapRb<VoiceFrame>` of `VOICE_RING = 256` frames (341 ms at 48 kHz: survives one stalled link-thread wake at any callback size):
  - `voice_out`: audio half (producer) → link thread (consumer).
  - `voice_in`: link thread (producer) → audio half (consumer), exposed as `pub voice: VoiceRx` on `HubLinkAudio` beside `events` (`link.rs:380`), with `kazoo_core::host::link::drain_voice(rx: &mut VoiceRx, on: impl FnMut(VoiceFrame))` beside `take_events` (`host/link.rs:219-228`): visits every queued frame in order, at most `VOICE_RING` per call, no allocation.
- **Audio half:** `pub fn send_voice(&mut self, frame: &VoiceFrame) -> bool`: `false` (and counted in `voice_dropped`) when not connected, when the desk lacks `FEATURE_VOICE`, or when `voice_out` is full; else push and `Thread::unpark` the link thread (constant time, no lock; B01's reason). `send_audio` already unparks; one extra unpark per callback is harmless.
- **Link thread:** on every wake, while connected and the desk carries voice, drain `voice_out` into batches of at most 32 frames (a stack buffer of 1284 bytes) and send each with `send_voice_batch`. On reading `MSG_VOICE`, `decode_batch` straight into `voice_in` (full → `voice_overflow` += the rest; a decode error is a protocol fault and is handled as the link handles any malformed hub message today). On `MSG_DESK_FEATURES`: store it; if `follow_voice` is set and the feature is present, send `MSG_VOICE_FOLLOW(1)`. On disconnect: `desk_features = 0`; frames left in `voice_out` are discarded and counted.
- **UI half (`HubLink`):** `pub fn follow_voice(&self, on: bool) -> Result<(), RequestError>`: stores the wish always; if connected and the desk carries voice, queues `Request::VoiceFollow(on)` (new variant) for the link thread; if connected and the desk does **not** carry voice, returns `Err(RequestError::Unsupported)` (new variant, `Display`: "this desk is too old to carry the voice"); if not connected, `Ok(())` (the wish is sent when the desk answers with its features). `pub fn desk_carries_voice(&self) -> bool`. `pub fn voice_stats(&self) -> VoiceLinkStats { sent, dropped, received, overflow }`. `LinkStatus` gains `follows_voice: bool` and `desk_carries_voice: bool`.

## Not this lane

The hub: announcing the feature, forwarding, one voice at a time, strip badges (V07b). Who calls `send_voice` (V08a) or `follow_voice`/`drain_voice` (V09, V10). Recording voice frames (C05a follow-up, PLAN.md Track V).

## Red tests first

Red for all: none of these names exist.

1. `ipc/voice.rs` tests: `a_frame_round_trips_bit_for_bit` (every field, every flag combination); `nonsense_is_sanitised_on_read` (NaN hz → 0 and not voiced, loudness 7 → 1, volts −40 → −8); `a_batch_of_32_round_trips_in_order`; `a_bad_batch_is_refused` (count 0, count 33, length 4 + 3·40 with count 4, truncated); `the_layout_is_pinned` (one frame's 40 bytes against a literal byte array).
2. `client.rs` (stand-in hub at `:413-440`): `desk_features_are_kept_and_voice_is_not_a_hub_message` (the hub end sends FEATURES then a VOICE batch then a NOTE: the client keeps features, the note comes through `HubMessage`, the voice does not).
3. `link.rs` tests (helpers at `:1135-1180`; `accept` sends FEATURES when the test asks):
   - `voice_waits_for_the_desk_to_say_it_carries_it`: hub end sends no FEATURES: `send_voice` is `false`, `voice_dropped` counts, `follow_voice(true)` is `Err(Unsupported)`; then FEATURES(1): `send_voice` is `true` and a `MSG_VOICE` arrives at the hub end.
   - `frames_go_out_in_batches_in_order`: 100 frames sent between two wakes arrive as batches of at most 32, `at` strictly increasing, all 100.
   - `the_follow_wish_survives_a_desk_restart`: `follow_voice(true)` before connecting: after FEATURES the hub end receives `MSG_VOICE_FOLLOW(1)`; drop the hub end, accept again with FEATURES: `MSG_VOICE_FOLLOW(1)` again; `follow_voice(false)`: `MSG_VOICE_FOLLOW(0)`.
   - `forwarded_frames_reach_drain_voice_in_order`: the hub end sends three batches (40 frames): `drain_voice` visits 40 frames in order with the values sent.
   - `a_flood_of_voice_never_crowds_out_notes`: the hub end sends 1,000 frames then one note: the note arrives through `take_events`; `voice_overflow` counts what did not fit 256; nothing blocks.
   - `voice_on_the_callback_never_allocates`: `send_voice` × 1,000 and `drain_voice` over a full ring inside `assert_no_alloc`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc kazoo-core/src/host
```

Doc tests: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`. `kazoo-mix` must still compile (`nice -n 10 env CARGO_BUILD_JOBS=4 cargo check -p kazoo-mix`, the only touch outside this lane's crate): this lane adds no hub handling, so a desk built from this lane alone never sends FEATURES and every instrument keeps its voice to itself.

## Definition of done

- [ ] The three message types and the 40-byte frame exist as the tables say; `PROTOCOL_VERSION` is still 3 and its doc (`types.rs:42-48`) says why (instruments send voice only after FEATURES; old instruments skip FEATURES and are never sent VOICE).
- [ ] `MSG_VOICE` never enters the `HubMessage` ring (say where it is intercepted).
- [ ] All red tests red first (paste), then green; every existing test green.
- [ ] `link.rs` does not grow by more than 30 lines (report before and after); the rest is in `link/voice.rs`.

## Code quality

As PLAN.md's STACK RULES. `send_voice` and `drain_voice` run on audio callbacks: fixed rings, no allocation, lock, format or I/O; `Thread::unpark` is the one system call (B01). The link thread may allocate only where it already does (connect time). Every frame is sanitised as it is decoded. No `as` casts clippy pedantic flags (byte fields through `to_le_bytes`/`from_le_bytes`). New files under 1000 lines.

## END

```
START KZ-V07a base=<sha> branch=lane/kz-V07a df=<GB> at <time>
END KZ-V07a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked; link.rs line count before and after
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
