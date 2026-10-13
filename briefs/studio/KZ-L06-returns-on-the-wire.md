# KZ-L06: an instrument can ask the desk for any strip's sound back, stamped with the frame of its own stream at which the desk played it, and can tell the desk a loop took a take

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: two new directions on the existing link, desk-to-instrument audio ("returns") and the loop-take notice, on the wire and in kazoo-core's client and link. The desk's side is KZ-L07; the wall's are KZ-L09 and KZ-L10. Crates: kazoo-core only (`ipc/types.rs`, `ipc/protocol.rs`, `ipc/client.rs`, `ipc/link.rs`, and a new `ipc/returns.rs` so `link.rs` does not grow past where B03a leaves it). Depends: KZ-B03a (named streams; same files, and its `StreamReady` pattern is copied here) and KZ-V07a (`MSG_DESK_FEATURES`, `RequestError::Unsupported`, `LinkState.desk_features`).

Read by Cassio, 13 Oct evening. Checked: 24 + 32 + 4 + 4 + 4 + 8 + 8 + 4 is 88; a second of stereo f32 at 48 kHz is 384,000 bytes. Change: the checks are written out in the lane form. Opens only after B03a and V07a have landed.

## Why returns exist

Today audio goes one way: instrument to desk (`MSG_AUDIO`, B02; named streams, B03). The wall takes nothing from the desk: `drain_hub` drops notes and parameters (`kazoo-wall/src/engine/mod.rs:1061-1078`), and V10 brings the voice to the wall only as control frames (pitch, gate, loudness), not as sound. Tom wants to sing a loop alongside the wall. Loops must record "the voice, any desk strip, or the wall's own output" (Cassio's brief for this track). So the wall needs a strip's sound back from the desk. A desk sends that the way a hardware console sends an aux to an outboard tape machine.

Every return block carries the frame of the receiver's own stream at which the desk played it (the hub maps studio frames to each instrument's stream frames already, `StudioStamp::stream_at`, `kazoo-mix/src/studio_clock.rs`). A loop can then put what Tom sang on the bar he heard, however late it arrives (KZ-L09).

## The design (decided; build exactly this)

### Wire (`ipc/types.rs`, `ipc/protocol.rs`)

New types, each with `WIRE_SIZE`, `encode` and `decode` in the file's style (`DeskHealthMsg`, `types.rs:298`), and a round-trip test:

