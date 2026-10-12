# KZ-D07: one screamer costs under a quarter of what it costs today and one amp under half, and a null test against the old model holds under a stated level

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Bench names: D01's brief builds `kazoo-fx/benches/circuit_models.rs` with ids `fx/<id>` at defaults. Read what D01 actually landed. If the `models` bench ids this brief names (`model/<id>/default|hot` and the named variants) or the fixture `kazoo_fx::bench::fixture(rate)` do not exist, this lane's first commit adds the ids and the fixture it needs beside D01's bench, without renaming D01's rows, and says so under FOUND.

Split: this file is two lanes, run in this order, each on its own branch with its own END block. **KZ-D07a** is the screamer (`lane/kz-d07a`); **KZ-D07b** is the amp (`lane/kz-d07b`, cut from main after D07a lands). Each is one mechanism, about 30 minutes of a builder's work. Crates: kazoo-fx only. Depends: KZ-D01 (the `models` bench and the fixture it publishes) landed on main. D07b depends on D07a only because D07a adds the null-test helper both use.

## Outcome

Today the screamer (`kazoo-fx/src/drive/screamer.rs`) runs its clipping stage at 705.6 kHz, sixteen times the host's rate (`INTERNAL_RATE = 4.0 * TARGET_RATE`, `screamer.rs:63`), and on every one of those samples solves the diode pair by bracketed Newton to a billionth of a volt (`rising_root` at `screamer.rs:178`, up to 48 iterations, one `exp` and one divide each, `solve.rs:61`). At 48 kHz that is 1.5 million solves a second for one pedal, and it is the heaviest module on the wall. The amp (`drive/amp.rs`) runs its two valves and the power stage at the same 705.6 kHz (`amp.rs:193`) and, though it has no solver, works out the cubic through its valve table afresh on every read (`Triode::coefficients`, `amp.rs:405`, four table reads and a dozen flops, called twice per valve per sample) and runs both tone stacks every sample when only one is heard (`amp.rs:592-594`).

After this lane the screamer runs at 352.8 kHz (eight times) and solves each sample with a table seed and two fixed Newton steps; the amp runs at 352.8 kHz with its cubics precomputed and only the tone stack that is heard. Measured by D01's benches: `model/screamer/default` and `model/screamer/hot` cost no more than a quarter of what they cost at the lane's base; `model/amp/default` and `model/amp/hot` no more than half. A null test renders D01's two-second fixture through the old model (kept as a reference constructor) and the new one and holds the difference under -40 dB at the default knobs and -30 dB flat out. Tom accepts the tone cost of the lower rate (the screamer's own doc at `screamer.rs:55-62` gives it: the worst fold on a -6 dBFS 5 kHz note goes from -93 dBc to -65 dBc; the amp's, `amp.rs:188-192`, from -65 to about -47 dBc flat out). Nothing clicks, nothing goes non-finite, nothing rings on.

## The design (decided; build exactly this)

Bench names: D01 landed `kazoo-fx/benches/circuit_models.rs` with ids `fx/<id>` at defaults (512-frame blocks). If the `models` bench ids this brief names (`model/<id>/default|hot`) or the fixture `kazoo_fx::bench::fixture(rate)` do not exist on the lane's base, the lane's first commit adds them beside D01's bench, without renaming D01's rows, and says so under FOUND. If another D lane (D10a) has already landed them, use those and add only the rows this lane needs.

