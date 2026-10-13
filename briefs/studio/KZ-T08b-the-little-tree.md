# KZ-T08b: the wall console's little tree: `Y` draws the whole tree in braille, the trunk growing up the middle and every bud branching off where it grew, thicker where more modules lived, with the marks, the lanterns and the wall's own place on it; every mark is a reading

Line numbers are from main at `820ccc5` and may have moved; find each item by what it does, never rename. Items from KZ-L12, KZ-T04d, KZ-T07d and KZ-T08a are named as those briefs name them (`Dots`, `Prefs.plain_glyphs`, `View::Tape`, `handle_tape_key`, `Mode::{Tree, Go, Lantern}`, `TreeList`, `TreeShape`, `Limb`, `Spot`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 14 Oct 02:5x, line by line. Checked test 2's depths (bud from trunk commit 10 starts at 11; a bud from its commit 4, at depth 15, starts at 16). Changes: the lane opens only after T08a, T07d and L12 have landed; the by-hand check is Cassio's and Tom's. Tom wants the tree badly and L12 sits at the end of the loop chain (L03, L04, L05), so: when T08a and T07d have landed and L12 has not, Cassio splits L12's `Dots` move, its plain-glyph switch and an empty `benches/tape_view.rs` into a small lane of their own, which both L12 and this lane then build on. Accepted otherwise as written.

Second of PLAN.md's T08 split in two (T08a the shape, **T08b** the view).

Time: about 40 minutes of a builder's work. One mechanism: a console view drawn from T08a's shape. Crates: kazoo-wall (`tui/` and one bench file). Depends on T08a, which must have landed; T07d, which must have landed ( lanterns open from here, and both edit `tui/app/mod.rs`, `tui/app/keys.rs`, `tui/draw/mod.rs`, `tui/draw/overlay.rs`); KZ-L12, which must have landed (or the small lane split from it, above) ( `Dots` in `tui/rack/dots.rs`, the plain-glyph switch `G` / `Prefs.plain_glyphs` / `KAZOO_PLAIN_GLYPHS`, the way a view is entered and left, and `benches/tape_view.rs`).

## Tom's words, and the rule

"I do like the idea of a tree visualization of some kind, even if it's just a little one … not required to play the music." Later, the Bevy tree: "thicker branches where more modules lived" (T09). And the rule for the whole console: "the dressing should never overpower function."

So the little tree is reached on purpose (`Y`), is never where the console opens, is never remembered between runs (`Prefs.rack` untouched), and every drawn thing carries a reading, listed in its help:

| mark | reading |
|---|---|
| the trunk, up the middle from the ground | `trunk`, oldest moment at the bottom, newest at the top |
| a branch leaving a line and rising beside it | a bud: it grew from the moment where it leaves, and its height is how many moments it holds |
| a line's thickness | modules alive there: thin under 24, medium under 56, thick from 56 |
| the bright line | the line the wall is on, from the ground up to where the wall is |
| `◉` | the wall: the moment it is at |
| `◆` | a mark |
| `▪` | a save's mark |
| `✦` `✧` `·` | a lantern: lit, dim, faint (it dims as the wall makes changes with no one visiting it) |
| a name at a tip | the branch: `trunk`, or a bud's moment and the time it grew |

Why 24 and 56: Tom's walls run from the 14-module seed to the 85-module evening wall (KZ-T01's fixture). Two thresholds split that range into three readable widths: a sparse wall, a working wall, a full one. One, two and three braille dots are the widths a braille cell can show side by side.

## The design (decided; build exactly this)

### Reaching it (`tui/app/mod.rs`, `tui/app/keys.rs`)

- `View` (`tui/app/mod.rs:115`) gains `Tree`. `App` gains `little: Little` (`tui/little/mod.rs`).
- `Y` from List, Rack, Tape or Loom (when L13 has landed) opens it and remembers where it came from; `Y` or `Esc` in it goes back. `v` in it does nothing and says "Y goes back to the <view>", as the tape view says.
- `Y` because it is free in every view and mode (`tui/app/keys.rs:90–141`; L12 `T`, `G`, `e`; L13 `W`; T04d `b`, `B`; T07d `n`), and a `Y` is a trunk that forks.
- Keys in the little tree (`handle_little_key` in `tui/little/mod.rs`, called before the global keys, as L12's `handle_tape_key` is): `j`/↓ and `k`/↑ move the selection; Enter acts on it (below); `r` fetches the shape again; `G` toggles plain glyphs (L12's toggle, the same preference); `y` and `n` answer the go question when it is asked. Every other key is the global key: `n` lights a lantern, `b` marks, `B` opens the list, `z` undoes, `?` helps.

