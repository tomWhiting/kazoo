# KZ-B02: a block is copied twice on its way to the desk, not six times, and a frame may be a mebibyte

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Depends: KZ-B01 landed (this lane builds on `LINK_TICK`, the unpark, and the hub's `poll`).

## Outcome

Today a block of audio is copied six times between the instrument's callback and the desk's strip: callback into the link ring (`kazoo-core/src/ipc/link.rs:646` `push_block`), ring into the link thread's scratch (`:943` `pop_block`), scratch into the frame payload as bytes (`client.rs:238` `encode_audio_payload`), payload into the outbox (`outbox.rs:73–74`), then after the socket, frame buffer into the hub's scratch (`kazoo-mix/src/hub/instrument.rs:235` `decode_audio_samples`) and scratch into the strip ring (`:274` `push_block`). When this lane is done there are two: the callback writes the frame, bytes and all, straight into a byte ring, and the link thread hands the ring's bytes to the socket; the hub reads the frame and decodes it straight into the strip ring. The frame limit is 1 MiB instead of 64 KiB, so a block of 16,384 frames goes whole. The link's backlog is a quarter of a second of audio, whatever the rate and channels, not 64 KiB; and the hub stopping is detected by how long the socket has refused bytes, not by how many are waiting. The wire format of a frame does not change; the protocol version goes to 3 so an old desk says "rebuild it" instead of dropping a big frame.

## The design (decided; build exactly this)

### Limits

- `kazoo-core/src/ipc/protocol.rs:21` `MAX_PAYLOAD_SIZE = 65_536` becomes `1_048_576` (1 MiB). `MAX_FRAME_SIZE` follows. `FrameBuffer::new` (`:106`) then allocates 1 MiB + 9 per buffer; that is fine for the client's two (`client.rs:135`, `:140`) and the hub's one per connection (`hub.rs:580`, kept by the `Instrument`). It is not fine for `Server::refuse` (`hub.rs:691`), which makes a fresh `FrameBuffer` per refusal: replace it with a stack `[u8; HEADER_SIZE + REFUSAL_BYTES]`, `const REFUSAL_BYTES: usize = 512`, the reason text cut at a char boundary to 512 bytes, `encode_header` then one `write_all`.
- `kazoo-core/src/ipc/types.rs:48` `PROTOCOL_VERSION = 2` becomes `3`, doc: "Version 3: frames up to 1 MiB and audio blocks up to 16,384 frames; the frame layout is version 2's." The hub's refusal text at `hub.rs:703–709` already says which version the desk speaks.
- `link.rs:75` `MAX_LINK_BLOCK_FRAMES = 4096` becomes `16_384`. `instrument.rs:24` `MAX_MESSAGE_FRAMES = 4096` becomes `16_384`. A 16,384-frame stereo block is 131,072 bytes of samples: well inside 1 MiB.
- The strip ring stays `MAX_SOURCE_RING_SAMPLES = 65_536` (`engine.rs:46`). A block must fit it twice, so `register` (`hub.rs:701`) refuses an instrument whose registered `buffer_size * channels * 2 > MAX_SOURCE_RING_SAMPLES` with a new `RefuseReason::BlockTooLarge { frames: u32, most: u32 }` displayed as "blocks of {frames} frames (at most {most} at {channels} channels)". `most_extra_lead` (`hub.rs:1055–1062`) today counts `MAX_MESSAGE_FRAMES` in its base; at 16,384 that would leave no extra lead for anyone. Give it the instrument's own registered block size instead: `Registration` (`instrument.rs:403–417`) gains `block_frames: u32` (the register's `buffer_size`, clamped 1..=`MAX_MESSAGE_FRAMES`), `Instrument` keeps it, and `fit_lead` (`:346–349`) passes it to `most_extra_lead(ring_frames, clock, sample_rate, block_frames)`.

### Instrument side: one copy, into a byte ring

