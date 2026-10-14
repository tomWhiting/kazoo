# KZ-R04 run 6 evidence

The requested repair is incomplete. The sweep remains red after the revised
published-note set and pitch-only cancellation. Pitch tuning stopped at the
first revised sweep failure. No clarity floor, cents limit or arrival limit
was changed. Changes remain uncommitted.

START KZ-R04 base=c511676 branch=lane/kz-R04 head=55d0e6adb7e6ce1aadd58294bcbb0d4dd6f875d1 df=31GiB load=142.15 at 2026-10-15 09:15:51 AEDT

## Retained mechanism

The raw worker search and its candidate-selection timing are restored from
commit 40d7c77, the committed run 4 tree. The run 5 cancellation module and
allocation-guard repair remain. A separate worker history computes residual
key maxima. Residual results change only the published pitch value, after
the raw gate and onset decisions. Every step with no residual key maximum
at 0.60 falls back to that raw reading. The residual window ends at the
next attack or 40 ms.

Published frames reach the worker through a preallocated feedback ring.
The offline adapters deliver the same frames synchronously. Each onset
adds its published period; later frames update that note's period so a
provisional correction belongs to the same note. Closing the actual gate
clears the set. At each attack the set is combined with pre-attack maxima
at or above 0.60. The existing stronger-member multiple/fraction rule then
filters it. No member-count cap is imposed; stage storage can grow on the
worker. The new feedback overflow counter is separate from dropped verdicts.

The sweep checks exactly six onsets across the entire take, including its
release. Its pitch check starts two truth periods after each stamped onset
and ends at the next onset, or the final release. The 50-cent limit and
existing onset latency limit remain. Onset-time pitches are still printed.

## Baseline red

The unchanged starting tree ran four selected tests to completion:
4 run, 0 passed, 4 failed, 1197 skipped; nextest exit 100.

Each following receipt contains that complete run, saved before production
edits. These are failures, without a claimed red-to-green result:

- red-a_sweep_is_one_onset_per_note-run6.txt: C3 reports 136.29094 Hz
  against 130.81277 Hz, 71.023 cents; later plucks are also wrong.
- red-the_frames_follow_a_glide_and_a_vibrato_without_steps-run6.txt:
  frames 10273 and 10305 change from 147.5684 to 147.8582 Hz.
- red-the_bass_profile_hears_low_b-run6.txt: frame 60725 reports
  468.73685 Hz against 30.867702 Hz.
- red-a_provisional_low_e_is_right-run6.txt: the assertion's truth lookup
  panics with `provisional note truth` before evaluating that frame's cents.

The first revised-path run completed 4 tests: 1 passed, 3 failed,
1197 skipped; exit 100. The sweep emitted seven onsets, including the
release onset at stamp 47801. The vibrato and provisional-truth failures
were reproduced. Receipt: run6-revised-sweep.txt. Later production edits
were compiler/lint corrections and diagnostic source snapshots; there was
no further pitch tuning.

## Ringing sources, combs and residual maxima

Receipt: run6-focused.txt, ringing_cancellation_sweep_measurements.
Each pair below records the delivered onset frame and the first frame at
or beyond two truth periods after its stamped onset. Delivery and frame
stamp are both given because onset stamps are backdated. The NSDF maxima
are from the residual history at that delivery, not a claim of observing
future samples at the earlier stamp. If fewer than three maxima exist,
all existing maxima are listed. Periods are samples; clarity is NSDF.
Published-source tuples retain the originating note stamp and the exact
period/strength present at the attack. Pre-attack maxima below 0.60 are
listed as diagnostic evidence but do not enter the comb set.

### Pluck 9600, truth 110.00000 Hz: delivered onset stamp 9601, delivery 10177, pitch 110.01006 Hz

published_sources: `[]`

pre_attack_sources: `[]`

ringing: `[]`

combs: `[]`

top_three: `[Peak { period: 436.32306, clarity: 0.9997622, window: 140 }]`


### two-period frame: stamp 10497, delivery 10497, pitch 109.978 Hz

published_sources: `[]`

pre_attack_sources: `[]`

ringing: `[]`

combs: `[]`

top_three: `[Peak { period: 436.37045, clarity: 0.99994665, window: 460 }]`


