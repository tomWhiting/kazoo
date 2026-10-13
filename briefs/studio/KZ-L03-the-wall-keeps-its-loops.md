# KZ-L03: the wall keeps every loop in its save as a reel's name and puts it back on load, undo and rebuild; a seat can erase a loop or name a desk strip for it; at most eight loops, and nothing culls one

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: the loop's place in the patch, and the tape thread that moves reels between the store and the loops. Crates: kazoo-wall only (new `src/daemon/loops.rs`, `src/daemon/loops/worker.rs`, `src/daemon/loops/tests.rs`; small additions to `patch.rs`, `migrate.rs`, `store.rs`, `protocol/mod.rs`, `change.rs`, `daemon/wall.rs`). Depends: KZ-L01 (the reel store), KZ-L02 (the loop module), KZ-D03 (sinks; landed at `247ca07`) and KZ-W01a (landed at `779e316`; its `seed` field sits beside the new ones in the same `PatchFile` literals).

Read by Cassio, 14 Oct 00:2x. Changes: the checks are written out in the lane form, and the live-wall listen is Cassio's and Tom's after landing.

**The tree (Track T, PLAN.md) and this lane.** Since this brief was written, Tom decided the wall is one tree versioned in git (KZ-T01 to T04). A loop's line in the patch names its reel by content hash, which is exactly what a tree commit should carry: the tree stores the patch text, never the audio. Two consequences:
- If KZ-T01 has landed when this lane branches, `PatchFile` already has T01's birth-mark fields; add `loops` and `retired_loops` beside them, and every `PatchFile { .. }` literal T01 touched gains the two fields too.
- A reel named by any commit in the tree must never be swept. That is KZ-L04's sweep, not this lane's; it is noted there.

Every test that sets a directory through the environment uses the wall tests' existing helper for the state dir. `std::env::set_var` is `unsafe` in this edition and the workspace forbids unsafe, so if the helper does not cover a second variable, pass the reels directory as a parameter instead (a `Loops::with_reels_dir` used by the tests) and say so under CHANGED.

## Outcome

A wall with loops on it is saved as text. Each loop's line names its reel (`9f3c01ab…`), its length in beats and frames, where it was spliced, which of its bars were written on which pass, and the desk strip it records from, if any. Loading that save, starting the daemon, undoing a removal, or the engine being rebuilt after the audio device changes all put every loop back with exactly the samples it had. The same save renders the same samples every time.

A seat can erase a loop (`wall_loop loop1 erase`; undoable) or tell it which desk strip to record (`from mouth/dry`; KZ-L09 makes that sound). The ninth loop is refused: "a wall holds eight loops, like the eight-track". A loop is always rendered, even with nothing patched out of it, because D03's culling would otherwise stop a loop recording that nobody is listening to yet.

## The design (decided; build exactly this)

### In the patch (`patch.rs`)

- New types, serde, beside `Words` (`patch.rs:178`):
  ```rust
  pub struct LoopRecord {
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub tape: Option<TapeRecord>,     // None: blank tape
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub from: Option<String>,         // a desk strip's label (KZ-L09)
  }
  pub struct TapeRecord {
      pub reel: String,                 // 64 hex, kazoo_core::reels::ReelId
      pub rate: u32,
      pub frames: u64,                  // N
      pub tail: u64,                    // seam tail frames
      pub beats: f64,
      pub anchor: f64,
      pub bars: Vec<Option<i64>>,       // the pass each bar was last written on; one per bar
  }
  pub struct RetiredLoop { pub module: String, pub record: LoopRecord }
  ```
