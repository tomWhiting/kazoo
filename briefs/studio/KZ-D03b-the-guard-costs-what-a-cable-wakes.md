# KZ-D03b: the load guard costs every module a change wakes, at what each one costs, and shares the load over heard modules only

Line numbers are from the sweep that wrote this brief, at `4c2b6a0` (D03 merged at `247ca07`), and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 45 minutes of a builder's work, plus the bench runs. One mechanism: the guard's costing of a change. The measured cost per module (step 1) is part of it, because "its measured cost" has nothing to read without it. Crates: kazoo-wall only. Depends: D03 (merged at `247ca07`: `Wall::unheard`, `heard_order`, `tables::heard`).

## Outcome

Found by the Opus review of KZ-D03: the load guard shares the measured render load over every module and cable on the wall (`held()`, `daemon/wall.rs:1332`), heard or not. Since D03, an unheard module costs nothing to render. So a wall with many unheard modules has a low average per module, and each addition is costed low. Worse, a seat can add 40 heavy modules while nobody hears them, then plug one cable that makes all 40 heard at once. The guard costs that cable as one unit at the average (`render_room`, `wall.rs:2668`, projection at `:2714`). The render jumps past the guard, and the wall drops out for everyone listening.

After this lane:
- The load is shared over heard modules and the cables into them only.
- Every change that can make a module heard (a patch, an add, a chain, an undo, a load, or a load undone) is costed after it is made to the patch and before it is logged. The cost covers the whole set of modules it makes heard, each at its own measured cost when it has one, else at its kind's cost from the bench.
- A change whose woken set would take the render past the guard is refused with the guard's `full`. The refusal says how many modules it would wake and the load it would take the wall to, and nothing is changed.
- A cable waking a cheap set goes through, as it should.

## What is wrong today (each found in the tree)

- `held()` (`wall.rs:1332`) is `modules().len() + cables().len()`. The `LOAD_GUARD` doc (`wall.rs:64–66`) says so: "Unheard modules and cables still count as held". D03's brief (step 5) argued this "errs toward refusing early", but it does the opposite. Unheard modules add nothing to the load, so they dilute the average the next addition is costed at.
- `room_to_grow` (`wall.rs:2549`) returns `Ok` at once when `growth == Growth::NONE`, and `render_room` returns `Ok` when `units == 0` (`:2669`). So a cable moved into a jack already in use is never costed, though it can wake any number of modules.
- An undone `Unpatch` is costed as one cable (`wall.rs:2896–2899`) and an undone `Remove` as one module and its cables, whatever they wake. A `ChainAdd` is costed as its own modules and cables at the average (`chains.rs:138`).
- A load, a new wall and an undone load are never costed (`LOAD_GUARD` doc, `wall.rs:86–88`: "the wall it puts up costs what it costs, which this wall's load cannot tell"). `replace` (`wall.rs:3420`) puts any wall up.
- No module's cost is measured anywhere. The engine measures one number, the whole render's thread CPU time (`engine/mod.rs:754–790`, `measure`). Nothing knows what a module costs before it renders.

## The design (decided; build exactly this)

**1. The engine measures each heard module's cost, one module per sub-block (`engine/room.rs`, `engine/mod.rs`).**

- `room.rs`: `Page` (`room.rs:44–54`) gains `pub(super) costs: Arc<CostCells>`, where `pub type CostCells = Box<[AtomicU32]>`, `SLOTS_PER_PAGE` long. Each cell holds the f32 bits of the slot's render time in nanoseconds per sub-block, and NaN until measured.
  - It is made in `Page::new` beside `published`, with `slice_of`.
  - `Room::costs()` returns `(page number, Arc<CostCells>)` like `Room::published()` (`room.rs:172`). It is control side only, and it allocates.
- `mod.rs`: `EngineControl` (`mod.rs:587`) gains `costs: Vec<Arc<CostCells>>`. It is filled where `knobs` is filled: in `engine()` (`:643–647`) and in `reserve_slots` (`:1372–1376`).
  - New `pub fn cost(&self, slot: usize) -> Option<f32>` reads a cell as `knob` does (`:1406`), returning `None` for NaN, not finite, or out of range.
  - New `pub fn forget_cost(&self, slot: usize)` stores NaN.
  - New `#[cfg(test)] pub(crate) fn set_cost(&self, slot: usize, ns: f32)` beside `set_render_load`.
