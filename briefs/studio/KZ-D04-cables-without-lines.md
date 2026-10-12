# KZ-D04: a cable with nothing to delay passes its signal by index; only a delayed cable owns a line

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. Crate: kazoo-wall. Depends: D02 (gathers by mask; `wall/cables/10000` baseline from D01).

## Outcome

Every cable owns a 2,048-frame delay line (8 KiB) that is written and read back every sub-block, at delay 0 too, through a cubic interpolator (`engine/lines.rs:44, 259–308`): 10,000 cables are 80 MB of lines thrashing the cache for nothing (survey §5.4). After this lane a cable whose delay is 0 reads its source's output block directly and scales it; a line exists only at a jack whose cable has ever needed a delay (latency compensation, `daemon/timing.rs`), and it lives with the jack in the page, so it goes wherever the page goes (D06 moves pages between threads). The number: `wall/cables/10000` at least 3× faster than D01's baseline; the engine's line count (`look`'s engine health) for that wall falls from 10,000 to 0. Feedback cables need no line: a back-edge reads the source's output buffer, which still holds the previous sub-block because the source has not rendered yet this block; that is exactly the one-sub-block delay `processing_order` promises (`engine/tables.rs:239–244`).

## The design (decided; build exactly this)

**1. Lines live in the page, one place per jack (`engine/room.rs`, `engine/lines.rs`).** `Page` (`room.rs:43–53`) gains `lines: Box<[Option<Box<Line>>]>` with `SLOTS_PER_PAGE × MAX_JACKS` places (64 × 28 = 1,792 pointers, 14 KiB per page), all `None` from `Page::new`. `Line` (`lines.rs:107–111`) becomes `pub`, with `pub fn new(sample_rate: f32) -> Option<Self>` (control side; `None` when there is no memory) that carries the glide bounds `Lines::new` computes today (`lines.rs:202–217`: `shortest`, `longest`) as fields of the line. `Line::carry(&mut self, route: &Route, source: &Block, out: &mut Block)` is `Lines::carry` (`lines.rs:259–308`) moved onto the line, minus the page lookup; `glide_block`, `gate_block`, `between`, `at` unchanged. `Lines`, `MoreLines`, `LINES_PER_PAGE`, `pages_for`, `Lines::grow` are deleted, and with them `Command::Lines`, `Retired::Lines`, `EngineControl::reserve_lines` (`mod.rs:1387–1400`), the `reserve_lines` call in `EngineControl::send` (`mod.rs:1423`), `INITIAL_LINES`, `Engine::lines`, and the `lines` arguments of `engine()`. `Shared::lines` (`mod.rs:210–211, 309–311`) stays and now counts lines in place (incremented when a `Line` lands, decremented on `Unline`), so `look`'s engine health keeps a number that means something.

**2. Two commands place and take lines (`engine/mod.rs:102–155, 863–913`).** `Command::Line { slot: usize, jack: usize, line: Box<Line> }`: the engine puts the line at `(slot, jack)`; a line already there is retired (`Retired::Line(Box<Line>)`), and so is the new one when the slot has no page (counted in `ignored`). `Command::Unline { slot, jack }`: the line there, if any, is retired. Both are real-time safe: one pointer moves. `EngineControl::send` for `Command::Line` reserves the slot (`reserve_slots(slot + 1)`) as `Insert` does, so a line never arrives before its page.

