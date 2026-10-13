# KZ-D16a: every circuit model's oversampler and the klon cost less and sound the same, and the wall can time a save module by module

Line numbers are from the sweep that wrote this brief, at `820ccc5`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, except that every cargo command takes the form under **Checks** below.

Read by Cassio, 13 Oct evening. Accepted as written. The branch is whatever `lane.sh open` names (`lane/kz-D16a`). Tom has not yet answered whether the fuzz may drop to 4x; this lane keeps 8x either way.

Time: about 90 minutes of a builder's work. Crates: kazoo-fx (the oversampler, the klon, its networks and its diode), kazoo-wall (one new bench target, no source change). Depends: nothing in flight. Base: main at dispatch (`820ccc5` when written). **KZ-D16b** (fuzz, tape, crush) branches from this lane's reviewed head and uses its harness and its bench; the two lanes together are judged against Tom's grimy save (below). D15 is taken by "the wall keeps its deadline"; D16 is the next free D number.

This lane does not touch the live wall: no MCP call, no load, no save, no daemon restart. The save it measures is read from disk, read-only, and rendered headless.

## Why this lane exists (measured 13 Oct 2026, 19:54 to 20:37, by Cassio)

Tom's save "grimy beat fuzzed siren octave" (`~/.kazoo/wall/saves/grimy beat fuzzed siren octave.json`, sha256 `70eb4a20b8d135f9db184ea92d165b84a7758d135b12efeaf875fd5b811da824`) overloaded the wall. A scratch profiler loaded it through the daemon's own `Wall::new`, wrapped every module's `process` in a timer, and rendered 512-frame callbacks at 48 kHz:

| run | render CPU per callback | share of the 10.67 ms deadline |
|---|---:|---:|
| real-time priority, paced, load 71 to 91 | 4,390 µs mean, 4,759 p99 | 41% |
| real-time priority, paced, load 17 to 30 | 3,814 µs mean, 4,269 p99 | 36% |
| normal priority (efficiency cores), load 52 to 80 | 11,490 µs mean | 108% |

The modules' own sum was 4,247.5 µs. The heavy ones: fuzz4 (stereo in) 804, klon1 798, tape1 522, fuzz2 379, fuzz3 360, hall1 283, crush2 248, pitch2 208. A second save, "gorgeous toms evening", sums 2,076 µs.

Two causes this lane fixes, both measured:

- **The oversampler's dot product cannot vectorise.** `Halfband::dot` (`kazoo-fx/src/drive/oversample.rs:155`) folds every tap into one chained `mul_add`, so each multiply waits on the last. It runs at 2x to 16x the host rate inside 15 effects: every circuit model (klon, screamer, fuzz, amp, fold, ringmod, comp), the tape and pitch effects, and the lofi family (radio, vhs, reel, genloss, cassette and vinyl, through their shared parts). Eight independent partial sums, measured in isolation on an identity round trip: 4x went from 61.2 to 28.2 µs per channel per block, 8x from 133 to 88. The output differed by 2.5e-6 at 8x.
- **The klon re-walks its circuit every tick.** At 4x the klon ticks 2,048 times a block. Every tick, `Network::open`, `sum_current` and `settle` (`network.rs:163`, `:184`, `:204`) walk all 16 parts of the back network (and the 7 of the front), matching on `End` for each end of each part, to rebuild what depends only on the parts and the rate. And `clipper_integral` (`klon.rs:246`) evaluates the same exponential twice: once in `GERMANIUM.pair`, once inside `cosh` in `pair_integral`.

## Outcome

After this lane:
1. A committed bench `save_profile` in kazoo-wall prints any save's per-module cost and render time, paced at real time and promoted like the audio thread, so the next overload takes one command to place.
2. Every effect that oversamples runs a dot product that vectorises, with its sound unchanged beyond the tolerances stated below.
3. The klon solves its networks from arrays built at design time and evaluates one exponential per clipper integral, with its sound unchanged beyond the tolerance stated below.
4. A null-test harness and a set of "as before" switches exist for both D16 lanes: each change can be switched back off in a test and the old code's output compared against the new.
5. `BENCH.md` holds before and after rows for each changed effect and for the grimy save, each with its load recorded.

