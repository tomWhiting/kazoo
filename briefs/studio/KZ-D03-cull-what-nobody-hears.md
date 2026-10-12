# KZ-D03: a module nobody can hear or read is left out of the render

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. Crates: kazoo-wall, kazoo-mcp (one sentence in `render.rs`). Depends: D02 (the `Order` carries `outs`; `wall/culled/1000` baseline from D01).

## Outcome

Every module renders every sub-block whether or not its signal reaches anything (survey §3: "No culling"). A chain whose `out` was removed, a VCO left over from an experiment, a reverb feeding nothing: each still costs its full price, and a reverb or a 16× drive costs a great deal. After this lane the render order holds only modules from which a path of cables leads to a *sink*: an `out`, a `scope` or a `readout`. Everything else sits silent, costs nothing, and `look` says so. The number: `wall/culled/1000` (1,000 `lfo → vca` chains of which 10 reach an out) must cost no more than 1.2× `wall/chains/10` after this lane; on D01's baseline it costs about as much as `wall/chains/1000`.

## The design (decided; build exactly this)

**1. Reachability on the control side (`engine/tables.rs`).** New function `pub fn heard(slots: &[usize], edges: &[(usize, usize)], sinks: &[usize]) -> Vec<usize>`: the slots from which some sink is reachable along `edges` (source → destination), in the order given by `slots`. Built as a reverse breadth-first walk: index the slots as `processing_order` does (`tables.rs:254–261`), build the reverse adjacency (`fed_by[to].push(from)`) in one pass over the edges, seed the queue with every sink that is in `slots`, mark and walk; return the slots whose mark is set, in `slots`' order. A self-cable (`from == to`) is skipped as in `processing_order`. Cost O(V + E), no `HashMap` past the index (a `Vec<bool>` for the marks, a `VecDeque<usize>` for the queue). A sink reaches itself: sinks are always kept.

**2. `send_tables` culls before it sorts (`daemon/wall.rs:1495–1579`).** After `edges` and `slots` are built (lines 1496–1510), compute `sinks`: the slots of modules whose `kind == Kind::OUT || probe::measures(kind)` (`probe.rs:37–39`: scope and readout). Then `let kept = heard(&slots, &edges, &sinks)` and `processing_order(&kept, &edges, &outs)` (D02's signature). `rendered` (line 1512), and so `timing` (1519) and the latencies it asks for, follow the order and therefore cover only heard modules. The unheard ids (slots in `slots` but not in `kept`, mapped back through `rack.id_at`) are kept in a new `Wall` field `unheard: Vec<String>` (ids in the order added), replaced on every `send_tables`. Cables into or out of an unheard module stay in the `CableTable` (the engine skips what the order does not name; a route from an unheard source reads that source's output buffer, which holds whatever it last rendered — see 4).

**3. `look` says which modules are unheard (`protocol/mod.rs`, `wall.rs:4230`).** `Timings` (the struct `timings()` at `wall.rs:4230` fills; `protocol/mod.rs`, `pub struct Timings`) gains `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub unheard: Vec<String>`: "modules from which no cable path reaches an out, a scope or a readout; they are not rendered". `timings()` copies `self.unheard`. The console marks an unheard module's panel: in `tui/draw/wall.rs`, `top_edge` (`draw/wall.rs:335`) writes the word `unheard` in `TEXT_DIM` at the right end of the panel's top edge when the module's id is in `snapshot.timing.unheard` (a `HashSet<&str>` built once per draw from the snapshot, beside `jack_cables`). kazoo-mcp's `render::look` (`kazoo-mcp/src/render.rs:23`) adds one sentence after the cable list when the list is not empty: "<n> modules reach no out, scope or readout and are not rendered: <ids, comma separated, at most 20 then 'and k more'>." The MCP tool text (`tools.rs:328`) is unchanged.