- `HubLinkAudio` (`link.rs:382–391`) loses `audio: AudioBlockProducer` and gains `wire: ringbuf::HeapProd<u8>`, `seq: u32`. `LinkWorker` (`:745–764`) loses `audio`, `scratch` and `pending`, gains `wire: ringbuf::HeapCons<u8>`.
- Ring size, in `hub_link` (`:412–417`): `fn link_ring_bytes(sample_rate: u32, channels: u16, block_frames: u32) -> usize`: `frames = sample_rate * LINK_BACKLOG_MS / 1000` with `pub const LINK_BACKLOG_MS: u64 = 250`; `headers = frames.div_ceil(64) * (HEADER_SIZE + AUDIO_PAYLOAD_HEADER)` (a header budgeted per 64-frame block, the smallest block worth sending); `bytes = frames * channels * 4 + headers`, and never less than two of the largest frame, `2 * (HEADER_SIZE + AUDIO_PAYLOAD_HEADER + block_frames * channels * 4)`. The backlog is therefore a quarter of a second at any rate and channel count. `AUDIO_BLOCKS` (`:78`) goes.
- `send_audio` (`:619–643`) keeps its signature and its stream-frame accounting. Per sub-block of at most `block_frames` (as now): `let payload = AUDIO_PAYLOAD_HEADER + count * channels * 4; let total = HEADER_SIZE + payload; if self.wire.vacant_len() < total { blocks_dropped += 1; continue; }`. Write a 21-byte stack head: `encode_header(&FrameHeader { msg_type: MSG_AUDIO, payload_len: payload as u32, seq: self.seq }, &mut head[..9])`, then `stream_frame.to_le_bytes()` and `count.to_le_bytes()`; `self.wire.push_slice(&head)`; then `self.wire.push_iter(samples[start..end].iter().map(|s| sanitize_sample(*s).to_le_bytes()).flatten())` (`Producer::push_iter`, ringbuf 0.4); `self.seq = self.seq.wrapping_add(1)`; `blocks_sent += 1`; `self.waker.unpark()`. Vacant space was checked and this is the only producer, so every frame enters whole; the ring never holds half a frame. `AudioBlockHeader` and `BlockFlags` are no longer used in `link.rs`.
- Sequence numbers: the audio frames count their own `seq`; the outbox counts the request frames' (`outbox.rs:47`). No reader uses `seq` for ordering (`instrument.rs` reads only `msg_type` and `payload_len`); say so in `protocol.rs:3–13`'s frame doc: "seq counts frames per sender stream and is diagnostic".
- `LinkWorker::serve` (`:926–1011`): requests first, as now. Then `client.flush()`; if `client.pending_bytes() > 0` the socket is full and no audio bytes may follow a half-written request frame: run the stuck check below and skip to reading. Otherwise `write_wire`: `loop { let (head, _) = self.wire.as_slices(); if head.is_empty() { break } match client.write_audio_bytes(head) { Ok(0) => return Err("the desk accepted nothing"), Ok(n) => { self.wire.skip(n); self.stuck_since = None; } Err(e) if WouldBlock => { stuck check; break } Err(e) if Interrupted => {} Err(e) => return Err(format!("sending audio failed: {e}")) } }`. `Consumer::as_slices` and `skip` are safe ringbuf 0.4 API. `HubIpcClient` gains `pub fn write_audio_bytes(&mut self, bytes: &[u8]) -> io::Result<usize>` (`(&self.stream).write(bytes)`); its `send_audio` (`client.rs:227–247`) stays as the one-frame path for tests and tools, encoding through the outbox as today (one copy more, off the real path).
- Stuck by time: `stuck_since: Option<Instant>` (exists, `:757`) is set when the socket refuses (`WouldBlock`) with bytes waiting and cleared on any write of n > 0; when `now - stuck_since > STUCK_LIMIT` (`:64`, 500 ms) `serve` returns `Err("the desk stopped taking audio for over 500 ms")`. The byte count no longer matters: `OUTBOX_LIMIT` (`outbox.rs:13`) still bounds the request outbox (small control frames; 64 KiB of them unread is already a dead peer), unchanged.
- `discard_backlog` (`:1057–1075`): `let bytes = self.wire.occupied_len(); self.wire.clear(); if bytes > 0 { blocks_dropped += 1 }`; the doc on `LinkStatus::blocks_dropped` (`:358–361`) says a discarded backlog counts once.
- `plug_in` (`:888–915`) and `disconnect` (`:1013–1037`): no `pending` any more; `disconnect` discards the ring as above.

