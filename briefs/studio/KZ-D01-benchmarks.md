# KZ-D01: every later number is measured against a baseline that exists first

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. Crates: kazoo-wall, kazoo-mix, kazoo-fx, kazoo-core (the link path lives there, so its bench does too). Depends: nothing. Runs beside A01.

## Outcome

Tom, 12 Oct, by voice: "thousands of modules across a track while delays and reverbs run"; "the Rust has to be perfect". Nothing in the tree measures anything today: no `benches/` directory, no criterion, no number for what a module, a cable, a strip or a circuit model costs (survey: `kazoo-wall` §3, `kazoo-mix` §4). After this lane there is a criterion bench for the wall's render per module kind, per chain count and per cable count, for the desk's callback at 8, 64 and 512 strips, for every effect in kazoo-fx on its own, for the control thread building and looking at a wall, and for the link's frame encode and decode; and `briefs/studio/BENCH.md` holds the first numbers from this Mac, so D02 through D13 each report "before" and "after" against them. The number this lane must produce: a BENCH.md with every bench named below filled in, and `wall/chains/1000` and `wall/cables/10000` measured in nanoseconds per 512-frame block and as a percentage of real time at 48 kHz.

## The design (decided; build exactly this)

**Dependency.** `criterion = { version = "0.5.1", default-features = false, features = ["cargo_bench_support"] }` as a `[dev-dependencies]` entry in each of the four crates (no HTML reports, no rayon, no plotters: the numbers go in BENCH.md by hand). Each bench is declared in its crate's `Cargo.toml` as `[[bench]] name = "<file>" harness = false`. criterion is a new crate in `Cargo.lock`; nothing else in the lock moves. Benches compile under `cargo clippy --all-targets -- -D warnings` with the workspace's pedantic and nursery lints and under `ast-grep scan`, so they are written to the tree's rules like any test (no `let _ =`, no `.ok()`, no `#[allow]`; `unwrap` is fine in a bench as in a test).

**The block.** Every real-time bench renders one *block* of 512 frames at 48 kHz, the wall's device buffer (`kazoo-wall/src/daemon/audio.rs`, `BUFFER_FRAMES`) and the desk's usual one. 512 frames at 48 kHz last 10,666,667 ns; "% of real time" is ns per block divided by that, times 100.

**Wall render bench: `kazoo-wall/benches/wall_render.rs`, with `benches/support/mod.rs` shared by both wall benches.** The support module is a fixture builder over the public engine API, the same shape as the `Model` in `kazoo-wall/src/engine/tests.rs:26–86` (which is private to the test module, so it is written again here, not reached into):

- `Fixture::new()` calls `engine(EngineConfig::new(48_000, 120.0, 0.0), None)` (`engine/mod.rs:623`) and keeps both halves plus `next_slot`, `next_tag`, a `Vec<(usize, Kind)>` of slots and a `Vec<Cable>` of `(from_slot, port, to_slot, jack, amount)`.
- `add(kind: Kind) -> usize`: sends `Command::Insert { slot, tag, kind, module: kazoo_wall::dsp::build(kind, 48_000.0), knobs }` with `knobs` from `kind.spec().defaults()` (as `engine/tests.rs:34–48`), returns the slot.
- `plug(from, port, to, jack, amount)`: replaces any cable in `(to, jack)`, pushes the cable. Jack numbering is the engine's: inputs `0..4`, knob `k` at `4 + k` (`MAX_INPUTS + k`, as `daemon/wall.rs:1553–1556`).
- `commit()`: builds a `CableTable` (`CableTable::with_slots(next_slot)`, then `set` per cable with `Route::new(from, port, amount, line, tag)`, where `line` is the cable's index and `tag` a number unique to the cable), sends `Command::Cables` and `Command::Order(processing_order(&slots, &edges))` (`engine/tables.rs:251`). After D04 lands, `Route` has no line field; D04 updates this support module (it is listed in D04's files).
- `render(&mut self, out: &mut [f32])`: `self.engine.render(out, 2)`; the bench's buffer is `vec![0.0; 512 * 2]`, allocated once outside the timed closure.
- Port and jack lookups by name through `kind.spec()`: `spec.outputs.iter().position(|p| p.name == name)`, `spec.inputs...`, `spec.knob_index(name)` (`catalogue.rs`).

Patches, all generated, never hand-written:

| bench id | what is built | N |
|---|---|---|
| `wall/kind/<kind>` | one module of `<kind>` with its default knobs; a `vco` into its first input if it has one; its first output into an `out`'s left input and (if it has a second output) its second into the right; so the module is fed and heard. Every kind in `Kind::all()` but `speak` (it needs macOS `say` and a phrase feed). | one bench per kind |
| `wall/chains/<N>` | N independent chains `vco → vcf (in) → out (left and right)`, each its own three modules | 10, 100, 1000 |
| `wall/unplugged/<N>` | N `lfo` modules, nothing plugged, no `out` | 100, 1000 |
| `wall/cables/<C>` | one `vco`; `M = ceil(C / 28)` `mix` modules; the vco's first output plugged into every input and every knob jack of each `mix` (28 jacks each) until C cables are placed; every `mix` output into one `out`'s left input, so the cables are heard (the out takes one cable; the rest of the mixes feed `mix`es that feed the out, four at a time, as a tree) | 10, 100, 1000, 10000 |
| `wall/culled/<N>` | N chains `lfo → vca (cv)`, of which the first 10 `vca`s feed an `out`; the other N−10 chains reach nothing | 1000 (the baseline D03 must move) |

