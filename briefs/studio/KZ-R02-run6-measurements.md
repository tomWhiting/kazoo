# KZ-R02 fix run 6 measurements

14 October 2026. Branch `lane/kz-R02`, HEAD
`bbcdc14b023aa36df61a614f0d3e5aad461a3b10`, base `8ed511d`.
START: 19:41:30 AEDT, 29 GiB available. First load reading:
108.14 at 19:42, after the initial red command started.
Changes are uncommitted and unpushed. The run-6 amendment supplies the
45-minute time box and restricts tests to the three named failures plus
`analysis::score::high_notes`. No whole-crate or workspace test ran.

## Outcome and retained implementation

Bass motion is green. Bass vibrato improves from 17.333334 to 12.000000 ms
against 16.615047 ms. All eight high_notes tests pass. Sweep remains twelve
against six. P07 still has five late note rows. Final receipt:
`target/kz-logs/run6-bass-only-nextest.txt`: eleven run, nine passed
(six slow), two failed, 1159 skipped, nextest exit 100.

`analysis/cycle.rs` retries acquisition only for the bounded, long-release
instrument profile: bass. Guitar, voice and line retain their acquisition
choices. The additional persistent state is one optional period.
A refused mark records its strongest local peak. Once per 64 samples while
unlocked, the tracker retries that peak and the newest rising and falling
intervals. It searches their original plus/minus 1/16-period neighborhood
instead of stopping at its first small local maximum. The candidate must
still be bracketed and meet the original 0.80 acquisition floor. The scan
does not climb farther after selecting its best point.

At bass-vibrato sample 576, the correct peak was 324.79584 samples with
clarity 0.812945. Starting at the available 328.82535-sample falling interval,
the greedy climb stopped at 327.6462 with clarity 0.09098947. At the later
mark the correct peak was below 0.80. Receipts: `run6-interval-peaks.txt`
and `run6-mark-evidence.txt`. Retrying between marks and searching the same
bounded neighborhood recovers the useful earlier evidence.

The test-only change prints existing onset counts and latency comparisons.
Every assertion, take, sample count and threshold is unchanged. Temporary
diagnostics and rejected experiments were removed.

Source review verified no new heap allocation, lock, formatting, I/O, unsafe
operation or unchecked indexing in processing. New state is scalar or stack
state; the existing history is sanitized and indexed through its mask.
Callback time and allocation guards did not run under this amendment.
The new normal search can perform more work than the previous greedy climb;
its callback cost is not verified. The early path is unchanged, including
its 0.90 floor, one-shot rule and seventeen-lag bound.

## Remaining limits and rejected trades

E2's first prescribed lobe candidate is 37.54435 samples, against a true
period of about 582.48. Its five scaled lags cannot bracket the true period.
On a fresh isolated E2, scanning every sample before the 20.20236 ms deadline
and integer lags within 50 cents of truth, using a true half-period window,
found maximum NSDF 0.68807745; at the deadline it was 0.6726404.
Normal and early floors remain 0.80 and 0.90. Receipt:
`run6-early-candidates.txt`. This diagnoses the specified candidate and that
window. It does not prove that every estimator or window must miss the
limit. Neither the floor nor the excitation was changed.

Sweep's polyphonic tail produces fast jumps between unrelated partial-cycle
estimates. These reach the unchanged two-frame 80-cent onset rule in
`voice/gate.rs:118`, subject to its unchanged 60 ms refractory period.
The final twelve emitted frames are in `run6-bass-only-nextest.txt`.
The last pluck is sample 26400; onsets continue afterward.

Applying the acquisition repair to guitar reduced sweep to nine onsets but
moved its second onset from sample 14464 to 18304. Against the second pluck
at 12960, those are 31.333333 and 111.333333 ms; the limit is 18.289026 ms.
That trade was rejected. Receipts: `red-run6.txt` and
`run6-verified-nextest.txt`. The bass-only scope restores the original guitar
count, times and pitch decisions.

Other rejected variants:

- Cached-peak and newest-interval retries with the old greedy climb retained
  bass vibrato at 17.333334 ms. Quarter-hop retries also retained it.