### The shape, fetched (`tui/little/mod.rs`)

`pub struct Little { pub shape: Option<TreeShape>, pub fetching: bool, pub asked: Option<Instant>, pub revision: u64, pub selected: usize, pub asking: Option<(String, String)>, pub came_from: View, cache: Option<Drawn> }`.
- Opening fetches `Request::Shape {}`. While open, when the snapshot's revision differs from `revision` and `SHAPE_EVERY = Duration::from_secs(1)` has passed since `asked`, it fetches again. Reason: the shape changes only with commits, and T04b's listing allowance is 10 in a burst refilled at one a second; one a second keeps within it with room for `B` and `r`.
- `on_result` (`tui/app/mod.rs:1145`) gains the arm: `Request::Shape` → `TreeShape` kept, `fetching = false`, the selection held on the same point when it still exists (by kind and commit), else on the wall.
- Before the first answer the body says "asking the wall for its tree…"; with no tree, the refusal's sentence; an empty tree "the tree has no moments yet: the first change makes one".

### Geometry (`tui/little/mod.rs`, pure functions, tested without drawing)

The drawing area is the body less a header row on top and the legend and selection rows below, less a column each side. In braille, a cell is 2 dots wide and 4 tall: the grid is `W = 2 × cols` by `H = 4 × rows` dots, `y` downward.

- **Depth.** A limb's first commit is at depth `start = depth(from) + 1` (0 for a limb with no `from`); its commit `at` is at `start + at`. `top = ` the largest depth of any limb's last commit (at least 1).
- **Height.** `k = min(4.0, (H − 1) / top)` dots per moment; depth `d` is at `y = (H − 1) − d × k`. Reason: the whole tree always fits; a small tree is stretched to at most 4 dots (one row) a moment so it does not sit in a corner.
- **Lanes.** The trunk (limb 0) at `x0 = W / 2`. Limb `i ≥ 1`: right when `i` is odd, left when even, `r = ceil(i / 2)` lanes out, `x = x0 ± r × g`, with `g = max(4, (W / 2 − 2) / max(1, ceil((n − 1) / 2)))` dots for `n` limbs. When `(n − 1) / 2` lanes do not fit at 4 dots apart, `r` wraps (`((ceil(i / 2) − 1) mod m) + 1`, `m = max(1, (W / 2 − 2) / 4)`) and the header says "<n> branches share lanes: a wider terminal parts them". Limbs are in T08a's order (by when they grew), so older buds sit nearer the trunk.
- **A limb's path.** It starts at the point of its `from` on its parent's path (a limb with no `from`: at its lane on the ground). Its commit `j` is `t = (j + 1) × k` dots up from that start: while `t` is within the horizontal distance to its lane it rises diagonally (one dot across per dot up, towards its lane), after that straight up its lane. So every commit's height is its depth, on every limb.
- **Thickness.** Commit `j`'s width is `w = 1 + (m ≥ 24) + (m ≥ 56)` for `m = modules[j / stride]`; each segment is drawn once more one dot to the right when `w ≥ 2`, and one dot to the left when `w = 3`.
- **The bright line.** The limb holding `here`, up to `here.at`, and each of its ancestors up to the point it grew from, is drawn on `Dots` layer `A`; everything else on layer `B`.

### Drawing (`tui/little/draw.rs`, new)

