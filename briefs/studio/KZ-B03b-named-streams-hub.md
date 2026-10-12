# KZ-B03b: the desk gives every stream its own strip, named `<instrument>/<stream>`

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Depends: KZ-B03a landed (the wire, the client and the link); KZ-B02 before it.

Split: this is the second half of PLAN.md's B03. B03a built the wire and the instrument side; this file is `kazoo-mix`'s hub seating streams on strips, naming them, adding and removing them live, and refusing what it cannot seat without dropping the instrument.

## Outcome

An instrument that registers three streams gets three strips, each labelled with the instrument's short name and the stream's name (`wall/out1`, `808/kick`). Audio marked for a stream plays through that stream's strip and no other. A stream added while the instrument is plugged in gets the next free strip and is told which; one removed gives its strip back. When the desk has no free strip for a stream, that stream is refused with the reason and the rest of the instrument carries on. An instrument from before B03a (one `main` stream) gets one strip labelled as today, so the desk looks the same for everything already built.

## The design (decided; build exactly this)

### One instrument, many streams (`kazoo-mix/src/hub/instrument.rs`)

- Today `Instrument` (`:118–152`) holds one `slot`, `channels`, `producer`, `stamp`, `placements`, `dropped`, `dropped_since`. Move those into `pub struct StreamSlot { pub id: StreamId, pub name: String, pub channels: u16, pub slot: usize, producer: AudioBlockProducer, stamp: StudioStamp, placements: u64, dropped: u64, dropped_since: bool, ring_frames: u64, block_frames: u32 }` and give `Instrument` `pub streams: Vec<StreamSlot>` (capacity `MAX_STREAMS`, filled at registration, grown live). `Registration` (`:403–417`) gains `streams: Vec<StreamSeat>` with `StreamSeat { id, name, channels, slot, ring_frames }`; `slot` and `channels` leave it.
- Stream 0 is the instrument's clock: `Instrument::stamp()` (`:307`) returns `streams[0].stamp`; `pace` (`:318–339`) and `fit_lead` (`:346–349`) use stream 0's stamp, and `fit_lead` sets the same extra lead on every stream's stamp (one instrument, one timer). `health()` (`:354–359`) sums `placements` and `dropped` over the streams. `needs_song` is set when stream 0 is re-placed, not another stream.
- `handle` (`:209–262`):
  - `MSG_AUDIO`: as now, into `streams[0]`.
  - `MSG_STREAM_AUDIO`: `need(AUDIO_STREAM_PAYLOAD_HEADER)`; `stream = decode_audio_stream_id`; find it in `streams`; not found: `Inbound::AudioDropped` plus a new counter `Instrument.unknown_stream_frames` (an add the instrument sent and the desk refused, or a remove racing audio already in flight; never fatal). Found: the same length checks against that stream's channels, then `push_audio` on that `StreamSlot`.
  - `MSG_STREAM_ADD`: `need(StreamAddMsg::WIRE_SIZE)`; `Inbound::StreamAdd(StreamAddMsg)`.
  - `MSG_STREAM_REMOVE`: `need(2)`; `Inbound::StreamRemove(StreamId)`.
- `push_audio` (`:264–303`) becomes a method on `StreamSlot`, taking the clock; its `resynced` result carries which stream: `Inbound::Audio { stream: StreamId, resynced: bool }`.
- `pub fn send_stream_ready(&mut self, stream: StreamId, strip: Option<u8>, reason: &str)` encodes a `StreamReadyMsg` (B03a) and queues it.

### Seating (`kazoo-mix/src/hub.rs`)