### Pluck 12960, truth 130.81277 Hz: delivered onset stamp 12967, delivery 13031, pitch 683.00446 Hz

published_sources: `[(9601, Peak { period: 436.48465, clarity: 0.9999933, window: 0 })]`

pre_attack_sources: `[Peak { period: 436.36197, clarity: 0.9999679, window: 1612 }]`

ringing: `[Peak { period: 436.48465, clarity: 0.9999933, window: 0 }]`

combs: `[Comb { period: 436.48465, gain: 0.9941825 }]`

top_three: `[]`


### two-period frame: stamp 13703, delivery 13703, pitch 142.00446 Hz

published_sources: `[(9601, Peak { period: 436.48465, clarity: 0.9999933, window: 0 })]`

pre_attack_sources: `[Peak { period: 436.36197, clarity: 0.9999679, window: 1612 }]`

ringing: `[Peak { period: 436.48465, clarity: 0.9999933, window: 0 }]`

combs: `[Comb { period: 436.48465, gain: 0.9941825 }]`

top_three: `[Peak { period: 338.01758, clarity: 0.7880477, window: 398 }]`


### Pluck 16320, truth 164.81380 Hz: delivered onset stamp 16345, delivery 16409, pitch 1397.3698 Hz

published_sources: `[(13575, Peak { period: 179.7296, clarity: 0.999288, window: 0 })]`

pre_attack_sources: `[Peak { period: 392.39487, clarity: 0.7719875, window: 1656 }, Peak { period: 746.4027, clarity: 0.2596689, window: 1302 }]`

ringing: `[Peak { period: 179.7296, clarity: 0.999288, window: 0 }, Peak { period: 392.39487, clarity: 0.7719875, window: 1656 }]`

combs: `[Comb { period: 179.7296, gain: 0.9 }, Comb { period: 392.39487, gain: 0.9 }]`

top_three: `[Peak { period: 34.35025, clarity: 0.9689162, window: 30 }]`


### two-period frame: stamp 16953, delivery 16953, pitch 161.04953 Hz

published_sources: `[(13575, Peak { period: 179.7296, clarity: 0.999288, window: 0 })]`

pre_attack_sources: `[Peak { period: 392.39487, clarity: 0.7719875, window: 1656 }, Peak { period: 746.4027, clarity: 0.2596689, window: 1302 }]`

ringing: `[Peak { period: 179.7296, clarity: 0.999288, window: 0 }, Peak { period: 392.39487, clarity: 0.7719875, window: 1656 }]`

combs: `[Comb { period: 179.7296, gain: 0.9 }, Comb { period: 392.39487, gain: 0.9 }]`

top_three: `[Peak { period: 298.04495, clarity: 0.9022362, window: 310 }]`


### Pluck 19680, truth 220.00000 Hz: delivered onset stamp 19709, delivery 19869, pitch 388.9464 Hz

published_sources: `[(17177, Peak { period: 354.04642, clarity: 0.8694402, window: 0 })]`

pre_attack_sources: `[Peak { period: 341.2457, clarity: 0.49868235, window: 1707 }]`

ringing: `[Peak { period: 354.04642, clarity: 0.8694402, window: 0 }]`

combs: `[Comb { period: 354.04642, gain: 0.9 }]`

top_three: `[Peak { period: 123.41032, clarity: 0.7270437, window: 37 }]`


### two-period frame: stamp 20157, delivery 20157, pitch 157.17517 Hz

published_sources: `[(17177, Peak { period: 354.04642, clarity: 0.8694402, window: 0 })]`

pre_attack_sources: `[Peak { period: 341.2457, clarity: 0.49868235, window: 1707 }]`

ringing: `[Peak { period: 354.04642, clarity: 0.8694402, window: 0 }]`

combs: `[Comb { period: 354.04642, gain: 0.9 }]`

top_three: `[Peak { period: 295.0139, clarity: 0.29181686, window: 153 }]`


### Pluck 23040, truth 261.62555 Hz: delivered onset stamp 23065, delivery 23289, pitch 382.21286 Hz

published_sources: `[(17177, Peak { period: 715.1755, clarity: 0.94632125, window: 0 }), (19709, Peak { period: 210.71407, clarity: 0.91226655, window: 0 })]`

pre_attack_sources: `[Peak { period: 425.5055, clarity: 0.37299526, window: 1622 }, Peak { period: 644.97076, clarity: 0.038016286, window: 1403 }]`

