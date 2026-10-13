# KZ-L12: the wall console's tape view, an eight-track machine drawn in braille: a channel strip per loop on top, two reels turning with the beat whose packs grow as the loop fills, and the tape's bars under the heads; every mark is a reading

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: a new console view drawn from the snapshot's loops. Crates: kazoo-wall only:
- new `src/tui/tape/mod.rs`, `src/tui/tape/reel.rs`, `src/tui/tape/strip.rs`, `src/tui/tape/plain.rs`, `src/tui/tape/tests.rs`;
- `src/tui/rack/dots.rs`, moved out of `rack/screen.rs`, below;
- additions to `tui/app/mod.rs`, `tui/app/keys.rs`, `tui/app/prefs.rs` and `tui/draw/mod.rs`;
- a bench `benches/tape_view.rs`.

Depends: KZ-L05 (`LoopView`, `beat_now`).

Read by Cassio, 14 Oct 01:4x. Checked: a time constant of 300 ms / ln 100 reaches 99% of a step in 300 ms. Changes: the checks and the bench are written out in the lane form, and the live check is Cassio's. KZ-T08b (the little tree) reuses this lane's `Dots` move and plain-glyph switch, so it waits for this lane. Opens only after L05 has landed.

## Tom's words, and the rule

"Try to create the effect of like a spinning reel to reel tape thing. I had an Ampex … an eight track. It had like the eight big preamps that were like as big as a VCR, kind of mounted on the top. The thing was taller than me." And: "the dressing should never overpower function."

So every drawn thing here carries a reading, and the view says which in its help (`?`):

| mark | reading |
|---|---|
| a channel strip's needle | that loop's output level, VU ballistics |
| its lamp | record state: red recording, amber armed, sage holding tape, dim blank |
| its numbers | length in bars, pass count, wear |
| the reels' spokes | where the song is in the bar: one turn per bar; the brass spoke is the downbeat; stopped when the beat stops; faster with the tempo |
| the take-up reel's pack | how much of the selected loop has been recorded; the supply reel holds the rest |
| the tape under the heads | the selected loop's bars, written (▮) or blank (▯), and the head (▲) |
| the counter | bar of bars, pass, time into the loop of its length |

The view is reached on purpose (`T`) and is never where the console opens. The list and rack views stay the console's standard views. The console does not remember the tape view between runs: `Prefs.rack` (`tui/app/prefs.rs`) is untouched.

## The design (decided; build exactly this)

### Reaching it

- `View` (`tui/app/mod.rs:115`) gains `Tape`. KZ-L13 adds `Loom` and its key `W`.
- `T` from List or Rack opens Tape and remembers where it came from. `T` or `Esc` in Tape goes back.
- `v` (List⇄Rack) does nothing in Tape and says "T goes back to the <view>".
- Keys in Tape (`tui/app/keys.rs`, a `handle_tape_key` before the global keys, as the rack view's own keys are):
  - `h`/`l` or ←/→ select the loop;
  - `1`–`8` jump to a loop;
  - `space` turns the selected loop's `record` knob to 1 or 0 with no glide (`Request::Turn`, the console's usual path). It is the machine's record button, quantised to the loop's bar line by the loop itself (KZ-L02);
  - `e` asks "erase loop1? y/n" and sends `Request::Loop { op: Erase }` (KZ-L03);
  - `G` toggles plain glyphs;
  - `?` shows the table above.

  `r` keeps its global meaning (record the wall to a WAV file). Every other global key behaves as it does in the rack view.
- The selected loop is the console's selected module when that is a loop, else the first loop. Selecting here selects it in the rack too.

### Layout (`tui/tape/mod.rs`), into the body `Rect` (`draw/mod.rs`, where `View::Rack` draws)

From top to bottom:

