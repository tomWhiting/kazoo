# KZ-B04: every `out` on the wall is its own strip on the desk

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename. Depends: KZ-B03b landed (streams on the wire, the link and the hub).

## Outcome

Today the wall sums every `out` into one stereo master and the desk gets one strip for the whole wall (`kazoo-wall/src/engine/mod.rs:1107–1118` the sum, `:805–808` the one `send_audio`; `kazoo-wall/src/daemon/control.rs:432` one link, two channels). When this lane is done, each `out` module is a stream on the wall's link, named by the module's name or, without one, its id (`out1`), and the desk shows a strip for each: `wall/drums`, `wall/out2`. Adding an `out` adds a strip; removing it frees the strip; loading a save does the same for the outs it brings. Stream 0 (`wall/main`) always exists because a link's stream 0 is its clock and cannot go (B03a): it carries the master when the wall has no `out`, and otherwise only what has no stream of its own (an `out` past the link's fifteen named streams), so with outs it is silent. `--no-desk` and the direct device path are untouched: the master is still summed, limited and played as today, and the device is silenced only while the desk takes the wall.

## The design (decided; build exactly this)

### Engine (`kazoo-wall/src/engine/mod.rs`)

- New `pub struct OutStream { pub slot: usize, pub stream: Option<StreamId>, scratch: Option<Box<[f32]>> }`: `stream` and `scratch` are `Some` for an out carried on its own stream (scratch is `MAX_CHUNK_FRAMES * 2` floats, allocated on the control side), `None` for an out summed into main. `Engine` (`:519–567`) gains `outs: Vec<OutStream>` with `Vec::with_capacity(MAX_OUTS)`, `pub const MAX_OUTS: usize = 256`, and `main: Box<[f32]>` (`MAX_CHUNK_FRAMES * 2`, allocated in `engine()`, `:623`).
- `Command` (`:102–157`) gains `OutStream(OutStream)` and `NoOutStream { slot: usize }`. `Retired` (`:159–172`) gains `Scratch(Box<[f32]>)`. `drain_commands` (`:863–913`): `OutStream`: if an entry for `slot` exists, replace it and retire the old scratch; else if `outs.len() == MAX_OUTS` count `shared.ignored` (`:379–383` doc) and retire the scratch; else push. `NoOutStream`: `swap_remove` the entry for `slot`, retire its scratch. `remove` (`:991`) also drops any `outs` entry for the slot (a remove without `NoOutStream` first must not leave a dangling slot).
- `render_frames` (`:791–829`): inside the per-frame loop, after the master carry is read, for each `out` in `outs`: `let feed = self.room.output(out.slot, FEED_LEFT/FEED_RIGHT)` (`room.output` at `:1319` gives the current sub-block's buffer; the out's feed for frame `carry_used` is `feed[self.carry_used]`); a `None` (slot emptied) is 0. Streamed outs write `scratch[frame * 2]`, `[frame * 2 + 1]`; unstreamed outs add into `self.main[frame * 2..]`. Before the loop `self.main[..frames * 2]` is filled with the master when `outs.is_empty()` and zeroed otherwise, so main is "the master, or what has no stream". After the loop: `to_desk = hub.send_audio(frames, &self.main[..frames * 2])` (stream 0; the one result that silences the device, as now, `:805–813`), then for each streamed out `if !hub.send_stream_audio(stream, frames, &scratch[..frames * 2]) { self.shared.streams_unsent.fetch_add(1) }` with a new `Shared` atomic `streams_unsent` and a reader `streams_unsent()`. The `stereo` buffer and the master chain are unchanged for the device, the listen ring and the recording.
- The room scan that sums outs for the master (`:1107–1118`) stays; D02 replaces it with this list. Say so in a comment on `outs`.
- `EngineControl::send` (`:1420`) needs no new reservation: `OutStream` carries its own scratch.

### Control side (`kazoo-wall/src/daemon/wall.rs`; put the new code in a new `daemon/streams.rs`, `wall.rs` is 4628 lines)

