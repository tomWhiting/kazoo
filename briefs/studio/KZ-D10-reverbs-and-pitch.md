# KZ-D10: the reverbs read their lines at whole samples where nothing moves, through a nearest-phase sinc a third the cost where something does, and Hermite only while a knob glides; the spring runs only the springs that are on; the shifters' alignment searches are bounded to a fixed budget per splice at every rate

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Bench names: D01's brief builds `kazoo-fx/benches/circuit_models.rs` with ids `fx/<id>` at defaults. Read what D01 actually landed. If the `models` bench ids this brief names (`model/<id>/default|hot` and the named variants) or the fixture `kazoo_fx::bench::fixture(rate)` do not exist, this lane's first commit adds the ids and the fixture it needs beside D01's bench, without renaming D01's rows, and says so under FOUND.

Split: two lanes in this file. **KZ-D10a** is the reverb lines (`lane/kz-d10a`: `space/line.rs`, `space/hall.rs`, `space/plate.rs`, `space/spring.rs`, `space/kit.rs`); **KZ-D10b** is the alignment searches (`lane/kz-d10b`: `space/shimmer.rs`, `time/pitch/taps.rs`). They touch different files and may run beside each other. Each is one mechanism, about 30 minutes of a builder's work. Crates: kazoo-fx only. Depends: KZ-D01 (benches and fixture) landed on main.

## Outcome

Today every read a reverb tank makes between samples goes through a 32-point Kaiser-windowed sinc tabulated at 256 phases and blended between two rows (`space/line.rs:153-180`): three flops per tap, about a hundred per read. The hall (`space/hall.rs:337-440`) makes about 34 such reads per sample (8 tank lines, 16 early taps, 2 predelay, 8 diffuser reads through `TapAllpass`, `space/kit.rs:411-423`), the plate (`space/plate.rs:251-265`, `:353-363`, `:366-416`) about 24, the spring (`space/spring.rs:539-568`, `:896`) 5, and the shimmer 8 plus its shifters. Most of those positions never move: early tap offsets, diffuser lengths, output taps and unmodulated line lengths are fixed by the size and the rate, and change only while a knob glides. The spring runs all three of its allpass cascades (`LOW_STAGES = 80` plus `HIGH_STAGES = 24` per spring, `spring.rs:70-73`, interleaved in `Chains::process`, `spring.rs:240-273`) even with the `springs` knob on 2. The shimmer's grain alignment (`Shifter::align`, `shimmer.rs:314-359`) and the pitch shifter's (`Taps::align`, `time/pitch/taps.rs:347-408`) score a cross-correlation over a +-12 ms or +-8 ms window at a 12 kHz coarse stride then every sample around the best: about 40,000 line reads per splice at 48 kHz, four times that at 192 kHz, all inside one frame.

After this lane: a read at a whole-sample position costs one load; a read at a static fractional position is rounded to a whole sample when it is designed; a read whose position is gliding (size or predelay moving) is four-point Hermite for the length of the glide; a modulated read (the hall's swept tank lines, the plate's two swept allpasses, the spring's wandering coils, the shimmer's grains) uses a 1024-phase nearest-row sinc at one fma per tap. The spring runs two cascades when two springs are on. Each alignment search is at most `COARSE_LAGS` coarse positions over `SPAN_POINTS` samples, then two narrowing passes, under 11,000 reads per splice at any rate, and at most one splice lands in a block. Measured by D01's benches: `model/hall/default` and `model/plate/default` at or under 0.45 of the lane's base, `model/spring/default` at or under 0.8 and `model/spring/two` at or under 0.6, `model/shimmer/default` and `model/pitch/up7` at or under 0.8. The reverbs' same-at-every-rate tests stay green (they are why the sinc stays in the loops); nothing clicks at a handover; nothing goes non-finite.

## The design (decided; build exactly this)