- Common and full-period windows did not meet sweep count. Full-period
  windows delayed A2/D3/G3 to 22.666666/16/12 ms against
  15.636364/12.215732/9.653147 ms. Those window changes were removed.
- Choosing the highest-clarity unrelated locked candidate retained eleven
  sweep onsets. That selection change was removed.

## Open-note arrivals

Early off and on have identical arrivals in each receipt. These are signal
clock measurements, not callback timing benchmarks.

| take | Hz | initial ms | final ms | unchanged P07 limit ms |
|---|---:|---:|---:|---:|
| bass_open | 30.86770 | 46.667 | 45.333 | 50.594 |
| bass_open | 41.20345 | 36.000 | 34.667 | 38.405 |
| bass_open | 55.00000 | 34.667 | 28.000 | 29.273 |
| bass_open | 73.41618 | 20.000 | 20.000 | 22.431 |
| bass_open | 97.99886 | 20.000 | 20.000 | 17.306 |
| guitar_open | 82.40690 | 29.333 | 29.333 | 20.202 |
| guitar_open | 110.00000 | 17.333 | 17.333 | 15.636 |
| guitar_open | 146.83237 | 10.667 | 10.667 | 12.216 |
| guitar_open | 195.99773 | 9.333 | 9.333 | 9.653 |
| guitar_open | 246.94162 | 12.000 | 12.000 | 8.074 |
| guitar_open | 329.62747 | 12.000 | 12.000 | 6.551 |

### Initial full instrument table: red-run6.txt

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| early off | bass_open | 46.67 | 46.67 | 0 | 27.33 | 36.00 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early on | bass_open | 46.67 | 46.67 | 0 | 27.33 | 36.00 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early off | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |
| early on | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |

### Final full instrument table: run6-bass-only-nextest.txt

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| early off | bass_open | 45.33 | 45.33 | 0 | 24.00 | 34.67 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early on | bass_open | 45.33 | 45.33 | 0 | 24.00 | 34.67 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early off | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |
| early on | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |

## Commands and complete counts

Every Cargo command used this prefix and the existing target directory:

```
nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=/Users/tom/Developer/projects/deno_rust/kazoo/.lanes/R02/target/kz-logs/nextest cargo
```

Nextest: `nextest run -p kazoo-core --no-fail-fast -E <selection>`.
The eleven-test runs also used `-j 4 --success-output immediate`.
Two-test runs used success-output capture when needed. Selections:

- motion: `test(slides_bends_and_vibrato_are_not_onsets)`;
- sweep: `test(a_sweep_is_one_onset_per_note)`;
- latency: `test(early_meets_its_targets)`;
- high: `test(analysis::score::high_notes::)`;
- three: motion, sweep and latency;
- two: motion and sweep;
- eleven: three and high.

All receipts are under `target/kz-logs`. Exit codes were read from the owned
tool sessions. All runs completed; none was interrupted.