- `Engine` gains `timed: usize`: the index in the order of the module to time next.
- `render_sub_block` (`mod.rs:1097`): in the loop over the order, the module at `index == self.timed` is rendered between two `std::time::Instant::now()` reads. Its nanoseconds go into its cell through a new `Page::note_cost(at, ns)`. After the loop, `self.timed = (self.timed + 1) % len`, or 0 when the order is empty. A shorter new order is handled by the modulo.
  - `render_module` already includes gathering the module's input cables (`mod.rs:1142–1170`), so the measured cost covers its cables too.
- `Page::note_cost(at, ns)`: let the held value be `v`.
  - If `v` is NaN, `v = ns`.
  - Otherwise `v += COST_FALL * (ns - v)` when `ns < v`, and `v += COST_RISE * (ns.min(COST_CLAMP * v) - v)` when not.
  - A sample that is not finite is ignored. The new value is stored with `Ordering::Relaxed`.
- The engine's `insert` (`mod.rs:961`) stores NaN in the slot's cell: a new module has measured nothing. The control side's `Wall::insert` (`wall.rs:1473`) calls `engine.forget_cost(slot)` before it sends `Command::Insert`. Without that, a cost read before the engine takes the command would be the slot's last occupant's.
- An unheard module is not rendered, so its cell keeps the last cost measured while it was heard. That is the "measured cost when it has one" a wake reads (step 3).
- Constants, in `engine/mod.rs`, each with its reason in its doc comment:
  - `pub const COST_RISE: f32 = 0.25;`: a cost that really rose (a knob raising oversampling) is followed in about four rounds (68% of the way). One odd sub-block moves it a quarter at most.
  - `pub const COST_FALL: f32 = 0.5;`: a sample is too high when the audio thread was preempted while timing it, and never too low. So a lower sample is believed sooner, and a first sample inflated by preemption is gone in about a dozen rounds.
  - `pub const COST_CLAMP: f32 = 4.0;`: a single sample counts at most 4× the held cost. A preempted timing of a 1 µs module may read milliseconds, and must not make it look like a reverb. A real 16× rise still arrives in two rounds.
- Why `Instant` and not the thread's CPU clock, which `measure` uses: `Instant` is read without a kernel call, in ticks of about 42 ns on Apple silicon. Many modules render a 32-frame sub-block in well under a microsecond, which the thread clock cannot resolve. Preemption is held off by `COST_CLAMP` and `COST_FALL`.
- Why one module per sub-block: two clock reads every 667 µs (32 frames at 48 kHz) are far below 0.1% of the time. A wall of 1,000 heard modules re-measures each one every 0.67 s.

Nothing in this step allocates, locks, formats or does I/O on the audio thread: two clock reads, float arithmetic, one atomic store.

**2. The load is shared over heard modules only (`daemon/wall/unheard.rs`, `wall.rs:1332`).**

- `heard_order` (`unheard.rs:11`) also counts `heard_units`: the slots it keeps, plus the edges whose destination is a kept slot (a cable into an unheard module is not gathered, so it costs nothing). It stores the count in a new `Wall` field `heard_units: usize`, replaced on every `send_tables`, beside `unheard`.
- `held()` returns `self.heard_units`. Its doc becomes: "Heard modules and the cables into them: what the render load is shared over (an unheard module renders nothing, see `unheard`)."
- `render_room`'s projection (`:2706–2714`) and `note_held` (`:2753`) are otherwise unchanged. So the load is shared over what it was measured on (heard units minus those added lately), and the change's own `units` are costed at that heard-only average.
- An unheard module added still counts as one unit of the change. That is the existing behaviour and it errs high. It no longer counts in what the load is shared over.
- The sink test moves into one function, `pub(super) fn is_sink(kind: Kind) -> bool` (`kind == Kind::OUT || probe::measures(kind)`), in `unheard.rs`. `heard_order` and step 3 both call it, so the guard's idea of "heard" can never drift from the render's.
- The `LOAD_GUARD` doc (`wall.rs:63–67`) replaces "Unheard modules and cables still count as held when costing additions against the measured load." with: "The load is shared over heard modules and the cables into them; an unheard module costs nothing until a change makes it heard, and that change is costed at what it wakes (see `Wall::wake_room`)." The sentence at `:86–88` about loads becomes: "A load (or a load undone) is costed at what its heard modules cost by the bench (see `Wall::load_room`); a new, empty wall never is."

**3. A change is costed at what it wakes (new file `daemon/wall/cost.rs`, an `impl Wall` block as `unheard.rs` is).**