ringing: `[Peak { period: 715.1755, clarity: 0.94632125, window: 0 }, Peak { period: 210.71407, clarity: 0.91226655, window: 0 }]`

combs: `[Comb { period: 715.1755, gain: 0.9 }, Comb { period: 210.71407, gain: 0.9 }]`

top_three: `[Peak { period: 125.58447, clarity: 0.80349356, window: 98 }]`


### two-period frame: stamp 23449, delivery 23449, pitch 157.90486 Hz

published_sources: `[(17177, Peak { period: 715.1755, clarity: 0.94632125, window: 0 }), (19709, Peak { period: 210.71407, clarity: 0.91226655, window: 0 })]`

pre_attack_sources: `[Peak { period: 425.5055, clarity: 0.37299526, window: 1622 }, Peak { period: 644.97076, clarity: 0.038016286, window: 1403 }]`

ringing: `[Peak { period: 715.1755, clarity: 0.94632125, window: 0 }, Peak { period: 210.71407, clarity: 0.91226655, window: 0 }]`

combs: `[Comb { period: 715.1755, gain: 0.9 }, Comb { period: 210.71407, gain: 0.9 }]`

top_three: `[Peak { period: 303.9805, clarity: 0.70917994, window: 80 }, Peak { period: 181.4866, clarity: 0.08556043, window: 203 }, Peak { period: 111.58605, clarity: 0.040644556, window: 272 }]`


### Pluck 26400, truth 329.62747 Hz: delivered onset stamp 26421, delivery 26517, pitch 507.8008 Hz

published_sources: `[(17177, Peak { period: 715.1755, clarity: 0.94632125, window: 0 }), (19709, Peak { period: 210.71407, clarity: 0.91226655, window: 0 }), (23065, Peak { period: 397.39417, clarity: 0.90038383, window: 0 })]`

pre_attack_sources: `[Peak { period: 203.84993, clarity: 0.2262289, window: 1844 }, Peak { period: 384.5411, clarity: 0.45506412, window: 1663 }, Peak { period: 568.9707, clarity: 0.19408467, window: 1479 }]`

ringing: `[Peak { period: 715.1755, clarity: 0.94632125, window: 0 }, Peak { period: 210.71407, clarity: 0.91226655, window: 0 }, Peak { period: 397.39417, clarity: 0.90038383, window: 0 }]`

combs: `[Comb { period: 715.1755, gain: 0.9 }, Comb { period: 210.71407, gain: 0.9 }, Comb { period: 397.39417, gain: 0.9825589 }]`

top_three: `[Peak { period: 57.85386, clarity: 0.3408785, window: 38 }]`


### two-period frame: stamp 26741, delivery 26741, pitch 130.8461 Hz

published_sources: `[(17177, Peak { period: 715.1755, clarity: 0.94632125, window: 0 }), (19709, Peak { period: 210.71407, clarity: 0.91226655, window: 0 }), (23065, Peak { period: 397.39417, clarity: 0.90038383, window: 0 })]`

pre_attack_sources: `[Peak { period: 203.84993, clarity: 0.2262289, window: 1844 }, Peak { period: 384.5411, clarity: 0.45506412, window: 1663 }, Peak { period: 568.9707, clarity: 0.19408467, window: 1479 }]`

ringing: `[Peak { period: 715.1755, clarity: 0.94632125, window: 0 }, Peak { period: 210.71407, clarity: 0.91226655, window: 0 }, Peak { period: 397.39417, clarity: 0.90038383, window: 0 }]`

combs: `[Comb { period: 715.1755, gain: 0.9 }, Comb { period: 210.71407, gain: 0.9 }, Comb { period: 397.39417, gain: 0.9825589 }]`

top_three: `[Peak { period: 198.26245, clarity: 0.5770505, window: 122 }]`


### Release: delivered onset stamp 47801, delivery 48025, pitch 304.8041 Hz

published_sources: `[(43925, Peak { period: 364.8433, clarity: 0.84877306, window: 0 })]`

pre_attack_sources: `[Peak { period: 389.55692, clarity: 0.4160991, window: 1658 }, Peak { period: 733.09595, clarity: 0.33952522, window: 1315 }]`

