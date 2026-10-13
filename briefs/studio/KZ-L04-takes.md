# KZ-L04: every recording a loop finishes becomes a reel and a take in the wall's change log, which can be undone; a long recording is checkpointed; a save waits for its loops; and the reel store is pinned, swept and held to its budget

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: the path from a finished recording to a durable, logged, undoable reel, and keeping the store tidy. Crates: kazoo-wall only (`daemon/loops.rs` and a new `daemon/loops/takes.rs`, `daemon/loops/pins.rs`; small additions to `protocol/mod.rs`, `change.rs`, `daemon/wall.rs`, `daemon/control.rs`). Depends: KZ-L03.

Read by Cassio, 13 Oct evening. Changes: the checks are written out in the lane form; the live-wall check is Cassio's and Tom's after landing; and one pin source is added for the tree, below.

**The tree keeps every take (Track T, PLAN.md).** Tom decided the wall is one tree in git that keeps every moment forever, and that going back to a moment (KZ-T04b) and replaying (KZ-T06, "the fire") must work for any moment. The log rotates and saves are pruned, so the five sources below would one day let the sweep delete a reel that a moment in the tree still names. So, decided:
- `pins/wall-history.txt` is a sixth pin source, written by this lane. Every `Take` whose `why` is `Punch`, `Save` or `Rebuild` appends its reel id there (one hex id a line, fsynced, the append done on the worker before the change is told to seats, like the main pin). It is append-only: nothing in the wall ever rewrites or shortens it.
- Checkpoint reels are not added to it. They stay prunable under the budget rule below. When the tree goes to a moment whose checkpoint reel was pruned, that loop is blank and the wall says "that checkpoint's reel was pruned for space at <time>", as an undo would.
- `ReelRefs` reads the file at start, and the pin the worker writes is the union of the six sources.
- Red test 7 gains a reel named only in `pins/wall-history.txt`: the sweep keeps it.
- This holds whichever of Track T and this lane lands first: the file is the tree's promise and needs nothing from T.

## Outcome

Tom sings four bars into `loop1` and turns `record` off. A moment after the post-roll, the log says "kazoo-wall: loop1 took 4 bars (pass 0, reel 9f3c01ab)". The take is change 812, and it is in `patch.json`. Undoing change 812 puts back what `loop1` held before: blank tape, or the take before. A `wall_save` waits up to five seconds for every loop that is recording to hand over its tape, then saves, so a save is the music at the moment it was asked for. A loop left overdubbing for an hour is checkpointed at its seam once a minute, so a crash loses at most a minute. The reel store never grows past 4 GiB because of checkpoints. It never deletes a reel any save, the log or a desk session names. It never deletes anything while it cannot read every pin list.

## The design (decided; build exactly this)

### The take, as a change (`protocol/mod.rs`, `change.rs`)

- `What::Take`:
  ```rust
  Take {
      module: String,
      tape: TapeRecord,              // what the loop holds after the take (reel, splice, bars)
      was: Option<TapeRecord>,       // what it held before: undo puts it back
      frame: u64,                    // the engine frame the snapshot is of (TakeMark::frame)
      beat: f64,                     // the wall's beat at that frame
      written: Vec<u16>,             // the tape bars written since the last take
      why: TakeWhy,                  // Punch | Checkpoint | Save | Rebuild
  }
  ```
  `TakeWhy` is snake_case serde. The change is logged by the seat `kazoo-wall` (`LOADER_SEAT`, `daemon/wall.rs:49`) through `commit_own` (`daemon/wall.rs:1995`), as the wall's own recording changes are (`What::Record`, `protocol/mod.rs:778`). It is told to every seat at the next tick.
- Unlike `Record`, a take can be undone, by any seat that may undo. The undo is a `What::Take` with `tape` and `was` swapped, `why` kept, `written` empty, and `undoes` set. Its mirror installs `was`'s reel (or a blank) with `only_if_blank: false`, through L03's `Fetch` and `Blank`.
- `describe`:
  - "loop1 took 4 bars (pass 0, reel 9f3c01ab)";
  - "loop1 was checkpointed while recording (pass 37, reel 01be77c2)";
  - "loop1 was taken for the save `rain brass 2`";
  - "loop1 was kept before the audio restarted".

  Undone, a take reads "… undid change 812: loop1 is back to reel 77aa0912", or "… back to blank tape".
- **What W02 replays.** A take records the frame and beat its snapshot is of, and the reel it made. Replay that installs `tape`'s reel at `frame` reproduces the loop exactly from that frame on, whatever it recorded from: the wall's own signals, or a desk return the log could never hold. Between a punch-in and its take, a replay plays what the wall alone would record. Write this in the `What::Take` doc comment, for KZ-W02 to build on. This lane does not replay.