- **The kind table.** `const KIND_BENCH_NS: &[(&str, u32)]` holds, for every kind, the midpoint of its `wall/kind/<name>` row in nanoseconds per 512-frame block. The builder fills it from the bench run in the definition of done.
  - The bench fixture (`benches/support/patches.rs:6`) renders the kind with a `vco` feeding it and an `out` it feeds. So each row includes their cost and errs high, which errs toward refusing.
  - `speak` has no bench row (the bench leaves it out at `wall_render.rs:37`, because it needs a phrase feed). Its row is the `vca` row: a speaker plays a phrase rendered off the audio thread, a buffer read with a gain, as a vca does. The builder reads the speaker adapter's `process` to confirm this, and writes under FOUND if it does more.
  - `fn kind_share(kind: Kind, rate: u32) -> f32` is `ns / (512 × 1e9 / rate)`: the share of real time at the engine's rate, because a module's work is per frame.
  - A kind with no row (a test-only kind) costs the dearest row, because a guess must err toward refusing.
  - The table is looked up through a `OnceLock<HashMap<Kind, u32>>` built from `Kind::from_name` on first use, on the control side.
- **A module's share.** `fn module_share(&self, id: &str, kind: Kind) -> f32` returns `engine.cost(slot)` converted to a share (`ns / (SUB_BLOCK × 1e9 / rate)`) when the module has a slot in the rack and its cell holds a number. Otherwise it returns `kind_share(kind, rate)`.
- **What a change wakes.** `fn woken(&self) -> Vec<(String, f32)>` runs after a change is made to the patch and before it is logged. It lists every module heard in the patch as it now stands that was not heard before, with its share.
  - "Heard now": `tables::heard` over the patch's modules indexed by position, the cables as index pairs, and the sinks by `is_sink`.
  - "Not heard before": the module has no slot in the rack yet (it came with this change), or it is in `self.unheard`. `self.unheard` and the rack are as the last `send_tables` left them; the change is mirrored only after it is logged (`apply`, `wall.rs:2092`).
  - It allocates, on the control side only, and is O(V + E), as `send_tables` already is.
- **The one guard.** `render_room(units)` becomes `render_room(units: usize, waking: f32)`. `room_to_grow` passes `0.0`.
  - The early return is `units == 0 && waking <= 0.0`.
  - The projection becomes:
    ```
    base      = if measured_on >= COSTED_FROM { load / measured_on × (held + units) } else { load }
    projected = base + Σ recent woken shares + waking
    ```
    and it refuses past `LOAD_GUARD`, as now.
  - `adding_lately` also counts a recent wake.
  - `LoadWatch::recent` (`wall.rs:350`) becomes `VecDeque<(u64, i64, f32)>`: frames, heard units added, and the share woken. A wake's modules are then both in `held` (at the average) and in the recent woken share (at their own cost) until absorbed. That double count lasts `ABSORBED_WINDOWS` windows and errs toward refusing.
- **`wake_room`.** `fn wake_room(&mut self) -> Result<(), WallError>` is called from `changed` (`wall.rs:2762`) before `commit`, and only when `self.chain_batch.is_none()` and the `What` can wake: `Patch`, `Add`, `Restore` and `Chain`. Every other change is skipped without computing anything; knob turns are frequent and wake nothing.
  - It computes `woken()`. If the list is empty or the guard is off, it returns `Ok`.
  - Otherwise it counts `units`: modules not yet in the rack plus cables not yet in `self.lines`, which are the change's own additions. It calls `render_room(units, Σ shares)`.
  - On `Err`, it sets `self.unmade = true`, so `request_as` puts the patch back (`wall.rs:2261`; `before()` already keeps the whole patch for every request that reaches here), and returns the error.
  - On `Ok`, it stores the woken share in a new `LoadWatch` field `waking: f32`. `note_held` pushes `(frames, delta, waking)` when `delta != 0 || waking > 0.0`, and takes `waking` back to 0. `request_as` sets `waking = 0.0` when it puts a change back, so a change the log refused leaves no wake behind.
- **The refusal, when `waking > 0` and it is the projection that refuses** (the existing texts stay for hysteresis and settling):
  `full`: `"this would wake {n} modules nobody heard ({ids}), taking the wall from {load:.0}% to about {projected:.0}% of its time rendering, past the {guard:.0}% it keeps to; take something away first, or wake them a few at a time"`.
  - `{ids}` are the first `WAKE_NAMES` woken ids in patch order, then `and {k} more`.
  - `pub const WAKE_NAMES: usize = 10;`: the MCP and the console show a refusal on a line or two, and ten ids are enough to find the set on the wall.
