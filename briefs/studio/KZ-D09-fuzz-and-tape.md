# KZ-D09: the fuzz spends one exponential per stage instead of two and skips its idle octave path, and the tape models read the Langevin curve from a table, so a fuzz costs under 0.6 of today and a tape stage under half, bit-for-bit or under -80 dB against the old code

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Bench names: D01's brief builds `kazoo-fx/benches/circuit_models.rs` with ids `fx/<id>` at defaults. Read what D01 actually landed. If the `models` bench ids this brief names (`model/<id>/default|hot` and the named variants) or the fixture `kazoo_fx::bench::fixture(rate)` do not exist, this lane's first commit adds the ids and the fixture it needs beside D01's bench, without renaming D01's rows, and says so under FOUND.

Split: two lanes in this file. **KZ-D09a** is the fuzz (`lane/kz-d09a`); **KZ-D09b** is the tape magnetics (`lane/kz-d09b`). They touch different files (`drive/fuzz.rs`; `lofi/parts.rs` and the bench) and may run beside each other. Each is one mechanism, about 30 minutes of a builder's work. Crates: kazoo-fx only. Depends: KZ-D01 (benches and fixture) landed on main; D09a also uses `null_db` from KZ-D07a's `drive/testkit.rs`.

## Outcome

Today the fuzz (`kazoo-fx/src/drive/fuzz.rs`) runs at 352.8 kHz, eight times the host (`fuzz.rs:70`), and each of its two transistor stages computes the antiderivative at the new voltage and again at the last one on every sample (`Transistor::smoothed`, `fuzz.rs:219-226`: `integral(volts) - integral(last)`, one `safe_exp` each), four exponentials per sample where two would do, and runs the octave doubler's filter (`fuzz.rs:281`) whether or not the octave is on. The tape models (cassette `lofi/cassette.rs:415`, reel `lofi/reel.rs:300`, VHS `lofi/vhs.rs:241`) record through `Magnetic::process` (`lofi/parts.rs:938`), a Jiles-Atherton hysteresis stepped by the explicit midpoint rule in one to eight pieces per sample depending on how far the field jumped (`parts.rs:985`), each piece evaluating the Langevin function twice (`slope`, `parts.rs:959`, through `langevin`, `parts.rs:882`: a `tanh`, three divides) in f64, inside a 2x or 4x oversampler.

After this lane: the fuzz caches each stage's last antiderivative (one exponential per stage per sample) and runs the doubler only while the octave weight is above zero; the Langevin function and its slope come from a Hermite table. Measured by D01's benches: `model/fuzz/default` and `model/fuzz/hot` at or under 0.6 of the lane's base; `model/cassette/default`, `model/reel/default` and `model/vhs/default` at or under 0.7, and a new `model/magnetic/step` micro-bench (added by this lane if D01 did not) at or under 0.5. The fuzz's change is bit-for-bit (a cached number is the same number); the magnetics hold under -80 dB against the exact Langevin. No clicks, nothing non-finite, nothing unstable.

## The design (decided; build exactly this)

### KZ-D09a: the fuzz

**One exponential per stage.** `Circuit::last: [f64; 2]` (`fuzz.rs:246`) becomes `last: [Memory; 2]` with `#[derive(Clone, Copy, Default)] struct Memory { volts: f64, integral: f64 }`. `Transistor::smoothed(&self, volts, last: Memory) -> (f64, Memory)` returns the stage's output and the new memory: in the normal branch (`step.abs() > TINY_STEP`) it computes `integral(volts)` once and divides the difference from `last.integral`; in the tiny-step branch it returns `swing(0.5 * (volts + last.volts))` and still computes `integral(volts)` for the memory (two exponentials there, as today, in the rare near-static case). `Circuit::stage` (`fuzz.rs:261-275`) stores the memory. On `reset` (`fuzz.rs:250`) the memory is `Memory { volts: 0.0, integral: transistor.integral(0.0) }`, which needs the transistors: `reset` is `pair::Circuit::reset(&mut self)` with no arguments, so instead the memory's integral is marked stale by a `fresh: bool` on `Circuit` set in `reset` and cleared on the first `tick`, which computes both integrals that one time. The naive path (`anti_alias` false) is unchanged.