##RETRY NOTE for KZ-D10a (Cassio, 13 Oct 03:40). The first run (branch lane/kz-D10a, head kept on origin) built read_at / Hermite, the handovers, the spring cascades, the `models` bench and `src/bench.rs`, and measured at a machine load near 240: hall 0.26-0.29, plate 0.12-0.24, spring default 0.68, hot 0.54, two 0.82 of before. Decisions for the retry:
- The sinc table is 2048 phases, not 1024. A nearest-row table at 1024 phases measures -66.2 dB against the exact sinc; the phase error halves at 2048 (about -72 dB), still one fma per tap, the table twice the memory. The -70 dB limit in `the_nearest_row_matches_the_exact_sinc` stays. The brief's 1024 was my error.
- Cost ratios: a bench at load 200+ is noise of about 2x (see D02). Record before and after interleaved (before, after, before, after) from the same build with the load average next to each; the reviewer judges the ratios on a quiet machine. Do not spend the time box chasing `spring/two`.
- Finish what was interrupted: `no_steady_tone_rings_more_than_six_decibels_over_the_dry` and `the_spring_keeps_the_contract` must run to completion and pass; run them alone first.
- The amp leak report (`drive::amp::tests::conformance::odd_blocks_are_handled`) is not this lane's; leave it.

# KZ-D10a: the lines

**Why not Hermite in the loops.** A four-point Hermite read loses about a decibel at half Nyquist on every pass (`line.rs:6-10`), and a tank passes its reads dozens of times a second, so the tail would darken dozens of decibels faster at 44.1 kHz than at 192 kHz; `the_hall_is_the_same_at_every_rate_with_long_even_decays` (`hall.rs:561`) and `the_plate_is_the_same_at_every_rate_with_the_treble_open` (`plate.rs:502`) exist to forbid exactly that. So Hermite is used only where a read is already changing every sample for a reason the ear accepts: a size or predelay glide, which bends pitch by up to a third of a semitone (`hall.rs:52-55`) and lasts as long as the knob moves. The modulated reads keep the sinc, made three times cheaper.

**Nearest-phase kernel.** `space/line.rs`: `PHASES` (`line.rs:28`) becomes `1024`; `Kernel::new` builds `PHASES + 1` rows as today; `Line::read` picks the nearest row (`phase = (position + 0.5) as usize`, held to `PHASES`) and sums one `mul_add` per tap with no blend. Position error is at most half a phase, `1 / 2048` of a sample: at 20 kHz and 48 kHz that is a phase error of `1.3e-3` radians, an amplitude error under `-70 dB`, far below the `0.1 dB` the flatness tests ask. Table: `1025 x 32 x 4 B = 131 KB` at 48 kHz, built once per `prepare` and shared by every line of an effect through the existing `Arc<Kernel>`. The module doc (`line.rs:17-21`) is rewritten.

**Whole-sample reads.** `Line` gains `pub fn read_at(&self, delay: f32, gliding: bool) -> f32`: if `(delay - delay.round()).abs() < WHOLE_EPSILON` (`1.0 / 4096.0`) it returns `self.tap(delay.round() as usize)`; else if `gliding` it returns `self.read_hermite(delay)`; else `self.read(delay)`. `read_hermite` is the four-point read through `crate::dsp::hermite` (`dsp.rs:174`) on the four samples around `delay`, with the same clamping as `read` (held to `[MIN_SINC_READ, max_delay]`, non-finite to `MIN_SINC_READ`). `TapAllpass` (`kit.rs:396-423`) gains the same `gliding` argument on `process` and `tap`, passed through to `read_at`. `read` itself stays for the modulated reads.

**Static positions rounded where they are designed.** Each effect computes its static read positions once per control block (`CONTROL = 32` samples, the `countdown` pattern at `hall.rs:344-353`, `plate.rs:368-372`, `spring.rs:849-853`) into fields, rounded to whole samples when nothing is gliding and exact when something is, and passes `gliding` to `read_at`:

- Hall: `early_at: [[f32; 8]; 2]` from `EARLY` (`hall.rs:76`), `at * ms + predelay`; `diffuser_at: [[f32; 4]; 2]` from `DIFFUSERS_MS` and the rate; the predelay read's position. `gliding = !(self.size.settled() && self.predelay_time.settled())` (`Slew::settled`, `kit.rs:319`). The predelay slew's target is rounded to a whole sample when set (`hall.rs:360-361`) so it settles on one. The tank reads (`hall.rs:395-401`) are modulated when `depth > 0` and use `read` as today; when `depth == 0.0` (the `mod` knob at zero, or frozen) they use `read_at` with the rounded length, which makes the freeze's exact-sample read (`hall.rs:399`) the same path.
- Plate: the 14 output taps (`taps`, `plate.rs:353-363`): `at * scale` rounded per block when `size.settled()`; the four input diffusers (`plate.rs:385-391`); `delay1` and `delay2` reads in `Half::process` (`plate.rs:256`, `:261`) and `allpass2` (`plate.rs:259`); the predelay as the hall's. The two modulated `allpass1` reads (`plate.rs:252-254`) keep `read`; when `depth == 0.0` they go through `read_at`.
- Spring: `fed.read(self.arrival)` (`spring.rs:896`) becomes `read_at` with `arrival` rounded (`spring.rs:780`); the coil reads (`spring.rs:540`, `:547`, `:563`, `:568`) are wandered every sample and keep `read`.
- Shimmer's plain reads (`shimmer.rs:533`, `line.delay.read(reading.base)`): `read_at` with `base` rounded when `size.settled()`; its shifter reads keep `read`. This is the one shimmer touch in D10a and it is three lines; D10b does not touch `frame`.

**Spring cascades.** `Chains` (`spring.rs:207`) gains `live: usize` (2 or 3). `Chains::process` runs only the first `live` cascades (the interleaved loop iterates `cells[..live]`; keep the three-wide interleave for the `live == 3` case by writing the inner loop over a slice, not a fixed array). `Spring::update` (`spring.rs:756`) sets `live = 3` while `self.third.value() > 0.0 || self.third.target() > 0.0` and, on the block where the third ramp settles at zero, clears the third cascade's history and Thiran state in both `lows` and `highs` and sets `live = 2`; coming back, the ramp fades the third coil in from exact silence (`spring.rs:784-788`, 50 ms), so nothing clicks. `LOW_STAGES` and `HIGH_STAGES` stay 80 and 24; their doc comments gain the reason the count is what it is and that it is fixed at every rate: Parker's interpolated stretched allpass spring (2011) needs about a hundred stages for a real spring's dispersion, 80 keeps the drip's chirp at this stretch, 24 lightly stretched stages give the splash; the stretch `K` carries the rate, so the cost is `(80 + 24) * live` stages per sample at any rate, 312 with three springs, 208 with two. That is the bound.

### KZ-D10b: the searches

**One budget, two places.** Both `align`s get the same three-pass search with the same named constants (declared in each file with the same values and the same doc; the two modules share no code today and the lane does not create a shared module for four constants):

```
COARSE_LAGS: usize = 64      // coarse positions across the whole search window, whatever the rate
SPAN_POINTS: usize = 64      // samples scored per coarse lag
FINE_SPAN_POINTS: usize = 128 // samples scored per lag in the two narrowing passes
```

Pass 1: lags from `-search` to `+search` (shimmer) or `low` to `high` (pitch, `taps.rs:359`) at stride `s1 = ceil(window / COARSE_LAGS)`, each scored over the span at step `ceil(span / SPAN_POINTS)`. Pass 2: lags within `+-s1` of the best at stride `s2 = max(1, s1 / 8)`, span step `ceil(span / FINE_SPAN_POINTS)`. Pass 3: lags within `+-s2` of the best at stride 1, the same span step. Then one full-span score (step 1) at the winner for `alike`, as today (`shimmer.rs:351-357`, `taps.rs:400-406`). Worst case per splice: `64 * 64 + 17 * 128 + (2 * s2 + 1) * 128 + span`; at 48 kHz (`s1 = 18`, `s2 = 2`) that is 7,400 reads for the shimmer against 40,000 today; at 192 kHz (`s1 = 72`, `s2 = 9`) 10,700 against about 160,000. `COARSE_RATE` (`shimmer.rs:102`, `taps.rs:22`) and the `stride` fields derived from it (`shimmer.rs:517`, `taps.rs:76`) go; the strides are computed from the window and the constants in `Reading` (`shimmer.rs:242`) and `Geometry` (`taps.rs:49`) at `prepare`. The pitch shifter's "the planned jump stands where nothing matches better" rule (`taps.rs:379-382`) stays.