**3. `Route` loses its line number (`engine/tables.rs:18–53`).** `Route { slot, port, amount, tag, delay, gate }`: `line` goes; `tag` stays and means what it means now (`tables.rs:28–30`: a line that changes hands starts empty; the engine compares `route.tag` with the line's `tap.tag` and clears it, as `carry` does at `lines.rs:277–287`). `CableTable::lines` and `lines()` (`tables.rs:66–67, 165, 198–200`) go. `Route::new(slot, port, amount, tag)`.

**4. The gather chooses (`engine/mod.rs:1309–1329`).** For a plugged jack whose route and source resolve: if `page.lines[at * MAX_JACKS + jack]` is `Some(line)`, `line.carry(&route, source, buffer)` (the line applies the delay, glides a changed delay, switches gates between pulses, scales by `amount`, passes `finite`); else the direct path: `for (o, s) in buffer.iter_mut().zip(source) { *o = finite(*s * route.amount) }`. The direct path is bit-identical to what a line at delay 0 and `t == 0` produces (`between`, `lines.rs:324–326`, returns `near` unchanged, then `finite(sample * amount)`), so a cable that gains a line at delay 0 and later loses it sounds the same either side. The borrow: `render_module` already splits its work into "gathers over `&self.room`" then "the module over `page_mut`"; the line is in the *destination's* page, and the gather reads the *source's* outputs: when both are the same page, `room.page_mut(slot)` for the line and `room.output(source)` for the source alias. Resolve it by taking the line out for the gather: `let mut line = page.lines[place].take()` (a pointer move), gather with `&self.room` and `line.as_mut()`, then put it back. No allocation, no copy, no unsafe.

**5. The control side gives a line only when a delay asks for one (`daemon/wall.rs`).** `Wall.lines: BTreeMap<u32, (u32, u32)>` (`wall.rs:473`) becomes `lined: BTreeMap<u32, u32>`: cable id → the tag its line was placed under. `assign_lines` (`wall.rs:1644–1665`) becomes `place_lines`, run inside `send_tables` after `timing` is known (line 1519) and before the routes are built: for every cable with `timing.delays[id] > 0` (or `gate`, see below) and no entry in `lined`, take a fresh tag (`line_tags`, as now), build `Line::new(rate)` (say `eprintln!` and skip the line when it is `None`: the cable then plays uncompensated and is listed under `timing.uncompensated`), and send `Command::Line { slot: to, jack, line }` *before* the `Cables` command of the same change; record `lined[id] = tag`. A cable that has an entry keeps it for its life even when its delay returns to 0 (its tap glides to 0 through the line, as `carry` does today). On every change, cables no longer in the patch (compare `lined`'s keys with the patch's cable ids, as `assign_lines` does at 1645–1646) get `Command::Unline` for their old `(slot, jack)`; a cable replaced in a jack by another (`What::Patch { replaced: Some(..) }`) likewise unlines the old before the new is placed (the new route's tag differs, so the engine would clear the line anyway; unlining keeps `lined` and the engine agreeing). `swap_engine` (`wall.rs:1399–1430`) clears `lined` with the rest (line 1405) and relies on `Remove` of every slot: `Engine::remove` (`mod.rs:991–1002`) must also retire the slot's 28 line places, so a removed module takes its lines with it. Routes are built with `Route::new(from, port, amount, tag)` where `tag` is `lined[id]` or 0 for a cable with no line (a tag of 0 never matches a line's tap, and a cable with no line never carries through one). The `reserve_lines` call in `room_to_grow` (`wall.rs:2507–2514`) goes. Gate cables: a gate's delay change must switch between pulses (`lines.rs:403–427`), which needs a line; a gate cable gets its line on the same rule (delay > 0) since at delay 0 a gate has nothing to switch.

**6. The bench support and the engine tests.** D01's `kazoo-wall/benches/support/mod.rs` and the `Model` in `engine/tests.rs:63–86` build routes with `Route::new(from, port, amount, line, tag)`: both change to the new signature; neither places lines (their patches have no latency). The existing lines tests (`engine/lines/tests.rs`) move onto `Line` and keep their assertions.

**Why.** A line per jack in the page rather than a line per cable in a separate table: the destination owns the delay (it is the arriving signal that is held back), the page is the unit D06 hands between threads, and a per-jack table is bounded (28) while per-cable fan-out is not. Keeping a line for the cable's life once it has had one: a line's glide to 0 needs the line, and freeing and re-placing lines as a delay flickers around 0 would churn. Direct reads for feedback: the engine already guarantees the previous block is there.

## Not this lane

No change to `timing.rs` (what delays are) or to `latencies()`. No incremental tables (D05). No threads (D06). No SIMD (D11; the direct path's loop is where D11's first vector goes). No change to `dsp/`.

## Red tests first

1. `engine::tests::a_cable_at_delay_zero_reads_its_source_directly_and_owns_no_line`: `vco → out`, no line placed: `Shared::lines() == 0`, the out is heard, and the signal equals the vco's output block scaled by `amount` bit for bit (a test module as the source writing a known ramp). Red: `Route::new` still takes a line; `Shared::lines()` reads 256.
2. `engine::tests::a_line_placed_at_a_jack_delays_and_a_removed_module_takes_it_with_it`: `Command::Line` at `(slot, 0)` then a route with `delay: 100`: the signal arrives 100 frames late; `Shared::lines() == 1`; `Command::Remove { slot }` retires the line over the retire ring (`Retired::Line`) and the count is 0. Red: no commands.
3. `engine::tests::a_feedback_cable_hears_the_previous_sub_block_without_a_line`: `a → b → a` where `a` adds its input to a constant: the value grows by one sub-block's step per sub-block, exactly as the same test with lines did on e0de0c0 (write the expectation from a run on e0de0c0 first, in a comment with the numbers). Red: compile.
4. `daemon::wall::tests::only_a_delayed_cable_gets_a_line_and_keeps_it`: the seed wall (its delay and reverb carry latency): after start, `Shared::lines()` equals the number of cables with `timing.delays > 0`; a plain `lfo → vcf.cutoff` cable adds no line; removing the reverb unlines its cables (`look`'s engine `lines` falls). Red: field names.
5. `engine::tests::random_patching_never_allocates_on_the_audio_thread` extended: random `Line`/`Unline` commands in the mix; still no allocation.

Bench before and after: `cargo bench -p kazoo-wall --bench wall_render -- 'wall/cables'`, four rows, in BENCH.md and the END block; and the engine's line count for `wall/cables/10000` (print `Shared::lines()` from the bench once).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render -- 'wall/cables'
```

## Definition of done

- [ ] Tests 1–4 red then green; 5 green; every lines test green on `Line`.
- [ ] `wall/cables/10000` at least 3× faster than its D01 row; all four `wall/cables` rows before and after in BENCH.md and the END block; line count 0 for that wall.
- [ ] `Lines`, `MoreLines`, `Command::Lines`, `Retired::Lines`, `reserve_lines`, `INITIAL_LINES`, `Route::line`, `CableTable::lines` are gone; `Shared::lines` counts lines in place.
- [ ] `DESIGN.md` "Engine" paragraph on delay lines (`design/wall/DESIGN.md:118`) rewritten in one sentence to say a line exists only at a jack whose cable has needed a delay.

## Code quality

Tree rules. Nothing allocates, locks or does I/O on the render thread: lines are built on the control side and move as pointers; the gather's `take`/put-back is two pointer moves. Unsafe forbidden: no raw pointers to sidestep the page borrow; the take-and-put-back is the way. NaN/Inf defence: both gather paths end in `finite`. `engine/mod.rs` must not grow: the removed `Lines` plumbing pays for the two commands. Files under 1000 lines. One behaviour per commit.

## END

```
START KZ-D04 base=<sha> branch=lane/kz-d04 df=<GB> at <time>
END KZ-D04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: wall/cables/10, 100, 1000, 10000 before and after; the line count at 10000
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
