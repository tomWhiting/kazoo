# KZ-L05: every seat can see a loop's state, length, place, pass and wear, erase it or point it at a desk strip, and is told when unequal loops will meet again

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: a loop's state in the snapshot, and its words in the MCP and the console's list view. Crates: kazoo-wall (`protocol/mod.rs`, `daemon/wall.rs`'s `snapshot`, `daemon/loops.rs`, `tui/draw/wall.rs`, `tui/layout.rs`) and kazoo-mcp (`render.rs`, `tools.rs`). Depends: KZ-L04.

Read by Cassio, 13 Oct evening. The arithmetic was checked: 17 and 23 bars are 68 and 92 beats, whose least common multiple is 1,564 beats, 391 bars, 782 s at 120 BPM; the eight-loop product is about 3.6 × 10^18 bars and fits in `i128`; 8 and 16 beats anchored at 0 and 4 never meet. Changes: the checks are written out in the lane form, the live check is Cassio's and Tom's after landing, the tool count is relative to the base, and `bars` includes 4 in its least common multiple.

## Outcome

`wall_look` says, under `loop1`:

> loop1 (loop): recording, armed for bar 9 · 17 bars (34.0 s at 120 BPM, spliced at bar 1) · bar 6 of 17, pass 3 · written bars 1–8 · wear 0.30: bars 1–4 are 2 passes old · from mouth/dry · take 812 (reel 9f3c01ab)

At the foot of the module list it says "Loops 17 and 23 bars long meet again every 391 bars (13 min 2 s at 120 BPM); next at bar 392." The console's list view shows the same in one line on the loop's panel. Claude can erase a loop or point it at a strip with `wall_loop`. Recording needs no new tool: it is `wall_turn loop1 record 1`, because every knob is already a seat's to turn.

## The design (decided; build exactly this)

### The snapshot (`protocol/mod.rs`)

- `ModuleView` (`:1300`) gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub tape: Option<LoopView>` for `loop` modules.
- `LoopView` holds:
  - `state: LoopState`, one of `Blank`, `Armed`, `Rolling`, `PostRoll`, `Holding`, `NoTape` or `Loading`. It is snake_case. `Loading` means a `Fetch` is in flight (L03);
  - `beats: f64` and `frames: u64`, both 0 when blank;
  - `rate: u32`, `anchor: f64` and `pass: i64`;
  - `written: Vec<bool>`, one per tape bar;
  - `ages: Vec<u32>`, passes since each bar was last written: the gauges' `pass` minus `bar_pass`, 0 for never;
  - `bins: Vec<u8>`, the 64 levels of L02;
  - `peak_db: f64`, the output peak since the last look, −120 for silence;
  - `from: Option<String>`;
  - `take: Option<u64>` (the change number of its newest take) and `reel: Option<String>` (8 hex);
  - `shortened: bool`;
  - `notes: Vec<String>`, the L03 and L04 notes for this loop not yet shown, each shown once.

  It is filled in `Wall::snapshot` (`daemon/wall.rs:3912`) from `Loops::view(id, &patch)`, which reads the gauges (L02) and the patch record. Reading atomics allocates nothing on the render thread, and the snapshot is built on the control thread as today.
- `Snapshot` (`:1071`) gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub loops_meet: Option<LoopsMeet>`. It holds `LoopsMeet { bars: Option<u64>, seconds: Option<f64>, next_bar: Option<u64> }`, present when at least two loops hold tape.
  - `bars` is the least common multiple of their lengths in beats and 4, over 4 (so it is a whole number of bars even when a loop's length is not a whole number of bars), computed in `u128` with a checked multiply. Past `10^9` bars, `bars`, `seconds` and `next_bar` are all `None`, and the MCP says "practically never".
  - `next_bar` is the next bar (1-based) at which every loop is at its seam. Find it with `meet(lengths: &[u64], anchors: &[i64], after_beat: f64) -> Option<u64>`, working in whole beats: anchors are bar lines and lengths are whole beats (L02).
    1. Solve the congruences `x ≡ anchor_i (mod length_i)` together with `x ≡ 0 (mod 4)` by merging them pairwise with the extended Euclid algorithm in `i128`.
    2. Merging fails when two anchors differ by something their lengths' gcd does not divide. Then the loops never meet: the answer is `None`, and the MCP says "these loops never all meet at their seams".
    3. The merged solution `x0 (mod L)` gives the first `x > after_beat`, as a bar `x / 4 + 1`.
  - Reason: this is the whole point of Eno's unequal loops, and nothing in a 17-and-23-bar piece can be seen without being told it.
- `Snapshot` also gains `#[serde(default)] pub checkpoints_paused: bool` (L04).

### The MCP (`kazoo-mcp/src/render.rs`, `tools.rs`)

- `render::look` (`render.rs:23`): a loop's module line (`:293`) adds the sentence shown above. Write it with the existing `round` and `format` helpers, and the tempo and seconds through `beats_in` (`kazoo_core::ipc::follow`).
  - Written bars are given as runs (`1–8, 12`).
  - Ages are given only when `wear > 0`, and only for bars older than 0.
  - Notes are given at the end of the line, each as its own sentence.
  - After the module list, the `loops_meet` sentence, and "Checkpoints are paused: the reel store is full." when `checkpoints_paused` is set.
- New tool `wall_loop`, with args `LoopArgs { module: String, erase: Option<bool>, from: Option<Option<String>> }`, serde `default`.
  - Exactly one of `erase: true` or `from` (a strip label, or `null` to record what is patched in) must be given, else a sentence saying so.
  - It sends `Request::Loop` (L03) through `self.change` as `wall_speak` does (`tools.rs:591`).
  - The description: "Erase a loop module's tape (undoable with wall_undo), or set which desk strip it records from (e.g. `mouth/dry` for Tom's voice; `null` to record what is patched into its left and right). To record, turn its `record` knob to 1 with wall_turn: it starts at the loop's next bar line and stops at the bar line after you turn it back to 0. Its `bars` and `beats` knobs set the length of a blank loop; two loops of unequal lengths (17 and 23 bars) drift against each other and meet again rarely."
  - `TOOLS` (`tools.rs:747`) gains `wall_loop`; the count becomes one more than it is on the base (19 at `247ca07`; Track T's `wall_mark`, `wall_go` and `wall_tree` may have landed since). Every test that pins the count follows.
- `wall_catalogue`'s entry for `loop` needs nothing: it comes from the catalogue (L02).

### The console's list view (`tui/draw/wall.rs`)

- A loop's panel (`panel`, `:243`) gets one extra line under its title, before its knobs, built by a pure `loop_line(view: &LoopView, tempo: f64, width: u16) -> Vec<Span>`:
  - a state lamp `●`: `RED` while rolling or post-roll; `AMBER` while armed, steady; `SAGE` while holding tape; `TEXT_DIM` when blank;
  - `17b`, then `6/17` (the bar under the head of the bars), then `p3`;
  - the written bars as a bar of `▮` (written) and `▯` (not), one per tape bar, cut to the width;
  - `← mouth/dry` when `from` is set.

  Colours are from `tui/theme.rs` only. `panel_height` (`tui/layout.rs:87`) adds that one line for a loop. Reason for one line: the list view is for reading every value in words, and the dressing belongs to the tape view (KZ-L12), not here.
- The bar under the head is computed from the snapshot's beat and the loop's anchor and beats, interpolated between looks by the tempo, as the rack view's knobs are not. The console polls every 100 ms (`tui/mod.rs:53`): `Snapshot.beat + elapsed × tempo / 60`. That interpolation is a pure function `beat_now(snapshot_beat, tempo, since: Duration) -> f64` in a new `tui/clock.rs`, reused by KZ-L12 and KZ-L13.

## Not this lane

The tape view and the loom view (KZ-L12, KZ-L13). The desk (KZ-L07 to KZ-L11).

## Red tests first

Each fails today because there is no `tape` field, no `loops_meet` and no `wall_loop`.

1. kazoo-wall `daemon/wall/tests.rs` `a_loop_shows_its_state_length_place_and_take`. Record a bar on an offline wall (L04's test helper). `snapshot().modules[..].tape` has `state: Holding`, `beats: 4.0`, `written: [true]`, `take: Some(seq)`, and an 8-hex `reel`. While recording, `state: Rolling`.
2. kazoo-wall `seventeen_and_twenty_three_bars_meet_every_391_bars`. Two loops with tape, both anchored at beat 0, at 120 BPM: `loops_meet == { bars: Some(391), seconds: Some(782.0), next_bar: Some(392) }` with the beat at 10.0. Anchored at 0 and 8 (bar 3), `next_bar` is the first bar both seams fall on, checked by brute force over 10,000 bars in the test.
3. kazoo-wall `meet_is_none_when_it_is_practically_never_or_never`:
   - eight loops of 233, 239, 241, 251, 253, 255, 256 and 257 beats, all anchored at 0: the LCM is about 3.6 × 10^18 bars, over `10^9`, so its three fields are `None` and the sentence says "practically never";
   - loops of 8 and 16 beats anchored at beats 0 and 4: `bars` is `Some(4)` but `next_bar` is `None`, and the sentence says they never all meet at their seams.
4. kazoo-mcp `render::tests::a_loop_is_described_in_words`. Pin the sentence for a `LoopView` fixture: armed, wear 0.3, from `mouth/dry`.
5. kazoo-mcp `the_meet_sentence_and_paused_checkpoints_are_said`.
6. kazoo-mcp `tools::tests` `wall_loop_needs_exactly_one_of_erase_and_from`, and the router test with `wall_loop` in `TOOLS`.
7. kazoo-wall `tui/tests/render.rs` `a_loop_panel_has_one_line_with_its_lamp_bars_and_source`. Render a snapshot fixture into `TestBackend` at 120×40: the line holds `17b`, `p3` and `← mouth/dry`, the lamp's colour is `RED` while rolling, and the panel is exactly one line taller than a loop panel without it.
8. kazoo-wall `tui/clock.rs` `beat_now_moves_with_the_tempo`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -p kazoo-mcp -j 4
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-mcp/src
```

## Definition of done

- [ ] The eight tests fail first (paste each under RED), then pass.
- [ ] `LoopView`, `LoopState`, `LoopsMeet`, `meet`, `loop_line`, `beat_now` and `wall_loop` exist as named.
- [ ] Not the builder's: the builder never touches the live wall. After landing and reinstalling, Cassio puts up two loops of 17 and 23 bars, checks the meet sentence in `wall_look`, erases and undoes one, and asks Tom whether the console's panel line moves its bar count with the music. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `protocol/mod.rs` is about 1860 lines, `kazoo-mcp/src/render.rs` about 1920, and `daemon/wall.rs` about 4700. Add to them only the fields and calls named here, put the sentence-building in a new `kazoo-mcp/src/render/loops.rs` and the view-building in `daemon/loops.rs`, and list the three files under FOUND with their line counts.

## END

```
START KZ-L05 base=<sha> branch=lane/kz-L05 df=<GB> at <time>
END KZ-L05 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
