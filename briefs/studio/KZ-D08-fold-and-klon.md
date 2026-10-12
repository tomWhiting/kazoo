# KZ-D08: the folder's Lambert W comes from a table and the Klon's diode from the shared table, so a Serge fold costs under a third of today and a Klon under two thirds, with null tests against the old models

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Split: two lanes in this file, run in this order. **KZ-D08a** is the fold (`lane/kz-d08a`); **KZ-D08b** is the klon (`lane/kz-d08b`, cut from main after D08a lands). Each is one mechanism, about 30 minutes of a builder's work. Crates: kazoo-fx only. Depends: KZ-D01 (benches and fixture) and KZ-D07a (`SinhTable`, `Diode::clip_volts` and `null_db` in `drive/testkit.rs`) landed on main.

## Outcome

Today the folder (`kazoo-fx/src/drive/fold.rs`) runs at 705.6 kHz, sixteen times the host (`fold.rs:697`, `4.0 * TARGET_RATE`), and on the Serge path every one of its four Lockhart stages solves a Lambert W by bracketed Newton to `1e-12` (`lambert_w_exp`, `fold.rs:269-283`, through `rising_root_within`, one `ln` and one divide per iteration), seeded with the last W. That is up to 4 solves x 16 x 2 channels per host frame. The Klon (`drive/klon.rs`) runs at 176.4 kHz, four times, and per sample runs two nodal networks (a 3x3 and an 8x8 mat-vec, `klon.rs:328-341`, `network.rs:163-180`) and a bracketed Newton diode solve to `1e-13` (`klon.rs:366`), then spends two more `exp`s on the clipper's antiderivative (`clipper_integral`, `klon.rs:239-245`).

After this lane: W(e^u) is read from a table and refined by two fixed Newton steps; the folder runs at 352.8 kHz (eight times); the Klon's diode is solved from D07a's `SinhTable` with three fixed steps and its antiderivative reuses the solve's exponential. Measured by D01's benches: `model/fold/buchla` at or under half of the lane's base, `model/fold/serge` at or under 0.35; `model/klon/default` and `model/klon/hot` at or under 0.6. Null tests hold under -40 dB (default knobs) and -30 dB (hot) for the fold, and under -60 dB for the Klon, whose change is only the solver's arithmetic. No clicks, nothing non-finite, nothing rings on.

## The design (decided; build exactly this)

### KZ-D08a: the fold

**Rate.** `self.pair.prepare(base, 4.0 * TARGET_RATE, FOLD_DELAY)` at `fold.rs:697` becomes `2.0 * TARGET_RATE`. `FOLD_DELAY` (`fold.rs:260`) is in samples at the folder's own rate and does not change. The module doc's sentence about the rate (find it near the top of `fold.rs`) is rewritten.

**The omega table.** W(e^u) is the Wright omega function, a pure one-dimensional function of `u`, so it is tabulated once per process, like the makeup table (`MAKEUP`, `fold.rs:323`):

- `static OMEGA: OnceLock<Box<[f64]>>`, `OMEGA_POINTS = 4097` values of `omega(u)` even over `[OMEGA_LOW, OMEGA_HIGH] = [-40.0, 40.0]` (`fold.rs:270` already treats `u < -40` as `e^u`), each solved at build by today's loop to `1e-14`, seeded from the previous point. 32 KB. Built on the first `Fold::new` (the first build of a folder happens off the audio thread, as the makeup table's does).
- `fn lambert_w_exp(u: f64) -> f64` (no `guess` argument any more): `u < OMEGA_LOW` returns `u.exp()` (unchanged); `u > OMEGA_HIGH` seeds from the asymptotic series `L1 - L2 + L2 / L1 + L2 * (L2 - 2) / (2 L1^2)` with `L1 = u`, `L2 = ln u`; otherwise linear interpolation in the table. Then exactly `OMEGA_STEPS = 2` Newton steps on `f(w) = w + ln w - u`, `f'(w) = 1 + 1 / w` (one `ln` each), `w` held at or above `1e-300` after each step. No loop, no bracket. A non-finite `u` returns 0.
- Why two steps are enough, written as the function's doc: the seed is within `5e-5` relative in the table (linear interpolation of a function whose second derivative is at most itself, step `80 / 4096`) and within `4e-3` beyond it; Newton's error constant `f'' / 2f'` is `1 / (2w (1 + w))`, so two steps take `5e-5` to below `1e-16` relative for small `w` and `4e-3` to below `1e-12` for large.
- Today's loop stays as `fn lambert_w_exp_loop(u: f64, guess: f64) -> f64` (the body of `fold.rs:269-283`): the builder of the table and the reference.
- `lockhart` (`fold.rs:288`) loses its `guess` argument and its third return value; `Stage::last_w` (`fold.rs:416`) goes; `serge` (`fold.rs:514-545`), `static_curve` (`fold.rs:302`), `Folder::reset` (`fold.rs:465-473`) follow. `lockhart` takes a `omega: Omega` argument, `#[derive(Clone, Copy)] enum Omega { Table, Loop }`, carried on `Settings` (`fold.rs:400`); `Loop` calls `lambert_w_exp_loop` seeded from `0.0` (the old warm start was the last W; seeding from zero makes the reference slower, not different, since the loop converges to `1e-12` either way).