RETRY NOTE for KZ-D07a (Cassio, 13 Oct 03:35). The first run (branch lane/kz-D07a, head 9fa8a35) built the table, the solve, the reference and the null helper, and got these after figures at a machine load near 240: cost 0.40 (default) and 0.30 (hot) of before; `no_subharmonics` -66.4 dBc against -80; conformance latency error 0.097 samples at 192 kHz against 0.05; worst spur -65.2 dBc at 44.1 kHz. Decisions for the retry:
- Cost: a bench at load 200+ is noise of about 2x (see D02). Record before and after in BENCH with the load average next to each, interleaved (before, after, before, after) from the same build; the 0.25 ratio is judged by the reviewer on a quiet machine, not by this run. Do not spend the time box chasing the ratio.
- `no_subharmonics`: the 12 dB relaxation cap is raised to 15 dB for this test only; if the measured figure is -66.4 dBc the limit becomes -66.0 and the commit names the number. Reason: the old -80 was set for 16x; the fuzz, already at 8x, is accepted at -70 spurs, and -66 dBc of subharmonic under full drive is 40 dB below the drive's own distortion products.
- Latency at 192 kHz (factor 2x): find why the measured delay is 0.097 samples off the reported whole-sample `latency()` at 2x and not at 4x or 8x (read `oversample.rs` where the padding to whole samples is computed for each factor). Fix the padding for the 2x case so the reported latency matches; never relax this limit. The first run's half-sample compensation broke the null and must not be repeated.
- `solve` must have no loop: write the two steps out (or a `const` step count unrolled by hand), as the brief says.

### KZ-D07a: the screamer

**Rate.** `INTERNAL_RATE` (`screamer.rs:63`) becomes `2.0 * TARGET_RATE`. The module doc paragraph at `screamer.rs:9-11` and `55-62` is rewritten to say 352.8 kHz and the -65 dBc figure it already gives for that rate. `Pair::prepare` (`pair.rs:65`) and `factor_for` (`oversample.rs:54`) then give 8x at 44.1 and 48 kHz, 4x at 96 kHz, 2x at 192 kHz, with nothing else to change.

**The table.** In `drive/solve.rs`, next to `Diode`, add `pub struct SinhTable` that solves the dimensionless clipper equation `y + mu * sinh(y) = xi`. Every antiparallel-pair clipper in the family reduces to it: for a diode with saturation `Is` and thermal voltage `nVt` (`Diode::new`, `solve.rs:97`), the circuit equation `v + r * I(v) = x` with `I(v) = 2 Is sinh(v / nVt)` becomes that form with `y = v / nVt`, `xi = x / nVt`, `mu = 2 Is r / nVt`.