## The design (decided; build exactly this)

### 1. The profiler bench, first commit, no sound change (`kazoo-wall/benches/save_profile.rs`, new; `kazoo-wall/Cargo.toml`)

- A `[[bench]] name = "save_profile"`, `harness = false`. It is the scratch tool at `/private/tmp/claude-501/-Users-tom-Developer-projects-deno-rust-kazoo/37fc610d-be0a-4b16-89d4-625e4821c0d9/scratchpad/wallprof/src/main.rs`, brought into the tree. Read it first. Its `sweep` mode stays out; the fx benches cover kinds.
- Arguments: `profile <save.json>` or `total <save.json>`, then `--rt` (promote the thread with `kazoo_core::realtime::RealTime::promote(Duration::from_micros(10_667))` and pace each callback to real time), `--secs <n>` (default 20), `--warm <n>` (default 3). It ignores the `--bench` flag cargo adds. A bad path, an unreadable save or a failed promotion prints one line saying which and exits 1; it never panics on its input.
- Load: `migrate::read`, `migrate::migrate`, `Patch::load`, `engine(EngineConfig::new(48_000, tempo, 0.0), None)`, `Wall::new`, exactly as the daemon does. The tempo is the save's own.
- Per module (`profile` only): each module is rebuilt with `dsp::build` and its seed (`chance::module_seed`), wrapped in a `Timed` module that implements every method of the `Module` trait by forwarding, and swapped in with `Command::Insert`. `process` records its elapsed nanoseconds into a pre-allocated `Box<[AtomicU32]>` slot taken with `fetch_add`: no allocation, no lock. The `Instant::now()` pair adds about 25 µs a callback across 41 modules; the output says so in one line.
- Output, as the scratch tool prints it: a header with the save's path, its sha256 (computed by the bench, no new dependency: shell out to `shasum -a 256`, which is bench code, never the audio path), `vm.loadavg` before and after, and whether real-time promotion took. Then one row per module sorted by mean (id, kind, mean, p50, p99, % of the deadline, calls per block), the line `sum of modules: <µs> = <%> of 10667 us`, and `render cpu` and `render elapsed` lines (n, mean, p50, p99, max).
- Why a bench target and not an example or a test: it must run optimised, and `cargo bench` builds with the bench profile; a test would run unoptimised and say nothing about the wall's real cost.

### 2. The null harness and the "as before" switches (`kazoo-fx/src/drive/nulls.rs`, new, `#[cfg(test)] pub(crate) mod nulls;` in `drive/mod.rs`)

- `pub(crate) enum Change { Dot, Network, ClipperIntegral }`. KZ-D16b adds its own variants. Each variant is one change this lane makes, and switching it on in a test runs the code it replaced, kept verbatim under `#[cfg(test)]`.
- The switches are a `thread_local!` `Cell<u32>` bit mask, test builds only. `pub(crate) fn as_before<T>(changes: &[Change], run: impl FnOnce() -> T) -> T` sets the bits, runs, and restores the previous mask in a guard's `Drop`, so a failing test leaves no switch on. `pub(crate) fn is_before(change: Change) -> bool` reads it. Production code checks a switch only inside `#[cfg(test)]` blocks, so release builds carry nothing. Why thread-local and not a constructor flag, as `Pair::running_idle` is: the oversampler sits inside 15 effects' private state, and a flag would need a test constructor on every one of them. Each nextest test is its own process, and the harness renders on the test's own thread.
- `pub(crate) enum Input { Mono, Stereo, MonoThenStereo, RightSilent }` and `pub(crate) struct Case { name: &'static str, knobs: &'static [(usize, f32)], turn: &'static [(usize, f32)], input: Input }`. The left channel is `crate::bench::fixture(48_000.0)` (two seconds: a sweep, a burst, silence, noise). `Stereo`'s right channel is the left, 37 frames later, at 0.8 times its level. `MonoThenStereo`'s right is the left for the first second, then `Stereo`'s right. `RightSilent`'s right is zeros. `turn` knobs are set at exactly frame 48,000, the one-second mark, so a moving knob, a retune and a glide are all in every case that asks for them.
- `pub(crate) fn hear(make: &dyn Fn() -> Box<dyn Effect>, case: &Case) -> Heard`: builds, prepares at 48 kHz, sets `knobs`, resets, and renders through `process_wet` (so the wet outputs are compared too) in blocks of the awkward sizes `drive::testkit::render` uses (1, 7, 64, 333, 512, 0, 129, cycling), splitting at frame 48,000 for `turn`. `Heard` holds the four outputs and the latency.
- `pub(crate) fn assert_bit_exact(now: &Heard, before: &Heard, case: &str)`: every sample of all four outputs equal by `to_bits`. On failure it names the case, the output, the first frame that differs and both values.
- `pub(crate) fn assert_null(now, before, peak: f32, rms_db: f64, case) -> (f32, f64)`: the largest absolute difference across the four outputs is at most `peak`, and the difference's RMS relative to `before`'s RMS (each output separately, latency removed as `testkit::null_db` does) is at most `rms_db`. It prints the measured peak and dB per case, for the END block.
- `pub(crate) fn fingerprint(heard: &Heard) -> u64`: FNV-1a 64 over the bits of all four outputs in order.
- The test `every_case_prints_its_fingerprint` renders every case in this file twice, as it is and `as_before` every `Change`, and prints `case, now, before` hashes. It asserts only that every output is finite and that every case whose input is not silent produces sound. **Commit the harness before any change** (the second commit). At that commit, `now` and `before` are equal, since no switch is wired yet. They are today's sound. Paste that table and the head's table into END. At the head, every `before` hash must equal the harness commit's `now` hash, bit for bit. That proves each switch really is the old code, across commits, without a fixture that would break on another machine's libm.