| type | id | direction | payload |
|---|---|---|---|
| `MSG_RETURN_ASK` | `0x15` | instrument → hub | `ReturnAskMsg { id: ReturnId (u16 LE), source: [u8; RETURN_NAME_LEN] }`: 26 bytes; `source` is a strip label as B03b names strips (`mouth/dry`, `808/kick`), `G1`–`G4` for a group, or `master`; NUL-padded, cut at a char boundary like `StreamSpec::new` (B03a) |
| `MSG_RETURN_READY` | `0x16` | hub → instrument | `ReturnReadyMsg { id: u16, ok: u8 }`: 3 bytes, then on `ok == 0` the reason as UTF-8, at most 256 bytes (B03a's `StreamReadyMsg` layout) |
| `MSG_RETURN_AUDIO` | `0x17` | hub → instrument | 16-byte head: `stream_frame: u64` (the *receiver's* stream frame at which the desk played the block's first frame), `frame_count: u32`, `id: u16`, `reserved: u16 = 0`; then `frame_count × 2` f32 LE samples, always stereo |
| `MSG_RETURN_DROP` | `0x18` | instrument → hub | `id: u16`: 2 bytes |
| `MSG_LOOP_TAKE` | `0x50` | instrument → hub | `LoopTakeMsg`, 88 bytes (below) |

- `pub const RETURN_NAME_LEN: usize = 24;` matches `STREAM_NAME_LEN` (B03a) and the wall's `MAX_NAME`.
- `pub const MAX_RETURNS: usize = 4;` per link. Reason: a wall has at most eight loops (KZ-L03), and in practice they record from one or two sources (the voice, a guitar strip). Four lets the wall hold a voice, a group, the master and one more without a ninth ring. Each costs 384 KB (below).
- `pub const RETURN_BLOCK_FRAMES: u32 = 256;` is the most frames in one `MSG_RETURN_AUDIO`. It equals the deck's block (C04), so the desk forwards each tapped block as it is.
- Ids `0x15–0x18` and `0x50` are free: B03a uses `0x11–0x14`, Track V uses `0x24`, `0x31` and `0x32` (PLAN.md Track V).
- `LoopTakeMsg { label: [u8; 24], reel: [u8; 32], frames: u32, rate: u32, beats: u32, anchor_beat: f64, take_beat: f64, passes: u32 }`: 24 + 32 + 4 + 4 + 4 + 8 + 8 + 4 = 88 bytes, all LE.
  - `label`: the loop's module name or id.
  - `reel`: the raw SHA-256 of `kazoo_core::reels::ReelId` (KZ-L01).
  - `frames`: N, not counting the seam tail.
  - `beats`: the loop's length in whole beats.
  - `anchor_beat`: the song beat of the loop's first frame.
  - `take_beat`: the song beat at which the take's first written bar began.
  - `passes`: how many passes the take spans.

  The desk turns this into a loop clip (KZ-L10).
- `PROTOCOL_VERSION` stays 3. An old instrument is never sent `0x16` or `0x17`, because it never asks. A desk built before KZ-L07 that receives `0x15` or `0x50` disconnects the instrument, because it does not know the type (`kazoo-mix/src/hub/instrument.rs:260`). So the link sends neither until the desk has said it knows them: `MSG_DESK_FEATURES` (`0x24`, a `features: u32`, sent once after `MSG_REGISTERED`; Track V's V07a, where `FEATURE_VOICE = 1`) gains bits `FEATURE_RETURNS: u32 = 1 << 1` and `FEATURE_LOOP_TAKES: u32 = 1 << 2`.

### The client (`ipc/client.rs`)

- A `0x16` decodes to `HubMessage::ReturnReady { id, ok: bool }`, with the reason kept in the client like B03a's stream refusal (`take_return_refusal(id)`).
- A `0x17` is checked: the head is present, `frame_count ≤ RETURN_BLOCK_FRAMES`, the payload length is exactly `16 + frame_count × 8`, and `reserved` is 0. A bad frame is a connection error, as every malformed frame is today. A good one returns the marker `HubMessage::ReturnAudio` without a payload. The link thread must call `HubIpcClient::take_return_audio(&mut self, sink: impl FnOnce(ReturnId, u64, u32, &[u8]))` before the next `try_recv`; it hands the head's values and the sample bytes from the client's read buffer, with no copy.
- `send_return_ask`, `send_return_drop` and `send_loop_take` queue frames through the outbox (`client.rs:384`'s pattern).

### The link (`ipc/returns.rs`, used by `ipc/link.rs`)

- **UI half (`HubLink`)**:
  - `ask_return(&self, source: &str) -> Result<ReturnId, RequestError>`. `Invalid` covers a bad name, or an id table already holding `MAX_RETURNS`. `NotConnected` means the link is down. `Unsupported` (V07a's `RequestError` arm) means the desk has not announced `FEATURE_RETURNS`, read from V07a's `LinkState.desk_features`. The call builds one `audio_block_ring` (`audio_transport.rs:378`) of `AudioRingConfig::new(BufferId(id), 2, RETURN_BLOCK_FRAMES, blocks)` with `blocks = ceil(sample_rate × RETURN_RING_SECONDS / 256)`. It gives the producer to the link thread through the `LinkState` table (a `Mutex`, taken only by the UI half and the link thread, as B03a's streams table) and the consumer to the audio half through a lock-free hand-off ring (`HeapRb<ReturnHandoff>` of `MAX_RETURNS × 2`). Then it queues the ask.
  - `pub const RETURN_RING_SECONDS: f64 = 1.0;`. Reason: twice the wall's longest wait for a return (`RETURN_DELAY_SECONDS = 0.5`, KZ-L09), so a block waits for the loop without being dropped. It is 384 KB a return at 48 kHz.
  - `drop_return(id)` queues `0x18`, removes the producer, and sends a hand-off asking the audio half to give the consumer back, through a second ring, to be freed on the UI half's next call.
  - `return_state(id) -> ReturnState`, one of `Asked`, `Ready`, `Refused(String)`, `Gone` (the link went down) or `Unknown`.
  - `send_loop_take(&LoopTakeMsg) -> Result<(), RequestError>`, refused with `Unsupported` without `FEATURE_LOOP_TAKES`.
- **Link thread**:
  - `ReturnReady` updates the table.
  - `ReturnAudio` calls `take_return_audio` and `push_block_le` into that id's producer (`audio_transport.rs:197`), stamping the block with `stream_frame` as its `start_frame`.
  - An id it does not hold (dropped while frames were in flight) is counted in `LinkState.returns_unknown`; a full ring is counted in `returns_dropped`.
  - On disconnect, every return is `Gone` and its producer is dropped. On reconnect, the link asks again for every return it held, with the same ids, as B03a re-registers streams.
- **Audio half (`HubLinkAudio`)**:
  - `pub fn pop_return(&mut self, id: ReturnId, out: &mut [f32]) -> Option<ReturnBlock>`, where `ReturnBlock { stream_frame: u64, frames: u32 }`. It pops one block into `out` (at least `2 × RETURN_BLOCK_FRAMES`). It installs pending hand-offs first. It allocates nothing, locks nothing and frees nothing: consumers live in `[Option<AudioBlockConsumer>; MAX_RETURNS]`, and a consumer to free goes back through the ring.
  - `pub fn stream_frame(&self) -> u64` already exists (it counts frames sent, `link.rs:386` `next_frame`). Its doc says it is the frame number a return's `stream_frame` is measured on.

## Not this lane

- KZ-L07: the desk taps strips and sends the blocks, and decodes `0x15`, `0x18` and `0x50`.
- KZ-L09: the wall asks and records. KZ-L10: the wall sends `0x50` and the desk makes clips.

## Red tests first

Each fails today because none of these types exists.

1. `types.rs`: `return_messages_round_trip` (ask, ready with and without a reason, drop), `loop_take_round_trips_in_88_bytes`, and `a_return_name_is_cut_at_a_char_boundary`.
2. `protocol.rs`: `return_audio_head_round_trips_and_refuses_a_bad_length`.
3. `client.rs`, with the stand-in hub (`client.rs:413–440`):
   - `return_audio_is_handed_over_without_a_copy`: a 0x17 with 256 frames; `take_return_audio` sees id, frame and length, and the bytes equal what was sent;
   - `a_malformed_return_frame_ends_the_connection`.
4. `link.rs` tests, or `returns.rs` tests with the link's helpers:
   - `a_return_is_asked_announced_and_filled`. The stand-in hub announces `FEATURE_RETURNS`. `ask_return("mouth/dry")` gives id 0, and a `0x15` arrives at the hub end. The hub end answers READY ok and sends three 0x17 blocks at stream frames 1000, 1256 and 1512. `pop_return(0, ..)` yields them in order with those frames and the samples bit for bit.
   - `no_ask_or_take_goes_to_a_desk_that_has_not_said_it_knows_them`. No features announced: `ask_return` is `Unsupported`, `send_loop_take` is `Unsupported`, and the hub end receives nothing.
   - `a_refused_return_says_why`. READY with ok 0 and "no strip called mouth/dry": `return_state(0) == Refused("no strip called mouth/dry")`.
   - `returns_are_asked_again_on_reconnect`.
   - `a_dropped_return_frees_its_ring_off_the_audio_thread`. Under `assert_no_alloc` around `pop_return`, as kazoo-core's link tests already do for `send_audio`.
   - `the_fifth_return_is_refused`.
   - `a_loop_take_reaches_the_hub_byte_for_byte`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo check -p kazoo-mix -p kazoo-wall -j 4
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc
```

The `cargo check` of kazoo-mix and kazoo-wall runs once, at the end, as B03a's lane did.

## Definition of done

- [ ] The tests above fail first (paste each under RED), then pass, and every existing kazoo-core test passes.
- [ ] `PROTOCOL_VERSION` is still 3, and the doc at `types.rs:42–48` says why: nothing new is sent to a desk that has not announced the feature.
- [ ] `HubMessage::ReturnAudio` never reaches an instrument's `try_recv` loop. Show the line where the link thread consumes it.
- [ ] `pop_return` allocates, frees and locks nothing (`assert_no_alloc` test green).
- [ ] BENCH: none asked. Report `link.rs`'s line count before and after: it must not grow by more than the call sites. Everything else is in `returns.rs`.

## Code quality

As PLAN.md's STACK RULES. Every sample pushed into a return ring is sanitised by `push_block_le` (it already sanitises, `audio_transport.rs:197`). The audio half makes no system call: `pop_return` only reads a ring.

## END

```
START KZ-L06 base=<sha> branch=lane/kz-L06 df=<GB> at <time>
END KZ-L06 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