ringing: `[Peak { period: 364.8433, clarity: 0.84877306, window: 0 }]`

combs: `[Comb { period: 364.8433, gain: 0.9 }]`

top_three: `[]`


### two-period frame: stamp 48121, delivery 48121, pitch 304.8041 Hz

published_sources: `[(43925, Peak { period: 364.8433, clarity: 0.84877306, window: 0 })]`

pre_attack_sources: `[Peak { period: 389.55692, clarity: 0.4160991, window: 1658 }, Peak { period: 733.09595, clarity: 0.33952522, window: 1315 }]`

ringing: `[Peak { period: 364.8433, clarity: 0.84877306, window: 0 }]`

combs: `[Comb { period: 364.8433, gain: 0.9 }]`

top_three: `[]`


## Static checks and execution scope

Every Cargo invocation used this prefix, without changing the target directory:

```
nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=/Users/tom/Developer/projects/deno_rust/kazoo/.lanes/R04/target/kz-logs/nextest cargo
```

Disk and uptime were read before each invocation. No lockf, cargo clean,
target override, dependency change, installation, branch change, commit,
push, full-core suite, full-mouth suite or workspace gate was run.
The filtered final nextest command is:

```
nextest run -p kazoo-core -j 4 --no-fail-fast --success-output immediate -E 'test(voice::) | test(analysis::cycle::) | test(analysis::score::tests::early::)'
```

The final filter selects the actual early-test namespace and includes the
profile tests, both allocation guards, the existing motion test, all early
measurement tests and early_meets_its_targets. No doc comments changed;
doc tests did not run. No timing benchmark or physical device test ran.

| receipt | command and complete result | exit |
|---|---|---:|
| red-revised-path-run6.txt and four named copies | nextest, 4 run, 0 passed, 4 failed, 1197 skipped | 100 |
| run6-revised-sweep.txt | nextest, 4 run, 1 passed, 3 failed, 1197 skipped | 100 |
| run6-fmt.txt | formatter execution | 0 |
| run6-clippy.txt | core and mouth, all targets; 7 library lint errors and 8 test-target lint errors, with shared errors repeated; fixed | 101 |
| run6-fmt-02.txt | formatter execution | 0 |
| run6-fmt-03.txt | formatter execution | 0 |
| run6-clippy-final.txt | core and mouth, all targets | 0 |
| run6-fmt-final.txt | formatter execution after diagnostic source snapshot | 0 |
| run6-fmt-check.txt | formatter check | 0 |
| run6-clippy-retained.txt | core and mouth, all targets, retained Rust tree | 0 |
| run6-ast-grep.txt | ast-grep scan kazoo-core kazoo-mouth | 0 |
| run6-ast-grep-final.txt | ast-grep scan kazoo-core kazoo-mouth, retained Rust tree | 0 |

The second formatter invocation began before the failed first clippy process
had fully exited. That violated the one-Cargo-command-at-a-time instruction.
Later Cargo invocations waited for the preceding tool session's completion.

## Findings for the lead

- The first revised sweep has seven onsets, including one at release stamp
  47801. Its raw acquisition path is restored from the committed run 4 code;
  that restoration does not establish the earlier arrival table on this tree.
- The live gate can close between sweep plucks while older strings still
  sound. The required gate-close clearing then removes their published
  periods (cycle/residual.rs:37). The source snapshots above show the actual
  sets, including periods subsequently published at wrong pitches.
- a_provisional_low_e_is_right selects guitar_open_noise at
  analysis/score/early_tests.rs:222, while the brief calls it guitar_open.
  Its truth lookup at line 230 fails before checking that provisional frame.
  The test was not weakened or changed to skip missing truth.
- The full glide/vibrato step assertion still fails at
  voice/profile/tests.rs:405. The eight motion-delay measurements use a
  different metric and do not override that failure.
- The asynchronous feedback path in voice/judge.rs:167 and
  kazoo-mouth/src/engine.rs:407 is compiled by clippy. Physical device
  behavior, worker lag, callback performance and overflow behavior are not
  established by the synchronous sweep replay.
- Existing future compiler-incompatibility warnings name block 0.1.6 and
  proc-macro-error2 2.0.1. Cargo.lock is unchanged.