**The doubler only when heard.** `Circuit::tick` (`fuzz.rs:277-287`) runs `octave_block` and the `abs` only when `settings.octave > 0.0`; otherwise `base = volts` and `octave_stale = true` on `Circuit`. When the octave comes back, `octave_block.reset()` first (the weight is gliding up from zero over 20 ms, `kit.rs:9`, so the filter's restart is under a weight starting at zero: no click). Pattern: the fold's `serge_stale`, `fold.rs:570-581`.

**f64 stays, and why.** The lane does not convert the fuzz to f32. The antiderivative quotient `(integral(v) - integral(last)) / step` with `step` down to `TINY_STEP = 1e-7` (`fuzz.rs:145`) and integrals of order ten needs sixteen digits; f32 has seven, and the quotient would be noise at full scale. That is the answer to "f64 to f32 where the null test allows": the arithmetic settles it without a test, and the brief says so. The `Iir3` filters (`drive/filter.rs`) are shared with every drive model and stay f64.

**Rate stays 8x.** `fuzz.rs:62-70` gives the measured reason (4x folds a -6 dBFS 5 kHz note at -62 dBc, -52 pushed); the lane does not lower it.

**Reference and null.** The caching is algebraically identical, so the proof is at the unit level, not a frozen effect: `Transistor::smoothed_recomputed(volts, last_volts)` is today's body kept under `#[cfg(test)]` and compared bit for bit. The doubler skip is proven by driving `Circuit::tick` with `octave = 0.0` through both paths (a `run_octave: bool` argument that `frame` passes as `settings.octave > 0.0`, and the test passes as `true` and `false` on two copies of the same `Circuit`). `null_db` against `Fuzz::new()` at the base is therefore expected at `-inf`; the test asserts `< -120.0` to catch a slip, using a `Fuzz::reference()` that is `build(true)` with `run_octave` forced true and `smoothed_recomputed` selected through a `Cache { On, Off }` on `Settings` (`Off` only from `reference()`).

### KZ-D09b: the tape magnetics

**What is already adaptive.** The plan's "tape substeps adaptive" is the case today: `Magnetic::step` (`parts.rs:978-997`) takes `ceil(|dh| / (JA_K / 2))` pieces, one to eight, so a quiet signal costs one piece and only a hard-driven one costs eight. The lane leaves that rule alone (its stability margin is the explicit midpoint's: `dh * d(slope)/dm` stays under one at `JA_K / 2`) and makes each piece cheap instead. Say this in the module doc.

**The Langevin table.** `static LANGEVIN: OnceLock<Box<[(f64, f64)]>>` in `lofi/parts.rs`: `LANGEVIN_POINTS = 8193` pairs `(L(q), L'(q))` even over `[-Q_MAX, Q_MAX]`, `Q_MAX = 24.0` (beyond it `coth(q) - 1` is under `3e-21`, so `L = signum(q) - 1 / q` and `L' = 1 / q^2` are exact there), built from today's `langevin` (`parts.rs:882-892`, kept, renamed `langevin_exact`) on the first `Magnetic` built, which is off the audio thread. 128 KB. `fn langevin(q: f64) -> (f64, f64)`: outside `Q_MAX` the closed forms; inside, cubic Hermite for `L` from the bracketing pair's values and slopes (step `h = 48 / 8192`; the Hermite error `h^4 / 384 * max|L''''|` is under `1e-12`), and linear interpolation of the two tabulated slopes for `L'` (error `h^2 / 8 * max|L'''|`, under `1e-6`, which `slope` only uses through the reversible term scaled by `JA_C / JA_A`, so under `1e-6` in `dm/dh` against a slope of order ten). A non-finite `q` returns `(0.0, 1.0 / 3.0)`. `slope` (`parts.rs:959`) calls it unchanged. `langevin_single` (`parts.rs:901`, f32, the anhysteretic curve) is cheap already and stays.