**At most one splice per block.** The shimmer's eight shifters start at phases `index / 8` (`shimmer.rs:643`) with the same grain, so their taps wrap at distinct times at least a sixteenth of a grain apart; the shortest grain (`grain_of`, `shimmer.rs:200`, at the largest shift) is over a millisecond, so at 48 kHz splices are over 60 samples apart and at most one lands in a 32-frame block; the pitch shifter has one `Taps` shared by both channels with splices at least `OVERLAP = 12 ms` apart. Write both facts as a doc comment on the constants; the per-block bound is therefore the per-splice bound.

**Reference.** Today's `align` body is kept as `align_exhaustive` under `#[cfg(test)]` in each file, the regression's reference.

## Not this lane

- Hermite anywhere inside a feedback loop at rest: forbidden by the rate tests above.
- The sinc kernel's length (32 at 48 kHz, `line.rs:67-73`) and window: unchanged.
- The pitch shifter's own sinc (`time/parts.rs:488-560`, 16 taps, 512 phases, already nearest-phase): untouched.
- The hall's and plate's decay filters, Householder mix, freeze; the spring's driver, governor, boing: untouched.
- Shimmer's tank and filters, `follow_shift`, the gate: untouched beyond the three-line `read_at` in D10a.
- D07, D08, D09, D12: untouched.

## Red tests first

**D10a**

1. BENCH before, at the base: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/hall' 'model/plate' 'model/spring'`.
2. `line::tests::the_nearest_row_matches_the_exact_sinc`: a `Line` of 4096 filled with LCG noise; 2000 random delays in `[MIN_SINC_READ + 1, 2000)`; against a reference that evaluates the Kaiser sinc (the formula in `Kernel::new`, `line.rs:79-94`) at the exact fraction for each read: `20 log10(rms(read - exact) / rms(exact)) < -70.0`. Written against today's blended 256-phase read first to record its figure in the commit message, then the nearest-row read must be no worse.
3. `line::tests::read_at_taps_whole_samples_and_hermite_only_while_gliding`: a counting line (`push(n)`): `read_at(20.0, false) == tap(20)` exactly; `read_at(20.0001, false) == tap(20)`; `read_at(20.5, false) == read(20.5)`; `read_at(20.5, true) == read_hermite(20.5)`, and `read_hermite(20.5)` is `33.5` on the counting line; `read_at(NAN, true)` and `read_hermite(1e9)` finite.
4. The existing `line` tests (`line.rs:192-258`) green with 1024 phases.
5. `hall::tests::settling_the_size_does_not_click` and `plate::tests::settling_the_size_does_not_click`: size swept from 1.0 to 1.4 over 0.5 s on a -12 dBFS 440 Hz tone with the tail running, then held; the largest sample-to-sample step within 10 ms after the slew settles (the Hermite-to-tap handover) is at most three times the largest step in the 100 ms before it. Red before the lane because the handover does not exist; written first.
6. `spring::tests::two_springs_run_two_cascades_and_the_third_comes_back_clean`: `springs` 2: after the third ramp settles, `lows.live == 2` and the third cascade's state is all zero; then `springs` 3 during a -12 dBFS tone: the largest step within 60 ms of the switch is at most three times the largest step in the 100 ms before; and the two-spring output equals, to within `-80 dB`, a three-spring `Spring` whose third coil is fed silence (`third` forced to 0 by the knob) at the base: prove by rendering both through the same fixture.
7. Existing space suite green: hall (`hall.rs:556-630`), plate (`plate.rs:497-655`), spring, shimmer `the_shimmer_keeps_the_contract`, and the `rates_agree_with` checks (`testing.rs:430`); `tests/space_no_alloc.rs` green.
8. BENCH after: hall and plate `<= 0.45`, `model/spring/default <= 0.8`, `model/spring/two <= 0.6`.