- `Wall` gains `out_streams: BTreeMap<String, StreamId>` (module id → stream) and `fn sync_out_streams(&mut self)`. The desk link is `self.desk: Option<HubLink>` (`:4335` reads it). `sync_out_streams`:
  - `wanted`: every module with `kind == Kind::OUT` (`patch.modules()`, `patch.rs:455`), in id order.
  - For each wanted id not in `out_streams` and with `self.desk` present: label = the module's `name` if `Some`, else its id; `link.add_stream(&label, 2)`; `Err(Invalid)` (the name is in use by another out) → try the id as the label; `Err(Full)` (fifteen streams held) → unstreamed. Then `self.engine.send(Command::OutStream(OutStream { slot: self.rack.slot_of(id), stream, scratch }))` with scratch allocated here for a streamed out, `None` otherwise; record the stream in `out_streams` when streamed.
  - For each id in `out_streams` no longer on the wall: `link.remove_stream(stream)` (the engine entry went with `Command::Remove`, see `remove`'s handling above; the `NoOutStream` below covers the ordered case).
  - With no desk (`DeskMode::Off`, `control.rs:427`): `self.desk` is `None` and nothing is sent; outs sum to the master as today.
- Where it is called: in `mirror` (`:1777`) after `What::Add` of an out and after `What::Restore` (both insert a module, `:1485`); before `Command::Remove` in `What::Remove` (`:1830–1840`): first `engine.send(Command::NoOutStream { slot })` for an out's slot, then `Remove`, then `sync_out_streams` (which removes the link stream). After `swap_engine` (`:1392–1402`, a load) once the new patch's modules are inserted. Audio for a stream already removed that is still in the link's ring is dropped by the hub without harm (B03b).
- Strip readback for people: `ModuleView` (`protocol/mod.rs:1264`) gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub strip: Option<u8>`: for an out with a stream, `link.stream_strip(stream)`; the snapshot builder (`:3890` region) fills it. `kazoo-mcp/src/render.rs`'s module line adds ` → desk strip {n}` when present (one line; the MCP crate is in this lane only for that). The console's faceplate title (`tui/draw/rack.rs:453` `title`) shows `·s3` after an out's name when `strip` is `Some(2)` (strips are shown 1-based on the desk, `hub.rs:225`).

### What does not change

`link_to_desk` (`control.rs:421–448`) still makes one link with `LinkConfig::new(DESK_NAME, 2, rate, MAX_CHUNK_FRAMES)`; its stream 0 is `main`. The headless loop (`daemon/audio.rs`), pacing, `desk_owes`, the master chain, hush, listen and record: untouched.

## Not this lane

- Replacing the room scan with the `outs` list for the master sum: KZ-D02.
- More than eight strips on the desk: KZ-C01. More than fifteen named streams per link: not planned (an out past that sums into `main`, stated above).
- Parameter changes from the desk: KZ-B06.

## Red tests first

1. `kazoo-wall/src/engine` tests (the crate's tests run under `assert_no_alloc`, `lib.rs`; the stand-in hub pattern is in `kazoo-arp/src/audio.rs:509–552`: `hub_link` with `HubAddress::Socket`, a `UnixListener`, answer the register with `RegisteredMsg`):
   - `an_out_with_a_stream_sends_its_feed_on_that_stream_and_main_is_silent`: engine with a connected hub; insert a `noise` and an `out`, cable `noise1.out → out1.left` (through `Command::Cables`/`Order` as the existing engine tests do); `Command::OutStream { slot: out's slot, stream: Some(1), scratch }`; render 1,024 frames; the hub end reads frames: every 0x11 frame has stream 1 and a non-zero sample; every 0x10 frame is all zeros.
   - `with_no_out_main_carries_the_master`: a `noise` feeding nothing... there is no master then; instead: an `out` with `stream: None` (`OutStream { stream: None, scratch: None }`); its feed arrives on 0x10 frames (main) and no 0x11 frame is sent.
   - `no_out_stream_stops_its_frames_and_retires_its_scratch`: after the first test, `NoOutStream { slot }`; the retire ring yields `Retired::Scratch`; rendering 1,024 more frames sends no 0x11 frame.
   - `the_device_is_silenced_only_by_stream_zero`: with outs streamed and the hub connected, the device buffer is zero; with the hub end gone, the device buffer carries the master again.
   - `a_257th_out_is_counted_as_ignored_and_its_scratch_retired`.
2. `kazoo-wall/tests/daemon.rs` (the daemon test harness there; start the daemon with `DeskMode::Socket(path)` and a stand-in hub listener in the test):
   - `adding_an_out_registers_a_desk_stream_named_after_it`: after the wall registers (streams `[main]`), `Request::Add { kind: "out", name: Some("drums") }` makes a `MSG_STREAM_ADD` frame arrive with name `drums`, channels 2; the hub end answers READY strip 2; `Request::Look` shows `out1.strip == Some(2)`.
   - `an_out_without_a_name_streams_under_its_id`: `Add { kind: "out", name: None }` → stream named `out2`.
   - `two_outs_with_one_name_stream_under_the_second_ones_id`.
   - `removing_an_out_removes_its_stream`: `Remove { module: "out1" }` → `MSG_STREAM_REMOVE` for its id; `Look` has no `out1`.
   - `loading_a_save_with_two_outs_registers_two_streams`: save a wall with two outs, load it: two `MSG_STREAM_ADD`s (or, for ids the link already holds, none) and no stale ones.
   - `with_no_desk_nothing_is_streamed`: `DeskMode::Off`; adding an out opens no socket and `Look` shows `strip: None`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall -p kazoo-mcp
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-mcp/src
```

Doc tests if doc comments changed: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-wall -p kazoo-mcp`.

## Definition of done

- [ ] `OutStream`, `Command::OutStream`/`NoOutStream`, `Retired::Scratch`, `Engine.outs` and `Engine.main` exist; `render_frames` sends stream 0 and one stream per streamed out.
- [ ] `sync_out_streams` runs after add, restore, remove and load; stream names are the module's name, else its id; a name clash falls back to the id; the sixteenth out is unstreamed and summed into main.
- [ ] `ModuleView.strip` is filled and shown by the MCP and the console.
- [ ] `--no-desk` and the device path behave as before (the sixth daemon test and the fourth engine test).
- [ ] Eleven red tests red first (paste output), then green; every existing wall, daemon and MCP test green.
- [ ] BENCH: the live wall's render load (`Shared::render_load`) with 3 outs streamed versus the master only, from a daemon test rendering 10 s headless: before and after numbers (the per-frame out reads are the only new audio-thread cost).

## Code quality

The tree's rules: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =` (a `send_stream_audio` result that is `false` is counted, never discarded); no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; new files under 1000 lines, `wall.rs` and `engine/mod.rs` not grown beyond the lines this lane needs; no TODO, no stubs. On the audio thread nothing allocates, locks or makes a system call except `Thread::unpark` inside `send_stream_audio` (B01's reason): `outs` has fixed capacity, every scratch is allocated on the control side and retired through the ring. The wall's tests run under `assert_no_alloc`; keep them that way. Every sample sent is sanitised by the link. Red first, then green, one behaviour per commit.

## END

```
START KZ-B04 base=<sha> branch=lane/kz-b04 df=<GB> at <time>
END KZ-B04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