**Bench.** If D01's `models` bench has no `model/magnetic/step`, add it: one `Magnetic` driven by D01's fixture at `drive = 1.5`, `loop_share = 0.15`, a 256-sample block per iteration.

## Not this lane

- The oversamplers (`Oversampler2`, `Oversampler4`, `parts.rs:469-575`), the head, wobble, hiss, dropouts and noise reduction of each tape model: untouched.
- `genloss` records through `Magnetic::curve` only (`lofi/genloss.rs:310`), not the hysteresis; it gains nothing here and is not touched.
- The piece rule and cap in `Magnetic::step`: untouched, as above.
- The remanence `Magnetic` leaves after a loud passage (`m` stays off zero at `h = 0`, a DC offset of `3 m / drive` scaled by `loop_share`) is physical and today's behaviour; note under FOUND whether each tape model blocks that DC downstream (grep its chain for a high-pass after the tape) and move on.
- D07, D08, D10, D12: untouched.

## Red tests first

**D09a**

1. BENCH before, at the base: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/fuzz'`.
2. `fuzz::tests::caching_the_integral_changes_nothing`: a random walk of 200,000 base voltages (LCG, fixed seed, steps up to `+-0.5 V` with one in ten under `TINY_STEP`) through `Transistor::new(A1, VE1)` and through the hot second transistor (`A2_HOT`, `THERMAL_VOLTAGE`): `smoothed` with the carried memory equals `smoothed_recomputed` to the bit (`to_bits()`) at every step.
3. `fuzz::tests::the_doubler_skipped_is_the_doubler_at_zero`: two copies of one `Circuit` fed the fixture at 352.8 kHz samples (just feed the 2 s fixture as if oversampled; the rate only sets the filters) with `octave = 0.0`, one with `run_octave` true and one false: outputs equal to the bit; then with `octave = 1.0` both paths run and are equal.
4. `fuzz::tests::the_octave_coming_back_does_not_click`: `OCTAVE` from 0 to 1 during a 110 Hz tone at -12 dBFS after two seconds with the octave off: the largest step within 30 ms of the switch is at most three times the largest step in the 100 ms before it.
5. `fuzz::tests::the_null_against_the_old_model_holds`: `null_db(&mut Fuzz::reference(), &mut Fuzz::new(), &[]) < -120.0`; with `&[(FUZZ, 100.0), (BIAS, 100.0), (OCTAVE, 1.0)]` `< -120.0`.
6. `fuzz::tests::flat_out_square_wave_stays_bounded_and_the_tail_dies`: `FUZZ 100`, `BIAS 100`, `LEVEL` max, one second of a 50 Hz square at `+-16.0`: finite, `<= 2.0`; one second of silence: last 100 ms RMS under `1e-7`.
7. Existing suite and `tests/drive_no_alloc.rs` green, limits unchanged.
8. BENCH after: both fuzz ids `<= 0.6` of before.

**D09b**

