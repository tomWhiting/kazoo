# KZ-T05c: a seat going to a moment can say how many beats the crossing takes (0 for a cut), and Tom's console shows the crossing in its go question and lets him change it with `[` and `]`

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Items from KZ-T04b, T04c, T04d and T05b are named as those briefs name them (`Request::Go`, `GoArgs`, `render::went`, `Mode::Go`, `go_key`, `confirm_go`, `DEFAULT_CROSS_BEATS`, `MAX_CROSS_BEATS`, `Source::Moment`); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Last of PLAN.md's T05 split in three (T05a the engine's glides, T05b the wall crosses, **T05c** the seats and the console choose the length).

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T05b has landed; the by-hand check is Cassio's with Tom after landing. Accepted otherwise as written.

Time: about 35 minutes of a builder's work. One mechanism: the crossing's length carried from a seat or the console to the wall. Crates: kazoo-wall, kazoo-mcp. Depends on T05b, which must have landed: every go crosses over `DEFAULT_CROSS_BEATS`, `Source::Moment` carries `beats`.

Files (ten):
1. `kazoo-wall/src/protocol/mod.rs`: `Request::Go` gains `beats`.
2. `kazoo-wall/src/daemon/wall/moments.rs`: `go` takes it.
3. `kazoo-wall/src/daemon/wall/moments/tests.rs`: its `Request::Go` literals gain `beats: None`; two tests.
4. `kazoo-mcp/src/tools.rs`: `GoArgs::beats`; `wall_go`'s description; its literal.
5. `kazoo-mcp/src/render/tree.rs`: `went` says the crossing.
6. `kazoo-wall/src/tui/app/mod.rs`: `Mode::Go` gains `beats`; `App::cross_beats`.
7. `kazoo-wall/src/tui/app/tree.rs`: `go_key`'s `[` and `]`; the request and the status words.
8. `kazoo-wall/src/tui/app/command.rs`: `:cross <beats>`; `:go` uses the console's length.
9. `kazoo-wall/src/tui/draw/tree.rs`: `confirm_go` shows the length.
10. `kazoo-wall/src/tui/tests/tree.rs`: its `Request::Go` and `Mode::Go` literals; three tests.

Before the first commit, `rg -n 'Request::Go \{|Mode::Go \{' kazoo-wall kazoo-mcp` and check every literal is in one of these files; one elsewhere is an eleventh file: change it all the same and say so under FOUND.

## Outcome

- `wall_go` takes `beats`: how long the crossing takes, 0 (a cut) to 64; left out, 16 (4 bars). The answer says how the wall is getting there: `… It is crossing over 16 beats (about 8 s): wall_look already shows the moment's wall; the sound gets there by the end.` or `… It was a cut.`
- In Tom's console the go question reads `go to “dawn”? crossing over 16 beats · [ ] longer or shorter, 0 is a cut · the wall as it is is kept first; z undoes it · y goes · n stays`. `[` and `]` step through 0, 1, 2, 4, 8, 16, 32 and 64 beats; the length chosen stays for the next go. `:cross 8` sets it from the command line, and `:go` uses it.
- The undo of a go needs nothing: it crosses back over the beats its go took (T05b).

## The design (decided; build exactly this)

### The request (`protocol/mod.rs`, `Request::Go`, T04b)

```rust
Go {
    moment: String,
    /// Beats the crossing takes, at the tempo playing as it begins: 0 is a
    /// cut; held to 0..=MAX_CROSS_BEATS; left out, DEFAULT_CROSS_BEATS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    beats: Option<f64>,
},
```
Reason it is held to the range rather than refused outside it: a turn's glide is held the same way (`Patch::turn`, `glide.clamp(0.0, MAX_GLIDE_BEATS)`, `patch.rs:575`), and a seat asking 100 beats wants a long crossing, not an error. A `beats` that is not a number is refused, as a turn's value is: `bad_request` "a crossing is a number of beats: 0 for a cut, up to 64".

### The wall (`moments.rs`, T04b's `go`)

`go` reads `beats`: `None` is `DEFAULT_CROSS_BEATS`; not finite is the refusal above, sent before anything is asked of the tree; the load allowance `request_as` took for the go comes back by T04b's rule (every refusal other than `internal` is refunded); else `beats.clamp(0.0, MAX_CROSS_BEATS)`. It rides in T04b's `Asked::Go` (which gains `beats: f64`) through the staleness retries, and goes into `Source::Moment { …, beats }` (T05b). Nothing else changes: T05b decides whether the machine can afford it and makes it a cut if not.

### The seats (`kazoo-mcp/src/tools.rs`, `render/tree.rs`)

- `GoArgs` (T04c) gains:
  ```rust
  /// Beats the crossing takes: 0 is a cut; at most 64; default 16 (4 bars).
  #[serde(default)]
  pub beats: Option<f64>,
  ```
  and `wall_go` sends `Request::Go { moment, beats }`.
- `wall_go`'s description: T04c's sentence "It is a cut: the master fades out and back in over a few hundredths of a second." becomes, exactly: "It crosses over beats (default 16, 4 bars; 0 to 64): modules both walls share (by birth) glide every knob to the moment's values, modules only the moment has fade in, those it lacks fade out and are then taken away, and cables that differ crossfade; the tempo glides too unless the desk keeps it. beats 0 is a cut (the master fades out and back in over a few hundredths of a second). wall_look shows the moment's wall at once; the sound gets there by the end." Everything else in it stays.
- `render::went` (T04c) adds, after the load's sentence and before the bud sentence, from the change's `What::Go { beats, tempo, .. }`: when `beats > 0`, ` It is crossing over <format beats> (about <s> s): wall_look already shows the moment's wall; the sound gets there by the end.` with `<s>` = `beats × 60 / tempo` to one decimal; when 0, ` It was a cut.` The notes T05b puts in the `LoadResult` (the counts, or why it was a cut) are printed as `render::loaded` prints every note.

