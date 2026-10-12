# KZ-B03a: one link carries named streams: the wire and the instrument side

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename. Depends: KZ-B02 landed.

Split: PLAN.md's B03 ("named streams on one link") is two builders' work. This file is the wire format and `kazoo-core`'s link (B03a); `KZ-B03b-named-streams-hub.md` is the desk seating the streams on strips (B03b). B03b depends on B03a. Nothing is dropped between them.

## Outcome

An instrument registers with a list of streams (the wall will register one per `out`), each with a name and a channel count. Every audio frame says which stream it belongs to. A stream can be added and removed while the link is up, and the instrument learns which desk strip each stream got. An instrument built before this lane registers exactly as it does now and is one stream called `main`; a desk built before this lane still accepts a new instrument's stream 0, because stream 0's audio frames do not change. The protocol version stays 3: nothing an old reader could misread is sent to it.

## The design (decided; build exactly this)

### Constants and types (`kazoo-core/src/ipc/types.rs`)

- `pub const MAX_STREAMS: usize = 16;` `pub type StreamId = u16;` `pub const STREAM_NAME_LEN: usize = 24;` (the wall's `MAX_NAME` is 24, `kazoo-wall/src/protocol/mod.rs:59`, so a module name always fits). `pub const NO_STRIP: u8 = 0xFF;` (the link already uses `u8::MAX` for "not plugged in", `link.rs:87`; make it this constant).
- `pub struct StreamSpec { pub channels: u8, pub name: [u8; STREAM_NAME_LEN] }` with `new(name: &str, channels: u8)` (NUL-padded, cut at a char boundary like `RegisterMsg::new`, `:170–185`) and `name_str()`. `STREAM_ENTRY_SIZE = 1 + STREAM_NAME_LEN = 25`.
- `RegisterMsg` (`:131–153`) gains `pub streams: Vec<StreamSpec>`. The registration is built once per connection on the link thread (`link.rs:847–853`), never on the callback, so a `Vec` is right. Wire: bytes 0..59 as today, `pace_lead_frames` at 59..63 as today (`:207`), then `stream_count: u8` at 63 and `stream_count` entries of 25 bytes. `WIRE_SIZE` (59) and `PACED_WIRE_SIZE` (63) stay; new `pub const fn streamed_wire_size(n: usize) -> usize { 64 + 25 * n }` and `pub fn valid_len(len: usize) -> bool` true for 59, 63, and `64 + 25n` with `1 <= n <= MAX_STREAMS`. `encode` writes the tail when `streams` is non-empty and returns the full length; `decode` reads the tail when the buffer carries it, and when it does not (59 or 63 bytes) sets `streams = vec![StreamSpec::new("main", channel_count)]`. Stream 0 is the first entry. `channel_count` (`:137`) is stream 0's channel count and is written as such. New `pub fn check_streams(&self) -> Result<(), &'static str>`: 1 to `MAX_STREAMS` entries, each channels 1 or 2, each name non-empty UTF-8, no two names equal.
- `RegisteredMsg` (`:339–383`): after the 22 bytes, an optional tail `stream_count: u8` then one `strip: u8` per stream (`NO_STRIP` when the desk could not seat it). New field `pub strips: Vec<u8>`; `encode` writes the tail when `strips` is non-empty; `decode` reads it when present, else `strips = vec![strip_index]`. `strip_index` (byte 0) is stream 0's strip, so `HubIpcClient::strip_index` (`client.rs:193`) keeps its meaning.
- New message types: `MSG_STREAM_AUDIO: u8 = 0x11` (instrument → hub, "audio for a named stream"), `MSG_STREAM_ADD: u8 = 0x12` (instrument → hub), `MSG_STREAM_REMOVE: u8 = 0x13` (instrument → hub), `MSG_STREAM_READY: u8 = 0x14` (hub → instrument). With structs: `StreamAddMsg { stream: StreamId, spec: StreamSpec }` (27 bytes: u16 LE, u8, 24), `StreamRemoveMsg { stream: StreamId }` (2 bytes), `StreamReadyMsg { stream: StreamId, strip: u8 }` (3 bytes; `strip == NO_STRIP` means refused, and the bytes after the 3 are the reason as UTF-8, at most 256 bytes; `decode` takes `&[u8]` and returns the struct and the reason as `&str`). Each with `WIRE_SIZE`, `encode`, `decode` in the file's style and a round-trip test.
- `MSG_AUDIO` (0x10) is unchanged and means stream 0. Doc it so at `:21–22`.

### Frames (`kazoo-core/src/ipc/protocol.rs`)

- `pub const AUDIO_STREAM_PAYLOAD_HEADER: usize = 16;` layout: `stream_frame: u64 LE` (0..8), `frame_count: u32 LE` (8..12), `stream: u16 LE` (12..14), `reserved: u16 = 0` (14..16); samples follow, as for 0x10. `pub fn encode_audio_stream_head(stream_frame: u64, frame_count: u32, stream: StreamId, buf: &mut [u8]) -> usize` and `pub fn decode_audio_stream_id(buf: &[u8]) -> StreamId`. `decode_audio_stream_frame`/`decode_audio_frame_count` (`:331–341`) read the same offsets for both frame types and need no change.

### The client (`kazoo-core/src/ipc/client.rs`)

- `register_at` (`:126–176`): send the register as given (with its streams); decode the `RegisteredMsg` with its strips; keep `registration`. `HubMessage` (`:35–44`) gains `StreamReady { stream: StreamId, strip: Option<u8> }` (`None` when refused; the reason text is kept in the client as `last_stream_refusal: Option<(StreamId, String)>`, taken by `take_stream_refusal()` the way `take_desk_health` works, `:324–326`; the reason allocates a `String` on the link thread, which already allocates on its way to a disconnection). `decode` (`:330–347`) learns `MSG_STREAM_READY`.
- New `send_stream_add(&mut self, msg: &StreamAddMsg)` and `send_stream_remove(&mut self, stream: StreamId)`, through the outbox like `send_note_event` (`:379–384`).
- `write_audio_bytes` from B02 is unchanged: audio bytes of either frame type are bytes.

### The link (`kazoo-core/src/ipc/link.rs`)

- `LinkConfig` (`:107–130`) gains `pub streams: Vec<StreamConfig>` with `pub struct StreamConfig { pub name: String, pub channels: u8 }`. `LinkConfig::new` leaves it empty; empty means one stream `main` with `channels`. `hub_link` (`:409`) resolves that once into `LinkState.streams: Mutex<[Option<StreamConfig>; MAX_STREAMS]>` (stream 0 always `Some`), which the UI half writes on add/remove and the link thread reads only in `try_connect` (`:836–886`, normal priority, allocation allowed there per `run`'s doc at `:768–773`) to build the register. `LinkState` also gains `strips: [AtomicU8; MAX_STREAMS]`, all `NO_STRIP` until the hub answers; `strip` (`:210`) becomes `strips[0]`.
- `HubLink` gains:
  - `pub fn add_stream(&self, name: &str, channels: u8) -> Result<StreamId, RequestError>`: `Invalid` for an empty name, a name over 24 bytes, a name already in use, or channels not 1 or 2; `Full` when all 16 ids are taken. Takes the lowest free id ≥ 1, records it, and if connected queues `Request::StreamAdd { stream, spec }` (new variant) for the link thread, which pushes `ToAudio::Stream { stream, channels: Some(c) }` to the callback and sends `MSG_STREAM_ADD`. Not connected: recorded for the next registration; returns `Ok(id)`.
  - `pub fn remove_stream(&self, stream: StreamId) -> Result<(), RequestError>`: `Invalid` for 0 or an id not in use; clears the record and `strips[stream]`, and if connected queues `Request::StreamRemove { stream }`, which pushes `ToAudio::Stream { stream, channels: None }` and sends `MSG_STREAM_REMOVE`.
  - `pub fn stream_strip(&self, stream: StreamId) -> Option<u8>` from `strips`. `LinkStatus::strip` (`:356`) stays stream 0's.
- The ring to the callback (`:418`, `HeapRb<HubMessage>`) becomes `HeapRb<ToAudio>` with `enum ToAudio { Hub(HubMessage), Stream { stream: StreamId, channels: Option<u16> } }`. `HubLinkAudio::try_recv` (`:692–694`) loops: a `Stream` entry updates its own table `stream_channels: [Option<u16>; MAX_STREAMS]` (and clears `next_frames[stream]` on `None`) and is not returned; a `Hub` entry is returned. The instrument's code never sees stream bookkeeping. `plug_in` (`:888–915`) stores every strip from `registration.strips` and pushes `TransportSync` as now; `serve` (`:990–1010`) on `HubMessage::StreamReady` stores the strip and passes the message on to the callback too.
- `HubLinkAudio` gains `next_frames: [u64; MAX_STREAMS]` (stream 0's is today's `next_frame`, `:389`), `stream_channels` (filled from the config's streams at `hub_link`), and `pub fn send_stream_audio(&mut self, stream: StreamId, frames: u32, samples: &[f32]) -> bool`: advances `next_frames[stream]`; an unknown stream returns `false` without counting (the instrument should then play it itself; the UI half has not finished adding it, or has removed it); otherwise exactly `send_audio`'s path (B02's byte ring) with a 0x11 head of 16 bytes carrying the stream id. `send_audio` (`:619`) is stream 0 through 0x10, unchanged on the wire. `pub fn stream_frame_of(&self, stream: StreamId) -> u64`; `stream_frame()` (`:606`) stays stream 0's. The transport clock of an instrument is stream 0: `TransportSyncMsg::at_frame` and `DeskPaceMsg::playing_stream` are stream 0 frames, written in their docs (`types.rs:400–403`, `:259–260`).
- Reconnect: `try_connect` builds the register from `LinkState.streams` as they stand, so streams added live survive a desk restart; `plug_in` resets `strips` from the new answer.

## Not this lane

- The desk seating streams on strips, the strip names, live add/remove on the hub, refusals when strips run out: KZ-B03b.
- The wall registering its `out`s: KZ-B04. Resampling: KZ-B05.
- The strip count (eight): KZ-C01.

## Red tests first

1. `types.rs`: `register_with_three_streams_round_trips` (names, channels, order, `valid_len`); `a_register_without_a_stream_list_is_one_stream_called_main` (59- and 63-byte buffers); `check_streams_refuses_a_duplicate_name_three_channels_and_seventeen_streams`; `registered_with_strips_round_trips_and_an_old_reply_is_one_strip`; `stream_add_remove_and_ready_round_trip` (READY with and without a reason).
2. `protocol.rs`: `stream_audio_head_round_trips_and_shares_offsets_with_audio` (`decode_audio_stream_frame` and `decode_audio_frame_count` read the same values from a 0x11 head).
3. `client.rs` (stand-in hub at `:413–440`): `register_at_sends_its_streams_and_reads_their_strips` (the hub end decodes the register's three streams and answers strips `[2, NO_STRIP, 5]`; `registration().strips == [2, 255, 5]`); `a_stream_ready_with_a_reason_is_kept_for_the_link` (`take_stream_refusal`).
4. `link.rs` (helpers at `:1135–1180`; `accept` must now answer with strips):
   - `a_link_registers_its_streams_and_learns_their_strips`: config with streams `kick`/1 and `pad`/2; `stream_strip(0) == Some(3)`, `stream_strip(1) == Some(4)` after the hub answers `[3, 4]`.
   - `stream_audio_carries_its_stream_id_and_stream_zero_is_plain_audio`: `send_stream_audio(1, 256, ..)` arrives as 0x11 with stream 1 and a 16-byte head; `send_audio(256, ..)` arrives as 0x10 with a 12-byte head.
   - `a_stream_added_live_is_announced_then_usable`: `send_stream_audio(1, ..)` is `false` before `add_stream`; `add_stream("lead", 2)` returns 1 and a `MSG_STREAM_ADD` frame arrives at the hub end; the hub end answers READY strip 6; `stream_strip(1) == Some(6)`; `send_stream_audio(1, ..)` is `true` and a 0x11 frame arrives.
   - `a_stream_removed_live_is_withdrawn`: after the above, `remove_stream(1)`: a `MSG_STREAM_REMOVE` frame arrives, `stream_strip(1)` is `None`, `send_stream_audio(1, ..)` is `false`, and `remove_stream(0)` is `Invalid`.
   - `a_refused_stream_has_no_strip_and_the_reason_is_readable`: the hub end answers READY `NO_STRIP` with "every strip is taken"; `stream_strip(1) == None`; `status().last_refusal` is untouched (the link is still up), and the reason is on `HubLink::last_stream_refusal()` (new, from the client's slot).
   - `streams_are_registered_again_on_reconnect`: add a stream, drop the hub end, accept again: the new register lists two streams.
   - `add_stream_refuses_a_bad_name_a_used_name_and_the_seventeenth`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc
```

Doc tests if doc comments changed: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`.

## Definition of done

- [ ] The six wire items above exist with the byte layouts as written; `PROTOCOL_VERSION` is still 3 and the doc at `types.rs:42–48` says why (no old reader is sent anything new unless it asked, by listing streams).
- [ ] A 63-byte register still decodes to one `main` stream; a 22-byte registered still decodes to one strip.
- [ ] `HubLink::add_stream`/`remove_stream`/`stream_strip`, `HubLinkAudio::send_stream_audio`/`stream_frame_of` exist with the signatures above; `send_audio` and `stream_frame` are unchanged for callers (`kazoo-mix` must compile: run `nice -n 10 env CARGO_BUILD_JOBS=4 cargo check -p kazoo-mix` once, as the only touch outside this lane's crate).
- [ ] Stream bookkeeping never reaches an instrument's `try_recv` loop.
- [ ] Sixteen red tests red first (paste output), then green; every existing test green.
- [ ] BENCH: none asked; report `link.rs`'s line count before and after (it must not grow past where B02 left it; split its tests out if needed).

## Code quality

The tree's rules: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines for anything new; no TODO, no stubs. On the audio callback nothing allocates, locks or makes a system call except `Thread::unpark` (B01's reason): `send_stream_audio` indexes fixed arrays and writes the byte ring; the `Mutex` on `LinkState.streams` is taken only by the UI half and by the link thread at connect time, never by the audio half. Every sample is sanitised as it enters the ring. Red first, then green, one behaviour per commit.

## END

```
START KZ-B03a base=<sha> branch=lane/kz-b03a df=<GB> at <time>
END KZ-B03a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
