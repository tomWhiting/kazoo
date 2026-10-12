# KZ-D05: a change costs the control thread what it touches, not the whole wall

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

**This file holds two lanes, dispatched one after the other: D05a (the patch's indexes and route deltas to the engine) and D05b (the order, timing and latencies kept incrementally). D05b depends on D05a. Nothing is dropped: together they are PLAN.md's D05.** Depends: D04 (lines live in pages; `Route` has no line number). Baselines: D01's `control/build/1000` and `control/look/10000`.

---

# KZ-D05a: the patch finds a jack, a cable and a module's cables without a scan, and the engine takes route changes one jack at a time

Time: about 30 minutes of a builder's work. Crate: kazoo-wall.

## Outcome

Every `plug`, `check_plug`, `is_plugged`, `unplug_jack` and `remove` walks every cable comparing strings (`patch.rs:611–660, 715–750, 821–852`); `hang_rows` holds the seen ids in a `Vec` and `contains` it (`patch.rs:1051–1079`); and every change sends the engine a whole new `CableTable` of `slots × 28` routes (`daemon/wall.rs:1527–1575`, about 670 B a slot, 6.7 MB at 10,000 slots) and makes the engine tell every rewired module by diffing the whole old table against the new (`engine/mod.rs:938–957`). Building a wall of N cables one change at a time is therefore O(N²) string compares and O(N) megabytes. After this lane a jack lookup is a hash lookup, a module's cables are a set, and a change sends the engine only the routes it moved. The number: `control/build/1000` at least 4× faster than D01's baseline, and the 1,000th change no more than 2× the cost of the 10th (measured by a new bench `control/change/<at>` below).

## The design (decided; build exactly this)

**1. Two indexes on the patch (`patch.rs:184–200`).** `Patch` gains `jacks: HashMap<(String, Jack), u32>` (destination module id and jack → the cable in it; at most one by the wall's rule) and `ends: HashMap<String, BTreeSet<u32>>` (module id → every cable touching it as source or destination). Both are kept by the only three places cables enter or leave: `plug` (`patch.rs:632–685`: insert into both; the replaced cable removed first), `unplug` (`693–710`: remove from both), and `remove` (`815–872`: the `gone` set is read from `ends[module]` and each gone cable removed from `jacks` and from the other end's `ends`; the `partition` over all cables goes, replaced by `cables.retain` over `gone`'s ids through `cable_at`). `from_file`/`to_file` (the load path) build both indexes once after the cables are in. `check_plug`, `is_plugged` and `unplug_jack` read `jacks`; `remove`'s re-plug check (`848–853`) reads `jacks`. `Jack` must be `Hash + Eq` (`catalogue.rs`; add the derives if missing). `cables: Vec<Cable>` stays sorted by id (`cable_at`'s binary search, `483`); `plug`'s `insert(at, ..)` stays (a memmove of at most E small structs, once per change, is not the problem; the string scans were).

**2. `hang_rows` and the places shift.** `seen` (`1052`) becomes a `HashSet<&str>`. `remove`'s `for later in places.values_mut()` (`840–844`) stays: it is O(N) integer work once per remove and keeps "in the order added" exact. `usual_place` (`1030–1039`) stays: it scans from the newest module back and stops at the first of the kind's group; a wall with none of that group pays O(N) once per add, which is acceptable and is said in its doc.

**3. The engine takes route deltas (`engine/mod.rs:102–155`, `engine/tables.rs`).** New `Command::Routes(Box<[RouteChange]>)` with `pub struct RouteChange { pub slot: u32, pub jack: u8, pub route: Option<Route> }`, retired back as `Retired::Routes(Box<[RouteChange]>)`. The engine applies each with a new `CableTable::set_in_place(slot, jack, route) -> bool`: exactly `set` (`tables.rs:113–168`) but refusing (`false`, nothing changed) instead of growing when `slot >= routes.len()` or `route.slot >= feeds.len()`; a refusal counts in `Shared::ignored`. `set` itself stays for the control side. After applying a `Routes` command the engine tells the rewired modules from the delta itself: group the changes by slot (the delta is small; walk it once per distinct slot, at most 28 entries per slot) and call `module.rewired(moved)` for `moved[input]` true where `jack < MAX_INPUTS` and the source (slot, port, amount bits) differs from before (read the old route with `self.cables.route(slot, jack)` before `set_in_place`). `self.tables` (`ProbeFrame::cables`, `mod.rs:532–533, 880`) increments for a `Routes` command as for `Cables`, so probes still say which table they were measured under.

**4. The control side mirrors the table and sends deltas (`daemon/wall.rs:1495–1579`).** `Wall.spare_table: Option<CableTable>` (`545`) becomes `table: CableTable`, the mirror of what the engine holds, built with `CableTable::with_slots` for the rack's slots (`room_to_grow`, `2520–2531`, keeps making sure it has room: when the rack grows past `table.slots()`, a new table of twice the slots is built, every route copied in, and sent whole as `Command::Cables`, the mirror replaced; that is the only time a whole table goes after start). `send_tables` becomes `send_tables(&mut self, touched: Touched)` where `Touched { cables: Vec<u32>, modules: Vec<String> }` is what the change moved (`apply`, `wall.rs:1818–1835`: a `Patch` names its cable and the replaced one; an `Unpatch` its cable; an `Add`/`Restore` the module and its cables; a `Remove` the module and the cables that went; a re-timing names the cables whose delay changed). For each touched cable the new route (or `None` if it is gone) is computed as today (`1545–1573`, with D04's `Route::new(from, port, amount, tag)`), compared with `table.route(to, jack)`, and when different, applied to the mirror and pushed into the delta. The delta goes as `Command::Routes`; nothing is sent when it is empty. `swap_engine` (`1399–1430`) keeps sending a whole table (the engine was emptied). `note_rewiring` (`1584–1639`) reads the delta instead of walking every cable: a scope or readout is noted when one of the delta's entries names its slot and an input jack.

**5. Timing still whole, for now.** `processing_order`, `latencies`, `timing`, `unsteady` run as today in D05a (D05b takes them). The delta covers the cables whose `timing.delays` entry changed between the old `self.timing` and the new (one pass over both `BTreeMap`s, which are sorted: a merge walk), so a re-timing that moves 3 delays sends 3 routes.

**6. A bench for the shape (`kazoo-wall/benches/control.rs`).** `control/change/<at>` for `at` = 10 and 1000: the wall built to `at` chains as `control/build` does (setup), then `b.iter` of one more `Request::Patch` and its `Unpatch` (so the wall does not grow across iterations). The ratio of the two rows is the number in the outcome.

**Why.** A hash by (module, jack) because the wall's rule is one cable per jack, so the key is the jack; a set by module because `remove` needs every cable at an end. Deltas rather than double-buffered whole tables because a whole table is O(slots) memory per change and the engine's rewire diff is O(slots) time; a delta is O(touched) for both, and `set_in_place` is `set` with growth refused, which keeps one code path. The mirror on the control side because the engine's table cannot be read from the control thread, and the control side must know the old route to build a delta and to tell rewiring.

## Not this lane

No incremental order, timing or latency (D05b). No change to `timing.rs`. No change to what `look` returns (D13). No threads (D06).

## Red tests first

1. `patch::tests::a_jack_is_found_without_a_scan`: after 1,000 cables, `is_plugged` for the last is true and for a free jack false; `remove` of a module with 28 cables leaves `ends` and `jacks` without any of them; plugging into a used jack replaces the cable and the index points at the new one. Red: no index (assert through a new `#[cfg(test)]` accessor `Patch::cables_at(module) -> &BTreeSet<u32>`).
2. `patch::tests::the_indexes_survive_a_file_round_trip`: `to_file` then `from_file` rebuilds the same indexes (compare `cables_at` for every module).
3. `engine::tests::a_route_delta_moves_one_jack_and_tells_only_that_module`: two modules with the rewiring test module (`tests.rs:1100–1123`): a `Routes` delta on slot 1's input 0 calls `rewired([true, false, false, false])` on slot 1 only; `ProbeFrame::cables` moved by one; a delta naming a slot past the table is refused and counted in `ignored`. Red: no command.
4. `daemon::wall::tests::a_patch_sends_only_its_route`: count the `Routes` commands' lengths through a test hook on `EngineControl` (`#[cfg(test)] pub(crate) fn last_routes_len(&self) -> Option<usize>`): one `patch` on a 50-module wall sends a delta of 1; an `unpatch` sends 1; a `remove` of a module with 3 cables sends 3; adding a reverb that re-times 4 cables sends 4 plus its own.
5. `engine::tests::random_patching_never_allocates_on_the_audio_thread` extended with `Routes` commands.

Bench before and after: `cargo bench -p kazoo-wall --bench control -- 'control/build|control/change'`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench control -- 'control/build|control/change'
```

## Definition of done

- [ ] Tests 1–4 red then green; 5 green; every existing test green.
- [ ] `control/build/1000` at least 4× faster than its D01 row; `control/change/1000` at most 2× `control/change/10`; rows in BENCH.md and the END block.
- [ ] No whole `CableTable` is sent after start except on `swap_engine` and when the rack outgrows the mirror (a test counts `Cables` commands as test 4 counts `Routes`).

## Code quality

Tree rules. Nothing allocates on the render thread: the delta is a boxed slice from the control side; `set_in_place` never grows. Unsafe forbidden. NaN/Inf: `set_in_place` keeps `set`'s refusal of non-finite amounts. `daemon/wall.rs` is 4628 lines: the delta building goes in a new `daemon/routes.rs` (the mirror, `Touched`, the delta builder), not in `wall.rs`. `patch.rs` is 1696 lines: the indexes are small; do not add more than they need. One behaviour per commit.

## END

```
START KZ-D05a base=<sha> branch=lane/kz-d05a df=<GB> at <time>
END KZ-D05a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: control/build/100, control/build/1000, control/change/10, control/change/1000 before and after
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```

---

# KZ-D05b: the render order, the timing and the latencies move only where the change moved them

Time: about 30 minutes of a builder's work. Crate: kazoo-wall. Depends: D05a.

## Outcome

Every change still rebuilds the whole order (`processing_order`, `engine/tables.rs:251–295`, O((V + E) log V) with a `HashMap` and a `BTreeSet`), asks every effect its latency with its knobs applied (`latencies`, `daemon/wall.rs:1671–1690`: `latency_of` for every module, every change), asks five more times per patched knob for `unsteady` (`1695–1736`), and walks the whole timing (`timing.rs:42`). After this lane a cable added or removed reorders only the modules between its ends, the timing is walked only from the earliest touched position to the end, and a module's latency is asked only when it is added or when its knobs settle. The number: `control/change/1000` at most 1.3× `control/change/10` (D05a brought it under 2×), and `control/build/1000` at least 2× faster than D05a's row.

## The design (decided; build exactly this)

**1. An incremental order (`engine/tables.rs`).** New `pub struct Ordering` on the control side (not the engine's `Order`, which stays the boxed slice the engine reads): `position: Vec<Option<u32>>` by slot (its place in the order, or `None` when not ordered), `slots: Vec<u32>` in order, `feeds: Vec<Vec<u32>>` and `fed_by: Vec<Vec<u32>>` adjacency by slot. Operations:
- `add_slot(slot)`: appended at the end (no edges yet).
- `remove_slot(slot)`: taken out of `slots` (O(N) memmove of `u32`s, positions after it renumbered), its edges dropped from both adjacency lists.
- `add_edge(from, to)`: the Pearce–Kelly step. If `position[from] < position[to]` nothing moves. Otherwise do a forward search from `to` bounded above by `position[from]` (visiting only slots with position ≤ `position[from]`) and a backward search from `from` bounded below by `position[to]`; if the forward search reaches `from`, the edge closes a cycle: the order is left as it is and the edge is a back-edge, heard one sub-block late, as `processing_order`'s cycle breaking gives today (`tables.rs:239–244`); the tie-break differs from the batch sort's "fewest unrendered sources" and that is accepted, since any order with the cycle's edges forward but one is legal. Otherwise the affected region is the union of the two visited sets; reorder it by placing the backward set (sorted by old position) before the forward set (sorted by old position) within the region's positions, the rest of the region unchanged, and renumber. Cost O(affected region), typically a handful of slots.
- `remove_edge(from, to)`: drop from adjacency; the order stays valid (removing an edge never breaks a topological order).
- `order(&self, outs: &[usize]) -> Order`: the boxed slice for the engine (`Order::new(&slots, outs)` from D02), restricted to heard slots (D03's `heard`, which stays a whole walk: it is O(V + E) over `Vec`s with no hashing, and the lane measures it; if `control/change/1000` misses its number because of it, `heard` becomes incremental in the same way, kept per slot as a `heard: Vec<bool>` updated by a forward walk from the touched slot; write which under DONE).
- `rebuild(slots, edges)`: the batch sort, used by `swap_engine` and on load: `processing_order` kept as the implementation.
Tests of `Ordering` compare its `slots` against the invariant "every forward edge's source comes first" after random sequences of adds and removes, and against `processing_order` on acyclic graphs for membership (not for identical order, which Pearce–Kelly does not promise).

**2. The timing walks a suffix (`daemon/timing.rs:42`).** `timing(patch, order, latency)` becomes `Timing::update(&mut self, patch, order: &[String], from: usize, latency)`: arrival and ready are recomputed for `order[from..]` only; `arrival` and `ready` become fields of `Timing` kept by id (`HashMap<String, u32>`, cleared of ids no longer in the order). The invariant that makes this right: a module's arrival depends only on its *forward* sources (`forward` at `timing.rs:48–51`), which sit before it in the order, so nothing before `from` can change. `from` is the smallest position among: every touched module (added, removed, its latency changed), every end of a touched cable, and the start of the affected region of an `add_edge` reorder (`Ordering::add_edge` returns it). The out levelling (`master`, `timing.rs:80–89`) is recomputed whole over the outs (O(outs)); when `master` changes, every out's cables are re-delayed (O(cables into outs)). `delays` is rebuilt only for cables whose destination is in `order[from..]` or is an out; the delta to the engine (D05a step 5) comes from comparing the old and new `delays` entries for those cables only.

**3. Latencies asked once per settle (`daemon/wall.rs:1671–1690`).** `latencies()` becomes a cache `latency: HashMap<String, u32>` on `Wall`, filled for a module when it is added or restored (`apply`, `What::Add | What::Restore`), when its knobs settle (`settle`, `wall.rs:1738` onward, already knows which modules settled), when the engine's rate changes (`load_engine`: clear the cache), and dropped when the module is removed. `unsteady` (`1695–1736`) likewise: `unsteady: HashSet<String>` recomputed for one module when a cable goes into or out of one of its knob jacks, or its knobs settle; the five probe calls run for that module alone. `send_tables` reads both caches; `self.probes` (prepared effects per kind) stays.

**4. `send_tables(touched)` (D05a) becomes the driver.** Per change: apply `Ordering` operations for the touched slots and cables; `from` as in 2; `timing.update`; the latency cache for touched modules; the delta (D05a). The whole path for one `patch` on a wall of N modules is O(affected region + suffix of the timing + the cables of the touched ends), never O(N) in strings.

**5. `swap_engine` and load.** `Ordering::rebuild` from the whole patch, `Timing::update` from 0, the latency cache filled for every module once (as today, but once per load, not once per change after it).

**Why.** Pearce–Kelly because it is the standard incremental topological order with O(affected region) per edge and a one-page implementation over `Vec`s; it keeps cycles legal the same way the batch sort does. A suffix walk for timing because the dependency on the order makes "from the earliest touched position" exact, not a heuristic. Caching latencies by settle because `settle` already knows when a knob stopped moving, and the probe calls were the heaviest thing on the control thread per change for a wall with effects.

## Not this lane

No change to the engine (D05a did the delta). No change to what `look` returns (D13; the timing fields keep their names). No threads (D06).

## Red tests first

1. `tables::tests::an_edge_added_reorders_only_between_its_ends`: order `[0,1,2,3,4]`; `add_edge(3, 1)` moves 3 before 1 and leaves 0 and 4 in place; `add_edge(1, 3)` after it closes a cycle and moves nothing; `remove_edge` and `remove_slot` keep the invariant. Red: no `Ordering`.
2. `tables::tests::random_edits_keep_every_forward_edge_forward`: 2,000 random adds and removes over 200 slots, the invariant checked after each; membership equals `processing_order`'s on the same graph when it is acyclic.
3. `timing::tests::a_suffix_update_equals_a_whole_walk`: for a chain with a reverb in the middle, after a cable is added at position k, `update(from = k)` gives the same `Timing` as a fresh whole walk; after an out is added, every out's cables are re-levelled. Red: no `update`.
4. `daemon::wall::tests::a_turn_asks_latency_once_when_it_settles`: count `latency_of` calls through a `#[cfg(test)]` counter on `Wall`: a knob turn on a reverb asks 0 times at the turn and 1 at settle; a patch into a vco asks 0 times; adding a reverb asks once. Red: today every change asks for every effect.
5. `daemon::wall::tests::a_load_orders_and_times_the_whole_wall_once`: after `load`, the order, timing and latencies equal a wall built by hand change by change (compare `look`'s `timing` and the engine's order through the test hook).

Bench before and after: `cargo bench -p kazoo-wall --bench control -- 'control/build|control/change'`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench control -- 'control/build|control/change'
```

## Definition of done

- [ ] Tests 1, 3, 4 red then green; 2, 5 green; every existing test green (the batch-sort tests in `tables.rs:312–350` unchanged).
- [ ] `control/change/1000` at most 1.3× `control/change/10`; `control/build/1000` at least 2× faster than D05a's row; rows in BENCH.md and the END block.
- [ ] `processing_order` is called only from `Ordering::rebuild`; `latency_of` is called only on add, restore, settle, rate change and load.

## Code quality

Tree rules. All of this is control-side; nothing here touches the render thread. `Ordering` lives in a new `engine/ordering.rs` (not `tables.rs`, which stays the engine's tables), under 1000 lines with its tests. `timing.rs` grows by the suffix update only. `daemon/wall.rs` must not grow net: the latency and unsteady caches replace the two functions they retire. One behaviour per commit.

## END

```
START KZ-D05b base=<sha> branch=lane/kz-d05b df=<GB> at <time>
END KZ-D05b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: control/build/100, control/build/1000, control/change/10, control/change/1000 before (D05a's rows) and after
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
