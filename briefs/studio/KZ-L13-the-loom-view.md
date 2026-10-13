# KZ-L13: the loom view: the wall's voices are the warp, its loops the weft, woven bar by bar down the screen, with what has played above the line and what the loops will play below it

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: a per-`out` level from the engine, and a console view that weaves outs and loops over time. Crates: kazoo-wall only:
- `engine/mod.rs`: the out peaks;
- `protocol/mod.rs`: `ModuleView.level_db`;
- `daemon/wall.rs`'s snapshot;
- new `tui/loom/mod.rs`, `tui/loom/history.rs`, `tui/loom/tests.rs`;
- `tui/app/keys.rs`;
- `benches/tape_view.rs` (one more row).

Depends: KZ-L12 (the view plumbing: `View::Tape`, the tape keys, plain glyphs; this lane adds `View::Loom` the same way, with `W` from List, Rack or Tape) and KZ-L05 (`LoopView`, `beat_now`, `loops_meet`).

Read by Cassio, 14 Oct 01:5x. Checked: loops of 2 and 3 bars meet on bar 6, and of 3 and 4 bars on bar 12. Changes: the checks and bench are written out in the lane form; the out levels carry the order's sequence so a level is never credited to the wrong out between an order being sent and applied; the live check is Cassio's. Opens only after L05 and L12 have landed.

## Tom's words

"A loom-style concept, this idea of things being sort of woven together … something we can do with the interface for a looping thing." The loom is a concept that keeps coming back in his work. Here it is literal:
- the wall's long voices (each `out`, the sound the desk gets as a strip since B04) run down the screen as warp threads;
- each loop is a weft thread, passed across them on every bar it sounds.

Where a loop and a voice both sound, the threads interlace.

## Every mark is a reading

| mark | reading |
|---|---|
| a warp thread `│` and its shade | one `out`, its level in that bar: bright when loud (≥ −12 dBFS), `STEEL` when quiet (≥ −30), dotted `┆` when silent |
| a weft thread `━` in a loop's colour on a bar's row | that loop sounds in that bar |
| the weft passing *over* a warp thread | the loop was (above the line), or will be (below), louder than that voice in that bar; *under* means the voice is louder |
| no weft on a loop's row | that loop is silent in that bar: nothing written there, worn out, or its level at 0 |
| `◆` in a loop's colour in the left gutter | that loop starts a pass on that bar. Unequal loops' diamonds drift apart and meet: the 17-and-23 drift made visible |
| `▶` and the highlighted row | the bar playing now |
| rows above the line | what played, from the console's own history |
| rows below the line | what the loops will play, predicted from what they hold. A loop is deterministic (KZ-L02), so the future is known while nobody records |
| the foot | names and lengths of the loops, in their colours; `loops_meet` |

The help (`?`) shows this table.

## The design (decided; build exactly this)

### A level per out (engine)

- `Shared` (`engine/mod.rs:201` region) gains `out_peaks: Box<[AtomicU32]>` of `MAX_METERED_OUTS = 64` entries (f32 bits, non-negative). Reason for 64: a wall with more than 64 outs cannot be drawn as warp in any terminal, and the 65th on is shown without a level.
- In `render_sub_block`, where each `out`'s feed is summed into the master (the loop over `self.order.outs()`, `engine/mod.rs:1107`; after B04 or D02 move that sum, wherever each out's feed is read): for out index `i < 64`, `fetch_max` the bits of `max(|left|, |right|)` over the sub-block. Bit order matches value order for non-negative finite f32, and feeds are finite (the engine checks them).
- On a `Command::Order` swap, the engine stores 0 into all 64 before the next sub-block, so a peak is never credited to the wrong out when the order changes.
- The control side keeps the out order it last sent (`send_tables`: map `order.outs()` slots back to ids through `rack.id_at`), with a sequence number. `Shared` gains `order_applied: AtomicU64`, which the engine stores after it applies a `Command::Order` and clears the peaks. The snapshot fills levels only when `order_applied` equals the sequence of the order last sent; otherwise every out's `level_db` is `None` for that look. So a look taken between sending an order and the engine applying it never credits one out's peak to another. Test 1 gains that case. The snapshot swaps each entry to 0 and fills `ModuleView.level_db: Option<f64>`: dBFS, −120 for silence, `None` past 64 or for non-outs. It is `#[serde(default, skip_serializing_if = "Option::is_none")]`.

### History (`tui/loom/history.rs`)

- On every snapshot, the console folds each out's `level_db` and each loop's `peak_db` (L05) into the bar at the snapshot's beat: `bar = floor(beat / 4)`, keeping the maximum.
- `pub const LOOM_HISTORY_BARS: usize = 256;`: a `BTreeMap<String, VecDeque<(i64 bar, f32 db)>>`, trimmed to that many bars. Reason: at 60 rows and 1 bar a row, a third of the screen is past, and 256 bars lets `-` zoom out to 8 bars a row with the past still filled.
- A beat that jumps backwards (a desk locate) starts the history afresh.

### Prediction (`tui/loom/mod.rs`)

