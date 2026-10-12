# KZ-D02: an unplugged jack, a resting knob and an empty slot cost the render nothing

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. Crate: kazoo-wall. Depends: D01 (the baseline rows `wall/unplugged/1000` and `wall/chains/1000`).

## Outcome

Today every module pays the same fixed price every sub-block whether or not anything is plugged into it: 28 `gather` calls that each zero-fill 32 floats when the jack is empty (about 3.5 KB of writes per module), 24 knob-glide advances, a finiteness scan of all four outputs, and the `out` sum walks every slot of every page the room has (`engine/mod.rs:1107–1117`) to find the handful that are outs. A wall of 1,000 idle LFOs costs the audio thread more than a wall of 100 busy chains (survey §3, §5.2–5.3). After this lane the render touches only what is plugged in, only knobs that are moving, only outputs somebody reads, and only the outs the order lists. The number: `wall/unplugged/1000` must be at least 5× faster than D01's baseline and under 25% of real time; `wall/chains/1000` must not get slower (within 3%).

## The design (decided; build exactly this)

**1. The cable table knows which jacks are plugged (`engine/tables.rs`).** `CableTable` (`tables.rs:59–68`) gains `plugged: Vec<u32>`, by destination slot, one bit per jack (bit `j` set when `routes[slot][j]` is `Some`; 28 jacks fit a `u32`). `set` (`tables.rs:113–168`) keeps it in step with `routes` (sets the bit when a route goes in, clears it when `None` replaces one, grows it with `routes`: the same `room_for` and `resize` the routes get, in the same place, so a table that refuses is left as it was). New method `plugged(&self, slot) -> u32` (0 past the table). `with_slots` sizes it with the others.

**2. Gathers only for plugged jacks (`engine/mod.rs:1159–1180`).** `render_module` reads `let mask = self.cables.plugged(slot)` once and loops over its set bits (`while mask != 0 { let jack = mask.trailing_zeros(); ...; mask &= mask - 1 }`), calling `gather` for each; `connected[i]` and `knob_patched[k]` are read from the mask (`mask & (1 << i) != 0`), not from `gather`'s return. The input and knob-CV buffers of unplugged jacks must still read zero: the engine keeps `filled: u32`, the jacks it filled for the previous module; before the gathers, every bit in `filled & !mask` has its buffer zeroed (`self.inputs[i]` or `self.knob_cv[k]`), then `filled = mask`. So a buffer is zeroed once when a jack stops being plugged, not 28 times a module. `gather` (`mod.rs:1309–1329`) keeps its signature but is only ever called for a plugged jack; a route whose source slot is empty (`room.output` is `None`) still zero-fills, as now.

**3. Knob glides only while moving (`engine/room.rs`, `engine/mod.rs`).** `Page` (`room.rs:43–53`) gains `gliding: Box<[u32]>`, one bit per knob per slot (24 knobs fit). `Engine::knob` (`mod.rs:1017–1044`) sets the bit when `glide.remaining > 0` after it has set the glide; `insert` (`mod.rs:959–989`) clears the slot's word (new modules rest). In `render_module` the loop at `mod.rs:1147–1158` becomes a walk over the slot's set bits: for each, `advance`, store the sweep in `self.sweeps[knob]`, publish the value as now, and clear the bit when `glide.remaining == 0` after the advance. `self.sweeps` is otherwise all `Sweep::REST`: after `module.process` returns, the same bits (a copy of the word taken before the walk) are reset to `REST`, so the next module never sees this one's sweeps. `Glide::advance` (`mod.rs:495–515`) is unchanged.