- `register_pending` (`:602–681`): the length checks at `:620–656` become `RegisterMsg::valid_len(len)`; otherwise refuse with "its registration was {len} bytes". After `decode`, `register.check_streams()` failing refuses with its text as `RefuseReason::Handshake`.
- `register` (`:701–784`): for each stream in order: `self.strips.iter().position(Free)`; stream 0 with no free strip refuses the whole instrument with `DeskFull` as now; a later stream with none gets `NO_STRIP` in the `RegisteredMsg.strips` tail, a `HubNotice::Refused { name: label, reason: DeskFull, told: true }`, and is not kept in `streams`. Every seated stream: `strip_ring(slot, channels)`, `Strip::Busy`, `PatchCommand::Attach { slot, name: short_name(&strip_label(&name, &stream_name, single)), consumer }`, `HubNotice::Joined { slot, name: label }`. `fn strip_label(instrument: &str, stream: &str, single: bool) -> String`: `display_name(instrument)` alone when the instrument has exactly one stream named `main` (so old instruments look as today), else `format!("{}/{}", display_name(instrument), stream)`; `short_name` (`engine.rs:639`) cuts it to 12 bytes on a char boundary. The notice's `name` is the full `kazoo-808/kick`. Block-size check (B02's `BlockTooLarge`) applies per stream with that stream's channels.
- `PATCH_CAPACITY` (`:87`) is `2 * DESK_CHANNELS`: one instrument seating up to eight streams at once queues eight attaches; `send_patches` (`:992–1000`) already waits for room, so no change, but assert in a test that a two-stream join does not stall.
- `read_instruments` (`:786–831`): `Inbound::Audio { stream: 0, resynced: true }` sets `needs_song`; other streams' resyncs count in `stats.resyncs` only. New arms: `Inbound::StreamAdd(msg)` → `self.seat_stream(index, msg)`; `Inbound::StreamRemove(id)` → `self.unseat_stream(index, id)`. Both are collected into a `Vec<(usize, StreamChange)>` kept on `Server` like `notes` (`:487`), applied after the loop, so a poll allocates nothing.
- `seat_stream`: refuse (READY with `NO_STRIP` and the reason; `Refused` notice) when: the id is 0 or already held ("stream {id} is already there"), `streams.len() == MAX_STREAMS`, `check_streams`-style name/channel faults, the block size does not fit, or no strip is free ("every strip is taken"). Otherwise: ring, `Busy`, `Attach`, push the `StreamSlot`, READY with the strip, `Joined`. The new stream's stamp starts fresh (`StudioStamp::with_extra_lead(current extra)`).
- `unseat_stream`: id 0 is refused (READY `NO_STRIP`, "stream 0 cannot be removed; say goodbye instead"); unknown id is READY `NO_STRIP` "no stream {id}"; otherwise `Strip::Releasing`, `PatchCommand::Detach { slot }`, `HubNotice::Left { slot, name: label, reason: LeaveReason::Goodbye }`, the `StreamSlot` dropped (its producer drops here, off the audio thread; the consumer comes back through `PatchEvent::Detached` as now, `:977–987`).
- `remove` (`:933–951`): for every stream of the leaving instrument, `Releasing`, `Detach`, `Left` with the instrument's reason.
- `collect_patch_events` `Refused { slot }` (`:966–976`): find the instrument and stream holding `slot`; the whole instrument leaves with `LeaveReason::Engine` as today (an engine refusal is a bug, not a seating problem); every other strip it held is released through `remove`'s path.
- `HubSnapshot` (`:283–294`) gains `streams: u64` (strips held by instruments now); `HubStats` likewise; `poll_once` (`:548–550`) stores both.
- `song_sync`/`follow_song` (`:859–901`, `:1082–1108`) use `instrument.stamp()` (stream 0): unchanged. `most_extra_lead` is computed on stream 0's ring; every stream of one instrument gets the same `strip_ring` size, so it holds for all.
- The hub module doc (`:1–26`) gets a paragraph: one instrument, several streams, one strip each, stream 0 the clock.

### The desk's header

`kazoo-mix/src/main.rs:182` reads notices into the news line; nothing to change: the notices already carry the labels. `ui.rs` draws `channel_readout(slot).name` (`shared.rs:517`), which is the 12-byte `short_name`; nothing to change.

## Not this lane

- The wire and the instrument side: KZ-B03a (landed). The wall's outs: KZ-B04.
- More than eight strips, buses, scrolling: KZ-C01. Resampling: KZ-B05.
- The demo 808 in `source.rs` registers one stream and is untouched.

## Red tests first

All in `kazoo-mix/src/hub/tests.rs` with the `Rig` (`:27–97`); `Rig::connect` (`:67–69`) gains a sibling `connect_streams(name, &[(&str, u8)])` that builds a `RegisterMsg` with those streams and calls `HubIpcClient::register_at`.

