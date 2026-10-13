# KZ-L07: the desk sends any strip, group or the master back to the instrument that asks, block by block, each stamped with the frame of the asker's own stream at which the desk played it

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: a return tap on the desk callback and its forwarding on the hub thread. Crates: kazoo-mix only (a new `src/returns.rs` and `src/hub/returns.rs`; small additions to `engine.rs`, `patchbay.rs`, `hub.rs`, `hub/instrument.rs`, `shared.rs`, `ui.rs`). Depends:
- KZ-L06 (the wire);
- KZ-B03b (strip labels);
- KZ-C01 (strips as a `Vec`, groups, `StreamKey`);
- KZ-C04 (the post-fader block each strip, group and the master keep per chunk: `strip.out`, the tap point C04 defines);
- KZ-V07b (the hub sends `MSG_DESK_FEATURES`).

Read by Cassio, 13 Oct evening. Changes: the checks are written out in the lane form, and the live check is Cassio's and Tom's after L09 lands. Opens only after L06, B03b, C01, C04 and V07b have landed.

## Outcome

The wall asks the desk for `mouth/dry` (KZ-L09). The desk answers "yes", and from then on every block that strip plays (after its fader and pan, what Tom hears from it) also goes back down the wall's link. Each block is stamped with the frame of the wall's own stream that was playing at the same studio frame. Asking for a strip that does not exist gets "no strip called mouth/drie" back in words. A strip forgotten with `D` while returned says "mouth/dry was taken off the desk" to whoever asked for it. Nothing on the desk's callback waits, allocates or frees for it, and a return that cannot keep up drops blocks and counts them where they can be seen. The strip head shows `ret` while a strip is being sent back, so Tom can see where his voice is going.

## The design (decided; build exactly this)

### Caps and rings

- `pub const DESK_RETURNS: usize = 8;` in `shared.rs`, desk-wide. Reason: the wall's eight loops are the largest user, and each tap costs one ring of 384 KB (`RETURN_RING_SECONDS = 1.0`, L06), so eight is 3 MB, allocated when asked, not at start.
- A tap's ring is `audio_block_ring(AudioRingConfig::new(BufferId(1000 + tap), 2, RETURN_BLOCK_FRAMES, blocks))`, with `blocks = ceil(rate × RETURN_RING_SECONDS / 256)`. It is built on the hub thread when an ask is accepted.

### The callback (`engine.rs`, `patchbay.rs`)

- `PatchCommand::Tap { tap: usize, source: TapSource, producer: AudioBlockProducer }` and `PatchCommand::Untap { tap: usize }`, where `TapSource` is `Strip(usize)`, `Bus(usize)` or `Master`. `PatchEvent::Untapped { tap, producer }` hands the producer back to the hub thread to be dropped there.
- `MixerEngine` gains `taps: [Option<Tap>; DESK_RETURNS]`, set and cleared only through the patchbay, so nothing is allocated or freed on the callback.
- After a chunk's samples are final, for each tap, push the source's post-fader stereo block as one or more `AudioBlock`s of at most 256 frames, with `start_frame` = the studio frame of their first sample.
  - For a strip that is C04's `strip.out`; for a group, the group's post-fader sum; for the master, the limited output.
  - **A strip's tap is after its fader and pan but before its mute and solo**, decided 13 Oct by Cassio. The engine already renders a muted strip (`render_into(.., audible)`, `engine.rs:503`), so the tap takes the block whether or not the room hears it. Reason: mute means "not in the room"; a return feeds a recorder. KZ-L08 seats Tom's dry voice muted so the laptop's mic and speakers cannot feed back, and a loop must still be able to record it. Red test 1 gains a case: A's strip muted, and B still receives its post-fader sound.
  - These are the same three places C04 pushes its deck lanes from: push from there, once per chunk, never per frame.
  - A `Full` ring adds one to `SharedState::return_dropped[tap]` (an `AtomicU64` per tap).
  - A disconnected strip (C01) renders silence, and its tap sends that silence: a return keeps the asker's timeline whole.

### The hub (`hub/returns.rs`)