### 3. The dot product (`kazoo-fx/src/drive/oversample.rs:155`)

- `Halfband::dot` becomes eight independent partial sums. `acc: [f32; 8]`. Over `taps[..count].chunks_exact(8)` zipped with `history.recent().chunks_exact(8)`, `acc[k] = t[k].mul_add(r[k], acc[k])` for k in 0..8. They are combined as `((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]))`. Then the remainder (fewer than 8 taps) is folded in order with `mul_add` onto that sum. This is the scratch study's `osbench/src/oversample_fast.rs` exactly, and the order is fixed so the output is deterministic.
- Why eight: an M1 core has four 128-bit FMA pipes, each four lanes of f32, with a latency of four cycles. Eight accumulators fill two vectors, which is enough to hide the latency, and the measured speed-up was 2.2x at 4x and 1.5x at 8x. `count` is 12 to 64, so there is always at least one full chunk.
- Today's fold stays, verbatim, as `#[cfg(test)] fn dot_chained`. `dot` begins with `#[cfg(test)] if nulls::is_before(Change::Dot) { return self.dot_chained(history); }`.
- **Tolerance, decided: a stated difference, not bit-exact.** The reordering changes rounding. Over an f32 dot of up to 64 terms, with taps whose absolute sum is under 1 and inputs at full scale, either order is within about 64 × 6e-8 ≈ 4e-6 of the exact sum. The study measured 2.5e-6 at 8x.
  - Through the oversampler alone, a 0 dBFS sweep and noise round trip at factors 1, 2, 4, 8 and 16 and host rates 44.1, 48 and 96 kHz: peak difference at most **1e-5** (−100 dBFS) and RMS null at most **−110 dB**.
  - Through every effect in `crate::catalogue()`: peak at most **1e-4** (−80 dBFS) and RMS null at most **−100 dB**. Reason: a fuzz's small-signal gain (up to about 40 dB) magnifies a 1e-6 difference to about 1e-4 at the edge of clipping, and −100 dB below the signal is under a 16-bit LSB (−96 dBFS), so no listener and no 16-bit render can tell the two apart.
  - These thresholds are fixed. If any effect misses them, the lane stops at that point: BLOCKED ON, naming the effect, the case and the measured numbers. A sound change Tom has not heard does not land.

### 4. The klon's networks (`kazoo-fx/src/drive/network.rs`)

What depends only on the parts and the rate is built in `design` (`:117`), which already runs whenever a pot moves. `open`, `sum_current` and `settle` then do no matching on `End` or `Kind`, and no branch on a part's kind.

