# KZ-R02 fix run 7 measurements

14 October 2026. Branch `lane/kz-R02`, HEAD
`8897121513f02fe2222e1d5b70fb6af4886fdd54`, base `8ed511d`.
START: 20:27:57 AEDT, 38 GiB available. The first load reading was 76.95
at 20:30, after the red command started. No commit or push was attempted.

## Outcome

Neither remaining acceptance failure is fixed. No production or test change
is retained. All five sweep experiments and the temporary measurement test
were removed. This report and the receipts preserve the investigation.

The initial run completed: two tests run, zero passed, two failed,
1168 skipped, nextest exit 100. Receipt: `target/kz-logs/red-run7.txt`.
The complete receipt is also saved under each required red name:

- `target/kz-logs/red-early_meets_its_targets-run7.txt`;
- `target/kz-logs/red-a_sweep_is_one_onset_per_note-run7.txt`.

## The guitar candidate is a noise lobe, not a miscomputed period

`analysis/score/instrument.rs:27` fills each initial delay line with seeded
noise, as P06 specifies. P07 instead assumes a first negative lobe whose
doubled duration is within about 25 percent of the fundamental period
(`KZ-P07-low-notes-speak-sooner.md:34-37`). That assumption does not hold
for these takes.

The native tracker measured isolated guitar E2's first pending candidate as
37.54435 samples, with deadline 55.3165245 samples. Its true period is
582.475525 samples. The candidate is 6.446 percent of the true period.
`analysis/cycle.rs` updates the interpolated rising crossing before calling
`early_push`; `analysis/cycle/early.rs:95` then computes precisely the
specified `2 * (rise - down)`. This is not a stale rising crossing or a
seconds-versus-samples conversion.

The independent deterministic reconstruction measured the downward and
upward crossings at 34.0209203 and 52.7930798 samples. Twice their difference
is 37.5443190 samples, agreeing with the native f32 result. Its first lobe
at sample 8 is outside the guitar range; the next armed lobe, ending at
sample 53, supplies the first in-range candidate. Receipts:
`target/kz-logs/run7-lobe-diagnostic.py` and
`target/kz-logs/run7-lobe-diagnostic.txt`.

The native measurement also scanned every armed lobe through E2's unchanged
20.20236 ms target. The largest doubled lobe was 95.81478882 samples. Even
that lobe's largest prescribed coarse lag is only 119.768486 samples,
whereas lags within 50 cents of truth are 566 through 599. Rearming after
the first refusal would not make any of those lobes bracket the fundamental.
The bounded local refits cannot bridge that gap either.

For a second check, the native tracker scanned every integer lag within
50 cents of truth, on every sample with the required full history before
the target. With a true half-period window of 291 samples, maximum NSDF
was 0.68807745 at sample 872 and lag 582. It is below the unchanged 0.90
early floor and 0.80 normal acquisition floor. These are measurements of
that window, not a proof that every estimator must miss the target.

The native diagnostic ran before any correlation arithmetic changed:
`run7-native-lobes.txt`, one test passed, 1170 skipped, exit 0. It constructed
the existing suite, reset a tracker at each existing pluck, retained all
samples up to the existing deadline, and printed geometry and correlations.
It made no acceptance assertion and is not counted as an acceptance pass.
The temporary test was removed before final verification.

| take | note Hz | true period, samples | largest armed doubled lobe before target | best NSDF in true half-period window |
|---|---:|---:|---:|---:|
| bass_open | 30.86770248 | 1555.02343750 | 366.96942139 | 0.9268898 |
| bass_open | 41.20344925 | 1164.95104980 | 243.79106140 | 0.91781604 |
| bass_open | 55.00000000 | 872.72729492 | 501.76571655 | 0.8168769 |
| bass_open | 73.41618347 | 653.80682373 | 320.53979492 | 0.8743267 |
| bass_open | 97.99886322 | 489.80160522 | 189.51473999 | 0.8762030 |
| guitar_open | 82.40689850 | 582.47552490 | 95.81478882 | 0.68807745 |
| guitar_open | 110.00000000 | 436.36364746 | 72.52735901 | 0.80085856 |
| guitar_open | 146.83236694 | 326.90341187 | 102.99478149 | 0.9096163 |
| guitar_open | 195.99772644 | 244.90080261 | 133.21884155 | 0.92767996 |
| guitar_open | 246.94161987 | 194.37792969 | 117.03536987 | 0.75021785 |
| guitar_open | 329.62747192 | 145.61892700 | 26.94453430 | 0.8569650 |

These rows are isolated-note diagnostics. They do not replace the continuous
open-string acceptance run. The diagnostic's first pending candidate was
observed after each push; an immediately consumed candidate is not captured
by that particular observation.