- Braille when the drawing area is at least 40 × 9 cells and plain glyphs are off; plain otherwise (below). Lines through L12's `Dots::line`; cells coloured `CREAM` for layer `A`, `STEEL` for layer `B`.
- Points, each in the cell of its commit's dot, drawn over the braille, one per cell by priority: the wall `◉` (`CREAM`, bold, on `PANEL_FOCUS`), a lantern (`✦` `AMBER`, `✧` `STEEL`, `·` `TEXT_DIM`, by glow; the brightest when several share a cell), a mark `◆` (`BRASS`), a save's mark `▪` (`TEXT_DIM`). The selected point's cell is drawn reversed.
- Tip names: `trunk`, and for `bud/<from>/<time>` the mark's name (T04a's `name_of`) or the 12 hex of `<from>`, then the local `HH:MM` of `<time>` (the date when not today), at most 16 characters, two cells right of the tip when those cells hold no dot and no point, else two cells left, else not drawn.
- Header: `the tree · <commits> moments · <limb_count> branches · <lanterns> lanterns · <one moment a dot | <stride> moments a dot>`, then any overflow sentence (lanes shared; `limb_count` above the limbs drawn: "the oldest <n> buds are not drawn").
- Legend: `thin <24 modules · medium <56 · thick · ◆ mark · ▪ save · ✦ lantern (✧ dim · faint) · ◉ the wall`.
- Selection row, for the selected point:
  - the wall: `◉ the wall: #<n> <short> on <branch | off any line> · <m> modules`;
  - a lantern: `✦ lantern <6 hex> by <seat> (<glow>) · #<n> · ⏎ shows its way back`;
  - a mark: `◆ mark “<name>” · #<n> · ⏎ goes there`; a save's: `▪ save “<name>” · …`;
  - a tip: `<branch>: #<n> · <m> modules · ⏎ goes there`;
  - while asking: `go to <label>? the wall as it is is kept first; z undoes it · y goes · n stays`.
- The points of interest, in selection order: the wall, the lanterns, the marks, the saves' marks, the tips, sorted top to bottom, then left to right. `j` moves down the screen, `k` up.

**Enter.** On a mark, a save's mark or a tip: `asking = Some((moment, label))` (the mark's name, the save's tag, or the branch's name, as T04d's rows use them); `y` sends `Job::Call(Request::Go { moment })` with T04d's status words, and its answer is T04d's; `n` or Esc clears the question. On a lantern: T07d's `TreeList.visiting = Some(0)`, a new `TreeList.visiting_little = true`, and `Job::Call(Request::Visit { lantern })`. On the wall: nothing ("the wall is here").