**D10b**

1. BENCH before: `... -- 'model/shimmer' 'model/pitch'`.
2. `shimmer::tests::the_search_budget_holds_at_every_rate` and `time::pitch::tests::the_search_budget_holds_at_every_rate`: for 44.1, 48, 96 and 192 kHz, the strides and spans the `Reading`/`Geometry` computes give pass sizes whose sum (the formula above, computed in the test from the fields) is under `11_000`, and pass 1 visits at most `COARSE_LAGS + 1` lags.
3. `shimmer::tests::the_bounded_search_finds_the_period` and `time::pitch::tests::the_bounded_search_finds_the_period`: a 220 Hz sawtooth at -12 dBFS (`saw`, `pitch/tests.rs:393`) plus a 1 kHz sine at -24 dBFS, shift 7 semitones; over the first 200 splices the bounded `align` lands within `+-1` sample of `align_exhaustive` on at least 95 percent, and never more than one period (218 samples at 48 kHz) away.
4. `shimmer::tests::one_splice_per_block`: at the largest shift step, over 10 s, count the frames in which any of the eight shifters aligned (a `#[cfg(test)]` counter on `Shifter`); no 32-frame window holds two.
5. Existing suites green: shimmer (`shimmer.rs:686-700`), the pitch tests (`pitch/tests.rs`: `splices_do_not_chop_a_steady_note`, `knob_moves_and_splices_never_click`, `a_sweeping_shift_stays_bounded_and_settles_on_the_interval`, `nonsense_in_is_silence_out_and_leaves_no_trace`, `every_attack_comes_out_once`, `a_chord_keeps_its_splice_sidebands_down`, and the rest); `tests/space_no_alloc.rs` and `tests/time_no_alloc.rs` green. If `a_chord_keeps_its_splice_sidebands_down` fails under the budget, raise `FINE_SPAN_POINTS` to 256 first, then `COARSE_LAGS` to 128, re-running the budget test each time, and put the measured numbers under FOUND; never return to the unbounded search.
6. BENCH after: shimmer and pitch `<= 0.8`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-fx --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/hall' 'model/plate' 'model/spring'   (D10a)
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-fx --bench models -- 'model/shimmer' 'model/pitch'              (D10b)
```

## Definition of done

**D10a**
- [ ] `PHASES = 1024`, nearest row, one fma per tap; `read_at`, `read_hermite`, `TapAllpass`'s `gliding`; static positions rounded per block in hall, plate, spring and the shimmer's plain read; `Chains::live` with the third cleared on the way out.
- [ ] Tests 2, 3, 5, 6 red then green with output pasted; tests 4 and 7 green.
- [ ] BENCH as above.
- [ ] Pushed to `origin/lane/kz-d10a`; QUEUE line.

**D10b**
- [ ] Three-pass bounded search with the three constants in both files; `COARSE_RATE` gone; `align_exhaustive` kept under `cfg(test)`.
- [ ] Tests 2-4 red then green; test 5 green (with any constant raise named in FOUND).
- [ ] BENCH as above.
- [ ] Pushed to `origin/lane/kz-d10b`; QUEUE line.

## Code quality

Rules of the tree as in PLAN.md's STACK RULES: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines (`spring.rs` is 1379 today, already over: `Chains` and `Coil` move to `space/spring/chains.rs` and `space/spring/coil.rs` in the lane's first commit, before any behaviour change; `hall.rs` 631 and `plate.rs` 655 stay); no TODO, no stubs. Nothing allocates in `process`, `set_param` or `reset`: the kernel is built in `prepare`. Every read returns finite for any delay. The reference searches are `cfg(test)`. Comments say why; no emoji; no dashes as punctuation in the tree.

## END

```
START KZ-D10a base=<sha> branch=lane/kz-d10a df=<GB> at <time>
END KZ-D10a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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

The same block, with `KZ-D10b`, ends the second lane.