An additional independent Python reconstruction expanded E2's window scan
to 232 through 365 samples, covering the rounded half-windows of candidates
whose five-lag range could bracket truth. Over the same integer lag range
and deadline, its maximum was 0.6925964332 at sample 894, lag 582, window 313.
Receipts: `run7-window-bounds.py` and `run7-window-bounds.txt`. This is model
evidence; the native test verifies the 291-sample row, not the expanded scan.

Lowering the initial filter cutoff in that reconstruction did not provide
a general repair. At 1400, 700, 500, 350, 250, 200, 150 and 100 Hz the first
in-range candidate was respectively 37.544, 51.548, 49.650, 47.305, 40.617,
34.734, 241.794 and 243.199 samples. At 60 Hz it was 612.394 samples, but
that changes the decided acquisition filter and has no verified high-note
latency or correlation proof. No cutoff change was applied to production.
Receipts: `run7-cutoffs.py` and `run7-cutoffs.txt`.

P07's prescribed lobe mechanism and P06's decided noisy excitation therefore
do not supply the candidate geometry required by the stated E2 target.
R02's instruction at `KZ-R02-the-tracker-meets-its-targets.md:21` says to stop
on a conflicting pair and report its measurements. No clarity floor, take,
deadline, one-shot rule, refinement bound or assertion was changed.

## Sweep measurements and rejected experiments

The unchanged sweep emits twelve onsets, against six. At the second pluck,
the first later onset is at sample 14464, reporting 790.5385 Hz while the
new string is 130.8128 Hz. The voice frame's confidence is 0.8374451.
`analysis/cycle.rs:649-679` admits an unrelated candidate at 0.80 and prefers
the shorter period among candidates with the same held status. During the
mixture, those short-window readings can choose unrelated periods.
`voice/gate.rs:118-127` then applies the specified 80-cent jump rule to the
published pitch. The false pitch changes reach the onset rule.

The count assertion runs before the sweep's latency assertions. The initial
receipt also proves five late first onsets after their respective plucks:

| note | pluck sample | first later onset sample | reported Hz | latency ms | unchanged limit ms |
|---|---:|---:|---:|---:|---:|
| A2 | 9600 | 10304 | 110.00029 | 14.667 | 21.182 |
| C3 | 12960 | 14464 | 790.5385 | 31.333 | 18.289 |
| E3 | 16320 | 18304 | 653.67004 | 41.333 | 15.135 |
| A3 | 19680 | 21184 | 1229.3835 | 31.333 | 12.091 |
| C4 | 23040 | 25472 | 573.07166 | 50.667 | 10.645 |
| E4 | 26400 | 28352 | 165.09244 | 40.667 | 9.067 |

The following experiments changed production arithmetic temporarily. Every
experiment was removed. Counts closer to six were not accepted as fixes.

| receipt | experiment | complete nextest summary | exit |
|---|---|---|---:|
| run7-common-window.txt | same held half-period window for instrument candidates | 1 run, 0 passed, 1 failed, 1169 skipped; 9 onsets | 100 |
| run7-common-strongest.txt | common window and greatest clarity within each held status | 1 run, 0 passed, 1 failed, 1169 skipped; 9 onsets | 100 |
| run7-pooled-mean.txt | remove a shared mean from both NSDF windows | 5 run, 4 passed, 1 failed, 1166 skipped; 11 onsets | 100 |
| run7-acquisition-window.txt | 1 ms minimum instrument acquisition window | 1 run, 0 passed, 1 failed, 1169 skipped; 10 onsets | 100 |
| run7-two-ms-window.txt | 2 ms minimum instrument windows | 1 run, 0 passed, 1 failed, 1169 skipped; 7 onsets | 100 |

The pooled-mean run's four passes were `slides_are_continuous`,
`a_note_that_starts_mid_glide`, `slides_bends_and_vibrato_are_not_onsets`,
and `the_frames_follow_a_glide_and_a_vibrato_without_steps`.

The two-millisecond variant did not emit a second onset until sample 21376,
after the fourth pluck at 19680. Reducing false short-period readings also
lost new-note acquisitions. Keeping seven events would conceal that failure.
The sweep fault remains an unresolved pitch acquisition problem in the
ringing mixture. No gate delay, refractory change or event filter was added.

## Open-note arrival table

The initial acceptance receipt measured identical early-off and early-on
latencies. No production change is retained, so there is no improved after
measurement to claim.