- `a_two_stream_instrument_gets_two_strips_named_after_its_streams`: streams `kick`/1, `pad`/2; `registration().strips == [0, 1]`; notices `Joined { slot: 0, name: "kazoo-808/kick" }` then `Joined { slot: 1, name: "kazoo-808/pad" }`; `name_str(&channel_readout(0).name) == "808/kick"`, `(1) == "808/pad"`; `snapshot().streams == 2`, `instruments == 1`.
- `an_old_single_stream_instrument_is_seated_as_before`: the existing `connect` (63-byte register): one strip named `808`, exactly as `an_instrument_plugs_into_the_first_free_strip_and_is_heard` (`:111`) asserts today.
- `stream_audio_plays_through_its_own_strip`: two stereo streams; send 0x11 frames for stream 1 at level 0.25 and 0x10 frames for stream 0 at 0.0 (the clock must advance too); the desk's render shows 0.25 on strip 1's meter (`shared.take_channel_meters(1)` peak ≈ 0.25 within 0.02) and strip 0's meter silent.
- `a_stream_added_live_is_seated_and_told_its_strip`: single-stream instrument; `client.send_stream_add(&StreamAddMsg { stream: 1, spec: StreamSpec::new("lead", 2) })`; `receive` a `HubMessage::StreamReady { stream: 1, strip: Some(1) }`; `Joined { slot: 1, name: "kazoo-808/lead" }`; `channel_readout(1).connected`.
- `a_stream_removed_live_frees_its_strip_for_the_next_instrument`: then `send_stream_remove(1)`; `Left { slot: 1, name: "kazoo-808/lead", reason: Goodbye }`; a second instrument connecting gets strip 1.
- `when_strips_run_out_a_later_stream_is_refused_and_the_instrument_stays`: reserve all but one strip; connect with streams `a`/2, `b`/2; `strips == [n, NO_STRIP]`; `Refused { name: "kazoo-808/b", reason: DeskFull, told: true }` after the `Joined`; `client.try_recv` keeps working and stream 0's audio is heard.
- `a_live_add_with_no_free_strip_is_refused_with_the_reason`: full desk; `send_stream_add` for stream 1; `StreamReady { stream: 1, strip: None }` and `take_stream_refusal()` says "every strip is taken"; the instrument is still connected.
- `removing_stream_zero_is_refused`: READY `NO_STRIP` with "stream 0".
- `audio_for_an_unknown_stream_is_dropped_not_fatal`: 0x11 frames for stream 7 (never added); `snapshot().dropped_blocks` grows; the instrument stays; stream 0 audio still heard.
- `an_instrument_leaving_frees_every_strip_it_held`: three streams; `send_shutdown`; three `Left` notices; all three strips free (`channel_readout(n).connected` false) and a new instrument gets strip 0.
- `a_registration_with_a_duplicate_stream_name_is_refused`: `check_streams`' text reaches the client as `PermissionDenied`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

Doc tests if doc comments changed: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-mix`.

## Definition of done

- [ ] `StreamSlot` exists; `Instrument` has no single `slot`/`producer`/`stamp` field; stream 0 is the clock, in code and in the module doc.
- [ ] `strip_label` names strips `<instrument>/<stream>` and a lone `main` as before.
- [ ] Live add and remove work through 0x12/0x13/0x14 with every refusal reason listed above reaching the instrument in words.
- [ ] A poll still allocates nothing: the stream-change list is kept on `Server` like `notes`.
- [ ] Eleven red tests red first (paste output), then green; every existing hub test green (their notices and names unchanged for single-stream instruments).
- [ ] BENCH: none asked; report `hub.rs` and `instrument.rs` line counts before and after. `hub.rs` is 1143 lines: put `seat_stream`, `unseat_stream` and `strip_label` in a new `hub/streams.rs` rather than growing it.

## Code quality

The tree's rules: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; new files under 1000 lines; no TODO, no stubs. The hub thread is real-time: no allocation in `poll_once`'s steady state (seating and unseating allocate, as joining does today, `hub.rs:22–23`). The desk callback is untouched: strips are attached and detached only through the patchbay, so nothing is freed on the audio thread. Every stream's samples are sanitised by `push_block_le`. Red first, then green, one behaviour per commit.

## END

```
START KZ-B03b base=<sha> branch=lane/kz-b03b df=<GB> at <time>
END KZ-B03b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