- **`open` (`:163`) becomes two matrix products, built at design.** C is the number of capacitors, listed in part order in `caps: [usize; P]` with `cap_count`. Two arrays are built in `design`:
  - `from_history: [[f64; P]; N]`, where column c is capacitor `caps[c]` = (a, b): `Z[i][a]` if a is a node, minus `Z[i][b]` if b is a node.
  - `from_sources: [[f64; S]; N]`, where `[i][k] = Σ g_p · Z[i][node]` over every part p with one end on a node and the other on `Source(k)`.

  Then `volts[i] = Σ_c from_history[i][c] · history[caps[c]] + Σ_k from_sources[i][k] · sources[k]`, each a `mul_add` chain in index order. A resistor's history is always zero, which is why only capacitors appear.
  - **Tolerance: a stated difference.** The product is reassociated against today's two-step injection then product, so f64 rounding moves by about 1e-15 relative.
- **`sum_current` (`:184`) and `settle` (`:204`) become slot lists, bit-exact.** At design, each part touching `End::Sum` (for `sum_current`) and each capacitor (for `settle`) is listed in part order with its sign and two slot indices into a scratch array `ends: [f64; ENDS]`:
  - `ENDS = 16`;
  - slots `0..N` are the node volts and `N..N+S` the sources;
  - slot `ENDS - 1` is a constant `0.0` for `Ground` and `Sum`;
  - `const { assert!(N + S < ENDS) }` in the impl.

  Each call fills `ends` (N + S stores), then computes `g · (ends[a] − ends[b]) − h` (or `(2g).mul_add(ends[a] − ends[b], −h)` with today's flush, for `settle`) in today's order. Every operation and its order are today's, so the results are equal bit for bit.
- Today's three bodies stay verbatim under `#[cfg(test)]` as `open_walked`, `sum_current_walked` and `settle_walked`. Each public function begins with `#[cfg(test)] if nulls::is_before(Change::Network) { return self.…_walked(…); }`.
- The new arrays are fixed-size members of `Network`, built only in `design`. Nothing allocates. `Network` stays `Copy`.
- Why matrices for `open` only: it is the one where precomputing removes a whole pass (the injection walk) and leaves 64 multiply-adds for the back network. For `sum_current` and `settle`, slot lists remove the matching and keep bit-exactness, and a matrix would save nothing more.

### 5. One exponential per clipper integral (`kazoo-fx/src/drive/solve.rs`, `kazoo-fx/src/drive/klon.rs:246`)

- New `Diode::pair_and_integral(&self, volts: f64) -> (f64, f64)`.
  - `rising` is computed exactly as `pair` computes it: the same clamp and the same `exp`.
  - `falling = 1.0 / rising`.
  - `current = saturation · (rising − falling)`: bit-identical to `pair(volts).0`.
  - `integral = saturation · thermal · (rising − 1)² / rising`. That equals `2 Is nVt (cosh(V/nVt) − 1)`, since cosh x − 1 = (eˣ − 1)² / (2eˣ). At the clamp (±80) the square is 3e69, far inside f64.
- Why not `(rising + falling)/2 − 1`: it cancels catastrophically for small voltages. The squared form is more accurate there than today's `cosh − 1`, not less.
- `clipper_integral` (`klon.rs:246`) and the restatement in `Circuit::design` (the `GERMANIUM.pair(self.diode)` then `clipper_integral(self.diode, …)` pair, `:440` to `:442`) both use it, so each costs one `exp`.
- Today's `clipper_integral` stays verbatim as `#[cfg(test)] clipper_integral_two_exponentials`. The new one begins with `#[cfg(test)] if nulls::is_before(Change::ClipperIntegral) { return clipper_integral_two_exponentials(volts, reach); }`.
- **Tolerance, for 4 and 5 together, through the klon: peak at most 1e-6 and RMS null at most −120 dB.** Reason: f64 differences of about 1e-15 relative come out of an f32 output, whose resolution is 6e-8 relative. Most samples are identical, and −120 dB leaves room for a one-ulp f32 flip where a value sits on a rounding edge.

### 6. What stays

The klon's 4x rate, the fuzz's 8x rate, the table diode solve (`clip_volts`), `Pair`, the twin and idle skips from D14, and every knob range. KZ-D16b does the fuzz, the tape and the crush.

## Not this lane

- Threading or rendering on more cores (KZ-D06).
- Changing the fuzz's 8x or the klon's 4x internal rate, or any change that alters sound beyond the tolerances above. Those need Tom to hear them.
- The fuzz's knob caching, the tape and the crush (KZ-D16b).
- The hall, pitch, bbd and vcf costs beyond what the dot change gives pitch.
- The live wall.

## Red tests first

This is a speed lane, so the reds are the harness and switches that do not exist yet. There is no timing assertion in any test. Reason: this Mac's own figures for one save swing 2.6x between performance and efficiency cores and with load from 17 to 350, so a time ceiling in a test would be flaky by design. The speed proof is the bench rows, with load recorded.

1. `drive::nulls::tests::every_case_prints_its_fingerprint`. Red: no `nulls` module.
2. `drive::oversample::tests::the_split_dot_nulls_the_chained_dot`. Random taps and histories at every `count` from 12 to 64 (seeded LCG, 10,000 draws each): `|split − chained| ≤ 64 × f32::EPSILON × Σ|t·x|`. Then the round trip at every factor and rate in 3, against `as_before(&[Change::Dot])`, within 1e-5 peak and −110 dB. Red: no `Change::Dot`.
3. `drive::nulls::tests::every_effect_nulls_the_chained_dot`. Every kind in `crate::catalogue()`, at its defaults and at the circuit-models bench's settings (`drive` or `gain` at 0.9 of its range, `mix` at max), `Mono` and `Stereo`. It uses a compact input of the fixture's frames 36,000 to 48,000 then 72,000 to 84,000 (the top of the sweep, then noise: 0.5 s), so the whole catalogue runs in the dev profile. The thresholds are 1e-4 peak and −100 dB. Red: no `Change::Dot`.
4. `drive::network::tests::compiled_steps_match_the_walk`. Both klon networks at 21 gain positions (0 to 1 in 0.05), 10,000 random draws of volts, sources and histories each (seeded):
   - `settle` and `sum_current` equal the walk bit for bit;
   - `open` is within `1e-12 × Σ|terms|` of it.

   Red: no `Change::Network`.
5. `drive::solve::tests::one_exponential_gives_the_pair_and_its_integral`, over volts from −(80·nVt + 0.1) to +(80·nVt + 0.1) in 100,001 steps:
   - `pair_and_integral(v).0` equals `pair(v).0` bit for bit;
   - `.1` is within `1e-15 × 2·Is·nVt·cosh(v/nVt)` of `pair_integral(v)`.

   Red: no method.
6. `drive::nulls::tests::the_klon_nulls_its_walked_network_and_two_exponentials`. Cases:
   - `default`;
   - `the save` (gain 100, level 8.4, treble 77.5);
   - `gain 20`;
   - `gain 0`;
   - `gain turns` (gain 0, turned to 100 at 1 s);
   - `treble turns` (treble 0, turned to 100 at 1 s).

   Each runs with `Mono`, `Stereo`, `MonoThenStereo` and `RightSilent`, against `as_before(&[Change::Network, Change::ClipperIntegral])`, within 1e-6 peak and −120 dB. Red: no switches.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs, on Tom's Mac (Tom's word of 9 and 10 Oct: kazoo builds and runs here). This replaces the STACK RULES' `nice -n 10 env CARGO_BUILD_JOBS=4 cargo` form for this lane. Read `df -h /System/Volumes/Data` and `uptime` before each and put both in START and END. 26 GB were free when this was written, under PLAN's 60 GB line. Note that, and do not delete anything that is not yours.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall -p kazoo-perc -p kazoo-sampler -p kazoo-speech --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx kazoo-wall
