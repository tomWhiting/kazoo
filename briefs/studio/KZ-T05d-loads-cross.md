# KZ-T05d: a load of a save, and the undo of a load, crosses over 16 beats as a go does, so reaching a moment sounds the same whichever verb reached it

Line numbers are from main at `004bac8` and may have moved; find each item by what it does, never rename. Items from KZ-T01 to KZ-T05c and KZ-D03b are named as those briefs name them (`Birth`, `What::Go::beats`, `DEFAULT_CROSS_BEATS`, `MAX_CROSS_BEATS`, `CrossTiming`, `CrossPlan`, `crossing_beats`, `cross`, `Wall::cross_plan`, `load_room_as`, …); none had landed when this was written, so use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Written by Cassio, 13 Oct evening, on Waffles' reading of T05b. T05b left loads as cuts ("a load is not a moment"), but a save is a mark on the tree (T04a: `refs/tags/save/*`), and Tom's word tonight is that moving between branches glides rather than cuts. So putting up a save by `wall_load` and reaching the same moment by `wall_go` must sound the same. Decided under rule 13 (the numbers a brief leaves open are ours, with a reason); Tom may overrule.

Time: about 35 minutes of a builder's work. One mechanism: `replace` crosses for a save and for an undone load, as T05b made it cross for a moment. Crates: kazoo-wall, kazoo-mcp (words and a test literal only). Depends on T05c, which must have landed before this lane opens.

The length is not chosen yet in this lane: every load crosses over `DEFAULT_CROSS_BEATS`, and its undo over the beats the load took, as T05b did for a go before T05c gave the go its `beats`. T05e gives `wall_load` and the console's load question their `beats` (0 for a cut).

Files (eleven: the last four change only a struct literal or a sentence that would turn false; a lane under ten files cannot add a field to `What::Load` without them):
1. `kazoo-wall/src/protocol/mod.rs`: `What::Load` gains `beats`.
2. `kazoo-wall/src/daemon/wall.rs`: `Source::{Save, Undo}` carry `beats`; `replace` asks `crossing_beats` for them; `mirror`'s `Load` arm crosses; `undo_load` passes the load's own `beats`.
3. `kazoo-wall/src/daemon/wall/cross/tests.rs` (T05b's): the tests.
4. `kazoo-wall/src/change.rs`: `loaded` says the crossing; its test's literal.
5. `kazoo-wall/src/daemon/wall/tests.rs`: its `What::Load` literals (`:3332`, `:3681`) gain `beats: 0.0`.
6. `kazoo-wall/src/protocol/tests.rs`: its literal (`:800`) gains `beats: 0.0`; the old line without `beats` reads as 0.
7. `kazoo-wall/src/store.rs`: its literal (`:2734`) gains `beats: 0.0`.
8. `kazoo-wall/src/tui/tests/saves.rs`: its literal (`:81`) gains `beats: 0.0`.
9. `kazoo-mcp/src/render.rs`: its literal (`:1889`, a test) gains `beats: 0.0`.
10. `kazoo-mcp/src/tools.rs`: `wall_load`'s description (`:650`).
11. `kazoo-wall/src/tui/draw/overlay.rs`: the two sentences that say a load fades (`:617`, `:814`).

## Outcome

`wall_load "dawn"` on a playing wall: the modules both walls share (by birth, T01) glide to the save's knobs, the cables that differ crossfade, the modules only one wall has fade in or out, the tempo glides, over 16 beats, exactly as `wall_go` to the same moment. The log line says it: `Tom loaded the save "dawn" (41 modules, 51 cables, 120 BPM), crossing over 16 beats; the wall as it was is saved as "before-load-20261013-213000"`. Undoing the load crosses back over the beats it took. A load the wall cannot carry crossing (D03b's guard, or memory) is a cut and says why, as a go does. Clearing the wall (`Request::New`) stays a cut: nothing is reached, so nothing is crossed to, and its words already say it fades.

## The design (decided; build exactly this)

### The record (`protocol/mod.rs`)

`What::Load` gains, after `tempo`:
```rust
/// Beats the wall crossed over to it (KZ-T05d); 0 for a cut. Loads
/// logged before this field read as cuts, which they were.
#[serde(default)]
beats: f64,
```
Reason for `#[serde(default)]` and not an `Option`: every old line was a cut, and 0 says that exactly.

### The wall (`daemon/wall.rs`)

