# KZ-T05a: the engine can glide a cable's amount to a new value, or bring a new cable in from silence, over any number of frames, and glide the tempo, with nothing on the audio thread that allocates, locks or can turn NaN

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

PLAN.md's T05 ("the crossing: going to a moment glides, fades and crossfades by birth mark over N beats") is three lanes, because it touches more than ten files: **T05a** (this: the two engine mechanisms a crossing needs), **T05b** (the wall crosses: matching by birth, the plan, the fades, the cost), **T05c** (the seats and the console choose the length). They run in that order.

Read by Cassio, 13 Oct evening, line by line. The arithmetic was checked: with the decrement before the step, frame `i` of a glide from silence plays `(i + 1) / glide`, and the last frame lands on the aim exactly. Change: red test 2's frame numbers now follow that (it said frame 0 is 0; it is `1 / glide`). Accepted otherwise as written.

Time: about 35 minutes of a builder's work. One mechanism: glides the engine runs itself, so the control side sends one command and the audio thread does the rest. Crates: kazoo-wall (`engine/` only). Depends: nothing beyond main (KZ-D03b, when it lands first, edits `engine/mod.rs` and `engine/room.rs` too; start from whichever is on main, and keep its `costs`/`timed` lines as they are).

Files (six):
1. `kazoo-wall/src/engine/tables.rs`: `Route` gains `glide`.
2. `kazoo-wall/src/engine/lines.rs`: `Tap` gains the cable's gain and its glide; `Lines::carry` applies it.
3. `kazoo-wall/src/engine/lines/tests.rs`: the cable glide tests.
4. `kazoo-wall/src/engine/mod.rs`: `Command::TempoGlide`; the engine's tempo glide.
5. `kazoo-wall/src/engine/tests.rs`: `mod glide;` beside `mod sparse;` (`:7`).
6. `kazoo-wall/src/engine/tests/glide.rs` (new): the tempo glide tests.

## Outcome

- A cable's amount no longer has to jump. `Route` carries `glide`, a number of frames. When the engine sees a route whose amount differs from what the cable is playing at, it moves there in a straight line over `glide` frames, frame by frame, from wherever it is now. When it sees a cable for the first time (a new tag on its line) and `glide` is above 0, the cable starts silent and comes in over `glide` frames. `glide` 0 is exactly today's behaviour, bit for bit: the amount is taken at once, and a new cable starts at its amount.
- The tempo can glide: `Command::TempoGlide { bpm, frames }` moves the wall's own tempo in a straight line (in BPM) to `bpm` over `frames` frames, sub-block by sub-block. `Command::Tempo` and a desk taking the tempo stop it where it is.
- Nothing else in the engine changes. The control side decides every number (T05b).

## The design (decided; build exactly this)

### Why these two, and why in the engine

T05b's crossing fades a module in or out by its cables: every sound reaches the master through at least one cable into an `out` (`render_sub_block`, `engine/mod.rs:1113–1123`, sums only `Kind::OUT` slots, and an `out` hears only what is plugged into it), so a module whose every cable has faded to nothing is silent, and one whose cables fade in from nothing fades in. That needs no per-module gain, no inserted VCA (which would change the patch and the tree's files), and no change to the render order. It needs cables whose amount the engine moves over time.

The engine moves it, not the control side, because the control side wakes every 20 ms at most (`Wall::tick`, `daemon/wall.rs:4383`) and each cable table it sends is a whole table (`Command::Cables`, `engine/mod.rs:879`): stepping 16 beats of fade from the control side would be a zipper of 400 steps and 400 tables. One table with a glide per route, and the audio thread interpolates per frame. The same reasoning holds for the tempo: one command, the engine moves it per sub-block.

### The cable's glide (`tables.rs`, `lines.rs`)

- `Route` (`tables.rs:39`) gains, last:
  ```rust
  /// Frames over which the cable moves to `amount` when the engine first
  /// sees this amount on it: 0 takes it at once. A cable new to its line
  /// (see `tag`) with a glide above 0 starts silent and comes in over it.
  pub glide: u32,
  ```
  `Route::new` (`:64`) sets `glide: 0`. Every `Route { .. }` literal in the tree already ends in `..route` or `..Route::new(…)` (`daemon/wall.rs:1591`, `tables.rs:144`, `:498`, `:507`, `:517`, `lines/tests.rs:10`, `:59`, `:312`, `:376`, `:390`, checked), so none needs touching. `CableTable::set` (`:138`) leaves `glide` as it is.
- `Tap` (`lines.rs:81`) gains four fields, in `Tap::EMPTY` (`:96`) as written here:
  ```rust
  /// What the cable is multiplied by now.
  gain: f32,            // EMPTY: 0.0
  /// The amount it is moving to (or rests on): the last amount it saw.
  aim: f32,             // EMPTY: 0.0
  /// How far `gain` is from `aim` per frame left, while moving.
  gain_step: f32,       // EMPTY: 0.0
  /// Frames left to reach `aim`; 0 at rest.
  gain_left: u32,       // EMPTY: 0
  ```