- STACK RULES override commit/push, lockf, the older time limit and whole-suite
  gates. No Git metadata write was attempted. The required integration adds
  fields and callback wiring to voice/tracker.rs, which is 509 lines; it was
  not split under the explicit no-reorganization instruction.

NOT CONFIRMED: sweep acceptance; all arrival limits and preservation against
run 4; completion of all four explicitly required regression/acceptance tests; complete
core, mouth or pooled gates; historical same-definition motion delays;
physical low-B or live audio behavior; callback CPU cost and judge budget.
Known assertion failures above are failures, not unverified passes.

BLOCKED ON: the sweep still misses after applying decisions 1 to 3. Pitch
tuning stopped. No threshold, floor, take or timing limit was loosened.
The main acceptance test was stopped at the time-box boundary; its completion requirement is not met.

## Arrival measurements on retained Rust

Receipt: run6-focused.txt, provisional_arrival_measurements: 1 test passed.
All 22 delivery arrivals are within their unchanged per-note limit. Two
are worse than run 4: bass D2 by 2.666668 ms and G2 by 2.000001 ms. Therefore
the decided no-worse-than-run-4 requirement fails. The measurement test
prints evidence; its own pass does not assert the comparison below.

Before values are the latest completed run 4 table, which that report
explicitly identifies as preceding its final snapshot correction. This run
did not remeasure that older tree. After values are actual publication
sample times, not the backdated frame stamps. The separate printed
`after_ms` stamp metric can report 0.020833 ms; it is not delivery latency.

| take | Hz | run 4 delivery ms | retained delivery ms | limit ms | no worse than run 4 |
|---|---:|---:|---:|---:|---|
| bass_open | 30.86770 | 42.020832 | 42.020832 | 50.59448 | yes |
| bass_open | 41.20345 | 32.687500 | 32.687500 | 38.40472 | yes |
| bass_open | 55.00000 | 24.687500 | 23.354166 | 29.27273 | yes |
| bass_open | 73.41618 | 18.041666 | 20.708334 | 22.43146 | FAIL |
| bass_open | 97.99886 | 14.020833 | 16.020834 | 17.30630 | FAIL |
| guitar_open | 82.40690 | 15.354167 | 15.354167 | 20.20236 | yes |
| guitar_open | 110.00000 | 15.354167 | 12.020833 | 15.63636 | yes |
| guitar_open | 146.83237 | 8.041667 | 8.041667 | 12.21573 | yes |
| guitar_open | 195.99773 | 6.687500 | 6.687500 | 9.65315 | yes |
| guitar_open | 246.94162 | 4.708334 | 4.708334 | 8.07431 | yes |
| guitar_open | 329.62747 | 4.020834 | 4.020834 | 6.55059 | yes |
| bass_open_noise | 30.86770 | 42.020832 | 42.020832 | 82.99080 | yes |
| bass_open_noise | 41.20345 | 31.354166 | 31.354166 | 62.67453 | yes |
| bass_open_noise | 55.00000 | 24.687500 | 24.687500 | 47.45454 | yes |
| bass_open_noise | 73.41618 | 17.354166 | 17.354166 | 36.05244 | yes |
| bass_open_noise | 97.99886 | 13.354167 | 13.354167 | 27.51050 | yes |
| guitar_open_noise | 82.40690 | 15.354167 | 15.354167 | 32.33727 | yes |
| guitar_open_noise | 110.00000 | 12.020833 | 12.020833 | 24.72727 | yes |
| guitar_open_noise | 146.83237 | 8.687500 | 8.687500 | 19.02622 | yes |
| guitar_open_noise | 195.99773 | 6.687500 | 6.687500 | 14.75525 | yes |
| guitar_open_noise | 246.94162 | 9.354167 | 9.354167 | 12.12385 | yes |
| guitar_open_noise | 329.62747 | 4.020834 | 4.020834 | 9.58432 | yes |

## Motion measurements on retained Rust

Each row's before value is an early-disabled replay on this tree. These
are following delays over moving segments after 200 ms. They do not prove
preservation against a historical run 4 tree. Every row has one initial
onset and no motion onset. The independent full vibrato trajectory still
fails the per-frame step assertion.