- `Inbound::ReturnAsk(ReturnAskMsg)`, `Inbound::ReturnDrop(ReturnId)` and, for KZ-L10, `Inbound::LoopTake(LoopTakeMsg)` are decoded in `Instrument::handle` (`hub/instrument.rs:209`), beside B03b's stream arms, with the same `need(..)` length checks. KZ-L10 handles `LoopTake`. Until then, this lane decodes it and drops it, with `HubNotice::LoopTake` counted and nothing else, so an instrument that sends it is not disconnected.
- `seat_return(index, ask)`:
  1. Resolve `source`:
     - a strip, live or disconnected, whose B03b label equals it. Either the full label (`kazoo-mouth/dry`, as B03b's notices carry it) or the shown one (`mouth/dry`, `display_name(instrument)/stream`) matches. For an instrument with one `main` stream, the lone name matches (`808`, `kazoo-808`). Reason: Tom and Claude type what the desk shows, and a program may know the full name;
     - `G1`–`G4` for the groups;
     - `master`.
  2. Refuse, with `MSG_RETURN_READY` `ok: 0` and the reason in words, when:
     - the name is unknown ("no strip called <name>");
     - the asker already holds `MAX_RETURNS` returns ("<instrument> already has four returns");
     - the id is in use ("return <id> is already asked for");
     - all `DESK_RETURNS` taps are busy ("the desk is sending eight returns already").
  3. Otherwise build the ring, send `Tap`, record `ReturnSeat { instrument, id, tap, source, sent: u64, dropped: u64, unplaced: u64 }`, and answer READY `ok: 1`.

  The `Vec<ReturnSeat>` has capacity `DESK_RETURNS`, reserved at start, so a poll allocates nothing (B03b's rule for streams).
- Forwarding, in `poll_once` (`hub.rs:598`) after `read_instruments`: for each seat, `pop_block` into a pre-allocated 512-float scratch until `Empty`. Map the block's `start_frame` to the asker's stream frame with its stream 0 stamp: `instrument.stamp().stream_at(start_frame)`, the mapping `song_sync` uses (`hub.rs:1164`).
  - `None` (the asker is not on the studio clock yet) counts `unplaced` and drops the block.
  - Otherwise encode the 16-byte head and the samples LE into the instrument's outbox as one `MSG_RETURN_AUDIO`.
  - An outbox that refuses for room counts `dropped` and drops the block: the asker's other traffic must never wait behind a return.
  - The hub thread wakes on `HUB_TICK` as well as on sockets (`hub.rs` module doc). At the tick it forwards whatever the taps hold, so returns flow while no instrument sends anything. Reason: a desk with only the mouth and the wall still sends the voice back.
- `unseat_return`: on `ReturnDrop`, when the asker leaves (`remove`, `hub.rs:1016`), and when the source strip is forgotten (C01's `Forget`). In the last case, send READY `ok: 0` "<name> was taken off the desk" first. Each sends `Untap`, then drops the seat and its consumer here. The producer comes back in `Untapped` and is dropped here.
- `MSG_DESK_FEATURES`, sent by V07b after `MSG_REGISTERED`, gains `FEATURE_RETURNS`. `FEATURE_LOOP_TAKES` is added by KZ-L10, not here, because until L10 the desk only drops takes.
- `HubSnapshot` (`hub.rs:312`) gains `returns: u64` (seats held), `return_sent`, `return_dropped` and `return_unplaced` (sums).

### The desk's face (`ui.rs`, `desk.rs`)

The strip head (`draw_strip_head`) shows `ret` in `TEXT_DIM` after the status while any return taps that strip, group or the master. The help (`ui.rs`'s help) gains one line: "ret: this strip's sound is being sent back to an instrument (the wall's loops)". The footer shows `ret drop 1.2k` in `AMBER` through `compact_count` (`ui.rs`) when any tap's `return_dropped` or the hub's `return_dropped` grows. No meter is added: function only.

## Not this lane

- KZ-L06: the wire and the link. KZ-L09: the wall asks and records. KZ-L10: the desk's handling of `LoopTake`.
- Sending a return to the instrument that owns the strip (the wall asking for `wall/out1`) is allowed and works the same way: it is how a loop records the wall's own mix as the desk hears it. No feedback guard is needed, because a return is never mixed back into the desk.

## Red tests first

In `kazoo-mix/src/hub/tests.rs` with the `Rig` (`:27–97`) and B03b's `connect_streams`. Each test fails today because there is no `Tap`.

1. `a_return_carries_the_strips_post_fader_sound_on_the_askers_frames`:
   - instrument A has a stream `dry` (mono) sending a 0.5 constant;
   - instrument B (`wall`, a plain stereo stream) asks for `A/dry` and gets READY ok;
   - both send audio for 0.5 s of desk time. B's hub end receives `0x17` blocks.

   Their samples equal A's strip output at 0 dB, pan centre (the pan law's constant, `pan_gains`, `engine.rs`, times 0.5, within 1e-6). Each block's `stream_frame` equals `B.stamp().stream_at(block.studio_frame)`, checked through a test accessor. Consecutive blocks' frames differ by their lengths.
2. `asking_for_a_strip_that_is_not_there_says_so`.
3. `the_fifth_return_of_one_instrument_and_the_ninth_of_the_desk_are_refused`.
4. `a_dropped_return_stops_and_its_rings_are_freed_off_the_callback`. Drop the return: no more frames arrive. The `Untapped` producer is dropped on the hub thread, proved by `Arc::strong_count` on a ring held in the test.
5. `the_asker_leaving_frees_its_taps`.
6. `forgetting_a_returned_strip_tells_the_asker`.
7. `returns_flow_with_no_instrument_sending`. Only the asker is connected, asking for `master`. Blocks arrive on the hub's tick.
8. `the_desk_announces_returns`. FEATURES includes `FEATURE_RETURNS`.
9. `a_loop_take_is_not_a_reason_to_disconnect`. B sends a `0x50`, and stays connected.
10. Engine test (`engine.rs` tests, fake clock): `a_tap_pushes_the_post_fader_block_once_a_chunk_and_allocates_nothing`. Wrap the render in `assert_no_alloc::assert_no_alloc`. kazoo-mix has no `assert_no_alloc` today, so add it as a dev-dependency at kazoo-wall's version, with kazoo-wall's `#[cfg(test)] #[global_allocator]` setup (`kazoo-wall/src/lib.rs`).

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-mix -j 4
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] The ten tests fail first (paste each under RED), then pass, and every existing hub and engine test passes.
- [ ] The callback allocates, frees and locks nothing for taps (test 10). Every ring is built and dropped on the hub thread.
- [ ] `DESK_RETURNS`, `TapSource`, `PatchCommand::Tap` and `Untap`, `ReturnSeat` and `FEATURE_RETURNS` on the desk exist as named.
- [ ] Not the builder's: after L09 has also landed, Cassio runs the desk, the mouth and the wall, points a loop at `mouth/dry`, checks the strip shows `ret`, and asks Tom to sing a bar and listen for it coming round on the bar. The builder never opens Tom's mic or speakers or touches the live wall, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `hub.rs` is about 1230 lines: the new hub code goes in `hub/returns.rs`, and `hub.rs` gains only the calls. `engine.rs` is about 2050: the tap code goes in `src/returns.rs`. List both under FOUND with their line counts.

## END

```
START KZ-L07 base=<sha> branch=lane/kz-L07 df=<GB> at <time>
END KZ-L07 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