- Axes: `xi` is tabulated through `t = asinh(xi)` on `T_POINTS = 1025` even points over `[-T_MAX, T_MAX]`, `T_MAX = 12.0` (that reaches `xi` of 8e4, 3.7 kV for the screamer, far past anything `guard`'s +24 dBFS limit at `kit.rs:13` can produce); `mu` on `MU_POINTS = 17` points even in `ln(mu)` between the `mu_low` and `mu_high` the table is built for. Values are `f32` (the seed only has to be within a thousandth; Newton does the rest). Storage: one `Vec<f32>` of `MU_POINTS * T_POINTS` (70 KB), allocated by `SinhTable::new(mu_low, mu_high)`, which solves every point with `rising_root_within` to `1e-12` seeded from the previous point. `new` is called only from an effect's `prepare` (allowed to allocate, `lib.rs:21-23`), never from `process`.
- `fn seed(&self, xi: f64, mu: f64) -> f64`: bilinear in `(t, ln mu)`, both clamped to the table's range.
- `pub fn solve(&self, xi: f64, mu: f64, steps: u32) -> f64`: the seed, then exactly `steps` Newton steps on `f(y) = y + mu * sinh(y) - xi`, `f'(y) = 1 + mu * cosh(y)`, one `exp` per step (`sinh` and `cosh` are its odd and even halves, as `Diode::pair` does at `solve.rs:110-115`), `y` held to `[-Y_MAX, Y_MAX]`, `Y_MAX = 40.0`, after every step. No loop, no bracket, no early exit: fixed cost. A non-finite `xi` or `mu` returns 0.
- `impl Diode { pub fn clip_volts(&self, table: &SinhTable, reach: f64, open: f64, steps: u32) -> f64 }`: the volts `v` with `v + reach * pair(v) = open`, through `solve` with the scaling above, result in volts.

**The screamer's use of it.** `Circuit::tick` (`screamer.rs:167-200`) today solves `v * linear + step * I(v) = target` over `[-bound, bound]`. Divide through by `linear`: `reach = step / linear`, `open = target / linear`, so the call is `SILICON.clip_volts(table, step / linear, target / linear, SCREAMER_STEPS)` with `const SCREAMER_STEPS: u32 = 2`, then `volts.clamp(-bound, bound)` (the old bracket kept as a clamp, since the new solve is unbracketed). `mu` depends on the drive knob alone (`rf` at `screamer.rs:293`; `step`, `leg`, `loop_gain` are fixed by the rate), so `Screamer::prepare` builds the table for the pot's two ends: `mu_low` from `rf = R6 + DRIVE_POT` and `mu_high` from `rf = R6` (compute `linear` as `frame` does at `screamer.rs:172-175` for each). The table lives in `Screamer` as `table: SinhTable` and is passed into the `Pair::process` closure by reference, exactly as the amp passes `&self.triode` at `amp.rs:776-782`. `Circuit::previous` (`screamer.rs:154`, the extrapolation seed) and its flush go; `feedback` stays (it feeds `target`).

**The old model as the reference.** `Screamer::build` (`screamer.rs:258`) gains two arguments: `internal: f32` and `solve: Solve`, where `#[derive(Clone, Copy)] enum Solve { Table, Loop }` lives in `screamer.rs`; `Stage` carries the `Solve`; `tick` matches on it, `Loop` being today's `rising_root` call unchanged. `Screamer::new` is `build(true, INTERNAL_RATE, Solve::Table)`; a `#[cfg(test)] pub fn reference() -> Self` is `build(true, 4.0 * TARGET_RATE, Solve::Loop)`, the model exactly as it is at the lane's base. This is the same pattern as `naive()` (`screamer.rs:254`) and `Pair`'s `anti_alias` flag. Reason for a frozen reference in code rather than a reference WAV under `tests/fixtures`: the change is local (one rate constant, one solve call), so the reference is twenty lines that cannot drift; a WAV would be 768 KB per model per rate of binary in git, would need `hound` (not a kazoo-fx dependency), and would silently go stale when D01's fixture or any shared filter changed.

**The null helper.** New `#[cfg(test)]` function in `drive/testkit.rs`: `pub fn null_db(reference: &mut dyn Effect, candidate: &mut dyn Effect, knobs: &[(usize, f32)]) -> f64`. It prepares both at 48 kHz, sets the knobs on both, resets both, feeds D01's fixture (`kazoo_fx::bench::fixture(48_000.0)`, or whatever D01 named the two-second sweep-burst-noise signal: find it by what it does) to both channels in 256-frame blocks, drops `latency()` frames from the front of each output (the two rates pad to different whole-sample latencies, `oversample.rs:409-422`), compares the overlap, and returns `20 log10(rms(candidate - reference) / rms(reference))` of the left channel. D07b, D08 and D09 use it.

**Conformance.** `worst_spur_db` in the screamer's `conformance!` (`screamer.rs:340-350`) goes from `-80.0` to `-70.0`, the fuzz's figure (`fuzz.rs:397`), since the fuzz already runs at this rate; `hot_spur_db` stays `-48.0`. If any conformance or `no_subharmonics` limit still fails at 8x, the measured figure goes under FOUND and the limit is set to that figure rounded down to the next multiple of 3 dB, by at most 12 dB in all, with the commit message naming the number; the lane never goes back to 16x, and the reviewer judges the number.

### KZ-D07b: the amp

**Rate.** `INTERNAL_RATE` (`amp.rs:193`) becomes `2.0 * TARGET_RATE`; the doc at `amp.rs:188-192` is rewritten with its own -47 dBc figure for 352.8 kHz. `HOT_SUBHARMONIC_DB` (`amp.rs:866`) becomes `-45.0` (the doc's -47 dBc measured at this rate, with 2 dB of margin); `worst_spur_db` (`amp.rs:871`) becomes `-70.0`. The same relaxation rule as D07a applies if a limit still fails.

**Precomputed cubics.** `Triode` (`amp.rs:351`) gains `segments: Vec<[f64; 4]>` of `TABLE_SIZE - 1` entries, built once in `Triode::new` by the existing `segment` (`amp.rs:339`) through the existing `coefficients` logic; `plate` (`amp.rs:425`) and `integral` (`amp.rs:440`) read `self.segments[index]` instead of calling `coefficients`. `coefficients` stays only as the builder's helper. The numbers are bit-identical to today's.

**One tone stack when one is heard.** `Circuit` (`amp.rs:565`) gains `fender_live: bool` and `marshall_live: bool`. In `tick` (`amp.rs:592-594`): with `voice == 0.0` only `fender` runs and `marshall` is marked stale; with `voice == 1.0` only `marshall` runs; between (the voice switch gliding over its 20 ms, `kit.rs:9`) both run. A stack coming back from stale is `reset()` first, so it starts from silence under a weight that is itself starting from zero: no click. Pattern: the fold's `serge_stale` (`fold.rs:570-581`).

**Reference.** `Amp::build` (`amp.rs:724`) gains `internal: f32`; `#[cfg(test)] pub fn reference() -> Self` builds at `4.0 * TARGET_RATE`. Both tone stacks always ran at the base, so the reference also takes a `both_stacks: bool` on `Settings`, true for the reference, false for `new()`.

## Not this lane

- D08 (fold, klon), D09 (fuzz, tape), D10 (reverbs, pitch): untouched. D08's klon will reuse `SinhTable` and `Diode::clip_volts`; build them as described so it can.
- The halfband filters (`drive/oversample.rs`), `Pair`, `Iir3`, `Network`: untouched.
- f64 stays everywhere in both models: the antiderivative quotients (`amp.rs:463-470`, `amp.rs:614-625`) divide differences by steps as small as 1e-7 and need f64's 1e-16; f32's 6e-8 would be noise at full scale. That is the answer to "f64 to f32 where the null test allows": here it does not, by arithmetic, and the brief says so rather than asking a test.
- The amp's cabinet (`amp.rs:655-685`, host rate) and sag: untouched.
- The wall's effect wrapper, the catalogue, kazoo-mix: untouched.

## Red tests first

Each lane: BENCH before, then each test red (paste its failing output), then green, one behaviour per commit.

**D07a**

1. BENCH before, at the lane's base before any change: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/screamer'`; the two numbers (ns per 256-frame block) go in the END's BENCH.
2. `solve::tests::the_table_seed_is_within_a_thousandth`: a `SinhTable::new(1.5e-3, 2.6e-3)`; over 9 `mu` values even in `ln mu` across that range (including both ends) and 2001 `t` values even over `[-12, 12]`, `|seed - y_loop| <= 1e-3 * max(1, |y_loop|)` where `y_loop` is `rising_root_within` to `1e-12` on the same residual. Proves the two-step budget is honest.
3. `solve::tests::two_newton_steps_reach_the_loop`: same grid, `|solve(xi, mu, 2) - y_loop| <= 2e-8` (that is a billionth of a volt for the screamer's `nVt` of 45 mV), and for `xi` beyond the table (`t = 14`, `t = -14`) and for `mu` beyond its range (half `mu_low`, double `mu_high`) the result is finite and within `1e-6` of the loop.
4. `solve::tests::nonsense_in_the_solve_is_silence`: `solve(NAN, mu, 2)`, `solve(xi, INFINITY, 2)`, `solve(INFINITY, mu, 2)` all return `0.0`.
5. `screamer::tests::the_null_against_the_old_model_holds`: `null_db(&mut Screamer::reference(), &mut Screamer::new(), &[]) < -40.0` and with `&[(DRIVE, 100.0), (TONE, 100.0)]` `< -30.0`. Red before the lane because `reference()` and `null_db` do not exist; the test is written first against the names, fails to compile, then passes.
6. `screamer::tests::flat_out_square_wave_stays_bounded_and_the_tail_dies`: `Screamer::new()`, `DRIVE 100`, `LEVEL 12`, one second of a 50 Hz square at `+-16.0` (the input limit) through `process`; every output sample finite and `<= 2.0` in magnitude (`ceiling`, `kit.rs:227`); then one second of silence; the RMS of the last 100 ms is under `1e-7`.
7. The existing `conformance!` suite (poison, silence, every parameter extreme, odd blocks, spurs, glides, latency), `oversampling_cuts_the_aliasing`, `no_subharmonics`, `the_mids_clip_and_the_bass_stays_clean`, `tone_opens_the_top` all green at 8x, with the limit changes above.
8. `tests/drive_no_alloc.rs` green: the table is read, never built, inside `process`.
9. BENCH after: the same command; the ratio `after / before` for both ids is `<= 0.25`. If it is not, the lane is not done: profile with the bench's own report, fix, re-run; the END states the final ratio.

**D07b**

1. BENCH before: `... -- 'model/amp'`.
2. `amp::tests::precomputed_segments_match_the_cubic`: for 10,000 grid voltages even over `[GRID_LOW - 1, GRID_HIGH + 1]`, `plate` and `integral` from the segments equal the values computed through `segment` on the fly, to the bit (`to_bits()` equality) inside the table and within `1e-12` outside it.
3. `amp::tests::the_null_against_the_old_model_holds`: `null_db(&mut Amp::reference(), &mut Amp::new(), &[]) < -40.0`; with `HOT` (`amp.rs:863`) `< -30.0`.
4. `amp::tests::switching_the_voice_does_not_click`: `VOICE` from 0 to 1 and back during a 220 Hz tone at -12 dBFS; the largest sample-to-sample step within 30 ms of each switch is no more than three times the largest step in the 100 ms before it (the pattern of `knob_jumps_do_not_click`, `testkit.rs:762`).
5. `amp::tests::flat_out_square_wave_stays_bounded_and_the_tail_dies`: as D07a's, with `HOT`.
6. The existing suite green with the limit changes; `tests/drive_no_alloc.rs` green.
7. BENCH after: ratio `<= 0.5` for both ids.

## Checks

Exactly these, each with its exit code read from the tool and written in the END:

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-fx --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/screamer'   (D07a, before and after)
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/amp'        (D07b, before and after)
```

Doc tests run because the module docs change. No workspace-wide command.

## Definition of done

**D07a**
- [ ] `INTERNAL_RATE` is `2.0 * TARGET_RATE`; `SinhTable`, `Diode::clip_volts` and `Solve` exist as described; `previous` is gone from `Circuit`.
- [ ] Tests 2-6 red then green, output pasted; test 7 green with the limit changes named in the commit; test 8 green.
- [ ] BENCH: both screamer ids `<= 0.25` of before, numbers in the END.
- [ ] `null_db` exists in `drive/testkit.rs` for the lanes after.
- [ ] Pushed to `origin/lane/kz-d07a`; QUEUE line.

**D07b**
- [ ] `INTERNAL_RATE` is `2.0 * TARGET_RATE`; segments precomputed; one stack when one is heard; `reference()` exists.
- [ ] Tests 2-5 red then green, output pasted; test 6 green.
- [ ] BENCH: both amp ids `<= 0.5` of before.
- [ ] Pushed to `origin/lane/kz-d07b`; QUEUE line.

## Code quality

Rules of the tree: `unsafe_code` forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines (`amp.rs` is 1131 today: the precomputed `Triode` moves to `drive/triode.rs`, its tests with it, in its own commit before the behaviour commits); no TODO, no stubs, no deferred work. Nothing allocates in `process`, `set_param` or `reset`: the table is built in `prepare`. Every solve returns finite for any input; a NaN or infinite `xi` or `mu` is silence. The reference path (`Solve::Loop`, the 16x rate) is production code used by tests, like `naive()`: it stays small and is never the default. Comments say why, in plain words, as the surrounding files do; no emoji; no dashes as punctuation in anything written into the tree.

## END

```
START KZ-D07a base=<sha> branch=lane/kz-d07a df=<GB> at <time>
END KZ-D07a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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

The same block, with `KZ-D07b`, ends the second lane.