| take | Hz | early off / on ms | unchanged P07 limit ms |
|---|---:|---:|---:|
| bass_open | 30.86770 | 45.333 / 45.333 | 50.594 |
| bass_open | 41.20345 | 34.667 / 34.667 | 38.405 |
| bass_open | 55.00000 | 28.000 / 28.000 | 29.273 |
| bass_open | 73.41618 | 20.000 / 20.000 | 22.431 |
| bass_open | 97.99886 | 20.000 / 20.000 | 17.306 |
| guitar_open | 82.40690 | 29.333 / 29.333 | 20.202 |
| guitar_open | 110.00000 | 17.333 / 17.333 | 15.636 |
| guitar_open | 146.83237 | 10.667 / 10.667 | 12.216 |
| guitar_open | 195.99773 | 9.333 / 9.333 | 9.653 |
| guitar_open | 246.94162 | 12.000 / 12.000 | 8.074 |
| guitar_open | 329.62747 | 12.000 / 12.000 | 6.551 |

## Command scope and static checks

Every Cargo command used the existing target directory and this prefix:

```
nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=/Users/tom/Developer/projects/deno_rust/kazoo/.lanes/R02/target/kz-logs/nextest cargo
```

All nextest runs used `nextest run -p kazoo-core --no-fail-fast` and a named
filter. The initial filter selected the two remaining failures. The sweep
experiments selected the sweep alone, except the named five-test mean run.
The native diagnostic selected only `run7_lobe_measurements` and enabled
`--success-output immediate`. Every other test invocation used `-j 4`.

The final permitted filter is:

```
test(analysis::cycle::) | test(voice::profile::) | test(analysis::score::high_notes::) | test(early_meets_its_targets)
```

Static checks on the restored source completed:

- `clippy -p kazoo-core --all-targets -- -D warnings`: exit 0,
  `run7-clippy.txt`;
- formatter execution `fmt --all`: exit 0, `run7-fmt.txt`;
- `fmt --all --check`: exit 0, `run7-fmt-check.txt`;
- restored-source `fmt --all --check`: exit 0, `run7-fmt-restored.txt`;
- `ast-grep scan kazoo-core`: exit 0, `run7-ast-grep.txt`;
- `git diff --check` after restoring all experiments: exit 0;
- the three independent Python diagnostics: exit 0.

Clippy ran before the final two window experiments; both were restored byte
for byte to HEAD afterward. The selected final test binary was built from
HEAD's source before those experiments. No changed Rust source remains.
No doc comments changed, so doc tests did not run. No bench, full-crate test,
workspace check, dependency edit, installation or live audio test ran.

## Final permitted verification

`run7-final-nextest.txt` completed with nextest exit 100, read from its
owned tool session. Its complete summary is:

```
Summary [ 357.048s] 50 tests run: 48 passed (6 slow), 2 failed, 1120 skipped
```

Unique test names, excluding nextest's repeated final failure list:

| selected group | run | passed | failed |
|---|---:|---:|---:|
| analysis::cycle | 28 | 28 | 0 |
| voice::profile | 13 | 12 | 1 |
| analysis::score::high_notes | 8 | 8 | 0 |
| early_meets_its_targets | 1 | 0 | 1 |

The only failures are `early_meets_its_targets` and
`a_sweep_is_one_onset_per_note`. Ten of R02's twelve original acceptance
names passed. The subset is not a whole-crate pass. Every selected test
finished; none was interrupted. Native source and test files are identical
to HEAD, so this binary and the static checks apply to the retained tree.

Allocation guards `analysis::cycle::tests::processing_never_allocates`,
`the_early_path_never_allocates`, and
`voice::profile::tests::processing_never_allocates_with_any_profile` passed.
`voice_is_track_v_exactly` passed. Complete fused voice/line replay was not
selected and is not established by that profile test.

Final open-string score table, with no improved after result:

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| early off | bass_open | 45.33 | 45.33 | 0 | 24.00 | 34.67 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early on | bass_open | 45.33 | 45.33 | 0 | 24.00 | 34.67 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early off | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |
| early on | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |

V12's inherited B1 is retained. `BENCH.md:948` records the historical cycle
splice_800 gross error of 2.56 percent; this run measures 0.00 percent.
All eight high_notes tests pass, including the engine judge. B2 was not
built. Current splice rows are preserved below; every settled octave-error
step count is zero.

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

## Findings and limits

Cargo reported existing future compiler incompatibilities in `block` 0.1.6
and `proc-macro-error2` 2.0.1 (`Cargo.lock:287` and `Cargo.lock:2615`). No
dependency or lockfile was changed. No selected test opened an audio device.

The original whole-crate gate is outside this run's explicit filtered scope.
The initial red launch and formatter execution did not record a load
preflight; later build and test launches recorded disk and load. The first
actual load observation is reported at the start, not attributed to launch.

Not confirmed: either remaining acceptance target; a whole-crate or pooled
stack-tip pass; complete voice/line fused replay; callback timing; the judge's
4.2 ms budget; physical low-B following; live held singing and whistling.
Signal-clock diagnostic timings do not establish callback performance.