**4. What an unheard module does not do, said plainly in the module docs of `engine/tables.rs` and in `wall.rs`'s `unheard` field doc.** Its knobs do not glide (the glide advances only in `render_module`, `engine/mod.rs:1137`); when it is heard again the glide carries on from where it stopped, and `look` shows its target as the knob's target and the frozen value as its value, as for any knob mid-glide. Its state (a reverb's tail, an envelope's stage, a sequencer's step) is held, not reset; heard again, it carries on. Its output buffers hold its last rendered block: a cable from it into a heard module (impossible by construction, since then it would be heard) never happens, so no stale block is ever read. A `scope` or `readout` plugged into an unheard module makes it heard: that is how a seat listens to something that reaches no out. A `speak` module nobody hears does not consume its phrase feed; its words wait in the feed and play when it is heard; `speak`'s answer comes when its words are rendered (`Wall::spoken`), not when they are played, so no request waits on an unheard module. The builder reads `Wall::spoken` and the speech thread to confirm this and, if the answer does wait on play, writes that under FOUND and makes `speak` a sink (adds it to `sinks`) so no request can hang.

**5. Fingerprints and the load guard.** Fingerprints (`fingerprints.rs`, "where the signal carried it") flow along cables as now; they say who touched what, not what is heard; unchanged. The load guard's `held()` counts modules and cables (`wall.rs:2650`): unchanged; an unheard module still counts as held, which errs toward refusing early, and the load it is costed against is the measured load, which an unheard module no longer adds to; so a wall of unheard modules is cheap to add to, as it should be.

**Why.** Reverse reachability from sinks is the whole of "what nobody hears"; it is O(V + E) and runs where `send_tables` already runs O((V + E) log V). Keeping state rather than resetting, because a module taken out of hearing and put back is a cable move, "weather, not a crash" (`DESIGN.md`), and a reset would restart every sequencer on the way. Marking in `look`, the console and the MCP, because Tom gets frustrated when the UI does not say what is happening, and a silent module with no mark reads as a fault.

## Not this lane

No change to the engine's render (D02 did that; D04 next). No incremental reachability (D05 may keep `heard` whole: it is linear). No change to `Fingerprints`. No muting of modules that are heard but silent (no silence detection). No change to the MCP tool arguments.

## Red tests first

1. `tables::tests::heard_keeps_only_what_reaches_a_sink`: slots `[0..6]`, edges `0→1, 1→2 (sink), 3→4, 5→5`, sinks `[2]`: `heard` returns `[0, 1, 2]`; a sink with no cables is kept; a sink not in `slots` is ignored. Red: no function.
2. `daemon::wall::tests::a_module_reaching_nothing_is_unheard_and_said_so`: on the seed wall, remove `out1` (whatever the seed's out is called): every module becomes unheard, `look`'s `timing.unheard` lists them all, and the engine's order (observe through the engine: render a second and `Shared::frames` moves while the master stays silent and a readout's probe never arrives) renders none; add an out and plug the vcf into it: `unheard` empties and the readout's probe arrives. Red: no `unheard` field.
3. `daemon::wall::tests::a_scope_makes_what_it_reads_heard`: an `lfo` alone is unheard; a `scope` plugged into it makes both heard. Red: as above.
4. `engine::tests::an_unheard_module_keeps_its_state`: a `seq` (or a test module counting its `process` calls) left out of the order for 100 renders then put back: its count carries on from where it was, nothing was reset. Red only if the engine resets modules not in the order: it does not today, so this is a guard, listed under NOT CONFIRMED as "not red".
5. `kazoo-mcp` `render::tests::look_says_what_is_unheard`: a snapshot with `timing.unheard = ["vco3", "vcf2"]` renders the sentence; an empty list renders no sentence. Red: no field.

Bench before and after: `cargo bench -p kazoo-wall --bench wall_render -- 'wall/culled|wall/chains/10$'` (the regex keeps `chains/10` and drops `chains/100`), numbers in BENCH.md and the END block.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall -p kazoo-mcp
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall kazoo-mcp
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render -- 'wall/culled|wall/chains/10$'
```

## Definition of done

- [ ] Tests 1, 2, 3, 5 red then green; test 4 green; all existing tests green.
- [ ] `wall/culled/1000` after is at most 1.2× `wall/chains/10` after; both and their D01 rows in BENCH.md and the END block.
- [ ] `look` lists unheard modules; the console marks them; the MCP says them.
- [ ] `engine/tables.rs` module docs and the `unheard` field doc say what an unheard module does and does not do (step 4), and the `speak` question is answered in the doc or under FOUND.

## Code quality

Tree rules. Nothing on the render thread changes but what it is told to render. `heard` allocates on the control side only. Unsafe forbidden. NaN/Inf defence unchanged. `daemon/wall.rs` is 4628 lines: add nothing to it that can live in `tables.rs` or `timing.rs`; the cull is one call and one field. Files under 1000 lines for anything new. One behaviour per commit.

## END

```
START KZ-D03 base=<sha> branch=lane/kz-d03 df=<GB> at <time>
END KZ-D03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: wall/culled/1000 and wall/chains/10, before and after
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