- `Lines::carry` (`lines.rs:259`):
  1. Where it makes a fresh tap for a new tag (`:278–287`), set `gain` and `aim` to `0.0` when `route.glide > 0`, else both to `route.amount`.
  2. Then, before the per-frame loop: if `route.amount.to_bits() != tap.aim.to_bits()`: with `route.glide == 0`, `gain = aim = route.amount`, `gain_left = 0`; else `gain_step = (route.amount - tap.gain) / glide as f32`, `gain_left = route.glide`, `aim = route.amount`.
  3. The loop that scales by the amount (`:303–305`) becomes, per sample: if `gain_left > 0`, `gain_left -= 1` and `gain = (-gain_step).mul_add(gain_left as f32, aim)`; then `*sample = finite(*sample * gain)`. Computing `gain` back from `aim` and the frames left, rather than adding `gain_step` each frame, means no rounding piles up over a long glide and the last frame lands on `aim` exactly. A `gain` that is not finite (it cannot be: `aim` is finite because `CableTable::set` refuses a non-finite amount, `:139–142`, and `glide` is at least 1 here) is set to `aim` and `gain_left` to 0, so a NaN can never pass.
  4. A table sent again with the same amount for a cable leaves a glide under way running: only a new amount (by bits) starts one. This is what lets T05b send tables mid-crossing (a seat's change) without restarting any fade.
- Why per frame and linear in amount: a fade of a cable is a fade of a signal level, and per-frame steps of 1/N are inaudible at any N (a 16-beat fade at 120 BPM is 384,000 frames at 48 kHz). The delay glide beside it (`glide_block`) is per frame for the same reason. The amount is linear and not equal-power because T05b never sums a fading-out and a fading-in cable into the same jack (a jack takes one cable, `CableTable`): each fade is a level, not one half of a crossfade.
- `glide` frames as `f32` lose precision above 2^24 frames (349 s at 48 kHz). T05b's longest glide is 64 beats at 20 BPM, 192 s, under it at 48 kHz and over it at 96 kHz and up, where the per-frame gain is still within 1e-6 of the line and the last frame exact. Write the cast as the tree writes such casts, with the comment (`engine/mod.rs:1034–1036`).

### The tempo's glide (`engine/mod.rs`)

- `Command` (`:102`) gains, after `Tempo`:
  ```rust
  /// Move the wall's own tempo in a straight line to `bpm` over `frames`
  /// frames, a sub-block at a time; 0 frames is `Tempo(bpm)`. A `Tempo`
  /// stops it where it is, and so does the desk setting the tempo.
  TempoGlide {
      /// Where it ends up, in beats per minute (held to the wall's range).
      bpm: f64,
      /// How long it takes.
      frames: u32,
  },
  ```
- `Engine` (`:519`) gains `tempo_aim: f64`, `tempo_step: f64` (BPM per frame) and `tempo_left: u32`, made as `bpm`, `0.0` and `0` in `engine()` (`:659`).
- `drain_commands` (`:865`): `TempoGlide { bpm, frames }` with a non-finite `bpm` is counted in `ignored` (`:894`) and changes nothing; with `frames == 0` it is `Tempo(bpm)`; else `tempo_aim = clamp_bpm(bpm)`, `tempo_step = (tempo_aim - self.bpm) / f64::from(frames)`, `tempo_left = frames`. `Command::Tempo` (`:904`) also sets `tempo_left = 0`.
- The step is a pure function beside `follow` (`:1289`), so it is tested as `follow` is (`engine/tests.rs:629`):
  ```rust
  /// The tempo after one sub-block of a glide: `(bpm, frames left)`. A desk
  /// setting the tempo (`desk`) ends the glide where the tempo is.
  #[must_use]
  pub fn glide_tempo(bpm: f64, aim: f64, step: f64, left: u32, desk: bool) -> (f64, u32)
  ```
  `desk` or `left == 0` gives `(bpm, 0)`. Else `n = left.min(SUB_BLOCK as u32)` (32: lossless), `left - n`, and the tempo `aim` when that is 0, else `clamp_bpm(step.mul_add(f64::from(n), bpm))`.
- `render_sub_block` (`:1098`), right after `self.follow_desk()` (`:1104`) and before the `Tick` is made: `(self.bpm, self.tempo_left) = glide_tempo(self.bpm, self.tempo_aim, self.tempo_step, self.tempo_left, desk)`, where `desk` is `self.following_desk` or a transport change fell due in this call of `follow_desk` (have `follow_desk` return whether `change` was `Some`). The sub-block then plays at that tempo (its `Tick` is made from `self.bpm`), and `shared.bpm` is stored as now (`:1136`).
- Why per sub-block: the beat advances a sub-block at a time already (`tick.beat_at(SUB_BLOCK)`, `:1130`), and 32 frames is 0.7 ms; a tempo that moves in 0.7 ms steps is a smooth ritardando.

## Not this lane

- Deciding which cables glide, by how much, and when (T05b). No change to `daemon/`.
- A glide on a knob: knobs glide already (`Command::Knob`, `Glide`, `:470`).
- Per-module gain, VCAs, or any change to the master or to `Hush`/`Unhush`.

## Red tests first

Cable tests in `engine/lines/tests.rs` beside `the_amount_scales_what_arrives` (`:56`), with its `run` helper; tempo tests in `engine/tests/glide.rs` driving `engine()` and `Engine::render` as `engine/tests.rs` does (`:372`). Each fails today because `Route::glide`, `Command::TempoGlide` and the tap's gain do not exist.

1. `a_glide_of_nothing_is_todays_cable_bit_for_bit`: the same signal through a route with `glide: 0` and through today's `carry` behaviour (an amount of -0.5 then 0.75 on the same tag) gives the same output bits sample for sample, a new tag starting at its amount.
2. `a_new_cable_with_a_glide_comes_in_from_silence`: a new tag, amount 1.0, `glide: 4_800`, a constant 1.0 signal: frame 0 is `1 / 4_800` (within 1e-7), frame 2,399 is within 1e-6 of 0.5, every frame from 4,799 on is exactly 1.0, and the output never falls.
3. `a_new_amount_glides_from_where_the_cable_is`: at rest at 1.0, a route of 0.0 with `glide: 1_000`, then, 500 frames in, a route of 0.5 with `glide: 1_000`: the second glide starts from the gain reached (0.5 ± 1e-6) and ends on 0.5 exactly 1,000 frames later; a fade to 0.0 ends on exactly 0.0.
4. `the_same_amount_sent_again_does_not_restart_a_glide`: mid-glide, the same route sent every sub-block: the glide ends at the frame it would have, to the frame.
5. `a_gliding_cable_never_passes_a_nan`: a NaN and an Inf in the signal mid-glide give 0.0 out for those frames (`finite`, as today) and the glide carries on; the gain stays finite throughout (read it through the output of a 1.0 signal).
6. `a_long_glide_lands_exactly`: `glide: 20_000_000` (past 2^24): the gain after `glide` frames is exactly the amount. Run it on a 1-frame-per-sample signal of 1.0 with `run` in sub-blocks; assert only the last sub-block and the monotonic rise, to keep the test under a second.
7. `the_tempo_glides_a_sub_block_at_a_time` (in `tests/glide.rs`): at 120 BPM, `TempoGlide { bpm: 90.0, frames: 48_000 }`, render 24,000 frames: `shared().bpm()` is within 0.1 of 105; render 24,000 more: exactly 90; the beat has advanced by the integral of the tempo over the second (within one sub-block's beats).
8. `a_tempo_or_the_desk_stops_a_tempo_glide`: through the engine, mid-glide, `Command::Tempo(100.0)`: 100 from the next sub-block and no further movement. Through `glide_tempo` as `the_wall_follows_a_playing_desk_and_keeps_its_own_beat_otherwise` (`engine/tests.rs:629`) tests `follow`: `desk: true` returns the tempo it was given and 0 frames left; the last step lands on `aim` exactly; a step past `MAX_BPM` is held to it.
9. `a_tempo_glide_to_nonsense_is_ignored`: `TempoGlide { bpm: f64::NAN, .. }` changes nothing and counts one `ignored`; `bpm: 1_000.0` glides to `MAX_BPM`; `frames: 0` is `Tempo`.
10. `gliding_allocates_nothing`: a crossing's worth of glides (64 cables gliding in, 64 out, a tempo glide) rendered through `engine/tests.rs`'s `render` helper (`:103`), which runs the engine under `assert_no_alloc` (`on_audio_thread`, `:13`; `tests/glide.rs` is a child of `engine::tests`, so it calls `super::render`), for 96,000 frames.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/engine
```
Doc tests, because `Route`, `Command` and the tap's docs change: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

Bench, because `carry` is on every cable's path every sub-block: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench wall_render -j 4 -- 'wall/chains/1000$|wall/chains/10$'`, base then head, three rounds alternating, `uptime`'s load average beside each row. The head's median must be within 3% of the base's: the added work at rest is one bits compare per cable per sub-block and one branch per sample.

## Definition of done

- [ ] The 10 tests fail first (paste each under RED), then pass; every existing engine and lines test is green unchanged.
- [ ] `Route::glide`, `Command::TempoGlide` and the tap's `gain`, `aim`, `gain_step`, `gain_left` exist as named, each with its doc comment.
- [ ] The bench table, base and head, three rounds, with load averages, in the END block and under a `## KZ-T05a` section of `briefs/studio/BENCH.md` (that file is the seventh; it holds numbers only).
- [ ] `git diff --stat` shows nothing outside `kazoo-wall/src/engine/` and `briefs/studio/BENCH.md`.

## Code quality

As PLAN.md's STACK RULES. On the audio thread: no allocation, lock, format or I/O; float arithmetic and integer counters only. `engine/mod.rs` (1,514 lines) gains only the variant, the three fields, the drain arm, the `Tempo` arm's line and the glide step in `render_sub_block`: list its line count under FOUND. No `as` cast clippy pedantic flags without the tree's comment.

## END

```
START KZ-T05a base=<sha> branch=lane/kz-T05a df=<GB> at <time>
END KZ-T05a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: base and head, three rounds, with load averages
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
