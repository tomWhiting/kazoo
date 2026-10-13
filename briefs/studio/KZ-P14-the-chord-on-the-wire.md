# KZ-P14: the chord on the wire: chord frames in batches, a follow request and a desk feature bit, beside the voice, on rings of their own

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 14. It is the wire for P13's frames, built exactly as V07a built the voice's: the same message pattern, the same feature handshake, the same ring discipline.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: the 64-byte layout adds up, and the 516-byte batch is far inside the frame limit. Change: a `count` that is not the kind's own number of tones is refused like an out-of-range one, since the pitch classes are derived from the kind.

Time: about 30 minutes. Crates: kazoo-core (`ipc/`, `host/link.rs`).

Depends: KZ-P13 (`ChordFrame`) and KZ-V07a (`MSG_DESK_FEATURES`, the voice's rings and `link/voice.rs`, which this lane mirrors) must have landed before this lane opens. If any lane editing `ipc/link.rs` or `ipc/client.rs` is in flight (L06), it must have landed first.

Files (7): `kazoo-core/src/ipc/chord.rs` (new), `ipc/mod.rs`, `ipc/types.rs` (constants beside the other `MSG_`s), `ipc/client.rs`, `ipc/link.rs` (calls only), `ipc/link/chord.rs` (new), `host/link.rs` (`drain_chord`).

Why: a chord played into the desk has to reach the arp and the wall, live, as the voice does. Notes cannot carry it: `NoteEventMsg` is 7-bit with no bend (`ipc/types.rs:482-540`), so a slid chord would step. P13's frames carry each tone's continuous pitch, and they need a carrier that keeps it.

## Outcome

An instrument's audio side can hand chord frames to its link (`send_chord`), which sends them to the desk in batches once the desk has said it carries chords. An instrument can ask to follow the chord (`follow_chord(true)`), and its audio side reads every forwarded frame in order (`drain_chord`). All of it is allocation-free on the callback, survives a desk restart, and is refused cleanly by a desk that does not carry chords. Every float crosses bit for bit, so a glide arrives as a glide.

## The design (decided; build exactly this)

### Wire (`ipc/chord.rs`)

- `pub const FEATURE_CHORD: u32 = 2;` (bit 1 of `MSG_DESK_FEATURES`; bit 0 is V07a's `FEATURE_VOICE`).
- `pub const MSG_CHORD: u8 = 0x33;` both directions, and `pub const MSG_CHORD_FOLLOW: u8 = 0x34;` instrument to hub, 1 byte (1 follow, 0 stop). Why 0x33 and 0x34: beside notes (0x30) and the voice (0x31, 0x32), the other performance data. Check both are free on the base, and if not take the next free pair and say so under CHANGED.
- `pub const CHORD_FRAME_WIRE: usize = 64;` little-endian:

| Bytes | Field |
|---|---|
| 0..8 | `at: u64` |
| 8 | `root` (0..=11) |
| 9 | `kind` (0..=9, `ChordKind` in declaration order) |
| 10 | `count` (2..=4) |
| 11 | flags: bit 0 sounding, bit 1 changed; bits 2 to 7 zero |
| 12..16 | `confidence: f32` |
| 16..64 | four tones of 12 bytes: `volts: f32`, `detune_cents: f32`, `level: f32` |

  A tone's `pitch_class` is not sent. The decoder computes it from `root` and `kind`'s intervals in tone order, which is how P13 orders tones, so a frame cannot contradict itself.
- `pub const CHORD_BATCH_MAX: usize = 8;` payload: `count: u8` (1..=8), 3 reserved bytes, then the frames: at most `4 + 8 × 64 = 516` bytes. Why 8: frames come every 10.7 ms, so eight is 85 ms of chords per wake, more than any link stall worth carrying.
- `encode_frame`, `decode_frame` (sanitising, as V07a's decode does):
  - non-finite becomes 0;
  - `volts` clamped to ±8, `detune_cents` to ±50, `level` and `confidence` to 0..=1;
  - an out-of-range `root`, `kind` or `count`, or a `count` that is not `kind`'s number of intervals (2 for power, 4 for the sevenths, 3 for the rest), makes the frame not sounding, with its tones zeroed.
- `encode_batch`, `decode_batch(payload, visit) -> Result<usize, ChordWireError>` (`Truncated`, `BadCount`, `LengthMismatch`), `ChordFollowMsg`.

### Client and link (`ipc/client.rs`, `ipc/link/chord.rs`): V07a's voice code, read and followed line for line

- `MSG_CHORD` never becomes a `HubMessage`: the read path hands its payload to the link thread, as V07a does for `MSG_VOICE`.
- `send_chord_batch` and `send_chord_follow` go through the outbox.
- `LinkState` gains `follow_chord: AtomicBool`, `chord_sent`, `chord_dropped`, `chord_received` and `chord_overflow` (`AtomicU64`). `desk_features` already exists (V07a).
- Two rings of `HeapRb<ChordFrame>`, `CHORD_RING = 64` each (680 ms of frames): `chord_out` (audio side to link thread) and `chord_in` (link thread to audio side), exposed as `pub chord: ChordRx` on `HubLinkAudio` beside `voice`.
- Audio side: `send_chord(&mut self, f: &ChordFrame) -> bool` (false and counted when not connected, when the desk lacks `FEATURE_CHORD`, or when full; else push and `Thread::unpark`).
- Link thread: drain `chord_out` into batches of at most 8; decode incoming batches into `chord_in`; on `MSG_DESK_FEATURES` with the bit and the wish set, send `MSG_CHORD_FOLLOW(1)`; on disconnect, discard and count.
- UI side: `follow_chord(&self, on) -> Result<(), RequestError>`, with the same three outcomes as `follow_voice` (`Unsupported` when connected to a desk without the bit; `Ok` and remembered when not connected). Also `desk_carries_chord()`, `chord_stats()`, and `LinkStatus` gains `follows_chord` and `desk_carries_chord`.
- `kazoo_core::host::link::drain_chord(rx, on)` beside `drain_voice`.

If `link.rs` would grow by more than 20 lines, the rest goes in `link/chord.rs`.

## Not this lane

The desk's forwarding and the source on the desk (P15). The arp (P16) and the wall (P17). Recording chord frames (a `.kzev` kind for C05a's record, written there when C05a is next written).

## Red tests first

Red for all: none of these names exist.

1. `ipc/chord.rs`:
   - `a_frame_round_trips_bit_for_bit`: every field, every flag, every kind; the volts of a gliding chord at 1e-7 V steps round-trip exactly, so a glide is never quantised by the wire;
   - `a_glide_and_a_vibrato_cross_the_link_unchanged`: P13's sliding E major (a perfect fourth over 2 s) and its ±30 cents at 6 Hz A minor, as frames, sent through a link pair: every received tone's `volts` and `detune_cents` equal the sent ones bit for bit, so nothing on the wire steps;
   - `pitch_classes_come_from_root_and_kind`;
   - `nonsense_is_sanitised_on_read` (NaN volts become 0, kind 12 is not sounding, a major with count 4 is not sounding, detune 90 becomes 50);
   - `a_batch_of_8_round_trips_in_order`;
   - `a_bad_batch_is_refused` (count 0, count 9, a length mismatch, truncated);
   - `the_layout_is_pinned` (one frame's 64 bytes against a literal array).
2. `client.rs`: `chord_is_not_a_hub_message` (FEATURES, a CHORD batch, then a NOTE: the note arrives as `HubMessage`, the chord does not).
3. `link.rs` tests (V07a's helpers):
   - `chords_wait_for_the_desk_to_carry_them`: FEATURES with only the voice bit gives `send_chord == false` and `follow_chord(true) == Err(Unsupported)`; FEATURES 3 gives true, and a batch arrives;
   - `frames_go_out_in_batches_in_order` (100 frames in batches of at most 8);
   - `the_follow_wish_survives_a_desk_restart`;
   - `forwarded_frames_reach_drain_chord_in_order`;
   - `a_flood_of_chords_never_crowds_out_notes_or_voice`;
   - `chords_on_the_callback_never_allocate`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc kazoo-core/src/host
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo check -p kazoo-mix -j 4
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`.

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-core test passes, V07a's unchanged.
- [ ] `PROTOCOL_VERSION` is unchanged, and its doc names the chord beside the voice as feature-gated.
- [ ] `link.rs` grows by at most 20 lines (report the line counts before and after).

## Code quality

As PLAN.md's STACK RULES. `send_chord` and `drain_chord` run on audio callbacks: fixed rings, no allocation, lock, format or I/O, and `Thread::unpark` is the one system call. Every frame is sanitised as it is decoded. Byte fields go through `to_le_bytes` and `from_le_bytes`, with no `as` casts clippy pedantic flags. New files under 1000 lines.

## END

```
START KZ-P14 base=<sha> branch=lane/kz-P14 df=<GB> at <time>
END KZ-P14 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