### The console (`tui/app/*.rs`, `tui/draw/tree.rs`)

- `App` (`tui/app/mod.rs`) gains `cross_beats: f64`, starting at `DEFAULT_CROSS_BEATS`. `Mode::Go` (T04d) gains `beats: f64`, set from `cross_beats` when the question opens.
- `go_key` (T04d, `tui/app/tree.rs`): `]` moves `beats` to the next rung of `CROSS_LADDER: [f64; 8] = [0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0]` above it, `[` to the next below (a value between rungs, from `:cross`, goes to the nearest rung in that direction), and `cross_beats` follows, so the length chosen stays; `y` sends `Request::Go { moment, beats: Some(beats) }` with the status "going to <label> over <format beats> (the wall as it is is kept first)…" or "cutting to <label> (the wall as it is is kept first)…" when 0. Reason for the keys: `[` and `]` already make the turn glide shorter and longer in normal mode (`change_glide`, `tui/app/keys.rs:123–124`), so they mean "longer, shorter" on this console already; the ladder doubles, as a musician counts bars.
- `confirm_go` (T04d, `tui/draw/tree.rs`): lines `go to <label>?`, `crossing over <format beats> · [ ] longer or shorter, 0 is a cut` (or `a cut · ] crosses instead` at 0), `the wall as it is is kept first; z undoes it`, keys `y goes · n stays`.
- The go's answer (T04d's `Request::Go` arm in `tui/app/tree.rs`): the status "went to <place> · crossing over <format beats> · the wall as it was is saved as “<backup>” · z undoes it", or "cut to <place> · …" when the logged `beats` is 0, read from the change's `What::Go`.
- `command.rs` (the table at `:49–53`): `("cross", "<beats>", "how many beats a go takes to cross (0 is a cut, up to 64; now <n>)")`. `run_command` (`:468`): `cross` with a number sets `cross_beats` to it held to 0..=64 and says "a go now crosses over <format beats>"; not a number says "a crossing is a number of beats: 0 for a cut, up to 64" and changes nothing. `:go <moment>` (T04d) sends `beats: Some(cross_beats)`. Reason `:go` takes no length of its own: a moment may be a mark whose name ends in a number ("gorgeous toms evening 2026-10-13 1913"), so `:go <moment> <beats>` could not be read.

## Not this lane

The crossing itself (T05b). The fire (T06). Loads crossing (not asked: T05b's decision list).

## Red tests first

Each fails today because `Request::Go` has no `beats`.

1. `moments/tests.rs`: `a_go_crosses_over_the_beats_asked`: `beats: Some(4.0)` logs a `What::Go` with `beats: 4.0`; `None` logs 16; `Some(100.0)` logs 64; `Some(0.0)` is the cut (the hush sent, `beats: 0.0`).
2. `moments/tests.rs`: `a_crossing_that_is_not_a_number_is_refused_and_costs_nothing`: `Some(f64::NAN)` is `bad_request` with the sentence, nothing is asked of the tree, and the seat's load allowance is as it was.
3. `kazoo-mcp/src/tools.rs` tests: `arguments_are_strict` gains `GoArgs` with `{"moment": "dawn", "beats": 8}` and without `beats`; `{"moment": "dawn", "bars": 4}` is refused. `the_router_holds_every_tool_with_a_description_and_schema` checks `wall_go`'s description holds "beats 0 is a cut".
4. `kazoo-mcp/src/render/tree.rs` tests: `a_go_says_how_it_crosses`: a `LoadResult` whose `What::Go` has `beats: 16.0` at 120 BPM says "crossing over 16 beats (about 8.0 s)"; with 0, "It was a cut."
5. `tui/tests/tree.rs`: `the_go_question_shows_the_crossing_and_brackets_change_it`: `B`, a row, Enter: the drawn question at 120 × 40 reads "crossing over 16 beats"; `]` gives 32 and `[` `[` gives 8; `y` sends exactly `Request::Go { moment, beats: Some(8.0) }`; the next question opens at 8.
6. `tui/tests/tree.rs`: `cross_sets_the_length_and_go_uses_it`: `:cross 4` then `:go dawn` sends `beats: Some(4.0)`; `:cross soon` changes nothing and says so; `:cross 0` then a go says "cutting to".
7. `tui/tests/tree.rs`: `a_go_answer_says_crossing_or_cut`: the status for a logged `beats: 16.0` and for `0.0`.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/protocol kazoo-wall/src/daemon/wall kazoo-wall/src/tui kazoo-mcp/src
```
Doc tests, because `Request::Go`'s doc changes: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 7 tests fail first (paste each under RED), then pass.
- [ ] `Request::Go::beats`, `GoArgs::beats`, `App::cross_beats`, `Mode::Go::beats`, `CROSS_LADDER` and the `cross` command exist as named.
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio does this with Tom: from a Claude seat `wall_go` to a mark with `beats` 4 and with 0; in Tom's console `B`, a mark, `[` to 8, `y`; `:cross 32`, `:go <a mark>`. Paste the answers and say what was heard.

## Code quality

As PLAN.md's STACK RULES. `tui/app/mod.rs` (1,644 lines before T04d) gains only the field and the variant's field; everything else lives in `tui/app/tree.rs`. Colours from `tui/theme.rs` only.

## END

```
START KZ-T05c base=<sha> branch=lane/kz-T05c df=<GB> at <time>
END KZ-T05c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