- `PatchFile` (`:121`) gains `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub loops: BTreeMap<String, LoopRecord>` and `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub retired_loops: Vec<RetiredLoop>`, after `retired_speech`. `Patch` gains the same two fields, private, with `loop_record(id)`, `set_loop(id, LoopRecord)` and `loops()`.
- `pub const MAX_RETIRED_LOOPS: usize = 64;`. Removal moves the record to `retired_loops` exactly as `remove` does for speech (`:836`). Restore brings it back.
- `FILE_VERSION` stays 2, following W01a's precedent for `seed`: an older build skips unknown fields. Every `PatchFile { .. }` literal gains the two fields (the same places W01a names: `migrate.rs:119`, `patch.rs:1225`, `:1257`, `:1273`, and the tests).
- `Patch::load` checks every record:
  - `reel` parses as a `ReelId`;
  - `frames ≤ MAX_LOOP_FRAMES`;
  - `tail ≤ 4096`;
  - `beats` is finite, at least 1 and at most 260;
  - `anchor` is finite;
  - `bars.len() == ceil(beats / 4)`, and at most `MAX_LOOP_BARS`;
  - `rate` is between 8,000 and 384,000;
  - `from` passes `valid_from` (below).

  A record that fails is dropped with a note: "loop1's tape was unreadable in the file (…); it is blank". The loop module stays. A record for an id that is not a `loop` module is dropped with a note.
- `pub const WORST_LOOP_BYTES: usize = 2 * (64 + 64 + 6 * 32 + MAX_LOOP_BARS * 24 + MAX_NAME * 6 + 96);` in `store.rs` beside `WORST_WORDS_BYTES` (`:110`), with the same doc pattern. `Wall::file_room` (`daemon/wall.rs:2609`) counts it for an `add` of `loop`. The compile-time assert at `store.rs:125` gains eight loops' worth.

### Requests and changes (`protocol/mod.rs`, `change.rs`)

- `Request::Loop { module: String, op: LoopOp }`, `name() == "loop"`, a change request (it joins the lists at `protocol/mod.rs:432` and `:453` as `Speak` does). The op is one of:
  - `LoopOp::Erase`;
  - `LoopOp::From { from: Option<String> }`.
- `What::Erase { module: String, was: Option<TapeRecord> }`: undo puts `was` back.
- `What::LoopFrom { module: String, from: Option<String>, was: Option<String> }`.
- `change::describe` writes, for the two cases:
  - "erased loop1 (it held 17 bars, reel 9f3c01ab)";
  - "set loop1 to record from mouth/dry", or "… to record what is patched into it".
- `pub fn valid_from(name: &str) -> bool`: 1 to 24 bytes of `[A-Za-z0-9/_.-]`. That covers every label B03b gives a strip (`808/kick`, `wall/out2`) and `master`. `MAX_NAME` is 24 (`protocol/mod.rs:59`).
- An `add` of `loop` when the patch already holds `MAX_LOOPS` loops is refused `ErrorCode::Full`, with "a wall holds eight loops, like the eight-track; remove one, or erase one and use it again". `pub const MAX_LOOPS: usize = 8;` sits in `daemon/loops.rs`. Reasons: eight is the machine Tom grew up beside, and eight full loops are 295 MB of memory at 48 kHz (36.9 MB each, KZ-L02), which is the most this lane lets a wall take for tape.

### The manager (`daemon/loops.rs`), the `Speech` pattern (`daemon/speech.rs:112`)

- `pub struct Loops { feeds: HashMap<String, LoopFeed>, worker: Option<TapeWorker>, waiting: HashMap<String, Waiting>, notes: Vec<String> }`. `Wall` gains `loops: Loops` beside `speech`.
- `TapeWorker` (`loops/worker.rs`): one thread named `kazoo-wall tape` that owns a `ReelStore` (`ReelStore::open(&reels_dir()?)`). A store that cannot open leaves `worker: None`, and every loop then holds only what it records live, which `look` says (KZ-L05).
  - It takes `TapeJob`s over `std::sync::mpsc::sync_channel(64)` and answers `TapeDone` over `mpsc::channel`, read in `Loops::poll` on every `Wall::tick`.
  - The jobs:
    - `Fetch { module, record: TapeRecord, capacity_frames: usize, tail_cap: usize }`: `get` the reel. Check that it has `frames + tail` frames, at `record.rate`, and fits the module's capacity. Allocate a buffer of the module's length `2 × (capacity + tail_cap + 1)`. That allocation happens on this thread: 36.9 MB never touches the control thread, and it is `try_reserve_exact` plus `resize`, so its pages are resident. Copy the reel in. Compute the 64 bins (peak per sixty-fourth, the u8 60 dB scale of KZ-L02).
    - `Blank { module, capacity_frames, tail_cap }`: a zeroed buffer of the module's length.
  - The answers are `Fetched { module, Result<(Box<[f32]>, Splice, [i64; 65], [u8; 64]), String> }` and `Blanked { module, Box<[f32]> }`.
  - Every disk read and every large allocation happens on this thread, never on the control thread and never on the render thread.