1. BENCH before: `... -- 'model/cassette' 'model/reel' 'model/vhs'` and, once added, `'model/magnetic'` (if the micro-bench is new, its before number is taken at the base with the bench file applied as the lane's first commit, before the table).
2. `lofi::parts::tests::the_langevin_table_matches_the_function`: 200,001 `q` even over `[-30, 30]` plus `+-Q_MAX` and their neighbours: `|L_table - L_exact| <= 1e-9` and `|L'_table - L'_exact| <= 1e-6`; `langevin(NAN)` is `(0.0, 1/3)`.
3. `lofi::parts::tests::the_magnetics_null_against_the_exact_curve`: two `Magnetic`s driven by the fixture at `drive 1.5` and again at `drive 16.0`, `loop_share 0.15`, one through the table and one through `langevin_exact` (a `Langevin { Table, Exact }` argument on `step` and `slope`, `Exact` only from the test): `20 log10(rms(diff) / rms(exact)) < -80.0` for both drives.
4. `lofi::parts::tests::hard_drive_stays_bounded_and_finite`: `drive 64`, one second of a 1 kHz square at `+-64.0` (the input clamp), `loop_share 1.0`: every output finite, `|m| <= 1` throughout (expose `m` to the test as a `#[cfg(test)] fn magnetisation`).
5. Existing lofi suite green (the cassette, reel and VHS tests in their files, the parts tests at `parts.rs:1536-1590`), `tests/lofi_no_alloc.rs` green: the table is filled on the first build, never in `process`.
6. BENCH after: cassette, reel, vhs `<= 0.7`; `model/magnetic/step` `<= 0.5`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-fx --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-fx/benches
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/fuzz'                                     (D09a)
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/cassette' 'model/reel' 'model/vhs' 'model/magnetic'   (D09b)
```

## Definition of done

**D09a**
- [ ] One exponential per stage per sample in the normal branch; the doubler runs only while `octave > 0`; `Fuzz::reference()` exists; f64 kept with the reason in the module doc.
- [ ] Tests 2-6 red then green, output pasted; test 7 green.
- [ ] BENCH: both `<= 0.6`.
- [ ] Pushed to `origin/lane/kz-d09a`; QUEUE line.

**D09b**
- [ ] `LANGEVIN` table and Hermite `langevin` exist; `langevin_exact` kept as builder and reference; the module doc says the substeps are already adaptive and why the rule stays.
- [ ] Tests 2-4 red then green; test 5 green.
- [ ] BENCH: tape models `<= 0.7`, magnetics `<= 0.5`.
- [ ] Pushed to `origin/lane/kz-d09b`; QUEUE line.

## Code quality

Rules of the tree as in PLAN.md's STACK RULES: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines (`parts.rs` is 1597 today, already over: the magnetics, `langevin`, its table and their tests move to `lofi/magnetic.rs` in the lane's first commit, re-exported from `parts` so the three tape models' imports stay; the fuzz is 462 and stays in place); no TODO, no stubs. Nothing allocates in `process`, `set_param` or `reset`. Every path survives NaN and Inf in (the conformance suites prove it). Reference paths are production code used only by tests, kept small, never the default. Comments say why; no emoji; no dashes as punctuation in the tree.

## END

```
START KZ-D09a base=<sha> branch=lane/kz-d09a df=<GB> at <time>
END KZ-D09a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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

The same block, with `KZ-D09b`, ends the second lane.

## Open from the D09a review (Cassio, 13 Oct 05:30)

`the_octave_coming_back_does_not_click` (`kazoo-fx/src/drive/fuzz.rs`) starts with the octave off, so its filter is already clean when it comes back; the reset of a stale octave filter (octave on, then off, then on again) is never exercised. Next fuzz work adds `the_octave_on_off_on_does_not_click`: octave on at full weight through a loud passage, off for one block, on again; the switch-on step under the same bound as the existing test. Red first if the stale filter clicks.

## Open from the D09b review (Cassio, 13 Oct 05:40)

- The VHS cost target (0.7) is withdrawn for this design: VHS runs one `Magnetic` at 2x (two steps per frame), so the magnetics are about 20% of its cost and even free magnetics leave it near 0.8. The magnetic step's 0.51 against 0.5 was measured at varying load; judge it on a quiet machine.
- VHS always runs both stereo hi-fi channels and four fractional delay reads per frame (`lofi/vhs.rs` near 462 to 474), even in linear mode where the hi-fi output is not heard. Next tape work skips the hi-fi path while linear mode is selected (with a handover test that switching modes does not click), which is where VHS's cost actually is.
- `lofi/parts.rs` is 1409 lines, over the 1000-line rule, from before this run; the next lane that touches it moves one named mechanism out and names it in its brief.
