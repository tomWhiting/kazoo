# KZ-T05e: a seat loading a save can say how many beats the crossing takes (0 for a cut), and Tom's console's load question shows the crossing and changes it with `[` and `]`, as the go question does

Line numbers are from main at `004bac8` and may have moved; find each item by what it does, never rename. Items from KZ-T05b to KZ-T05d are named as those briefs name them (`DEFAULT_CROSS_BEATS`, `MAX_CROSS_BEATS`, `CROSS_LADDER`, `App::cross_beats`, `Source::Save { kept, beats }`, `What::Load::beats`, `go_key`, `confirm_go`, …); none had landed when this was written, so use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Written by Cassio, 13 Oct evening, with T05d (loads cross). Decided under rule 13; Tom may overrule.

Time: about 30 minutes of a builder's work. One mechanism: the load's crossing length carried from a seat or the console to the wall. Crates: kazoo-wall, kazoo-mcp. Depends on T05d, which must have landed before this lane opens.

Files (thirteen: 2, 6, 10 and 13 change only a literal or a pattern, which adding a field to `Request::Load` forces; the lane is still one mechanism):
1. `kazoo-wall/src/protocol/mod.rs`: `Request::Load` gains `beats`.
2. `kazoo-wall/src/protocol/client.rs`: `load`'s literal (`:484`) gains `beats: None`.
3. `kazoo-wall/src/protocol/tests.rs`: its literals (`:662`, `:900`) gain `beats: None`; the wire test below.
4. `kazoo-wall/src/daemon/wall.rs`: `load` (`:3231`) takes the request's `beats`.
5. `kazoo-wall/src/daemon/wall/cross/tests.rs` (T05b's): two tests.
6. `kazoo-wall/src/daemon/wall/tests.rs`: its `Request::Load` literals (`:1917`, `:3198`, `:4252`) gain `beats: None`.
7. `kazoo-mcp/src/tools.rs`: `LoadArgs::beats`; `wall_load`'s literal (`:655`) and description.
8. `kazoo-wall/src/tui/app/saves.rs`: `confirm_load` takes `[` and `]`; `y` sends `cross_beats`.
9. `kazoo-wall/src/tui/app/command.rs`: `:load` sends `cross_beats` (`:538`).
10. `kazoo-wall/src/tui/app/mod.rs`: the request's words (`:1627`) gain `..`.
11. `kazoo-wall/src/tui/draw/overlay.rs`: `confirm_load`'s lines and the saves list's status line.
12. `kazoo-wall/src/tui/tests/saves.rs`: the console's tests; its literal (`:244`).
13. `kazoo-wall/src/tui/tests/command.rs`: its literal (`:190`).

## Outcome

- `wall_load` takes `beats`: 0 (a cut) to 64; left out, 16 (4 bars). The answer's change line says the crossing (T05d's words).
- In Tom's console, the load question reads `Load "dawn" onto the wall?` / `crossing over 16 beats · [ ] longer or shorter, 0 is a cut` / `The wall as it is now is saved first (before-load-…),` / `and z undoes the load.`, keys `y loads it · [ ] crossing · any other key goes back to the list`. `[` and `]` move the console's one crossing length (`App::cross_beats`, T05c), so a length chosen for a go is the length a load takes, and back.
- `:load dawn` loads at the console's crossing length, as `:go` goes at it.

## The design (decided; build exactly this)

### The request (`protocol/mod.rs`)

`Request::Load` gains, after `name`:
```rust
/// Beats to cross over (KZ-T05e): 0 is a cut, held to 0..=64; left out,
/// 16.
#[serde(default, skip_serializing_if = "Option::is_none")]
beats: Option<f64>,
```
Held to the range rather than refused outside it, as T05c holds a go's: a seat asking 100 beats wants a long crossing. Not a number: `bad_request` "a crossing is a number of beats: 0 for a cut, up to 64" (T05c's sentence), checked in `load` before the save is read, and it costs no allowance: find how a load's other refusals give the allowance back (`request_as`, `daemon/wall.rs:2195`; T05c named T04b's rule for a go) and follow it; if a refusal of a load does not give it back today, say so under FOUND and give it back for this one.

### The wall (`daemon/wall.rs`)