BLOCKED ON: P07's first-lobe candidate/search and P06's noisy excitation fail
the measured E2 geometry and 0.90 confirmation requirement before 20.20236 ms.
The sweep still emits twelve events and misses five per-pluck onset limits.
No experiment met the full sweep test. Further mechanism work is required;
no universal impossibility claim is made for other pitch estimators.

If this code were used in a hospital, the worst credible failure is a missed
or false audio alarm if pitch events were used for alarm detection, and it
could harm about one monitored person per tracker channel; deployment size
is unknown.

## Retained artifacts

Only this report changes the Git tree. The ignored diagnostic and check
receipts remain under `target/kz-logs`; no build output was deleted.
All run-7 text and Python receipts were scanned for secret-like strings
before the handback; zero matched.

| receipt | added lines | removed lines |
|---|---:|---:|
| red-a_sweep_is_one_onset_per_note-run7.txt | 99 | 0 |
| red-early_meets_its_targets-run7.txt | 99 | 0 |
| red-run7.txt | 99 | 0 |
| run7-acquisition-window.txt | 34 | 0 |
| run7-ast-grep.txt | 0 | 0 |
| run7-clippy.txt | 5 | 0 |
| run7-common-strongest.txt | 34 | 0 |
| run7-common-window.txt | 34 | 0 |
| run7-cutoffs.py | 42 | 0 |
| run7-cutoffs.txt | 9 | 0 |
| run7-final-nextest.txt | 561 | 0 |
| run7-fmt-check.txt | 1 | 0 |
| run7-fmt-restored.txt | 1 | 0 |
| run7-fmt.txt | 1 | 0 |
| run7-lobe-diagnostic.py | 68 | 0 |
| run7-lobe-diagnostic.txt | 29 | 0 |
| run7-native-lobes.txt | 30 | 0 |
| run7-pooled-mean.txt | 38 | 0 |
| run7-two-ms-window.txt | 34 | 0 |
| run7-window-bounds.py | 44 | 0 |
| run7-window-bounds.txt | 1 | 0 |

## Final handback

START KZ-R02 base=8ed511d branch=lane/kz-R02 df=38GiB load=not-recorded at 2026-10-14 20:27:57 AEDT

END KZ-R02 head=8897121513f02fe2222e1d5b70fb6af4886fdd54 base=8ed511d pushed=no df=69.5GiB load=142.44 at 2026-10-14 20:47:38 AEDT minutes=19.69

DONE: Investigation and permitted checks complete. The requested repair is
not complete: ten original acceptance tests pass and two remain red. No
production or test experiment is retained.

RED: early_meets_its_targets: G2/E2/A2/B3/E4 are late; receipt
red-early_meets_its_targets-run7.txt. Sweep: twelve against six; receipt
red-a_sweep_is_one_onset_per_note-run7.txt. Neither has a green result.

TESTS: kazoo-core selected nextest: 50 run, 48 passed, 2 failed, 1120 skipped,
exit 100. Cycle 28/28, high_notes 8/8, profile 12/13. Native measurement:
one passed, 1170 skipped, exit 0. Every experimental nextest summary and
every static-check exit code is recorded above. Clippy, formatter execution,
both formatter checks, ast-grep and final diff checks exit 0. Doc tests did
not run because no doc comments changed.

COMMANDS: The command scope, filters, five experiment results, two red copies,
final selected result and static-check receipts above are the full check
record. No interrupted command or partial count is reported as a pass.

BENCH: No timing benchmark ran. Native E2: candidate 37.54435 samples against
582.475525 true; largest armed candidate before its target 95.814789; maximum
true-half-window NSDF 0.68807745. Open-note latencies are unchanged. Inherited
B1 splice_800 gross error is 0.00 percent against the historical 2.56 percent;
B2 was not built. Full measured tables are above.

CHANGED: briefs/studio/KZ-R02-run7-measurements.md +384/-0. Rust source
and tests +0/-0. Generated receipt line counts are above.

FOUND: P07's bounded first-lobe assumption does not hold for P06's noise
excitation, at the source and brief lines above. Sweep's false high-frequency
readings reach the decided jump onset rule. Existing dependency
future-compiler warnings remain. The first red and formatter launches lack
a recorded load preflight. Commit and push are prohibited by this run.

NOT CONFIRMED: Both remaining acceptance targets, whole-crate/stack-tip gates,
complete fused voice/line replay, callback and judge budgets, physical bass
and live held high-note behavior.

BLOCKED ON: E2's prescribed lobe search cannot bracket its fundamental before
the target on the decided take; the measured confirmation window also stays
below 0.90. Sweep remains twelve against six with five late per-pluck events.
Five sweep experiments failed and were reverted. Investigation and all
selected verification took 19.69 minutes in total.

If this code were used in a hospital, the worst credible failure is a missed
or false audio alarm, and it could harm about one monitored person per
tracker channel; deployment size is unknown.