```

The dot change reaches every crate that renders a kazoo-fx effect, hence the five-crate nextest. If an existing test there pinned an oversampled output's exact bits and now differs within this brief's tolerance, change that test, in its own commit, to compare within the tolerance, and name it in END. A test that fails by more is BLOCKED ON.

**Benches, measured on this Mac, alternating.** M is this lane's first commit (the profiler alone, no sound change). H is the head. For each of three rounds, check out M, then H (`git checkout --detach`, back to the branch after), and run, recording `uptime` before and after each run:

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-fx --bench circuit_models -j 4 -- 'fx/(klon|fuzz|screamer|amp|fold|ringmod|comp|tape|radio)$'
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench wall_render -j 4 -- 'wall/kind/(klon|fuzz|tape|crush|pitch)$'
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench save_profile -j 4 -- profile "/Users/tom/.kazoo/wall/saves/grimy beat fuzzed siren octave.json" --rt --secs 20
```

A run whose 1-minute load is above 30 at its start is kept and marked "loaded". The medians are taken over the unloaded runs where there are any, and the table says which.

## Definition of done

- [ ] Reds 1 to 6 fail first (paste the failure), then pass. All existing tests in the five crates pass, with any tolerance edit named.
- [ ] The fingerprint tables from the harness commit and the head are both pasted. Every `before` hash at the head equals the harness commit's `now` hash.
- [ ] `every_effect_nulls_the_chained_dot` prints each kind's measured peak and dB. Paste the five worst.
- [ ] `the_klon_nulls…` prints each case's measured peak and dB. Paste them all.
- [ ] `BENCH.md` gains "## KZ-D16a", with:
  - the machine line, as D01's;
  - M and H shas;
  - per round, the load before and after each run;
  - the `fx/*` and `wall/kind/*` rows before and after (medians), with the change in %;
  - the grimy save's full per-module table at M and at H (medians of the three rounds, per module), and the sum of modules and render CPU mean, p99 and max for each.

  The save's sha256 printed by the bench equals `70eb4a20…a824`. If it does not, the save changed; say so and measure the new one.