- `Loops::attach(module, feed)` is called from `Wall::insert` (`daemon/wall.rs:1473`), which gains a third arm beside `speaks`: `Kind::LOOP` builds with `dsp::looper::build_loop(rate)` (KZ-L02) and attaches the feed.
  - If the patch holds a `TapeRecord` for the module, `Loops::put_back(module, record)` sends `Fetch`.
  - When `Fetched` comes back, the manager installs it into the feed, with `only_if_blank: true` (below).
  - A failure is a note on the console log and in `look`: "loop1's reel 9f3c01ab is missing from ~/.kazoo/reels; it is blank". The record stays in the patch, so restoring the file and loading again brings it back.
- `LoopIn::Install`'s `only_if_blank` (KZ-L02) is `true` for every put-back. The module refuses (`Refused`) a put-back install if it has spliced tape of its own since it was built. A musician who started recording in the 50 ms before the reel arrived keeps what they recorded, and the console says "loop1 was recorded before its saved tape arrived; the saved tape was not put back". Erase and undo install with `false`.
- `Loops::forget(module)` is called from `mirror`'s `What::Remove` arm (`:1872`) beside `speech.forget`. `Loops::replace_wall()` is called from `swap_engine` (`:1413`) beside `speech.replace_wall()`. Both drop the feeds; jobs in flight for them are answered into nothing. The module itself goes back through `Retired::Module` and is freed on the control side, as every module is.
- `Loops::collect()` (in `poll`):
  - drains each feed's `LoopOut`;
  - drops `Retired` buffers here, off the audio thread;
  - turns `Refused` into notes;
  - hands `Took` and `Snapped` to KZ-L04. Until L04 lands, `Snapped` is dropped with no note and `Took` is counted only.

### Mirroring the requests (`daemon/wall.rs`)

- `What::Erase` sets `patch.loops[module].tape = None`, sends `Blank`, and installs the blank buffer when it arrives. The undo is a `What::Erase` reversed: `tape = was`, and `Fetch` installs `was`'s reel with `only_if_blank: false`.
- `What::LoopFrom` sets `from`. Nothing else here: KZ-L09 asks the desk for the return.
- `Wall::insert` for a module restored by undo, by load or by `attach` (an engine rebuilt after a device change) goes through `put_back`, so all four ways back are one path.

### A loop is never culled

In `heard_order` (`daemon/wall/unheard.rs:11`, called from `send_tables`), D03's `sinks` filter (`module.kind == Kind::OUT || probe::measures(module.kind)`, `:21`) gains `|| kind == Kind::LOOP`. D03's `unheard` list and its MCP sentence ("reach no out, scope or readout") gain "or loop". The reason is that a loop must record whether or not anything is listening yet: arming a loop, then patching its output later, is the normal order.

## Not this lane

- KZ-L04: turning a recording into a reel (takes, checkpoints, a save that waits for its loops), the change-log entries for them, pins and the sweep. Until then a recorded loop is kept only while the daemon runs, and a save holds its last fetched reel. That is stated in the `Loops` module doc and removed by L04.
- KZ-L05: `look`, the MCP tool and the console text. KZ-L09: what `from` does.

## Red tests first

In `daemon/loops/tests.rs` and `daemon/wall/tests.rs`. Build the wall offline as W01a's tests do: `Wall::new(.., None, Vec::new(), None)` with an engine at 48 kHz. Use a temporary reels directory through `KAZOO_REELS_DIR` set for the test process. The wall's tests already set the state dir the same way; follow that helper, and never touch `~/.kazoo`. Each test fails today because there is no `loops` field and no `Loop` request.