1. **The head stack**: eight strips across the full width, one per loop in rack order. Empty places are drawn as blank strips, so the machine always has eight. Each strip is `width / 8` columns (12 at least, by the plain rule below), 6 rows:
   - the name in `BRASS`, or `TEXT_DIM` `—` for an empty place;
   - a 4 × 2-cell braille needle: `dial::small` (`tui/rack/dial.rs:159`), with the knob position replaced by the level fraction. `VU_MIN` (−20) maps to 0 and `VU_MAX` (+3) to 1, the desk's VU span (`kazoo-mix/src/meters.rs:16`, `:19`), copied as constants with a comment naming their source, because kazoo-wall does not depend on kazoo-mix. The ballistics are a pure `needle(prev: f32, target: f32, dt: Duration) -> f32`: first order with time constant `300 ms / ln 100`, reaching 99% of a step in 300 ms. That is the desk's VU (`meters.rs:24`), so a needle reads the same on the wall as on the desk;
   - the lamp and a word: `● REC`, `◌ ARM` (amber, blinking at 2 Hz from the frame clock), `▶ 17b`, or `· blank`;
   - `p12 w.30`;
   - a selected underline in `BRASS` under the selected strip.
2. **The transport**: the remaining height less 3 rows. Two reels, centred in the left and right thirds, with the head block (a 10 × 3 box, `STEEL`) between them, and the tape path as braille lines from each reel's pack edge to the head. Drawn in braille through `Dots` (below):
   - the reel radius in dots is `R = min(third_width × 2, rows × 4) / 2 − 1`, and the hub radius `R / 5`;
   - the supply reel's pack edge is at `hub + (1 − fill) × (R − hub)`, the take-up's at `hub + fill × (R − hub)`;
   - `fill = (bars written + the fraction of the bar under the head while rolling) / bars`, from `LoopView.written` and the interpolated beat;
   - the spokes are three, 120° apart, from the hub to the pack edge, at angle `2π × frac(beat_now / 4)` clockwise; the first spoke is `BRASS`, the rest `CREAM`; the pack outline is `TEXT`.
3. **The tape**: one row, the selected loop's bars as `▮`/`▯` in `SAGE`/`TEXT_DIM`, `RED` for bars being written now, and `▲` under the bar under the head. Past 1 cell a bar, bars share cells: written if any is.
4. **The counter**: one row: `bar 6/17 · pass 12 · 0:11.4 / 0:34.0 · from mouth/dry`.
5. **The foot**: one row: song position `bar.beat`, tempo, and the meet line from `Snapshot.loops_meet` (L05) shortened: `meet again in 52 bars (1:44)`, or `never meet at their seams`, or `meet practically never`.

### Braille drawing, reused

`Dots`, the layered braille grid inside `tui/rack/screen.rs` (`:42–115`), moves unchanged to `tui/rack/dots.rs` as `pub(crate) struct Dots`, with the same name and methods. `screen.rs` uses it from there. It gains one method, `circle(&mut self, layer, centre: (f64, f64), radius: f64)`: one dot per `1/radius` radians, so a circle of radius 40 dots costs 250 sets. The braille bits come from `dial::braille_bit` and `dial::braille` as they do now. Reason: one braille grid in the console, with its existing tests.

### Frame rate and CPU caps

- `pub const TAPE_FPS: u32 = 30;`. The console loop draws up to 60 times a second (`tui/mod.rs:56`). The tape view recomputes its dot grids only when one of these changes:
  - the angle, quantised to 1/48 of a turn;
  - the fill, to 1/96;
  - the size;
  - the selected loop;
  - the snapshot revision;

  and at most `TAPE_FPS` times a second. Otherwise it re-blits the cached cells. Reason: at 300 BPM a reel turns 1.25 times a second, and 30 frames gives 24 steps a turn: smooth to the eye. Recomputing at 60 would double the work for nothing seen.
- Trigonometry comes from a 256-entry sine table built once (`OnceLock`), never `sin`/`cos` per dot.
- Budget: at most 1 ms per drawn frame at a 200 × 60 terminal on Tom's M1 Max, when recomputing; under 0.2 ms when blitting. The bench `benches/tape_view.rs` draws into a ratatui `Buffer` both ways and reports both. BENCH.md gets the rows with the target and the reason: the console shares the machine with the wall's render thread, and a 1 ms frame at 30 fps is 3% of one core.