**Reference.** `Fold::build` (`fold.rs:628`) gains `internal: f32` and `omega: Omega`; `new()` is `(true, 2.0 * TARGET_RATE, Omega::Table)`; `#[cfg(test)] pub fn reference()` is `(true, 4.0 * TARGET_RATE, Omega::Loop)`. Same reasoning as D07's reference: the change is local, so a frozen reference in code cannot drift, where a WAV under `tests/fixtures` would be binary in git, need `hound`, and go stale.

**Conformance.** The fold's limits (`fold.rs:742-750`: `worst_spur_db -70`, `hot_spur_db -48`) stay. If one fails at 8x: the measured figure under FOUND, the limit set to it rounded down to the next multiple of 3 dB, at most 12 dB in all, named in the commit; never back to 16x.

### KZ-D08b: the klon

**The solve.** `Circuit::clip` (`klon.rs:361-386`) replaces `rising_root_within(-bound, bound, guess, CLIP_TOLERANCE, ...)` with `GERMANIUM.clip_volts(table, reach, open, KLON_STEPS)`, `const KLON_STEPS: u32 = 3`, then `.clamp(-bound, bound)`. Three steps, not D07's two, because the Klon's antiderivative divides by steps down to `TINY_STEP = 1e-6` (`klon.rs:172`, `:377`) under a diode slope of order a thousand, which is why `CLIP_TOLERANCE` is `1e-13` (`klon.rs:170`): from a seed within a thousandth, three Newton steps in the diode region (error constant about one half) reach `1e-13` and the fourth would be below f64's resolution. `before` (`klon.rs:274`) and the extrapolation go; `diode` stays. `CLIP_TOLERANCE` stays for the reference path.

**The table's range.** `mu = 2 Is reach / nVt` depends on `reach` (`klon.rs:412`, the back network's impedance at the diode node), which depends on the gain knob and the rate. `Klon::prepare` (`klon.rs:511`) builds `table: SinhTable` in `Klon` for `mu_low` and `mu_high` taken as the minimum and maximum of `reach` over `GAIN_POINTS = 17` gain fractions even over `[0, 1]`: for each, a scratch `Network::<8, 16, 2>::new(BACK, c)` with the two pot resistances set as `design` sets them (`klon.rs:407-412`) and `impedance(DIODES, DIODES)` read; the scratch networks are stack values and the loop runs in `prepare`. The table is passed into the `Pair::process` closure by reference (`klon.rs:504-506`).

**One exponential for the antiderivative.** `Diode` gains `pub fn pair_from(&self, rising: f64) -> (f64, f64)` and `pub fn pair_integral_from(&self, rising: f64) -> f64`, both computed from `rising = exp(v / nVt)` (`cosh = (rising + 1 / rising) / 2`); `pair` and `pair_integral` call them after their own `exp`. `clipper_integral` (`klon.rs:239`) takes `rising` and uses the `_from` forms, and `clip` computes `rising` once after the solve (one `exp`) and uses it for both the integral and nothing else. Per sample: three `exp`s in the solve plus one, where today it is the loop's four to seven plus two.

**Reference.** `Klon::build` (`klon.rs:470`) gains `solve: Solve` (reuse D07a's `Solve` enum by moving it to `drive/solve.rs` as `pub enum Solve { Table, Loop }` in D07a's shape; if D07a left it in `screamer.rs`, move it in this lane's first commit and point the screamer at it); `tick` and `clip` take it; `Loop` is today's `rising_root_within` call and today's two-`exp` `clipper_integral`. `#[cfg(test)] pub fn reference()` builds with `Solve::Loop` at the same 4x rate.

## Not this lane

- The Klon's per-sample redesign while the gain or treble glides (`Retune::due` every sample, `klon.rs:499`; each `design` inverts the 8x8 nodal matrix, `network.rs:117-138`): a real spike (about 1000 inversions over a 20 ms glide at 48 kHz, 4x) and the next thing to fix in this file, but not this mechanism. Name it under FOUND with the numbers if the bench shows it.
- The fold's Buchla path (`buchla`, `buchla_integral`, `fold.rs:221-239`): closed-form already; untouched beyond the rate.
- The makeup table (`fold.rs:326-360`): rebuilt by the same code at the new rate-independent form; untouched.
- D07 (screamer, amp), D09, D10: untouched. `SinhTable` is used, not changed; if it needs a change the change goes under FOUND and D07a's tests must stay green.
- f64 stays: both models' antiderivative quotients need it (see D07's reasoning).

## Red tests first

**D08a**