- **`load_room`.** `fn load_room(&self, incoming: &Patch) -> Result<(), WallError>` is called at the top of `replace` (`wall.rs:3420`), after the store check and before `pay_owed`. That covers a load, a new wall and an undone load.
  - **Tom's console is never refused (Cassio's decision, 13 Oct).** A load, new wall or undone load requested by Tom's console (the seat the wall already treats as Tom's: find how `replace`'s caller knows a console request from a seat's, e.g. the console client kind on the session) skips `load_room` and is put up as today; if its projection is past the guard it is logged once (`kazoo-wall: Tom loaded "<name>": about N% of the wall's time, past the G% guard`) and reported in the answer's text. Reason: the saves are Tom's work, and a wall he cannot put back is worse than one that strains; the guard exists to stop seats knocking the wall over, which seats' loads can still do, so theirs are costed.
  - When the guard is on, it costs the incoming wall's heard modules (by `tables::heard` and `is_sink` over `incoming`) each at `kind_share`. Every module of a wall put up is a new engine module with nothing measured.
  - `rest = max(0, load − Σ module_share of every heard module now)`. That is the master and the engine's own work, which carry on through a load.
  - `projected = rest + Σ`. When `Σ > 0` and `projected > LOAD_GUARD`, it refuses with `full`: `"the wall this would put up renders {n} modules, taking it to about {projected:.0}% of its time rendering, past the {guard:.0}% it keeps to, so it was not put up"`.
  - An empty wall has `Σ = 0` and is never refused. Hysteresis, measuring and settling do not apply to a wall put up; they never have.
  - A refused load costs no allowance: its code is `Full`, which `request_as` already refunds (`wall.rs:2271–2277`).

**Why.** "Heard" is what costs; D03 made that true for the render, and the guard must agree with it. A wake is costed module by module, not at an average, because the average is exactly what a seat can game: fill the wall with cheap heard modules and heavy unheard ones, and the average is cheap. A measured cost is preferred because it is this machine, these knobs and this oversampling. The bench cost is a stand-in only until the module has rendered once, and it errs high because its fixture includes a vco and an out.

## Not this lane

- No change to what is heard (D03), to the render order or to the cull.
- No per-module cost shown in `look`, the console or the MCP: the guard reads it, nobody else does yet.
- No change to the protocol.
- No muting of heard-but-silent modules.
- No change to `COSTED_FROM`, `SETTLE_SHARE`, `RESUME_SHARE` or `ABSORBED_WINDOWS`.

## Red tests first

Each must fail before the fix for the reason given, then pass. Wall tests go in a new `daemon/wall/tests/cost.rs` (registered as `mod cost;` in `tests.rs`, as `mod unheard;` is). Engine tests go in a new `engine/tests/cost.rs`.

1. `daemon::wall::tests::cost::one_cable_waking_forty_heavy_unheard_modules_is_refused`.
   - Setup: with the guard off, add 40 modules of the kind with the dearest `KIND_BENCH_NS` row that has an audio input and an audio output, chained output to input, the last reaching nothing. Turn the guard on, call `measured_long_enough`, and set the render load to 0.30. (0.30 is below `SETTLE_SHARE` of the guard, 0.6, so settling cannot answer first, and is not 0, so the share costing is live.)
   - Precondition, asserted: 0.30 + 40 × that kind's share > `LOAD_GUARD`.
   - A cable from the chain's last module into a free jack of a new `mix` already feeding `out1` is refused with `Full`. The message contains `"wake 40 modules"` and a projected percentage above 80. The cable is not on the patch, `look`'s `timing.unheard` still lists all 40, and `load_watch.refusing` is false (costed, not refusing).
   - The same cable moved into `out1.left`, a jack already in use, is refused the same way. Plugged and unplugged with the guard off, the unplugging's undo with the guard on is refused the same way.
   - Red: today the first cable is costed as one unit at the average and goes through, the moved cable is `Growth::NONE` and never reaches the guard, and the undo is costed as one cable.
2. `daemon::wall::tests::cost::unheard_modules_do_not_lower_the_average`.
   - Setup: on the seed wall (every module heard; `U` heard units, asserted `>= COSTED_FROM`), add 200 `lfo`s with the guard off. They are unheard, so `timing.unheard` lists 200. Then turn the guard on and call `measured_long_enough`.
   - Set the load to `LOAD_GUARD / U × (U − 0.5)`. Adding one `mix` is refused with `Full` and `"would take it"`: shared over the `U` heard units, one more unit passes the guard.
   - Red: today the load is shared over `U + 200`, the projection stays under the guard, and the add goes through.