| receipt | selection | tool's complete summary | exit |
|---|---|---|---:|
| red-run6.txt | three | Summary [ 222.602s] 3 tests run: 0 passed, 3 failed, 1167 skipped | 100 |
| run6-vibrato-evidence.txt | motion | Summary [   1.134s] 1 test run: 0 passed, 1 failed, 1169 skipped | 100 |
| run6-mark-evidence.txt | motion | Summary [   0.660s] 1 test run: 0 passed, 1 failed, 1169 skipped | 100 |
| run6-frame-acquisition.txt | two | Summary [   0.610s] 2 tests run: 0 passed, 2 failed, 1168 skipped | 100 |
| run6-retry-evidence.txt | motion | Summary [   0.521s] 1 test run: 0 passed, 1 failed, 1169 skipped | 100 |
| run6-frame-intervals.txt | two | Summary [   0.579s] 2 tests run: 0 passed, 2 failed, 1168 skipped | 100 |
| run6-frame-arrivals.txt | two | Summary [   5.446s] 2 tests run: 0 passed, 2 failed, 1168 skipped | 100 |
| run6-interval-peaks.txt | motion | Summary [   2.412s] 1 test run: 0 passed, 1 failed, 1169 skipped | 100 |
| run6-quarter-hop.txt | two | Summary [   1.133s] 2 tests run: 0 passed, 2 failed, 1168 skipped | 100 |
| run6-common-window.txt | two | Summary [   1.236s] 2 tests run: 0 passed, 2 failed, 1168 skipped | 100 |
| run6-bracket-search.txt | two | Summary [   1.988s] 2 tests run: 1 passed, 1 failed, 1168 skipped | 100 |
| run6-full-window.txt | two | Summary [   4.242s] 2 tests run: 1 passed, 1 failed, 1168 skipped | 100 |
| run6-original-window.txt | two | Summary [   1.843s] 2 tests run: 1 passed, 1 failed, 1168 skipped | 100 |
| run6-clairty-bounds.txt | sweep | Summary [   6.508s] 1 test run: 0 passed, 1 failed, 1169 skipped | 100 |
| run6-early-candidates.txt | sweep | Summary [   6.637s] 1 test run: 0 passed, 1 failed, 1169 skipped | 100 |
| run6-strongest-peak.txt | two | Summary [   6.717s] 2 tests run: 1 passed, 1 failed, 1168 skipped | 100 |
| run6-final-nextest.txt | eleven, rejected guitar scope | Summary [ 191.898s] 11 tests run: 9 passed (5 slow), 2 failed, 1159 skipped | 100 |
| run6-verified-nextest.txt | eleven, rejected guitar scope | Summary [ 180.613s] 11 tests run: 9 passed (5 slow), 2 failed, 1159 skipped | 100 |
| run6-bass-only-nextest.txt | eleven, retained implementation | Summary [ 304.305s] 11 tests run: 9 passed (6 slow), 2 failed, 1159 skipped | 100 |


The three named red copies preserve the complete initial three-test output,
not three separate executions:
`red-early_meets_its_targets-run6.txt`,
`red-a_sweep_is_one_onset_per_note-run6.txt`, and
`red-slides_bends_and_vibrato_are_not_onsets-run6.txt`.
Initial output was six late P07 rows, sweep twelve against six, and bass
vibrato 17.333334 against 16.615047 ms. Motion is now green; the other two
remain red.

Clippy: `clippy -p kazoo-core --all-targets -- -D warnings`.

| receipt | exit | result |
|---|---:|---|
| run6-clippy.txt | 101 | verbose_bit_mask at cycle.rs:406 |
| run6-clippy-final.txt | 101 | is_multiple_of needs 1.87; crate MSRV is 1.85 |
| run6-clippy-msrv.txt | 0 | modulo, earlier guitar scope |
| run6-clippy-bass.txt | 0 | retained bass-only implementation |

Formatter execution `fmt --all`: `run6-fmt.txt` and `run6-fmt-bass.txt`,
exit 0 each. Formatter checks `fmt --all --check`: `run6-fmt-check.txt`,
`run6-fmt-check-final.txt` and `run6-fmt-check-bass.txt`, exit 0 each.
Direct `ast-grep scan kazoo-core` receipts `run6-ast-grep.txt`,
`run6-ast-grep-final.txt`, and `run6-ast-grep-final-bass.txt`, exit 0 each.
An additional scan in `run6-ast-grep-bass.txt` was part of a combined inspection;
the later direct scan supplies final exit-code proof.
`git diff --check`: exit 0.
No doc comments changed; doc tests did not run. No bench, installation,
workspace check, whole-crate test or live audio ran. No dependency,
Cargo.lock, branch or Git metadata changed.

The first report-generation command exited 1 with
`Missing complete nextest summary: run6-vibrato-evidence.txt` because its
parser required plural `tests`. It wrote neither report nor BENCH index.
The corrected generator preserves the complete summaries, including singular
`test`, and exited 0.

## V12 retained decision and current tables

B1 is inherited and unchanged. `BENCH.md:948` records historical cycle
splice_800 gross error 2.56% before B1. The current row is 0.00%; every
splice row has zero settled octave-error steps. All eight high_notes tests
pass in the final retained run. B2 was not built. These current tables are
copied from `target/kz-logs/kz-v12` so later runs cannot overwrite this
run's measurements.