1. BENCH before, at the base: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/fold'`; both numbers to BENCH.
2. `fold::tests::the_omega_seed_is_close`: 20,001 `u` even over `[-45, 60]`: the seed (table interpolation inside the range, the series above it, `e^u` below) is within `5e-5` relative inside the table and `4e-3` beyond it, against `lambert_w_exp_loop(u, 0.0)` run to `1e-14`.
3. `fold::tests::two_steps_reach_the_loop`: same grid, `|lambert_w_exp(u) - loop| <= 1e-12 * max(1, loop)`, with the three switch points (`-40`, `40`) and both neighbours of each included; `lambert_w_exp(NAN)` and `lambert_w_exp(INFINITY)` return `0.0` and `lambert_w_exp(-INFINITY)` returns `0.0`.
4. `fold::tests::the_null_against_the_old_model_holds`: `null_db(&mut Fold::reference(), &mut Fold::new(), &[]) < -40.0`; with `&[(MODEL, 1.0), (FOLDS, 100.0)]` `< -30.0`.
5. `fold::tests::flat_out_square_wave_stays_bounded_and_the_tail_dies`: `FOLDS 100`, `MODEL 1`, `LEVEL` at its max, one second of a 50 Hz square at `+-16.0`; every sample finite and `<= 2.0`; then one second of silence; last 100 ms RMS under `1e-7`. Then the same with `MODEL 0`.
6. Existing suite green (conformance, `oversampling_and_adaa_cut_the_aliasing`, `folds_keep_the_level`, `a_hard_pick_keeps_its_attack`, `symmetry_brings_even_harmonics`, `a_dry_mix_is_the_input`, `the_antiderivatives_are_right`, `a_lockhart_stage_folds_where_the_paper_says`), `tests/drive_no_alloc.rs` green.
7. BENCH after: `model/fold/buchla <= 0.5`, `model/fold/serge <= 0.35` of before.

**D08b**

1. BENCH before: `... -- 'model/klon'`.
2. `solve::tests::the_integral_from_one_exponential_matches`: for 10,001 volts even over `[-1.5, 1.5]`, `GERMANIUM.pair_from(rising)` equals `pair(v)` and `pair_integral_from(rising)` equals `pair_integral(v)` to the bit, `rising` being `exp(v / nVt)` computed once.
3. `klon::tests::three_steps_reach_the_old_tolerance`: with the table `Klon::prepare` builds at 48 kHz, for 9 gain fractions even over `[0, 1]` (each giving its `reach` through the scratch network) and 2001 `open` values even in `asinh` over `+-30 V`: `|clip_volts(table, reach, open, 3) - rising_root_within(..., 1e-13, ...)| <= 1e-12`.
4. `klon::tests::the_null_against_the_old_model_holds`: `null_db(&mut Klon::reference(), &mut Klon::new(), &[]) < -60.0`; with `&[(GAIN, 100.0), (TREBLE, 100.0)]` `< -60.0`.
5. `klon::tests::flat_out_square_wave_stays_bounded_and_the_tail_dies`: as D08a's with `GAIN 100`, `LEVEL` max.
6. Existing suite green (`oversampling_cuts_the_aliasing`, `the_small_signal_response_is_the_netlists`, `more_gain_is_louder`, `more_gain_is_more_distortion`, `treble_tilts_the_top`, `level_is_decibels`, conformance with its limits unchanged), `tests/drive_no_alloc.rs` green: the table is built in `prepare` only.
7. BENCH after: both klon ids `<= 0.6` of before.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-fx --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/fold'   (D08a, before and after)
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/klon'   (D08b, before and after)
```

## Definition of done

**D08a**
- [ ] The folder runs at `2.0 * TARGET_RATE`; `OMEGA` table, `lambert_w_exp` (table plus two steps), `lambert_w_exp_loop`, `Omega` and `Fold::reference()` exist; `Stage::last_w` is gone.
- [ ] Tests 2-5 red then green with output pasted; test 6 green.
- [ ] BENCH: buchla `<= 0.5`, serge `<= 0.35`.
- [ ] Pushed to `origin/lane/kz-d08a`; QUEUE line.

**D08b**
- [ ] `clip` solves through `clip_volts` with three steps from a table built in `prepare` over the gain pot; one `exp` feeds the antiderivative; `before` is gone; `Solve` lives in `drive/solve.rs`; `Klon::reference()` exists.
- [ ] Tests 2-5 red then green; test 6 green.
- [ ] BENCH: both `<= 0.6`.
- [ ] Pushed to `origin/lane/kz-d08b`; QUEUE line.

## Code quality

Rules of the tree as in PLAN.md's STACK RULES: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines (`fold.rs` is 901 today and gains a table builder: if it passes 1000, the Lockhart stage, `lambert_w_exp`, its loop and the omega table move to `drive/lockhart.rs` in their own commit first); no TODO, no stubs. Nothing allocates in `process`, `set_param` or `reset`: the omega table is a `OnceLock` filled on the first build, the Klon's `SinhTable` is built in `prepare`. Every solve returns finite for any input. Reference paths are production code used only by tests, kept small, never the default. Comments say why; no emoji; no dashes as punctuation in the tree.

## END

```
START KZ-D08a base=<sha> branch=lane/kz-d08a df=<GB> at <time>
END KZ-D08a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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

The same block, with `KZ-D08b`, ends the second lane.
