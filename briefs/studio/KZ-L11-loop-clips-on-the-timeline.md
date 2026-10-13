# KZ-L11: on the desk's timeline a loop clip is drawn as one run of repeats with its seams marked, moved and deleted whole, and the tracking view stays plain

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 20 minutes of a builder's work. One mechanism: drawing and editing a loop clip on C07's timeline. Crates: kazoo-mix only (`src/ui/timeline.rs` from C07a, the clip editing from C07b, `src/tape.rs`'s peaks request). Depends: KZ-L10 (loop clips), KZ-C07a (the timeline view) and KZ-C07b (clip editing).

Read by Cassio, 14 Oct 01:3x. Changes: the checks are written out in the lane form, test 1's length text is exact, and the live check is Cassio's. The view keeps Tom's rule as written. Opens only after L10, C07a and C07b have landed.

## Tom's rule for this view

"The dressing should never overpower function. The tracking view is why I'm saying that: we need to be careful VU meters and such don't take over." The timeline is the standard tracking view, and it stays C07a's: lanes, clips, waveforms, a ruler and a playhead. A loop clip gets exactly two marks, its name with `↻` once and a thin seam tick. There are no reels, needles, packs or looms here: those live in the wall's tape view (KZ-L12, KZ-L13), which is reached on purpose and never by default. The lane height stays C07a's `LANE_ROWS = 3`, and no meter is added to a lane.

## The design (decided; build exactly this)

- **Peaks.** A loop clip's waveform comes from `<reels>/<hex>.peaks` beside its reel, through C07a's `DeckCommand::Peaks { path }` (its WAV path is `ReelStore::path`, L01). C07a writes the file beside the WAV, and L01's sweep deletes it with its reel. The clip's columns read `Peaks::columns` over the clip's first `N` frames. The seam bake (L10) is not in the peaks: it changes 8 ms, and that cannot be seen at a terminal column.
- **Drawing** (`ui/timeline.rs`): the repeats of one `LoopClip` (L10) are drawn as one run across `repeats × beats` beats, with:
  - the name row: `loop1-003 ↻17b` once, at the run's start, cut to the run's width; and `┊` in `TEXT_DIM` on the name row at every seam (between repeats);
  - the waveform rows: each repeat's waveform, as C07a draws a clip, in `SAGE`, or `BRASS` when the loop clip is selected (C07b), the whole run selected together;
  - a repeat narrower than 3 columns, when zoomed out: no seam ticks, only the run, so marks never crowd the waveform.
- **Editing** (C07b's keys):
  - move (`m` and the arrows, or drag) moves every repeat by the same beats;
  - delete removes them all, through L10's `d`;
  - trim and split are refused, with the footer note "a loop clip plays whole passes; < and > change how many", because a loop is its passes.
  - `<` and `>` work here on the selected loop clip as in L10's list.
  - Hit targets: `Clip { lane, id }` on any repeat selects the loop clip it belongs to.
- **Help:** the timeline group gains "↻: a loop from the wall, repeated; ┊ marks each pass".

## Not this lane

Everything on the wall's side. Desk automation of a loop's level: C10, which automates any strip, and the `loops` strip is one.

## Red tests first

UI tests with C07a's `render_with` and `text` helpers. Each fails today because loop clips are drawn as separate clips.

1. `a_loop_clip_is_named_once_with_its_seams_marked`. Three repeats of a 4-beat loop at 1 beat per column: the name appears once, `┊` at columns 4 and 8 from the run's start, and `↻1b` for its length (one bar).
2. `zoomed_out_a_loop_clip_drops_its_seam_ticks`.
3. `selecting_any_repeat_selects_the_whole_loop_clip`.
4. `moving_a_loop_clip_moves_every_repeat`. The `TakeBay` sees one `Move` per repeat, by the same beats.
5. `trim_and_split_are_refused_on_a_loop_clip_with_a_note`.
6. `a_lane_with_a_loop_clip_is_no_taller_than_any_other`. The lane is `LANE_ROWS` tall, and no cell outside the clip run changes compared with the same lane holding a plain take of equal span. This guards Tom's rule.
7. `the_timeline_renders_at_many_sizes_with_loop_clips` (C07a's size loop).

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] The seven tests fail first (paste each under RED), then pass.
- [ ] Not the builder's: after landing, Cassio places a loop clip with four repeats on the desk, checks it reads as one named run with three ticks, moves it a bar, and shows Tom. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The loop-clip drawing and its tests go in a new `ui/timeline/loops.rs`; `ui/timeline.rs` gains only the call.

## END

```
START KZ-L11 base=<sha> branch=lane/kz-L11 df=<GB> at <time>
END KZ-L11 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