### high_whistles_hold

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cycle | whistle_800 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 3.35 | 9.53 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | whistle_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 3.34 | 9.87 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | whistle_1100 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 3.07 | 9.60 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | whistle_1100 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 3.09 | 9.56 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | whistle_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.70 | 8.89 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | whistle_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.66 | 8.74 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | whistle_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.42 | 7.98 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | whistle_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.51 | 7.88 | 0.00 | 0.00 | 0.00 | 0.00 | - |

| tracker | take | octave error steps after 100 ms |
|---|---|---:|
| cycle | whistle_800 | 0 |
| voice | whistle_800 | 0 |
| cycle | whistle_1100 | 0 |
| voice | whistle_1100 | 0 |
| cycle | whistle_1500 | 0 |
| voice | whistle_1500 | 0 |
| cycle | whistle_2200 | 0 |
| voice | whistle_2200 | 0 |

### high_sung_seconds_hold

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cycle | sung2_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.46 | 1.49 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung2_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.47 | 1.65 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung2_1100 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 0.88 | 1.77 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung2_1100 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.90 | 1.88 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung2_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.65 | 1.29 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung2_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.66 | 1.43 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung2_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.32 | 1.01 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung2_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.36 | 1.13 | 0.00 | 0.00 | 0.00 | 0.00 | - |

| tracker | take | octave error steps after 100 ms |
|---|---|---:|
| cycle | sung2_800 | 0 |
| voice | sung2_800 | 0 |
| cycle | sung2_1100 | 0 |
| voice | sung2_1100 | 0 |
| cycle | sung2_1500 | 0 |
| voice | sung2_1500 | 0 |
| cycle | sung2_2200 | 0 |
| voice | sung2_2200 | 0 |

### high_sung_thirds_hold

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cycle | sung3_800 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 0.35 | 1.12 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung3_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.41 | 1.27 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung3_1100 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 0.73 | 1.56 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung3_1100 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.76 | 1.66 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung3_1500 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 0.48 | 1.16 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung3_1500 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.43 | 1.29 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung3_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.86 | 1.59 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung3_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.81 | 1.70 | 0.00 | 0.00 | 0.00 | 0.00 | - |

| tracker | take | octave error steps after 100 ms |
|---|---|---:|
| cycle | sung3_800 | 0 |
| voice | sung3_800 | 0 |
| cycle | sung3_1100 | 0 |
| voice | sung3_1100 | 0 |
| cycle | sung3_1500 | 0 |
| voice | sung3_1500 | 0 |
| cycle | sung3_2200 | 0 |
| voice | sung3_2200 | 0 |

### high_splices_hold

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cycle | splice_800 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 3.33 | 10.39 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | splice_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 3.32 | 10.58 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | splice_1100 | 2.00 | 2.00 | 0 | - | - | 0 | 0 | 3.16 | 11.54 | 0.00 | 0.00 | 0.02 | 0.00 | - |
| voice | splice_1100 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 3.20 | 11.44 | 0.00 | 0.00 | 0.04 | 0.00 | - |
| cycle | splice_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.78 | 11.26 | 0.00 | 0.16 | 0.02 | 0.00 | - |
| voice | splice_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.72 | 11.78 | 0.00 | 0.18 | 0.00 | 0.00 | - |
| cycle | splice_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.54 | 11.73 | 0.00 | 0.11 | 0.00 | 0.00 | - |
| voice | splice_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.54 | 11.54 | 0.00 | 0.13 | 0.00 | 0.00 | - |

| tracker | take | octave error steps after 100 ms |
|---|---|---:|
| cycle | splice_800 | 0 |
| voice | splice_800 | 0 |
| cycle | splice_1100 | 0 |
| voice | splice_1100 | 0 |
| cycle | splice_1500 | 0 |
| voice | splice_1500 | 0 |
| cycle | splice_2200 | 0 |
| voice | splice_2200 | 0 |

### high_notes_hold_at_96k

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cycle | whistle_2200 | 1.00 | 1.00 | 0 | - | - | 0 | 0 | 1.92 | 6.17 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | whistle_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 1.95 | 6.29 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | sung2_2200 | 1.00 | 1.00 | 0 | - | - | 0 | 0 | 0.21 | 0.56 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sung2_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.25 | 0.63 | 0.00 | 0.00 | 0.00 | 0.00 | - |