**4. The finiteness scan reads only outputs somebody hears (`mod.rs:1206–1219`).** The scan runs over `outputs[port]` for each port with `fed[port]` true, plus ports `FEED_LEFT` and `FEED_RIGHT` when the kind is `Kind::OUT` (they are not patchable, so never "fed", and the master reads them). An output nothing reads may hold anything (`Io::fed`'s own words, `dsp/mod.rs:140–143`: "whatever is in it then is never heard"); a module whose state has gone non-finite shows it on the output it is heard through, and is silenced and reset there as now. The fault path (`Fault`, `shared.faults`, the probe ring) is unchanged.

**5. The out sum walks a list, not the room (`engine/tables.rs`, `mod.rs:1105–1117`, `daemon/wall.rs:1495–1579`).** `Order` (`tables.rs:205–234`) gains `outs: Box<[u32]>`: the slots of the `out` modules, in render order. `Order::new(slots, outs)` takes both (repeats and overflow dropped as for `slots`); `processing_order(slots, edges, outs: &[usize])` passes them through, keeping only outs that are in the order. `send_tables` passes the slots of `patch.modules()` whose `kind == Kind::OUT`; the test `Model` in `engine/tests.rs:63–86` passes the slots whose kind is `OUT`; D01's bench support does the same. The engine's sum becomes `for &slot in self.order.outs() { if let Some((page, at)) = self.room.page(slot) { if page.kinds[at] == Some(Kind::OUT) && page.modules[at].is_some() { add outputs[FEED_LEFT], [FEED_RIGHT] } } }`: the kind check stays, so an order that names a slot the control side has since re-used for another kind adds nothing. Between an `Insert` of an out and the `Order` that lists it there is no block: both go in one change (`apply`, `wall.rs:1818–1825`: insert then `send_tables`) and the engine drains them in one `drain_commands`.

**6. The stale comment.** `daemon/wall.rs:91–94` says the load is wall-clock time. Since the render-load fix it is the thread's CPU time (`engine/mod.rs:757–762`). Rewrite those four lines: "The load is the audio thread's CPU time, not the time that passes: a render the machine preempts is the machine's load, not the wall's, and is not counted."

**What is not touched.** `Io` (`dsp/mod.rs:118–145`) keeps its shape: modules still receive `connected`, `knob_patched`, `fed`, `inputs`, `knob_cv`, `sweeps`; nothing in `dsp/` or `adapters/` changes. `render_sub_block`'s other work (transport, master, hush, publish) is unchanged. The capture call stays per module (a scope must hand its sweep on whether or not it moved).

**Why.** Bitmasks over `u32` because 28 jacks and 24 knobs both fit one word, a word is read once per module, and `trailing_zeros` walks the set bits with no branch per empty jack. Zeroing on transition (`filled & !mask`) rather than per call because `Io` lends the buffers immutably, so a zero stays zero until a gather overwrites it. Outs in the `Order` rather than kept by the engine on insert, because the engine cannot grow a list without allocating and the control side already builds the order every change. Fed-only scanning because an unfed output is never read by anything the design admits.

## Not this lane

No culling of unreachable modules (D03). No change to delay lines or `Lines::carry` (D04). No incremental tables (D05). No SIMD (D11). No change to `Io` or any module. No change to the control side beyond passing the outs and the comment.

## Red tests first

In `kazoo-wall/src/engine/tests.rs`, under `on_audio_thread` where a render is involved:

1. `an_unplugged_jack_reads_silence_and_the_table_knows_it`: a test module (as `Probe` at `tests.rs:231–248`) that records `connected` and `knob_patched` and asserts every unplugged input buffer and knob-CV buffer it is given reads all zeros, after a cable into input 0 is plugged, rendered, unplugged and rendered again (the stale-buffer case, which only the zero-on-transition keeps right once gathers are skipped); and that `CableTable::plugged(slot)` has exactly bit 0 set while the cable is in and is 0 after. Red: `plugged` does not exist, so the test does not compile; the zero-buffer assertion is the guard for step 2 once it does.
2. `a_resting_knob_is_not_advanced`: `Page::gliding` word is 0 after insert, has bit `k` set after `Command::Knob { knob: k, glide_frames: 1_000 }`, and is 0 again after enough renders; the published cell moves only while the bit is set. Red: no `gliding` field.
3. `a_sweep_never_leaks_to_the_next_module`: two modules in order, a glide on the first's knob 3; the second (a test module) asserts `io.sweeps[3].frames == 0`. Red on the base only if sweeps leak today: they do not (each module overwrites all 24), so this test guards the new reset path; it must be green before and after, and is listed under NOT CONFIRMED as "not red".
4. `the_out_sum_follows_the_order_s_list`: an `out` inserted into slot 70 (second page) and listed in `Order::outs` is heard; the same out with an `Order` whose `outs` is empty is not. Red: `Order::outs` does not exist.
5. `an_unfed_output_is_not_scanned_and_a_fed_one_is`: a test module that writes NaN to output 3 (unfed) and 0.5 to output 0 (fed): no fault; the same module with NaN on output 0: one fault, silence, reset. Red: today both fault.

Bench before and after: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render -- 'wall/unplugged|wall/chains/1000'`, once on the base commit before the first code change and once at the end; both pairs of numbers go in BENCH.md under a `KZ-D02` section and in the END block.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render -- 'wall/unplugged|wall/chains/1000'
```

## Definition of done

- [ ] Tests 1, 2, 4, 5 red then green; test 3 green; every existing engine test green.
- [ ] `wall/unplugged/1000` at least 5× faster than the D01 row and under 25% of real time; `wall/chains/1000` within 3% of its D01 row; both pairs in BENCH.md and the END block.
- [ ] `daemon/wall.rs` load-guard comment says thread CPU time.
- [ ] `random_patching_never_allocates_on_the_audio_thread` (`engine/tests.rs:104`) still green: no allocation came in with the masks.

## Code quality

Tree rules. Nothing allocates, locks, formats or does I/O on the render thread: the masks are words in pre-allocated pages and tables; the outs list is a boxed slice built on the control side. Unsafe forbidden: no `std::arch`, no raw pointers. NaN/Inf defence: the fed-only scan still silences and resets a faulting module; `gather` still passes everything through `finite`. Files under 1000 lines: `engine/mod.rs` is about 1513 lines at the sweep and already over; do not add to its length net of what this lane removes, and put nothing new in it that can live in `tables.rs` or `room.rs` (the mask walks are small; the `outs` accessor is on `Order`). One behaviour per commit, red before green.

## END

```
START KZ-D02 base=<sha> branch=lane/kz-d02 df=<GB> at <time>
END KZ-D02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: wall/unplugged/100, wall/unplugged/1000, wall/chains/1000: before and after, ns per block and % of real time
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