3. `daemon::wall::tests::cost::a_cable_waking_a_cheap_set_is_allowed`.
   - The same shape as test 1 with the cheapest such kind, the guard on, and a load of 0.30. Precondition, asserted: 0.30 + 40 × its share + the average × 2 < `LOAD_GUARD × SETTLE_SHARE`.
   - The waking cable goes through, and `timing.unheard` no longer lists the 40. `load_watch.recent`'s last entry has a woken share equal to 40 × the kind's share (to 1e-6) and units of at least 41.
   - The next addition inside `ABSORBED_WINDOWS` is costed with that share included: set the load so the projection without the woken share is just under the guard and with it just over; the next `add("mix")` is refused.
   - Red: no woken share in `recent`, so the last assertion fails (the first part is green today, which is the point: it must stay green).
4. `daemon::wall::tests::cost::a_measured_cost_is_used_before_the_bench_cost`.
   - Setup: one `vco` heard through `out1`, with `set_cost(slot, ns)` making it 0.9 of a sub-block. Unplug it (guard off) so it is unheard. Turn the guard on, call `measured_long_enough`, and set the load to 0.10.
   - The cable that makes it heard again is refused with `Full`, `"wake 1 modules"`.
   - With the cell cleared (`forget_cost`), the same cable goes through on the bench cost.
   - Red: no cost cells, no `set_cost`.
5. `daemon::wall::tests::cost::a_save_too_heavy_to_render_is_not_put_up`.
   - Setup: with the guard off, build 40 of test 1's heavy kind each into its own `out`, save it as `heavy`, then load an empty wall (`Request::New`). Turn the guard on and set the load to 0.05.
   - Loading `heavy` is refused with `Full`, and the message names 40 modules and a projected percentage. The wall is still the empty one, and the seat's load allowance is unchanged (refunded).
   - Undoing the `New` (which brings back the heavy wall) is refused the same way.
   - Loading an empty save, or `New`, is never refused at any load.
   - The same `heavy` load requested as Tom's console is put up, not refused, and its answer says it is past the guard.
   - Red: today a load is never costed.
6. `daemon::wall::tests::cost::a_chain_brought_in_with_its_out_is_costed_at_what_it_wakes`.
   - Setup: a chain saved from test 1's 40 heavy modules plus an `out` (made with the guard off, then the wall emptied of them). With the guard on and a load of 0.30, `ChainAdd` of it is refused with `Full` and `"wake 41 modules"`, and the patch is unchanged.
   - Red: today a chain is costed as its own units at the average.
7. `engine::tests::cost::every_heard_module_is_timed_in_turn`.
   - Setup: an order of 5 modules and one module inserted but not in the order. After 10 sub-blocks, each of the 5 slots' `cost` is `Some`, finite and above 0, and the one not in the order is `None`.
   - Inserting a new module into a timed slot makes its `cost` `None` until it has been timed again.
   - A `note_cost` sample of 1e9 ns after a held 1,000 ns leaves the cell at 1,000 + 0.25 × 3,000 = 1,750 ns. A sample of 500 ns after 1,000 ns leaves 750 ns.
   - Red: no cells, no `cost`, no `note_cost`.
8. `daemon::wall::cost::tests::every_kind_has_a_bench_cost`.
   - Every kind in `Kind::all()` that is not a test-only kind (`dsp::effect::testing::KINDS`) has a `KIND_BENCH_NS` row above 0, and every row names a kind that exists.
   - Red: no table. This test is what makes a kind added later bring its bench row with it.

All existing tests stay green. In particular `each_addition_is_costed_and_they_come_apart_near_the_guard` and `what_was_taken_away_lately_counts_against_the_cost` (`tests.rs:385`, `:443`) run on the seed wall, where every module is heard, so `held()` is unchanged for them. Their `recent[0].1` still reads the units. If one fails, the seed wall is not wholly heard: write that under FOUND and make the test's own `units` count heard units, never weaken an assertion.

## Bench