`load` takes `beats: Option<f64>`: `None` is `DEFAULT_CROSS_BEATS`; else the check above, then `beats.clamp(0.0, MAX_CROSS_BEATS)`, into `Source::Save { kept, beats }` (T05d). The undo of a load is unchanged: it crosses back over the beats the load took (T05d).

### The seats (`kazoo-mcp/src/tools.rs`)

- `LoadArgs` gains:
  ```rust
  /// How many beats the crossing takes: 0 is a cut, up to 64; left out, 16
  /// (4 bars).
  #[serde(default)]
  pub beats: Option<f64>,
  ```
- `wall_load` sends `Request::Load { name: args.name, beats: args.beats }`.
- `wall_load`'s description: T05d's "it crosses over 16 beats (4 bars), as wall_go does" becomes "it crosses over beats (default 16, 4 bars; 0 is a cut; up to 64), as wall_go does"; the rest as T05d wrote it.

### The console (`tui/app/saves.rs`, `tui/draw/overlay.rs`, `tui/app/command.rs`)

- `confirm_load` (`saves.rs:260`): `[` and `]` step `self.cross_beats` along `CROSS_LADDER` exactly as T05c's `go_key` steps a go's (a value between rungs goes to the nearest rung that way) and leave the question up (`Mode::LoadSave { name, index }` again); `y` sends `Request::Load { name, beats: Some(self.cross_beats) }` and says `loading "<name>" over <beats> beats (the wall as it is is saved first)…`, or `… as a cut …` at 0. Any other key goes back to the list, as now. Reason for sharing `cross_beats` with the go question rather than a second length: one crossing length on the console is one thing to remember, and Waffles' reading was that the verb must not change the sound.
- `overlay.rs`'s `confirm_load` (`:807`) gains the `beats` it shows: its lines as in the Outcome (at 0: `a cut · ] crosses instead`, as T05c's `confirm_go`); the saves list's status line (`:617`) becomes `" the wall plays on while a save loads: it crosses over <beats> beats"` (or `" … : it cuts"` at 0), read from `app.cross_beats`.
- `:load <name>` (`command.rs:531`) sends `beats: Some(self.cross_beats)` and says it, as `confirm_load` does.
- `tui/app/mod.rs:1627`: `Request::Load { name, .. }`.

## Not this lane

Clearing the wall (`n` in the saves list, `Request::New`): a cut. The fire: unchanged.

## Red tests first

1. `protocol/tests.rs`: `a_load_carries_its_crossing`: `{"op":"load","name":"dawn"}` reads with `beats: None` and writes back without it; `{"op":"load","name":"dawn","beats":4}` with `Some(4.0)`.
2. `cross/tests.rs`: `a_load_takes_the_beats_asked`: `beats: Some(0.0)` is a cut (`What::Load { beats: 0.0, .. }`, the hush sent); `Some(4.0)` crosses over 4; `Some(100.0)` over 64; its undo crosses back over the same.
3. `cross/tests.rs`: `a_load_with_no_number_is_refused_and_costs_nothing`: `Some(f64::NAN)`: `bad_request` with the sentence; the save was not read; the allowance is as before.
4. `tools.rs`: `arguments_are_strict` gains `LoadArgs` taking `{"name": "dawn", "beats": 8}` and refusing `{"name": "dawn", "bars": 2}`.
5. `tui/tests/saves.rs`: `the_load_question_shows_and_changes_the_crossing`: on a fresh console (16 beats), Enter on a save, `]` `]`, `y`: exactly `[Request::Load { name, beats: Some(64.0) }]`; on another fresh console, `[` four times then `y`: `Some(1.0)`; at 0 the render holds `a cut`; the go question then opens at the same length.
6. `tui/tests/command.rs`: `:load dawn` sends the console's crossing length.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-mcp/src
```

## Definition of done

- [ ] The 6 tests fail first (paste each under RED), then pass.
- [ ] Files: the thirteen above; 2, 6, 10 and 13 change only by the literal or the pattern (paste their diffs).
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, in Tom's console with Tom there: load a save at 64 beats, then at 0, and hear the difference; from a Claude seat, `wall_load` with `beats: 8`.

## Code quality

As PLAN.md's STACK RULES. Colours from `tui/theme.rs` only.

## END

```
START KZ-T05e base=<sha> branch=lane/kz-T05e df=<GB> at <time>
END KZ-T05e head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
