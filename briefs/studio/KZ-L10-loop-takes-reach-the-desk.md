# KZ-L10: every take a wall loop finishes reaches the desk, where it is the same reel as a clip on a `loops` strip, to place on the song, repeat, mix and record

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: the take notice from wall to desk, and the desk's loop clip. Crates: kazoo-wall (one call in `daemon/loops/takes.rs`) and kazoo-mix (new `src/loops.rs`; additions to `hub/instrument.rs`, `hub.rs`, `desk.rs`, `tape.rs` (C04's deck), `takes.rs` (C06), `session.rs` (C08a), `ui.rs`). Depends:
- KZ-L04 (takes);
- KZ-L06 (`MSG_LOOP_TAKE`);
- KZ-L07 (the hub already decodes it);
- KZ-L01 (the desk reads reels);
- KZ-C06 (take players, the `T` list, `TakeClip`);
- KZ-C08a (`session.json`, `HubCommand::Reserve`).

Read by Cassio, 13 Oct evening. Checked: anchor 0, 68 beats, take at beat 140 gives a pass start of 136 and repeats at 136, 204 and 272. Changes: the checks are written out in the lane form, and the live check is Cassio's and Tom's. Opens only after L01, L04, L06, L07, C06 and C08a have landed.

## Outcome

Tom records four bars into `loop1` on the wall. The desk's footer says "loop1-003: 17 bars from the wall". A strip called `loops` shows the take in its `T` list as `loop1-003 ↻ 17b ×2` (17 bars, recorded over 2 passes). `Enter` places it on the song where it was recorded, as two repeats. `>` and `<` add and take away repeats. Each repeat plays the very samples the wall plays, seam crossfade and all, through the `loops` strip's fader, EQ and inserts. Everything the desk records (C04's master take) has it. The session keeps it by its reel's name, and pins it, so the wall's sweep never deletes a loop the session still uses.

The wall goes on playing its own loop live on its own out. The desk's clip is a copy to arrange, and the desk does not play it until Tom places it, so nothing is doubled unless he asks.

## The design (decided; build exactly this)

### The wall sends it (`kazoo-wall/src/daemon/loops/takes.rs`)

After a take with `why: Punch` is logged (L04), and only then, send `desk.send_loop_take(&LoopTakeMsg { .. })` (L06):
- `label`: the module's name, or its id;
- `reel`: the take's reel id;
- `frames`: N;
- `rate`;
- `beats`;
- `anchor_beat`;
- `take_beat`: the beat of the first written bar's start;
- `passes`: `ceil((mark.beat − take_beat) / beats)`, at least 1.

Checkpoints and save takes are not sent: they are snapshots of a recording still going, not something Tom finished. `NotConnected` and `Unsupported` are counted in `LoopView.takes_unsent` (L05's view gains it) and noted ("the desk did not get loop1's take: no desk"). They are never dropped silently.

### The desk takes it (kazoo-mix)

- **Hub** (`hub/returns.rs`, where L07 left `LoopTake` dropped): `Inbound::LoopTake(msg)` becomes `HubNotice::LoopTake { from: <instrument label>, take: LoopTakeMsg }`. `MSG_DESK_FEATURES` gains `FEATURE_LOOP_TAKES`.
- **The `loops` strip.** On the first loop take, the desk reserves a strip with C08a's `HubCommand::Reserve { slot: <next free>, stream: "desk/loops", name: "loops" }`. No instrument can register under it: `desk` is not an instrument name, and B03b's labels are `<instrument>/<stream>`, so B03b's registration refuses `desk` as an instrument name. Add that refusal here, in `register_pending`, with "the name desk is the desk's own".
  - The strip is C01's `Disconnected { key }` as far as the engine is concerned: no live source, and takes play through it (C06).
  - `strip_status` (`ui.rs`) shows `tape` for that key instead of `off`.
  - It is saved and restored with the session like any strip (C08a).
  - When the desk is full (C01's `MAX_STRIPS`), the take is refused with a footer note, "no strip free for loops", and kept in the list of unplaced takes until a strip frees.
- **Loading** runs on C04's deck thread, never the UI thread: a reel is up to 36.9 MB.
  1. `DeckCommand::LoadReel { reel, label, frames, tail_hint }` calls `kazoo_core::reels::ReelStore::get` (L01).
  2. It bakes the seam: the clip's first `tail` frames become `cos θ · reel[N + i] + sin θ · reel[i]`, with `θ = (i / tail) · π/2`, exactly the wall's read at speed 1 (KZ-L02). `tail = frames_in_reel − N`. So repeats laid end to end on the desk join exactly as the wall's passes do.
  3. It resamples each side with `resample_mono` when the reel's rate is not the desk's (C06's rule).
  4. It answers `DeckNotice::Reel { reel, Ok((ClipData, ClipData)) | Err(String) }`.

  A missing or damaged reel is a footer note with L01's sentence, and the take stays listed as missing.
- **The loop clip** (`src/loops.rs`): `LoopClip { take: String /* loop1-003 */, reel: ReelId, label: String, beats: f64, first_beat: f64, repeats: u8, clips: Vec<ClipId> }`.
  - `first_beat` is the pass start at or before `take_beat`: `anchor_beat + floor((take_beat − anchor_beat) / beats) × beats`.
  - Placing it places `repeats` C06 `TakeClip`s that share the two `Arc`s, at `first_beat + i × beats`, through the `TakeBay`.
  - The default `repeats` is `passes`.
  - `pub const MAX_LOOP_REPEATS: u8 = 16;`. Reason: C06 gives a strip 64 takes (`MAX_TAKES_PER_STRIP`), so four loop clips at full repeat fit the `loops` strip, and 16 repeats of a 4-bar loop is a 64-bar section.
  - On the desk the clip plays at its recorded rate and does not follow a later tempo change (C06's rule for every take). The wall's loop does follow, like tape. Say so in the help line.
- **Keys**, in C06's `T` list on the `loops` strip:
  - `Enter` places;
  - `>` and `<` change repeats, 1 to 16, re-placing through the bay;
  - `d` removes all its repeats.

  A loop take's row reads `loop1-003 ↻ 17b ×2`, plus `placed` when placed. Take names number per label, as C04 numbers per slug, without reuse.
- **The session** (C08a's `session.rs`): `TakeSave` gains `#[serde(default, skip_serializing_if = "Option::is_none")] loop_clip: Option<LoopClipSave { reel, label, beats, first_beat, repeats }>`. On load, each is fetched as above. On every session write, the deck thread writes the pin list `ReelStore::pin("session-<name>", every reel the session names)` (L01), after `session.json` is written. Reason for that order: a reel is never unpinned while a file that names it is on disk. A pin write that fails is a header alert, as a session write failure is.

## Not this lane

- Drawing loop clips on the timeline: KZ-L11. Sending a desk clip *to* a wall loop: not in this track. The wall records any desk strip itself (KZ-L09), so a second way in would be two ways to do one thing.
- The separate tape app of `recording-path`: the desk's master take (C04) already records whatever the `loops` strip plays.

## Red tests first

Each fails today because no take reaches the desk.

1. kazoo-wall `daemon/loops/tests.rs` `a_punch_take_is_sent_to_the_desk_and_a_checkpoint_is_not`. The stand-in hub announces `FEATURE_LOOP_TAKES` and receives one `0x50` per punch, byte for byte as specified, and none for a checkpoint.
2. kazoo-wall `a_take_with_no_desk_is_counted_and_said`.
3. kazoo-mix `hub/tests.rs` `a_loop_take_becomes_a_notice`.
4. kazoo-mix `hub/tests.rs` `nobody_may_register_as_the_desk`.
5. kazoo-mix `tape.rs` (deck) `a_reel_is_loaded_with_its_seam_baked`. A reel with a known end, tail and start: the clip's first `tail` samples equal the formula, and the rest are bit-identical to the reel. Resampled when the rates differ: the length scales.
6. kazoo-mix `loops.rs` `a_loop_clip_places_its_repeats_from_the_pass_start`. With `anchor` 0, `beats` 68 and `take_beat` 140: `first_beat` 136, and three repeats at 136, 204 and 272.
7. kazoo-mix `the_seventeenth_repeat_is_refused`.
8. kazoo-mix engine test (C06's fake clock) `repeats_join_without_a_step`. Two repeats of a clip whose reel has a step at its seam: at the join, the largest sample-to-sample step is at most 1.5 times the signal's own largest step.
9. kazoo-mix `session.rs` `a_loop_clip_round_trips_through_the_session_and_is_pinned`. Save, load and compare. `pins/session-<name>.txt` names the reel and ends with `end`.
10. kazoo-mix `the_loops_strip_shows_tape_not_off`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mix --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -p kazoo-mix -j 4
cargo fmt --all --check
ast-grep scan kazoo-wall/src/daemon/loops kazoo-mix/src
```

## Definition of done

- [ ] The ten tests fail first (paste each under RED), then pass.
- [ ] No reel is read on the UI thread or the callback. Name the deck command that reads it.
- [ ] Not the builder's: after landing and reinstalling, Cassio records a loop on the wall with the desk running, checks that the footer names the take and `T` on `loops` lists it, places and plays it, and asks Tom whether it plays in step with the wall's own loop (the two together should sound like one, slightly louder); then restarts the desk with the same session and checks the clip is still there. The builder never touches the live wall or Tom's speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The desk's new code goes in `src/loops.rs`. `desk.rs` (about 1535 lines) and `ui.rs` (about 1890) gain only the key arms, the footer line and the `tape` status. List both under FOUND with their line counts.

## END

```
START KZ-L10 base=<sha> branch=lane/kz-L10 df=<GB> at <time>
END KZ-L10 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