All on Tom's Mac, `uptime`'s load average written beside every row (BENCH.md's D01–D03 sections were all taken under load; a row without its load average cannot be judged).

1. **D01's missing rows** (D03's review noted that D01's section has neither). Before creating the branch, in this checkout: `git checkout --detach 7f4b334` (the D01 merge), then
   `nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render -- 'wall/culled|wall/chains/10$'`.
   Record both midpoints and intervals as D01 rows in BENCH.md's D01 section, under a line saying they were taken on 13 Oct (or the day run) at `7f4b334` by KZ-D03b, with the load average. Then `git checkout -b lane/kz-D03b <base sha>`. Fill D03's table's "D01 ns per block" column from them, leaving the other columns as they are.
2. **The kind rows**, on the lane base before any change: `... -- 'wall/kind/'`. Every row goes into BENCH.md under a `## KZ-D03b` section (ns per 512-frame block, % of real time), and into `KIND_BENCH_NS`. Taken under load they err high, which errs toward refusing; say the load average.
3. **The render path did not get dearer.** `... -- 'wall/chains/1000$|wall/chains/10$|wall/culled'`: base then head, three rounds alternating, as D03's review did. The median over the rounds of head's `chains/1000` must be within 3% of base's. Two clock reads per 667 µs sub-block are about 0.01%, so anything past 3% means something else got onto the render path.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render -- 'wall/chains/1000$|wall/chains/10$|wall/culled'
```

The doc tests run because the `LOAD_GUARD`, `held` and `unheard` docs change.

## Definition of done

- [ ] Tests 1, 2, 4, 5, 6, 7 and 8 red, then green. Test 3's last assertion red, then green, and its first part green throughout. All existing kazoo-wall tests green.
- [ ] D01 rows for `wall/chains/10` and `wall/culled/1000`, run at `7f4b334`, are in BENCH.md's D01 section with their load average, and in D03's table's D01 column. Paste the two Criterion lines.
- [ ] Every `wall/kind/*` row is in BENCH.md and in `KIND_BENCH_NS`, with test 8 green. `speak`'s row is the `vca` row, with the speaker adapter's `process` read and the answer under DONE or FOUND.
- [ ] The head's `wall/chains/1000` median is within 3% of the base's over three alternating rounds, with the table in BENCH.md and the END block.
- [ ] `rg 'Instant::now' kazoo-wall/src/engine/mod.rs` shows exactly the two new reads in `render_sub_block` and nothing else on the render path. Paste the output.
- [ ] The `LOAD_GUARD`, `held()`, `unheard` and `heard_units` docs say what the guard shares the load over and how a wake and a load are costed (step 2).
- [ ] By hand on a test daemon with the guard on (or a release wall that is not Tom's live wall): add heavy modules unheard, plug the waking cable, and paste the refusal text.

## Code quality

- Tree rules apply: unsafe is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`.
- The audio thread gains two `Instant` reads, float arithmetic and one relaxed atomic store per sub-block, and nothing else. No allocation, lock, format or I/O on the render path. Every new `Vec`, `HashSet` and `HashMap` is on the control side.
- `note_cost` ignores a sample that is not finite, so NaN and Inf never reach a cell. `module_share` treats a cell that is not finite as unmeasured.
- No `as` casts that clippy pedantic flags. Nanoseconds go `u128 → u64` by `try_from` with saturation, then `u64 → f32` through the same comment-justified pattern the tree uses (`wall.rs:2713`).
- These files are already over 1000 lines: `daemon/wall.rs` (4691), `engine/mod.rs` (1513), `engine/tests.rs` (1219), `daemon/wall/tests.rs` (5078). Add to them only the lines this brief names: `held`, `render_room`'s signature, projection and message, `LoadWatch`, `note_held`, `request_as`'s put-back line, `changed`'s one call, `replace`'s one call, `insert`'s `forget_cost`, the two docs, the engine's field, loop and `insert` line, `EngineControl`'s field and three methods, and two `mod` lines. Never split an existing file. All new logic goes in `daemon/wall/cost.rs` (under 1000 lines, the table included). List each over-long file you touched under FOUND with its line count.
- At most ten files: `engine/room.rs`, `engine/mod.rs`, `engine/tests.rs`, `engine/tests/cost.rs`, `daemon/wall.rs`, `daemon/wall/cost.rs`, `daemon/wall/unheard.rs`, `daemon/wall/tests.rs`, `daemon/wall/tests/cost.rs`, `briefs/studio/BENCH.md`.
- Red test first, one behaviour per commit, in this order: cost cells, heard-only share, wake costing, load costing, kind table with bench rows, BENCH.md.

## END

```
START KZ-D03b base=<sha> branch=lane/kz-D03b df=<GB> at <time>
END KZ-D03b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: D01 chains/10 and culled/1000 at 7f4b334; every wall/kind row; chains/1000 base and head over three alternating rounds; each with its load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