### From finished recording to take (`daemon/loops/takes.rs`)

1. `LoopOut::Took(mark)` arrives in `Loops::collect`. The manager asks the worker for a spare buffer of the loop's length (`TapeJob::Spare { module, len }`, allocated on the worker as L03's `Blank` is, but not zeroed: `try_reserve_exact` plus `resize`), then sends it into the feed as `LoopIn::Snapshot { into }`. Reason for asking after the post-roll rather than keeping a spare ready: a spare per armed loop would double the eight loops' 295 MB. The snapshot begins within a few tens of milliseconds. A new punch-in cannot start before the next tape bar line, at least 0.8 s away even at 300 BPM, so the snapshot is still the take. When it is not (a snapshot that began after a new recording started), it is stamped with its own frame, and the take is exact for that frame.
2. `LoopOut::Snapped { tape, at, splice, bar_pass }` arrives. `TapeJob::Store { module, rate, samples: tape, frames: N + tail, why, mark: at }`. The worker calls `ReelStore::put(rate, &samples[..2 × (N + tail)])` and answers `Stored { module, Result<ReelId>, splice, bar_pass, why, mark }`. It hands the buffer back to be freed on the control side, never kept.
3. `Stored(Ok)`: build the `TapeRecord`, log `What::Take` via `commit_own`, set `patch.loops[module].tape`, and update the pins (below). `Stored(Err)`: a note on the console and in `look` ("loop1's take could not be kept: <error>; it is still playing, and is only in memory"). A `put` that is `NotFinite` is a bug in L02 and says so.

### Checkpoints

While a loop's gauge state stays `Rolling`, the manager reads the gauges' `pass` each tick. At the first pass change at least `CHECKPOINT_SECONDS` after that loop's last take or checkpoint, it starts steps 1 to 3 with `why: Checkpoint`. The snapshot is taken mid-recording: L02's copy-on-write keeps it exact.

`pub const CHECKPOINT_SECONDS: u64 = 60;`. Reason: a crash or power cut loses at most a minute of an endless overdub. The disk cost is bounded by the loop: one reel per pass at most, and at most one a minute. A 96 s loop writes 36.9 MB every 96 s (23 MB/minute); an 8-bar loop at 120 BPM writes 6.1 MB a minute. The budget below caps the total.

### A save waits for its loops (`daemon/wall.rs` `save_as`, `:3069`)