For a loop with tape and a song bar `b` at or after the current bar:
- `lb = floor(((4b − anchor) mod beats) / 4)`;
- it sounds when `written[lb]`, its bins over that bar (`bins[lb × 64 / bars ..= ((lb + 1) × 64 − 1) / bars]`, max) are above −50 dBFS after wear (`− 3 × wear × (age + passes ahead)`, KZ-L02's law), and its `level` knob is above 0;
- its predicted level is that peak plus `20 log10(level)`.

A loop that is `Rolling` predicts nothing past the next seam: its rows below that are drawn dim with `·`, because it is being recorded. "Louder than the voice" for a future bar compares with the voice's latest level in the history.

### Layout

- **Gutter**, 7 columns: the bar number every 4 bars, `▶` on the now row, and pass diamonds.
- **Top row**: each warp's name, cut to its column spacing.
- **Body**:
  - rows from the top: the now row sits at a third of the height;
  - each bar is `k` rows, where `k` is the number of loops with tape (at least 1, at most 8): one row per loop, so every weft has its own pick. Zoom `+`/`-` sets bars per row: 1, 2, 4 or 8, folding with the max;
  - the warp is spread evenly across the remaining width, at least 3 columns apart. Past that, the leftmost outs in rack order are shown, with `+k` at the right of the top row.
- **Foot row**: the legend and the meet line (L12's short form).
- Weft colour: `theme::CABLES[loop index % 10]`, the colour family the rack uses for cables.
- Keys in Loom: `W` or `Esc` back to the view it came from; `T` to the tape view; `h`/`l` select a loop (its legend entry is underlined); `space` and `e` as in the tape view; `+`/`-` zoom; `G` plain glyphs.

### Caps and the plain fallback

- `pub const LOOM_FPS: u32 = 10;`. The grid is rebuilt only when the bar, the zoom, the size or the snapshot revision changes, and at most ten times a second. Reason: its content changes at most once per look (every 100 ms, `tui/mod.rs:53`).
- Budget: at most 1 ms per rebuild at 200 × 60, as a bench row in `benches/tape_view.rs`.
- Plain glyphs (the same switch as L12): `|` for warp, `:` for silent warp, `=` for weft over, nothing for under, `*` for diamonds and `>` for now. Below 80 columns the body is too narrow for more than one warp and one weft, and the view says "the loom needs 80 columns" in place of drawing. The console's own minimum is already 80 × 24 (`draw/mod.rs:32`).

## Not this lane

The tape view (KZ-L12). Booking a solo and the "what's coming" of seqs (Tom's 13:52 idea): the loom shows what the loops will play, not the sequencers.

## Red tests first

Each fails today because there is no `level_db` and no `View::Loom`.

1. `engine` `each_out_reports_its_own_peak_and_an_order_change_clears_them`. Two outs fed 0.5 and 0.1: after a render, the entries hold those, and after a `Command::Order` they are 0 before the next render.
2. `daemon` `look_gives_each_out_its_level`.
3. `loom::tests::w_opens_the_loom_and_goes_back`.
4. `a_loop_that_sounds_in_a_bar_has_a_weft_on_that_bar_and_one_that_is_silent_has_none`. Fixture: a 2-bar loop with bar 0 written and bar 1 blank. Below the now row, wefts alternate with gaps.
5. `the_weft_passes_over_a_quieter_voice_and_under_a_louder_one`. With a loop at −10 dB and outs at −20 and 0 dB: at the first warp the cell is the weft glyph, at the second the warp glyph.
6. `diamonds_mark_each_pass_and_unequal_loops_drift`. Loops of 2 and 3 bars anchored at 0: diamonds on bars 0, 2, 4, … and 0, 3, 6, …, together on bar 6.
7. `a_rolling_loop_predicts_nothing_past_its_next_seam`.
8. `history_keeps_256_bars_and_a_locate_starts_afresh`.
9. `the_loom_rebuilds_at_most_ten_times_a_second`.
10. `plain_glyphs_keep_every_reading`. No box drawing or braille characters, and the same over/under and diamond cells as test 5 and test 6 in their plain glyphs.
11. `the_loom_renders_at_many_sizes_without_panicking`, with 0 outs, 0 loops, 70 outs and 8 loops.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-wall/benches
```

The bench row, run once with `uptime` before and after: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench tape_view -j 4`. The engine change runs under the crate's `assert_no_alloc` tests: `fetch_max` allocates nothing.

## Definition of done

- [ ] The 11 tests fail first (paste each under RED), then pass.
- [ ] A BENCH row for the loom rebuild.
- [ ] Not the builder's: after landing and reinstalling, Cassio puts two loops of 3 and 4 bars on a wall with two outs, opens `W`, checks that the diamonds drift and meet at bar 12, that wefts show where the loops sound, and that the line moves down the past as the music plays, and shows Tom. The builder never touches the live wall, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `engine/mod.rs` gains only the field, the `fetch_max` lines and the clear on `Order`. List it under FOUND with its line count. Colours from `tui/theme.rs` only.

## END

```
START KZ-L13 base=<sha> branch=lane/kz-L13 df=<GB> at <time>
END KZ-L13 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