### The plain fallback (`tui/tape/plain.rs`)

It is used when the body is narrower than 100 columns or shorter than 30 rows, or when `Prefs.plain_glyphs` is set (`G`, saved in `console.json`), or when `KAZOO_PLAIN_GLYPHS=1`. Reason: there is no braille font everywhere, and at 80 × 24 a reel of a few dots tells nothing. Plain uses ASCII only:
- each strip is one row: `loop1  [####----]  REC  17b  p12`;
- each reel is `(|)`, `(/)`, `(-)` or `(\)` by the quarter-bar, with the pack `[=====.....] 50%`;
- the tape row is `#` and `.` with `^`;
- the counter and foot are as above.

Every reading in the table is still there. That is the test: the plain view names each reading the full view draws.

## Not this lane

The loom view (KZ-L13). The desk (KZ-L11 keeps it plain). Mouse handling in the tape view: clicks select a strip, and nothing else, because a strip's knobs are turned in the rack view. A click on a strip selects it, through a new `TapeHit::Strip(index)` in the console's hit map.

## Red tests first

In `tui/tape/tests.rs` with a `Snapshot` fixture holding three loops (L05's `LoopView`), rendered into `TestBackend`. Each fails today because there is no `View::Tape`.

1. `t_opens_the_tape_view_and_t_goes_back_where_it_came_from`, from both List and Rack.
2. `eight_strips_are_drawn_with_three_loops_and_five_empty`. The names, `—` five times, `REC` on the rolling one, and `17b`.
3. `the_spokes_turn_one_turn_per_bar`. At beats 0, 1, 2 and 3 the brass spoke's dot cells differ, and at beat 4 they equal beat 0's. With the tempo 0 (beat frozen), two frames 1 s apart are identical.
4. `the_take_up_pack_grows_as_the_loop_fills`. With `written` = 0 of 4, 2 of 4 and 4 of 4, the pack's outer radius (the outermost lit dot from the take-up centre along the horizontal) increases strictly, and the supply's decreases.
5. `the_tape_row_shows_written_bars_and_the_head`.
6. `space_turns_record_and_e_asks_before_erasing`. Check the jobs the app queues.
7. `nothing_is_recomputed_when_nothing_moved`. Two draws at the same beat, size and revision: the reel cache's recompute counter does not move. A beat a 1/48-turn later moves it once, and two draws 10 ms apart move it at most once (the cap).
8. `plain_glyphs_name_every_reading`. At 80 × 24, and with `plain_glyphs` at 200 × 60: no character in `U+2800..=U+28FF` is drawn, and the text holds each strip's name, level bar, state, length and pass, each reel's pack percentage, the tape row and the counter.
9. `the_tape_view_renders_at_many_sizes_without_panicking`. The size loop the rack tests use, with 0, 1 and 8 loops.
10. `needle_ballistics_match_the_desks_vu`. A step from 0 to 1 reaches 0.99 ± 0.005 at 300 ms, in steps of 16 ms.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tui kazoo-wall/benches
```

The bench, built and run once for the BENCH rows, with `uptime` before and after:

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench tape_view -j 4
```

## Definition of done

- [ ] The ten tests fail first (paste each under RED), then pass, and every existing console test passes: the `Dots` move changes no scope test.
- [ ] BENCH rows written for both the recompute and blit cases.
- [ ] Not the builder's: after landing and reinstalling, Cassio opens `T` at a full-size terminal with two loops playing and checks that the reels turn with the beat and stop with it, the needles move with the loops, and `space` arms the selected loop (amber, then red on the bar); then narrows the terminal below 100 columns and checks the plain view has every reading. Cassio shows Tom. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. Colours from `tui/theme.rs` only. Each new file under 1000 lines. `tui/app/keys.rs` is about 1220 lines and `tui/app/mod.rs` about 1650: they gain only the `View` variants, the `T` arm and the call to `handle_tape_key`, which lives in `tui/tape/mod.rs`. List both under FOUND with their line counts.

## END

```
START KZ-L12 base=<sha> branch=lane/kz-L12 df=<GB> at <time>
END KZ-L12 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