- A named save (not `before-…`) finds every loop whose state is `Rolling` or `PostRoll`, or that has a `Took` without a take yet. For each, it starts steps 1 to 3 with `why: Save`.
- The request is answered later: `Outcome.pending` (`daemon/wall.rs:427`, as `speak` does). It is answered by a new `Wall::saved()`, polled beside `Wall::spoken()` in `control.rs:665` (`answer_speech`'s pattern), once every one of those loops has logged its take. Then the save is written as today.
- `pub const SAVE_WAIT: Duration = Duration::from_secs(5);`. A loop not back by then (an engine that has stopped rendering) is saved with its last take. The answer says "loop1's newest recording was not ready in 5 s; the save holds its take from 12:04:31". Reason for five: a full-length snapshot takes 0.38 s and its write well under a second on Tom's SSD, so five is ten times the need, and still short enough that a seat is never left waiting.
- Loads do not wait. `keeping()` (the `before-…` save a load writes, `daemon/wall.rs:3320`) holds each loop's last take, and the load's answer names every loop that was recording: "loop1 was recording: up to 41 s since its last take are not in before-0412". Reason: the load path is already the wall's most delicate (`undo_load`, `:3268`). A loop recording at the moment of a load is rare, and it is said, not hidden.
- On `Wall::attach` (`daemon/wall.rs:1252`, an engine rebuilt after the audio device changed), the old modules have stopped. Each loop is put back from its last take (L03), and a note says how much was lost, from the gauges' last pass and the last take's frame.

### Pins, the sweep and the budget (`daemon/loops/pins.rs`)

- `ReelRefs`: the set of reel ids the wall names, from five sources:
  - the patch's `loops` and `retired_loops`;
  - every save's `loops` and `retired_loops`, read through `store.read_save`. Saves are bounded by `AUTO_SAVES_MOST` (`store.rs:161`) plus the named ones. Each is read once at start and again when it is written; the wall forgets a save's references when it is deleted;
  - every `Take` (`tape.reel` and `was`) and `Erase` (`was`) in `log.jsonl` and `log.1.jsonl`, re-read at start and after each rotation (`store.rs:1659`), and added to on each append.

  When the set changes, `TapeJob::Pin { ids }` has the worker call `ReelStore::pin("wall", ids)`. The pin is written before the change that added a reel is told to seats, so no other program can see a take whose reel is not yet pinned.
- **The sweep**: `TapeJob::Sweep` at daemon start (after the first pin), and after every `prune_auto_saves` (`daemon/wall.rs:3376`). `ReelStore::sweep(SWEEP_GRACE, now)`. A `PinUnreadable` is a note naming the holder and deletes nothing (L01).
- **The budget**: after each `Stored`, the worker reads `usage()`. Above `REELS_BUDGET_BYTES` (4 GiB, L01), it prunes checkpoints oldest first until usage is under 90% of the budget. A checkpoint is pruned only if every one of these holds:
  - its take's `why` is `Checkpoint`;
  - it is named by no save, not by the patch, and not by `retired_loops`;
  - `pinned_elsewhere("wall")` does not hold it;
  - it is not the newest take of its loop.

  A pruned checkpoint's take line stays in the log. Undoing a later take that would return to it says "that checkpoint's reel was pruned for space at <time>", and changes nothing. If the store is still above budget, the wall stops writing checkpoints, sets `checkpoints_paused`, and notes "the reel store is full (4.0 GiB); checkpoints are paused; takes and saves still write". Punch-out takes and saves always write, because they are what the musician asked for. Paused checkpoints resume when usage falls under 90%.

## Not this lane

- Showing takes and the paused flag in `look`: KZ-L05. Sending takes to the desk: KZ-L10. Replaying the log: KZ-W02.

## Red tests first

In `daemon/loops/tests.rs` and `daemon/wall/tests.rs`, offline walls with a temporary reels directory, as in L03. Each test fails today because there is no `What::Take`.

1. `a_finished_recording_becomes_a_logged_take_with_its_exact_samples`. Record one bar of a deterministic `vco` into `loop1` and punch out. Within 2 s of ticking, the log's last change is a `Take` by `kazoo-wall`, its reel exists, and `ReelStore::get` of it equals the module's tape bit for bit. Compare against a `Snapshot` taken by the test at the same frame through the feed.
2. `undoing_a_take_puts_back_what_was_there`. Take A, then take B: undo B and the loop plays A. Undo A and it is blank.
3. `a_long_recording_is_checkpointed_once_a_minute`. Record continuously on a 1-bar loop at 120 BPM with `CHECKPOINT_SECONDS` lowered through a test-only constructor parameter, never a global: checkpoints land on seams at least that far apart, and each reel matches its frame.
4. `a_save_waits_for_a_recording_loop_and_holds_its_newest_tape`. Mid-recording, `save`: the answer is pending. When it comes, the save's `loops.loop1.tape.reel` equals the newest take, and that take's frame is after the request.
5. `a_save_that_cannot_wait_says_so`. Stop the engine rendering (the test does not call render). The save is answered at 5 s with the old take and the sentence.
6. `a_load_names_the_loops_that_were_recording`.
7. `pins_name_every_reel_the_wall_names_and_the_sweep_keeps_them`:
   - set up reels named by the patch, by a save, by a log `Take`, and by nothing;
   - write a `pins/session-jam.txt` naming a fifth reel;
   - make every reel 2 days old and run the sweep.

   Only the unnamed reel is deleted.
8. `checkpoints_are_pruned_for_space_but_takes_and_saves_never_are`. Use a test budget of 3 reels' bytes, again a constructor parameter. Six checkpoints, one punch take and one save: the oldest checkpoints go, and the take, the save's reel and the newest checkpoint stay. Push past it with saved reels: `checkpoints_paused` is set, and the next punch still writes.
9. `a_take_whose_reel_cannot_be_written_says_so_and_the_loop_plays_on`. Make the reels directory read-only.

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

- [ ] The nine tests fail first (paste each under RED), then pass.
- [ ] Every reel write is on the worker thread. Every buffer handed back is freed on the control side. Name the lines.
- [ ] `What::Take`, `TakeWhy`, `CHECKPOINT_SECONDS`, `SAVE_WAIT`, `ReelRefs`, `Wall::saved` and `checkpoints_paused` exist as named.
- [ ] Not the builder's: the builder never touches the live wall (no MCP call, load, save or daemon restart). After landing and reinstalling, Cassio records a loop and sees the take in `wall_log`; saves it, loads another save and loads it back, and asks Tom to listen; undoes the take and asks Tom to listen for the tape before it; and checks that `~/.kazoo/reels` holds one file per take and that `pins/wall.txt` and `pins/wall-history.txt` name them. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `daemon/wall.rs` gains only the `save_as` pending branch, the `Wall::saved` poll and the `attach` note. Everything else is in `daemon/loops/`. No `.ok()` on any worker send or store call: each failure is a note that names the loop and what was lost.

## END

```
START KZ-L04 base=<sha> branch=lane/kz-L04 df=<GB> at <time>
END KZ-L04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