- `Source::Save(Kept)` becomes `Source::Save { kept: Kept, beats: f64 }`, and `Source::Undo { load, kept }` gains `beats: f64`. `Source::Empty` is unchanged. Every match on `Source` follows (`how`, `save`, and T04b's/T05b's arms).
- `load` (`:3231`) passes `beats: DEFAULT_CROSS_BEATS` (T05e makes it the request's).
- `undo_load` (`:3268`) reads `beats` from the `What::Load` it undoes (beside `backup` and `fingerprint` at `:2870`) and passes it. Reason: T05b undoes a go over the beats the go took; an undone load is the same move backwards.
- `replace` (`:3420`): T05b step 1 (`crossing_beats` after `load_room_as`, before `keeping`) runs for `Source::Save` and `Source::Undo` as for `Source::Moment` and `Source::Back`, with the source's `beats`. `What::Load { …, beats }` takes the answer (0 when it became a cut). T05b step 3 (`self.cross_plan = plan`) as for a go.
- `mirror`'s `Load` arm (`What::Load { .. } => self.swap_engine()`, `:1899`) becomes what T05b made the `Go` arm: take `self.cross_plan`; `Some(plan)` with `beats > 0` → `self.cross(*plan)`; else `self.swap_engine()`.
- Nothing else of a load changes: the wall as it was is kept first, the load is one change, `wall_replaced` is answered to a seat that has not looked since, `Event::Loaded` is sent, the load allowance is drawn. A fire's step that `replaces` (T06b) stays a cut: a fire's steps never fade (T05b, `fades: 0`).
- The matching is T05b's, by birth. A save written before T01 has modules with no birth: none match, so every module of the wall up fades out and every module of the save fades in, over the beats. Reason: that is still a crossing, never a click, and it is the honest result of not knowing which modules are the same.

### The words

- `change.rs`'s `loaded` (`:216`) takes `beats` and, when `beats > 0` and it is a load of a save or an undone load, puts `, crossing over <beats> beats` after the counts' parenthesis (beats through `format::beats` if it exists, else `{}` of the `f64`, as T05c writes them). A cut says nothing more (as today).
- `wall_load`'s description (`tools.rs:650`): "the master fades out and back in over a few hundredths of a second" becomes "it crosses over 16 beats (4 bars), as wall_go does: modules both walls share (by birth) glide to the save's knobs, the rest fade in or out, cables crossfade and the tempo glides; a crossing the wall cannot carry is a cut, and the answer says so". The rest is unchanged.
- The console (`overlay.rs`): `:617` becomes `" the wall plays on while a save loads: it crosses over 16 beats"`; `:814` becomes `"The wall plays on: it crosses over 16 beats to the save."`. T05e makes both name the length chosen.

## Not this lane

`beats` on `Request::Load`, `wall_load`'s argument, the console's `[ ]` in the load question and `:load`'s length: T05e. Clearing the wall: stays a cut. The fire: unchanged.

## Red tests first

In `daemon/wall/cross/tests.rs` (T05b's), with its harness (a real `Wall` and `Engine` on a `Store::peek` state directory, rendered in 512-frame blocks).

1. `a_load_crosses_as_a_go_does`: a wall, saved; three turns and an added module; load the save: `What::Load { beats: 16.0, .. }`; after 8 beats' frames the shared modules' knobs are between old and new; after 16 beats every knob is the save's and the added module is gone from the rack; no hush was sent (`crossings_sent` raised, `swaps_sent` not).
2. `the_undo_of_a_load_crosses_back_over_its_beats`: undo test 1's load: `What::Load { save: None, beats: 16.0, .. }`; after 16 beats the wall is the one before, knobs exact.
3. `a_load_past_the_guard_is_a_cut_and_says_so`: as T05b's test 13 with a load: `beats: 0.0`, the note with the percentage, the hush sent.
4. `a_save_from_before_births_crossfades_whole`: a save whose modules have no birth: every old module fades out and every new one in; nothing is matched.
5. `old_load_lines_read_as_cuts` (`protocol/tests.rs`): a logged `What::Load` without `beats` reads with `beats: 0.0`, and writes back with it.
6. `the_load_line_says_the_crossing` (`change.rs`): exact strings for a load crossing, an undone load crossing, and a cut.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-mcp/src
```

## Definition of done

- [ ] The 6 tests fail first (paste each under RED), then pass.
- [ ] Files: the eleven above, and files 5 to 9 change only by `beats: 0.0` (paste their diffs).
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, on Tom's wall with Tom there: load a save that shares modules with the wall, listen to the 16 beats, undo it, listen back.

## Code quality

As PLAN.md's STACK RULES. `daemon/wall.rs` gains only the `Source` fields, the `replace` lines and the `mirror` arm.

## END

```
START KZ-T05d base=<sha> branch=lane/kz-T05d df=<GB> at <time>
END KZ-T05d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