| tracker | take | octave error steps after 100 ms |
|---|---|---:|
| cycle | whistle_2200 | 0 |
| voice | whistle_2200 | 0 |
| cycle | sung2_2200 | 0 |
| voice | sung2_2200 | 0 |

### high_notes_hold_through_the_engine_judge

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| engine voice | whistle_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 3.34 | 9.87 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | sung2_800 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.47 | 1.65 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | whistle_1100 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 3.09 | 9.56 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | sung2_1100 | 2.67 | 2.67 | 0 | - | - | 0 | 0 | 0.90 | 1.88 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | whistle_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.66 | 8.74 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | sung2_1500 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.66 | 1.43 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | whistle_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 2.51 | 7.88 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| engine voice | sung2_2200 | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.36 | 1.13 | 0.00 | 0.00 | 0.00 | 0.00 | - |

| tracker | take | octave error steps after 100 ms |
|---|---|---:|
| engine voice | whistle_800 | 0 |
| engine voice | sung2_800 | 0 |
| engine voice | whistle_1100 | 0 |
| engine voice | sung2_1100 | 0 |
| engine voice | whistle_1500 | 0 |
| engine voice | sung2_1500 | 0 |
| engine voice | whistle_2200 | 0 |
| engine voice | sung2_2200 | 0 |

## Found, not confirmed, and blocked

P07's original normal-path exclusion at
`KZ-P07-low-notes-speak-sooner.md:54` conflicts with this P06 repair:
the retained change improves bass normal acquisition. R02's acceptance
repair uses that instrument mechanism; it changes no decided constant.
The run-6 45-minute and filtered-check amendment overrides the earlier
25/30-minute and whole-crate clauses. No commit or push was attempted.

Cargo reports future compiler incompatibility in block 0.1.6
(`Cargo.lock:287`) and proc-macro-error2 2.0.1 (`Cargo.lock:2615`).
Those dependencies were not changed. The initial load preflight was read
after red launch; its actual time is recorded above.

Not confirmed: the two remaining acceptance tests; the other nine original
acceptance tests on this new tree; the whole crate and pooled stack-tip gate;
voice/line bit-for-bit whole-take replay on the new tree; allocation guard
execution; the new normal search's callback time; the 4.2 ms judge budget;
physical low-B response; live held singing/whistling with the actual devices.
These checks were outside run-6's filtered scope. Source isolation and
high-note passes do not replace them. No audio-device sandbox failure
occurred in the selected tests.

BLOCKED ON: G2/E2/A2/B3/E4 arrivals
20.000/29.333/17.333/12.000/12.000 ms against
17.306/20.202/15.636/8.074/6.551 ms. E2's prescribed first-lobe candidate and
NSDF floor conflict is measured above. Sweep is twelve against six with
unchanged 80-cent and 60 ms rules; the nine-onset variant traded away
second-pluck latency and was removed. These are unresolved measured
mechanism/target pairs, not claims that no estimator could meet the targets.
Diagnosis and selected validation took 34 minutes from 19:41:30 to 20:15:28,
with interleaved work on both pairs; the remaining time produced this handback.
At 20:15:28, disk had 15 GiB free and load was 40.10.

If this code were used in a hospital, the worst credible failure is a missed
or false audio alarm if pitch events were used for alarm detection, and it
could harm about one monitored person per tracker channel; deployment size
is unknown.

## Final handoff state

START KZ-R02 base=8ed511d branch=lane/kz-R02 df=29GiB at 2026-10-14 19:41:30 AEDT

END KZ-R02 head=bbcdc14b023aa36df61a614f0d3e5aad461a3b10 base=8ed511d pushed=no at 2026-10-14 20:24:15 AEDT minutes=42.76

All twelve printed guitar sweep onset frames match the initial receipt exactly. Final diff whitespace validation exited 0. No process remains running from this job. The next step is Cassio's commit and pooled stack gate; the two unresolved acceptance pairs above require further mechanism work.