**Lanterns opened from here** (`tui/app/lanterns.rs`, T07d's file): `Mode::Lantern` gains a field `little: bool`. T07d's `Request::Visit` arm also opens the lantern when `visiting_little` is set, the view is `View::Tree` and the mode is `Normal`, with `index: 0` and `little: true`, and clears both. `lantern_key`: Esc, `q` or Enter go to `Mode::Normal` when `little` (back to the little tree), else T07d's `Mode::Tree { index }`; `n` in a lantern opened from here says "B, then the lantern's row, adds a note" and stays. Its keys line, when `little`: `j k scroll · Esc back`.

### Frame cap and the cache

- `pub const LITTLE_FPS: u32 = 10;` The cells are rebuilt only when the shape, the size, the selection, the question or the plain switch changes, and at most ten times a second; otherwise the cached cells are drawn again. Reason: the content changes at most once a second (the fetch), and the selection with a key press; ten a second keeps a key press immediate to the eye.
- Budget: at most 2 ms a rebuild at 200 × 60 for a shape of a 6,400-commit trunk, 50 buds, 30 marks and 20 lanterns; under 0.2 ms a cached draw. Two rows in L12's `benches/tape_view.rs`, `little_tree/rebuild` and `little_tree/blit`; the numbers under BENCH for BENCH.md. Reason: the console shares the machine with the wall's render thread; ten 2 ms rebuilds a second is 2% of one core.

### The plain fallback (`tui/little/draw.rs`)

Below 40 × 9 cells, with `G`, or with `KAZOO_PLAIN_GLYPHS=1`: one character a point, the same geometry with a cell as the unit (`k` in rows a moment, at most 1). Lines: `:` thin, `|` medium, `#` thick going up; `/` and `\` rising diagonally; the bright line in `CREAM`, the rest in `STEEL`. Points: `@` the wall, `O` lit, `o` dim, `.` faint, `*` mark, `+` save's mark. The legend in the same characters. Every reading of the table is still there: that is the test.

### Help (`tui/draw/overlay.rs`)

After T07d's `n` line: `("Y", "the little tree: every branch as it grew, thicker where more modules lived, with marks, lanterns and where the wall is (j k choose · ⏎ go there or see a lantern · G plain · Y back)")`, and the table above as help lines under a `the little tree` heading.

### Dispatch (`tui/draw/mod.rs`)

`View::Tree` draws `little::draw(buf, body, app, now)` where `View::Rack` draws (`:75`); its log height is the rack view's (`:55`). `tui/mod.rs` gains `mod little;`.

## Not this lane

The shape (T08a). The Bevy tree (T09). Zoom, panning and the mouse: the whole tree always fits. Notes from the little tree (`B` has them). Remembering the view between runs: never.

## Red tests first

In `tui/little/tests.rs`, with `TreeShape` fixtures built in the test and rendered into `TestBackend`. Each fails today because there is no `View::Tree`.

1. `y_opens_the_little_tree_and_y_goes_back_where_it_came_from`: from List and from Rack; `Request::Shape {}` is sent on opening; `v` inside says so.
2. `a_bud_leaves_the_trunk_where_it_grew`: trunk of 40, a bud from commit 10 of 12, a bud from that bud's commit 4 of 3: the geometry functions give each commit's dot exactly (depths 0–39 on `x0`; the bud's first commit at depth 11, `k` dots right of `x0` (one dot across per dot up); the second bud's first commit at depth 16), and the rendered cells have dots there.
3. `thickness_is_modules_alive`: samples of 10, 30 and 60 modules give widths 1, 2 and 3 (the lit dots across the trunk's line at those heights).
4. `the_walls_line_is_bright`: with the wall on the second bud, the cells of trunk 0–10, bud 1's 0–4 and bud 2 up to `here.at` are `CREAM`, every other line cell `STEEL`.
5. `points_carry_their_readings`: `◉`, `✦`, `✧`, `·`, `◆`, `▪` in their cells and colours; two lanterns in one cell show the brighter.
6. `j_k_and_enter`: the selection goes wall, lantern, mark, tip by height; Enter on a mark asks, `y` sends `Request::Go { moment: "dawn" }`, `n` clears; Enter on a lantern sends `Request::Visit`, its answer opens `Mode::Lantern { little: true, .. }`, and Esc returns to the little tree, not the list.
7. `the_tree_always_fits`: a 6,400-commit trunk with stride 1 and 50 buds at 120 × 40: no dot outside the area, the header says lanes are shared, nothing panics; the same at every size the rack tests loop over.
8. `nothing_is_rebuilt_when_nothing_moved`: two draws with the same shape, size and selection: the rebuild counter does not move; a new shape moves it once; two draws 10 ms apart move it at most once.
9. `plain_glyphs_name_every_reading`: at 80 × 24 with `plain_glyphs` and at 200 × 60 with `plain_glyphs`: no character in `U+2800..=U+28FF` is drawn, and `@`, `O`, `o`, `.`, `*`, `+`, `:`, `|`, `#`, `/` and the legend are there.
10. `the_shape_is_asked_again_only_when_the_wall_moved`: snapshots with the same revision ask nothing; a new revision asks once a second at most.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tui kazoo-wall/benches
```
The bench: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench tape_view -j 4 -- little_tree`, once, for the BENCH rows.

## Definition of done

- [ ] The 10 tests fail first (paste each under RED), then pass, and every existing console test passes.
- [ ] Files: `tui/little/mod.rs`, `tui/little/draw.rs`, `tui/little/tests.rs`, `tui/mod.rs` (one line), `tui/app/mod.rs`, `tui/app/keys.rs`, `tui/app/lanterns.rs`, `tui/draw/mod.rs`, `tui/draw/overlay.rs`, `benches/tape_view.rs`: ten.
- [ ] BENCH rows for the rebuild and the cached draw, and whether the machine was quiet.
- [ ] Not the builder's: after landing and reinstalling, Cassio opens `Y` in Tom's console at a full-size terminal on his tree after the first rings (KZ-T03) and a few buds, and checks the trunk up the middle, the buds off it, the wall's line bright, a lantern and a mark on it; `j` to a mark, Enter, `y` (the wall goes there; a new bud appears with the next turn); then narrows the terminal and presses `G` for the plain tree with every reading. Cassio shows Tom. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. Colours from `tui/theme.rs` only. Each new file under 1000 lines. `tui/app/keys.rs` and `tui/app/mod.rs` gain only the `View` variant, the `Y` arm, the call to `handle_little_key`, the `little` field and the `Shape` result arm; `tui/app/lanterns.rs` only the `little` field's handling. List their line counts under FOUND. No trigonometry: every path is straight lines, diagonal or upright.

## END

```
START KZ-T08b base=<sha> branch=lane/kz-T08b df=<GB> at <time>
END KZ-T08b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the rebuild and the cached draw at 200 × 60
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
