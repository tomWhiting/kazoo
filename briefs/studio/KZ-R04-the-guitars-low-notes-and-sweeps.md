# KZ-R04: the guitar's low notes speak in time, and a sweep is one onset per note

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-core`.

Place: Track R, after KZ-R02 (seven runs, last report `KZ-R02-run7-measurements.md`). Written by Cassio, 14 Oct 2026, 20:5x. Decided by Cassio under Tom's rule that internal calls are not parked on him.

Time: 45 minutes. Crates: kazoo-core (`analysis/cycle.rs`, `analysis/cycle/early.rs`, `voice/gate.rs`, their tests). Read `KZ-R02-run7-measurements.md` first. It is the evidence this brief rests on.

## Why

Two acceptance tests are still red on main:

- `early_meets_its_targets` (`analysis/score/early_tests.rs`): guitar E2 must speak within `1500 / hz + 2` ms (20.2 ms). Run 7 measured that P07's first-lobe candidate cannot bracket the period on P06's seeded-noise pluck (largest armed lobe 95.8 samples against a true period of 582.5), and that the best NSDF any window reaches on the true lag before the deadline is 0.688. That is below the 0.90 early floor and the 0.80 acquisition floor. No floor can be met in time, so the early path for long periods needs a different test, not a lower bar on the same one.
- `a_sweep_is_one_onset_per_note` (`voice/profile/tests.rs`): twelve onsets against six. While the old string still rings, short windows admit unrelated periods at the 0.80 floor (`cycle.rs`, `consider`), and the shorter period wins ties. Each wrong pitch then passes `voice/gate.rs`'s 80-cent jump rule and becomes an onset.

## The design (decided; build exactly this)

### A. A provisional reading for long periods

1. When the early path is on and no estimate is held, for candidate periods above 400 samples at 48 kHz (below 120 Hz; scale by rate), compute the NSDF over the largest window the history allows (`available - lag`, at least a quarter period), across the profile's whole lag range, every judge step after the onset.
2. Choose by McLeod's key-maximum rule: the first local NSDF maximum whose value is at least 0.9 × the highest maximum in the range. Accept it as **provisional** when that value is at least 0.60 and the next highest key maximum is not within 5 cents of half or double its period with a higher value.
3. A provisional reading is published as a voiced frame with a `provisional` flag (add it to the frame type, false everywhere else). The onset fires on it. The normal acquisition path confirms it: the first normal estimate within 50 cents clears the flag. One that differs by more than 50 cents replaces the pitch with **no** second onset, and is counted in the score's octave-frame figure. Nothing in A runs on the output callback (the judge thread allocates freely; preallocate anyway where it is a per-step buffer).

### B. A jump is an onset only when it holds

4. In `voice/gate.rs`, a pitch jump of more than 80 cents starts a **pending** onset rather than firing at once. It fires, stamped at the time of the first reading, when either (a) a second judge reading within 30 cents of the first arrives within 2 judge steps, or (b) the onset detector's amplitude onset lies within ±10 ms of the first reading. A pending onset that neither confirms is dropped, and the published pitch stays where it was.
5. In `cycle.rs`'s `consider`, a candidate not held by the current estimate needs clarity `CLARITY_ON + 0.08` (0.88) while the held estimate's own clarity is still at least `HOLD_CLARITY`, so the old string ringing does not let a weak unrelated period take over. Once the held estimate falls below `HOLD_CLARITY`, the floor is `CLARITY_ON` as today.

### What stays

Every target, take, budget, floor not named above, and every other acceptance test stays exactly as it is. If A or B as written cannot meet a target, do not tune past what is written: stop, and report the measurements in the shape of run 7's report.

## Red tests first

The two red tests above are the red. Add:

1. `a_provisional_low_e_is_right`: on guitar_open, every provisional frame is within 50 cents of truth, or is corrected within two periods.
2. `a_lone_wrong_reading_is_no_onset`: a fixture with one injected 0.82-clarity unrelated reading mid-note gives no onset.
3. `slides_bends_and_vibrato_are_not_onsets` and every other P06/P07/V03 test stay green, with their printed latencies in the END block compared to before.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `uptime` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core
```

## Definition of done

- [ ] `early_meets_its_targets` and `a_sweep_is_one_onset_per_note` pass; the new tests failed first and pass; all of kazoo-core passes; clippy, fmt and ast-grep exit 0.
- [ ] The latency table for every take, before and after, in the END block.

## END

```
START KZ-R04 base=<sha> branch=lane/kz-R04 df=<GB> load=<1m> at <time>
END KZ-R04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the latency table, before and after
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