- [ ] Expected, not required: klon1 under 600 µs, fuzz2 and fuzz3 each about 45 µs lower and fuzz4 (stereo) about 90 µs lower at H (the dot alone: 8x went from 133 to 88 µs per channel). Report what was measured, whatever it is.
- [ ] Nothing allocates on the audio path: the existing `assert_no_alloc` tests in kazoo-fx pass, and one is added, `drive::nulls::tests::a_klon_block_does_not_allocate`. It renders 512 frames through a prepared klon under `assert_no_alloc`, with gain turning.

## Code quality

Tree rules. In this lane only `#[cfg(test)]` code reads a switch, and release builds contain no trace of `nulls`. `Network` stays `Copy` with fixed arrays. The oversampler's state is unchanged in size. NaN and Inf: `Pad` and `History` keep their guards, the network keeps its `QUIET` flush, and the klon keeps its rails. File lengths at `820ccc5`:

| file | lines |
|---|---:|
| `oversample.rs` | 775 |
| `klon.rs` | 787 |
| `network.rs` | 355 |
| `solve.rs` | 392 |
| `drive/testkit.rs` | 978 |

Do not add to `drive/testkit.rs`; the harness lives in `nulls.rs`. If any file would pass 1,000 lines, move that file's new tests into `nulls.rs`. One behaviour per commit, in this order:
1. the profiler (M);
2. the harness and switches;
3. the dot;
4. the network;
5. the integral;
6. any tolerance edits;
7. BENCH.md.

Files (9, under the cap of ten):
- `kazoo-wall/benches/save_profile.rs` (new)
- `kazoo-wall/Cargo.toml`
- `kazoo-fx/src/drive/nulls.rs` (new)
- `kazoo-fx/src/drive/mod.rs`
- `kazoo-fx/src/drive/oversample.rs`
- `kazoo-fx/src/drive/network.rs`
- `kazoo-fx/src/drive/solve.rs`
- `kazoo-fx/src/drive/klon.rs`
- `briefs/studio/BENCH.md`

A tolerance edit in another crate's test is a tenth file. If more than one is needed, stop at the second and put it under BLOCKED ON.

## END

```
START KZ-D16a base=<sha> branch=lane/kz-d16a df=<GB> load=<1/5/15> at <time>
END KZ-D16a head=<sha> base=<sha> measure=<M sha> pushed=lane df=<GB> load=<1/5/15> at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
NULLS: the fingerprint tables (harness commit and head); the five worst dot nulls; every klon case's peak and dB
BENCH: the BENCH.md section added, as written
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