Each `wall/...` bench: build the fixture outside the timed closure, render 64 blocks once as a warm-up (the halfband filters and reverb tails fill), then `bench_function(id, |b| b.iter(|| fixture.render(&mut buffer)))`. Group `wall` with `sample_size(50)` and `measurement_time(3 s)` (a 10,000-cable wall is slow before D04; 3 s keeps the whole file under ten minutes).

**Control bench: `kazoo-wall/benches/control.rs`.** The control thread's cost, driven through `Wall::request` (`daemon/wall.rs:2135`) with no sockets and no store, as the wall's own tests do (`daemon/wall/tests.rs:16–19`: `Wall::new(seed().unwrap(), control, None, Vec::new(), None)`):

- `control/build/<N>` for N = 100, 1000: starting from `Patch::new()` (an empty patch; `seed()` holds eleven modules the bench does not want) and a fresh engine, N times: `Request::Add { kind: "vco" }`, `Request::Add { kind: "vcf" }`, `Request::Patch { from: "vco<i>.out", to: "vcf<i>.in" }`, `Request::Patch { from: "vcf<i>.out", to: "out1.left" }` (one `out1` added first). The measured quantity is the whole build, as `iter_batched` with the fresh wall in the setup closure and `BatchSize::PerIteration`. Two things would refuse a bench: the flood guard (`FLOOD_BURST = 30` changes per seat per 10 s, `wall.rs:52–56`) and the load guard (`render_room`, `wall.rs:2612`, which answers `slow_down` for an engine that has never rendered). The bench turns the load guard off through the wall's own method for it (`Wall::set_load_guard(false)`, named in `room_to_grow`'s doc at `wall.rs:2486`) and names its seat `bench-<change / 30>` so no seat makes more than 30 changes. A refusal from `request` is a bench failure (`expect` with the error's message), never skipped.
- `control/look/<N>` for N = 100, 1000, 10000: a wall of N `vco`s built as above in the setup, then `b.iter(|| wall.request("seat", &Request::Look, now))`. The 10,000 case is the baseline D13 must move. It also records, under the table, the length of the serialised `Outcome.result` in bytes (`serde_json::to_vec(&outcome.result).len()`, printed once from the bench with `eprintln!` before the timing loop), so BENCH.md can say how large a `look` is.

**Circuit model bench: `kazoo-fx/benches/circuit_models.rs`.** One bench per effect in `kazoo_fx::catalogue()` (`kazoo-fx/src/lib.rs:198`), `fx/<id>`, built with `kind.build()` then `prepare(48_000.0)` (the `Effect` trait, `lib.rs:124–160`). This isolates the model: no wall, no cables, no `Box<dyn Module>` wrapper, just `Effect::process` on 512 frames. Knobs: defaults, then every param whose name is `mix` set to its maximum (all wet), and every param named `drive` or `gain` set to `min + 0.9 × (max − min)` (the heavy end of the model); a param is found by name in `kind.params` (`ParamSpec`: its name, min and max fields, by what they are). Input: a stereo sine at 110 Hz, −12 dBFS, 512 frames, generated once; output buffers allocated once; `Context { bpm: 120.0 }`. Warm 64 blocks, then `b.iter(|| effect.process(&context, [&l, &r], [&mut ol, &mut or]))`. Group `fx`, `sample_size(50)`, `measurement_time(3 s)`.

**Desk callback bench: `kazoo-mix/benches/desk_callback.rs`.** `desk/callback/<S>` for S = 8, 64, 512 strips. `MixerEngine::new(S, 48_000)` (`kazoo-mix/src/engine.rs:268`; it takes any count, and `DESK_CHANNELS` is the desk's, not the engine's). Each strip gets a consumer: `audio_block_ring(AudioRingConfig::new(BufferId(slot), 2, 512, 8))` from `kazoo_core::audio_transport`, attached with `engine.attach_consumer(slot, short_name("b"), consumer)` (as `callback.rs:178–183`), its producer kept. Each iteration: push one 512-frame stereo block of a −12 dBFS sine at `engine.next_frame()` into every producer (outside the timed region), then time `engine.render_f32(&mut buffer, 2)` (`engine.rs:466`) alone, with `iter_custom`: `for _ in 0..iters { push all; let t = Instant::now(); render; total += t.elapsed(); }`. Every strip's fader is left at its default and no strip is muted, so the strip path runs in full. Group `desk`, `sample_size(50)`.

**Link bench: `kazoo-core/benches/link_path.rs`.** `link/encode/512`: `encode_audio_payload` (`kazoo-core/src/ipc/protocol.rs:312`) of 512 stereo frames into a pre-sized `Vec<u8>` then `write_frame` (`protocol.rs:140`) into a `Cursor<Vec<u8>>` reset each iteration. `link/decode/512`: the bytes written once in setup; each iteration `FrameBuffer::try_read_frame` (`protocol.rs:199`) over a `Cursor` of them then `decode_audio_samples` (`protocol.rs:347`) into a pre-sized `Vec<f32>`. These are the two copies B02 keeps; B01 and B02 report against them.

**`briefs/studio/BENCH.md`.** Written by hand from criterion's console output (the `time: [low mid high]` line; the mid value is the one recorded). Its shape:

```
# Benchmarks

Numbers from Tom's Mac unless a row says otherwise. A block is 512 frames at
48 kHz (10,666,667 ns). Benches run as:
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p <crate> --bench <name> -- '<filter>'

## <date> at <sha> (lane KZ-D01, baseline)

Machine: <model>, <n> cores, macOS <version>, rustc <version>.

| bench | N | ns per block | blocks per second | % of real time |
|---|---|---|---|---|
| wall/kind/vco | 1 | ... | ... | ... |
| wall/chains | 10 | ... | ... | ... |
| ... | | | | |

Control-thread benches (not real time: ns per operation and operations per
second; the look's size in bytes in the last column):

| bench | N | ns per op | ops per second | bytes |
|---|---|---|---|---|
| control/build | 100 | ... | ... | |
| control/look | 10000 | ... | ... | ... |
```

Every bench named in this brief has a row. Each later lane appends a section `## <date> at <sha> (lane KZ-Dnn, after)` with the rows it moved and the baseline row beside each for the eye. "blocks per second" is `1e9 / ns`.

**Why these choices.** criterion over divan: it is the tool every Rust contributor knows, its output is stable text, and `cargo_bench_support` alone keeps it from pulling rayon into a workspace that forbids a thread pool on the audio path by accident. A 512-frame block rather than one sub-block: it is what the device asks for, and a sub-block alone would make the per-callback costs (command drain, `measure`, the stereo copy) invisible. Fixtures generated in code rather than saved patches: a saved patch would tie the bench to the patch format and the store; the engine API is the stable surface. One bench per kind: D07–D10 each move a handful of kinds and must see their own rows.

## Not this lane

No change to any `src/` file in any crate. No optimisation. No HTML reports. No CI. No `Justfile` (A08). No bench of the TUI, the socket server or the disk store. No bench of `speak`.

## Red tests first

Benches are not tests, but the support module is code and gets tests of its own in `kazoo-wall/tests/bench_support.rs` (an integration test, so it reaches the same public API the bench does; it `#[path]`-includes `benches/support/mod.rs`):

1. `a_chain_fixture_is_heard`: `chains(1)` rendered for 64 blocks produces a non-zero master peak (the vco reaches the out).
2. `an_unplugged_fixture_is_silent`: `unplugged(10)` renders silence for 64 blocks.
3. `a_cable_fixture_places_every_cable`: `cables(100)` sends a table holding exactly 100 routes (count the fixture's cable list and assert the `CableTable::set` results were all `true`).
4. `the_culled_fixture_has_ten_outs_fed`: `culled(100)` has exactly 10 `vca`s plugged into an `out`.

Then every bench runs once end to end (below) and its numbers go into BENCH.md.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-wall -p kazoo-mix -p kazoo-fx -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-wall -p kazoo-mix -p kazoo-fx -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall kazoo-mix kazoo-fx kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench wall_render
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-wall --bench control
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench circuit_models
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-mix --bench desk_callback
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-core --bench link_path
```

The first `cargo bench` is a release build of kazoo-wall and everything under it: minutes, once; it is warm after. Paste each bench's `time:` lines into the log under `<log dir>/bench-<name>.txt`.

## Definition of done

- [ ] criterion is a dev-dependency of kazoo-wall, kazoo-mix, kazoo-fx and kazoo-core; five bench files exist and every bench id in this brief runs.
- [ ] `briefs/studio/BENCH.md` holds a row for every bench id, with ns per block, blocks per second and % of real time (or the control-thread columns), the machine line, the date and the sha.
- [ ] The four support tests are red (the support module does not exist) then green.
- [ ] clippy, fmt, ast-grep and nextest clean for the four crates.

## Code quality

Tree rules (unsafe forbidden; no `#[allow]`; no `let _ =`; no `.ok()`; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines; no TODO; no stub). Benches and the support module allocate only in setup; nothing is timed that allocates except what the code under test allocates itself (and the wall's render allocates nothing, which `engine/tests.rs` already proves). No bench reads a file or a device. No bench writes to the tree except BENCH.md by the builder's hand.

## END

```
START KZ-D01 base=<sha> branch=lane/kz-d01 df=<GB> at <time>
END KZ-D01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: every row written to BENCH.md, repeated here
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