### Hub side: one copy, into the strip ring

- `kazoo-core/src/audio_transport.rs`: new `AudioBlockProducer::push_block_le(&mut self, header: AudioBlockHeader, bytes: &[u8]) -> Result<(), AudioRingPushError>` beside `push_block` (`:159–193`): `expected = frames * channels`; `bytes.len() == expected * 4` or `InvalidSampleCount { expected, actual: bytes.len() / 4 }`; same vacancy and meta checks; `self.sample_prod.push_iter(bytes.chunks_exact(4).map(|b| sanitize_sample(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))))`; same meta push and sequence rule. One pass over the bytes: decode and sanitise while copying.
- `Instrument::handle` MSG_AUDIO (`instrument.rs:224–238`): keep the length checks; drop `decode_audio_samples` and `scratch`; `push_audio` (`:264–303`) takes `&payload[AUDIO_PAYLOAD_HEADER..AUDIO_PAYLOAD_HEADER + samples * 4]` and calls `push_block_le`. Borrow: `payload` borrows `self.read_buf`; `push_audio` needs `&mut self.stamp` and `&mut self.producer`; split the borrows by taking `let (read_buf, producer, stamp) = (&self.read_buf, &mut self.producer, &mut self.stamp)` or by making `push_audio` a free function over those fields. `scratch` (`:128`, `:185`) goes; `decode_audio_samples` stays in `protocol.rs` for its tests and any other caller.
- `MAX_MESSAGE_FRAMES` doc: "the instruments' own ceiling, `MAX_LINK_BLOCK_FRAMES`".

### Copies, counted (for DONE)

After: (1) callback → link byte ring, encoding as it copies (`send_audio`); kernel `write` from the ring; kernel `read` into the hub's `FrameBuffer`; (2) `FrameBuffer` → strip ring, decoding and sanitising as it copies (`push_block_le`). Bytes per block per copy: `frames * channels * 4` (+21 of header in the first). The desk callback's own pop into `buffered_samples` (`engine.rs:1008`) is the desk's render, not transport, and is unchanged.

## Not this lane