| profile | take | early-disabled reference ms | retained following ms |
|---|---|---:|---:|
| Guitar | slide | 3.333333 | 3.333333 |
| Guitar | bend | 2.000000 | 2.000000 |
| Guitar | vibrato | 3.333333 | 3.333333 |
| Guitar | glide | 7.333333 | 7.333333 |
| Bass | slide | 2.000000 | 2.000000 |
| Bass | bend | 0.666667 | 0.666667 |
| Bass | vibrato | 2.000000 | 2.000000 |
| Bass | glide | 7.333333 | 7.333333 |

## Retained low-B failure

The final focused run completed the_bass_profile_hears_low_b with exit 100
for that test after 249.679 seconds. At voice/profile/tests.rs:235 the output
is `settled octave: 30.867702`, followed by frame 74911 at 469.82056 Hz,
voiced and gated, provisional=false, confidence=0.9974247. The baseline
failed earlier at frame 60725, 468.73685 Hz. Neither result is a low-B pass.

## Changed paths

Counts are relative to starting commit 55d0e6a. No Git metadata was written.
The report and residual module are new files. Generated receipts are ignored
under target/kz-logs and remain on disk.

| path | added | removed |
|---|---:|---:|
| kazoo-core/src/analysis/cycle.rs | 1 | 0 |
| kazoo-core/src/analysis/cycle/cancellation.rs | 5 | 2 |
| kazoo-core/src/analysis/cycle/provisional.rs | 90 | 85 |
| kazoo-core/src/analysis/cycle/provisional_tests.rs | 24 | 13 |
| kazoo-core/src/analysis/score/early_measurements.rs | 12 | 0 |
| kazoo-core/src/analysis/score/early_tests.rs | 7 | 1 |
| kazoo-core/src/analysis/score/fused.rs | 6 | 0 |
| kazoo-core/src/voice/judge.rs | 25 | 0 |
| kazoo-core/src/voice/profile/tests.rs | 19 | 3 |
| kazoo-core/src/voice/tests.rs | 7 | 2 |
| kazoo-core/src/voice/tracker.rs | 25 | 10 |
| kazoo-mouth/src/engine.rs | 1 | 0 |
| kazoo-core/src/analysis/cycle/residual.rs | 145 | 0 |

## Final focused result

Summary [ 949.529s] 111 tests run: 106 passed (3 slow), 5 failed, 1090 skipped; nextest exit 100, read from the owned session.

FAIL [  11.020s] ( 32/111) kazoo-core analysis::score::tests::early::a_provisional_low_e_is_right
FAIL [   8.853s] ( 63/111) kazoo-core voice::profile::tests::a_sweep_is_one_onset_per_note
FAIL [   2.632s] ( 74/111) kazoo-core voice::profile::tests::the_frames_follow_a_glide_and_a_vibrato_without_steps
FAIL [ 249.679s] (110/111) kazoo-core voice::profile::tests::the_bass_profile_hears_low_b
SIGINT [ 935.399s] (111/111) kazoo-core analysis::score::tests::early::early_meets_its_targets

The complete count is 111 run: 106 passed, 4 assertion failures, 1 SIGINT, 1090 skipped. No interrupted test is counted as a pass. early_meets_its_targets ran past 15 minutes and was interrupted to honor the 25-minute task limit. Its final assertion and score table are not verified. All other selected tests finished. No Cargo process remains in the owned session.

DONE: incomplete. Sweep, vibrato, low-B and provisional-truth failures remain. The main acceptance result is not verified. All 22 delivery arrivals meet their limit, but two regress against the recorded run 4 table. Full suites did not run under the explicit filtered scope. Clippy, fmt-check and ast-grep exit 0.

NOT CONFIRMED: completed early_meets_its_targets; all acceptance requirements; full core, mouth and pooled gates; live audio; asynchronous feedback under load; callback and judge timing budgets.

BLOCKED ON: sweep misses after decisions 1 to 3, so pitch tuning stopped; the remaining main acceptance test exceeded the time box. No limit was loosened.

If this code were used in a hospital, the worst credible failure is a missed or false pitch-triggered audio alarm, and it could harm about one monitored person per tracker channel; deployment size is unknown.

END KZ-R04 head=55d0e6adb7e6ce1aadd58294bcbb0d4dd6f875d1 base=c511676 main=bb69a16ff612c5abe87233ad4da3af32b2c8d92c pushed=no at 2026-10-15 09:40:42 +1100 minutes=24.86