1. `a_saved_loop_comes_back_with_its_exact_samples`. Put a one-bar reel in the store with `ReelStore::put`, and write a patch whose `loops` names it. Load it, then render 4 bars after the `Fetched` arrives (poll `tick` until the gauge state is `Holding`, within 2 s). The loop's output equals `level ×` the reel's samples on the frames the beat says. Then do it on a second fresh wall: the two renders are bit-identical.
2. `a_missing_reel_leaves_a_blank_loop_and_a_note_and_the_record_stays`.
3. `a_damaged_record_is_dropped_with_a_note`. Use `beats: NaN`, `frames: 10^9`, and `reel: "xyz"`. Each gives its own note, and the module is still there.
4. `erase_then_undo_brings_the_tape_back`. Load the patch from test 1, erase (the output goes silent from the next sub-block after `Blanked`), then undo (the reel's samples return).
5. `remove_then_undo_brings_the_loop_and_its_tape_back`. `retired_loops` holds the record between the two.
6. `the_ninth_loop_is_refused_by_name`.
7. `from_is_kept_in_the_save_and_checked`. `from mouth/dry` is kept. `from "a b"` and a 25-byte name are `BadName`. Undo of `from` gives back the old value.
8. `a_loop_nobody_hears_is_still_rendered`. A loop with nothing patched out of it, recording a constant into its `left`: after one bar and a pass, the gauges show `Holding` and `written` bit 0 set. Without the sink, it would be culled and stay `Armed`.
9. `a_reel_arriving_after_a_recording_began_does_not_overwrite_it`. Hold the worker (a test hook delays `Fetch` by 200 ms). Arm, record a bar, let the fetch land: the module refuses it, the note says so, and the recording plays.
10. `an_engine_rebuilt_after_a_device_change_puts_the_tapes_back`. Call `Wall::attach` as `daemon/wall/tests.rs` does for a device change: the loop holds its reel again.
11. `an_old_save_without_loops_reads_as_before`. The fixture patches in `migrate/tests.rs` load unchanged, with no `loops` key written back.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4
cargo fmt --all --check
ast-grep scan kazoo-wall/src
```

## Definition of done

- [ ] The 11 tests fail first (paste each under RED), then pass.
- [ ] No reel is read, and no buffer over 1 MB is allocated, on the control thread or the render thread. Name in DONE every place a loop's buffer is allocated and freed: it is allocated on the worker and in `LoopModule::new` (build, control side), and freed in `Loops::collect` and in the retire drain.
- [ ] `MAX_LOOPS`, `WORST_LOOP_BYTES`, `MAX_RETIRED_LOOPS`, `valid_from`, `Request::Loop`, `What::Erase` and `What::LoopFrom` exist as named.
- [ ] D03's sinks include `loop`. Name the line.
- [ ] Not the builder's: the builder never touches the live wall (no MCP call, load, save or daemon restart). After landing and reinstalling, Cassio adds a loop, patches a `vco` into it and an `out` from it, records a bar, and asks Tom to listen for the bar coming round. Saving it is KZ-L04's check, because no reel is written until then. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `daemon/wall.rs` is about 4700 lines and `store.rs` 3369: add only the lines named here to them (the `insert` arm, `mirror` arms, `file_room`; the sink is in `unheard.rs`), put everything else in `daemon/loops.rs`, and list both under FOUND with their line counts. The worker's channel is bounded, and a full channel is a note and a retry on the next tick, never a block. No `.ok()`: a send to a worker that has gone is `worker: None` plus a note.

## END

```
START KZ-L03 base=<sha> branch=lane/kz-L03 df=<GB> at <time>
END KZ-L03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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

## Note: KZ-L02 as landed (review head 652404d)
The feed's requests and results travel boxed, one box made on the control side and handed back: `LoopIn::Install { tape: Box<LoopTape>, only_if_blank }`, `LoopIn::Snapshot { into: Box<LoopSnapshot> }`, `LoopOut::Retired(Box<LoopTape>)`, `LoopOut::Refused(LoopIn)`, `LoopOut::Snapped(Box<LoopSnapshot>)`. `LoopFeed::install`, `snapshot` and `collect` keep their signatures. Read every variant above in those shapes; the fields named above are the fields inside the box. A buffer the audio side cannot hand back is leaked and counted in `LoopGauges::leaked`, never freed there. Loops built from the catalogue have no feed until this lane gives them one.