- Waking threads: KZ-B01 (landed). Streams: KZ-B03a/b (they add a 0x11 frame; this lane's ring carries any frame type as bytes, so B03a needs no ring change). Resampling: KZ-B05.
- The strip ring's size and the desk callback: unchanged (C01 makes strips a `Vec`).
- `HubIpcClient::send_audio` for tests stays; do not route the link through it.

## Red tests first

1. `kazoo-core/src/audio_transport.rs`: `push_block_le_matches_push_block_and_sanitises`: the same samples (with NaN and Inf) pushed as f32 through `push_block` into one ring and as LE bytes through `push_block_le` into another pop out bit-identical; a byte slice of the wrong length is `InvalidSampleCount`.
2. `kazoo-core/src/ipc/protocol.rs`: change `frame_buffer_rejects_oversized_payload` (`:480`) to `a_payload_of_one_mebibyte_is_read_and_one_byte_more_is_refused`.
3. `kazoo-core/src/ipc/link.rs` tests (helpers at `:1135–1180`):
   - `a_block_bigger_than_sixty_four_kib_reaches_the_hub_whole`: 16,384 stereo frames; the hub end's `read_frame` returns one `MSG_AUDIO` frame with `payload_len == 12 + 16_384 * 8`, and the samples decode to what was sent (red today: `TooLarge`).
   - `the_backlog_holds_a_quarter_second_then_counts_drops`: hub end never reads; push 512-frame stereo blocks at 48 kHz until `blocks_dropped > 0`; the blocks queued before the first drop cover at least 250 ms and at most 250 ms + the kernel's socket buffer (measure the socket buffer by what `write` accepted: assert queued frames * 512 / 48_000 ≥ 0.25).
   - `a_hub_that_stops_reading_for_half_a_second_is_let_go_by_time`: hub end reads nothing; push blocks every 5 ms; within 1.5 s `link.is_connected()` is false and `status().last_refusal` contains "stopped taking audio"; and (the time part) with a hub end that reads one frame every 100 ms the link stays connected for 2 s even though more than 64 KiB is queued at times.
   - `request_frames_and_audio_frames_never_interleave_on_the_wire`: a slow hub end (reads one frame per millisecond) while the link pushes 8,192-frame blocks and `send_note` every block; 500 frames read, each decodes as `MSG_AUDIO` with the right length or `MSG_NOTE_EVENT` with 36 bytes; nothing else.
   - `blocks_dropped_counts_a_discarded_backlog_once`: queue blocks with no hub, then connect; `blocks_dropped` grew by exactly 1 for the discard.
4. `kazoo-mix/src/hub/tests.rs` (`Rig`, `:27–97`):
   - `an_instrument_sending_8192_frame_blocks_is_heard`: as `an_instrument_plugs_into_the_first_free_strip_and_is_heard` (`:111`) with 8,192-frame blocks (register `buffer_size` 8192).
   - `an_instrument_whose_blocks_would_not_fit_its_strip_twice_is_refused`: register `buffer_size = 32_768`, stereo; refused with "blocks of 32768 frames"; `HubNotice::Refused { reason: RefuseReason::BlockTooLarge { .. } }`.
   - `a_version_2_instrument_is_told_to_rebuild`: a hand-built `RegisterMsg` with `protocol = 2`; refused; the text contains "protocol 2" and "3".
   - `extra_lead_is_held_to_the_instruments_own_block_size`: two paced instruments (`pace_lead_frames = 4800`), one registered with 256-frame blocks and one with 16,384; the first's `DeskPaceMsg::lead_frames` is at least 4,800 larger than its base; the second's is the ring's half less its base (read both from the pace messages they receive).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc kazoo-core/src/audio_transport.rs kazoo-mix/src
```

Doc tests if doc comments changed: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core -p kazoo-mix`.

## Definition of done

- [ ] `MAX_PAYLOAD_SIZE` is 1 MiB; `PROTOCOL_VERSION` is 3; `MAX_LINK_BLOCK_FRAMES` and `MAX_MESSAGE_FRAMES` are 16,384.
- [ ] `HubLinkAudio` writes frames into a byte ring sized by `LINK_BACKLOG_MS`; `LinkWorker` writes from `as_slices` and `skip`s; no `scratch`, no `pending`, no `AudioBlockProducer` in `link.rs`.
- [ ] The hub decodes into the strip ring with `push_block_le`; no `scratch` in `Instrument`.
- [ ] Copies counted by reading the final code path, written in DONE as the two named above with bytes per block per copy for a 512-frame stereo block (4,096 bytes each, 4,117 with the header).
- [ ] Stuck is by time (`STUCK_LIMIT`), proven by the test; the outbox limit still guards requests.
- [ ] `refuse` allocates no `FrameBuffer`.
- [ ] Twelve red tests red first (paste output), then green; every existing test green. The existing `audio_is_local_until_the_hub_appears_then_goes_to_the_desk` (`link.rs:1253`) still passes with its `payload_len` assertion.
- [ ] BENCH: not asked; report the B01 wake-latency tests' p50/p99 after this change so the next lane has a baseline.

## Code quality

The tree's rules: `unsafe_code` forbidden (no `bytemuck` casts, no mmap: the byte ring and `push_iter`/`as_slices`/`skip` are the whole mechanism); no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; `link.rs` is already over 1000 lines: this lane must leave it no longer than it found it (the ring code replaces the block-ring code; move the `tests` module to `link/tests.rs` if that is what it takes); no TODO, no stubs. On the audio callback nothing allocates, locks or makes a system call except `Thread::unpark` (B01's reason: a futex wake, constant time, never blocks); `push_slice`/`push_iter` on a pre-sized `HeapProd` allocate nothing. Every sample is sanitised at both copies, so NaN and Inf never cross the wire or enter a strip. Red first, then green, one behaviour per commit.

## END

```
START KZ-B02 base=<sha> branch=lane/kz-b02 df=<GB> at <time>
END KZ-B02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
