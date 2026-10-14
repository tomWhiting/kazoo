# KZ-R04 measurements

E2 improves from 29.333 ms to 16.000 ms, below its unchanged 20.202 ms target.
The design does not meet all targets. The retained sweep emits nine onsets against six.
Other open-note arrivals remain late or regress. No threshold, take or acceptance limit was tuned.
Changes are retained uncommitted for review. This is not a completed lane.

START KZ-R04 base=8ed511d branch=lane/kz-R04 head=276f4b0ce5e375d919037bd86d2bc4c6d978af55 df=87GiB load=76.27 at 2026-10-14 20:56:09 AEDT
END KZ-R04 head=276f4b0ce5e375d919037bd86d2bc4c6d978af55 base=8ed511d pushed=no df=84GiB load=38.97 at 2026-10-14 21:21:00 AEDT minutes=24.85

## Implementation

The full lag-range search lives in `analysis/cycle/provisional.rs`, with preallocated maxima storage.
`judge::spawn_with_settings` runs it on the worker; callback consumption receives a plain `JudgeResult`.
The existing short-period lobe path remains. Offline instrument helpers drive the same worker search synchronously.
`VoiceFrame.provisional` is encoded in formerly reserved flag bit 3. The 40-byte wire shape remains.
Old frames decode with the new flag false; the pinned old layout and all 16 flag combinations are tested.
Normal acquisition clears provisional state. Corrections suppress another onset.
Gate jumps wait for confirmation and hold the published pitch. Confirmed onsets retain the first reading's timestamp.
The refractory interval prevents onsets while permitting confirmed pitch publication.
`consider` uses 0.88 for unrelated candidates while the held reading retains 0.70 clarity.
The scorer counts provisional errors above 50 cents in the octave figure, including its settling interval.

## Open-note arrival table

Before is the current lane's unmodified-source baseline. After is the retained-source native frame measurement.
The later score accounting change does not alter pitch publication. These are sample-clock measurements, not CPU benchmarks.

| take | Hz | before ms | after ms | unchanged limit ms |
|---|---:|---:|---:|---:|
| bass_open | 30.86770 | 45.333 | 41.333 | 50.594 |
| bass_open | 41.20345 | 34.667 | 48.000 | 38.405 |
| bass_open | 55.00000 | 28.000 | 42.667 | 29.273 |
| bass_open | 73.41618 | 20.000 | 38.667 | 22.431 |
| bass_open | 97.99886 | 20.000 | 32.000 | 17.306 |
| guitar_open | 82.40690 | 29.333 | 16.000 | 20.202 |
| guitar_open | 110.00000 | 17.333 | 18.667 | 15.636 |
| guitar_open | 146.83237 | 10.667 | 12.000 | 12.216 |
| guitar_open | 195.99773 | 9.333 | 10.667 | 9.653 |
| guitar_open | 246.94162 | 12.000 | 12.000 | 8.074 |
| guitar_open | 329.62747 | 12.000 | 12.000 | 6.551 |

## Continuous motion

All eight motion rows passed in the earlier six-test run, before the refractory pitch-publication repair.
That repair's D3 regression passed afterward. The motion matrix was not repeated on the final source.

| profile and take | before ms | earlier after ms |
|---|---:|---:|
| Guitar slide | 14.667 | 12.000 |
| Guitar bend | 8.000 | 8.000 |
| Guitar vibrato | 13.333 | 13.333 |
| Guitar glide | 14.667 | 12.000 |
| Bass slide | 14.667 | 12.000 |
| Bass bend | 8.000 | 8.000 |
| Bass vibrato | 12.000 | 12.000 |
| Bass glide | 14.667 | 12.000 |

## Sweep

Before: twelve onsets. Retained after: nine. Each row is the first later onset, including wrong pitches.
These rows do not prove successful arrival of the new note. The count assertion remains unchanged.

| note | pluck sample | before first later onset | after first later onset |
|---|---:|---|---|
| 0 | 9600 | 14.667 ms at 110.000 Hz | 12.000 ms at 109.999 Hz |
| 1 | 12960 | 31.333 ms at 790.538 Hz | 31.333 ms at 790.538 Hz |
| 2 | 16320 | 41.333 ms at 653.670 Hz | 41.333 ms at 653.670 Hz |
| 3 | 19680 | 31.333 ms at 1229.383 Hz | 30.000 ms at 109.983 Hz |
| 4 | 23040 | 50.667 ms at 573.072 Hz | 88.000 ms at 895.005 Hz |
| 5 | 26400 | 40.667 ms at 165.092 Hz | 18.000 ms at 895.005 Hz |

## Checks and red receipts

DONE: Both required new tests passed after failing first. The correction-accounting regression also failed first and passed.
The two original acceptance failures remain unresolved. A complete retained-tree crate gate is not verified.
The before/after native open-note table is complete; fused and complete P06/P07/V03 retained-tree tables are not verified.

RED: `red-early_meets_its_targets.txt`: E2 29.333 ms against 20.202 ms, plus other late notes; no final green.
RED: `red-a_sweep_is_one_onset_per_note.txt`: twelve against six; retained after nine against six.
RED: `red-a_provisional_low_e_is_right.txt`: guitar_open published no provisional readings; retained green.
RED: `red-a_lone_wrong_reading_is_no_onset.txt`: injected unrelated reading fired an onset; retained green.
RED: `red-a_guitar_repluck_keeps_the_normal_path_latency.txt`: D3 61.333 ms against 12.216 ms; retained green.
RED: `red-corrected_provisional_frames_are_counted.txt`: octave_pct=0.0 after an early correction; retained green.

TESTS: Every Cargo command used the requested nice/env prefix, four jobs, lane-local nextest state and the existing target directory.
Disk and load were read before each Cargo launch. No shared Cargo lock, workspace build, installation, commit or push was used.

- `red-baseline.txt`: Summary [ 587.646s] 3 tests run: 1 passed, 2 failed, 1169 skipped.
- `red-new-tests.txt`: Summary [   1.709s] 2 tests run: 0 passed, 2 failed, 1170 skipped.
- `after-focused.txt`: Summary [ 619.630s] 6 tests run: 4 passed (1 slow), 2 failed, 1166 skipped.
- `full-nextest.txt`: Summary [ 579.905s] 95/1172 tests run: 90 passed (4 slow), 5 failed, 0 skipped.
- `retained-focused.txt`: Summary [   1.762s] 11 tests run: 11 passed, 1161 skipped.
- `retained-measurements.txt`: Summary [  23.054s] 3 tests run: 3 passed, 1170 skipped.
- `red-corrected_provisional_frames_are_counted.txt`: Summary [   0.035s] 1 test run: 0 passed, 1 failed, 1173 skipped.
- `complete-focused.txt`: Summary [ 133.148s] 20 tests run: 20 passed (1 slow), 1154 skipped.
- `complete-sweep.txt`: Summary [  36.976s] 1 test run: 0 passed, 1 failed, 1173 skipped.

Clippy final: exit 101, `clippy-complete.txt`; sole error is four booleans in VoiceFrame at `voice/api.rs:33`.
Formatter execution and final formatter check: exit 0. Ast-grep on kazoo-core and the two mouth literal fixtures: exit 0.
Doc tests: exit 0, two passed, zero failed, doc-complete.txt.
Git diff whitespace check: exit 0. Receipt secret-pattern scan: zero matches (rg exit 1).
The full run was interrupted through its owned tool session. It used test binaries rebuilt during subsequent edits and is not a retained-tree gate.
Its four interrupted tests were early_meets_its_targets, high_notes_hold_through_the_engine_judge, high_whistles_hold and voice_and_line_are_unchanged.
An attempted SIGINT to the proven owned PID was rejected by the sandbox; tool-session interruption completed successfully.

BENCH: Native arrival and onset tables above. No timing bench or live device test ran.

CHANGED:

| added | removed | file |
|---:|---:|---|
| 18 | 2 | kazoo-core/src/analysis/cycle.rs |
| 13 | 4 | kazoo-core/src/analysis/score.rs |
| 36 | 1 | kazoo-core/src/analysis/score/early_tests.rs |
| 14 | 0 | kazoo-core/src/analysis/score/fused.rs |
| 7 | 3 | kazoo-core/src/analysis/score/metrics.rs |
| 1 | 0 | kazoo-core/src/engine/processing/voice.rs |
| 2 | 0 | kazoo-core/src/ipc/client/voice_tests.rs |
| 1 | 0 | kazoo-core/src/ipc/link/tests/voice.rs |
| 5 | 1 | kazoo-core/src/ipc/voice.rs |
| 16 | 4 | kazoo-core/src/ipc/voice/tests.rs |
| 2 | 0 | kazoo-core/src/voice/api.rs |
| 70 | 6 | kazoo-core/src/voice/gate.rs |
| 34 | 1 | kazoo-core/src/voice/judge.rs |
| 14 | 4 | kazoo-core/src/voice/profile/tests.rs |
| 1 | 0 | kazoo-core/src/voice/quantise/tests.rs |
| 1 | 1 | kazoo-core/src/voice/tests.rs |
| 30 | 5 | kazoo-core/src/voice/tracker.rs |
| 1 | 0 | kazoo-mouth/src/synth/tests.rs |
| 1 | 0 | kazoo-mouth/src/ui/tests.rs |
| 147 | 0 | kazoo-core/src/analysis/cycle/provisional.rs |
| 31 | 0 | kazoo-core/src/analysis/score/early_measurements.rs |
| 36 | 0 | kazoo-core/src/analysis/score/provisional_tests.rs |
| 25 | 0 | kazoo-core/src/voice/gate/tests.rs |

FOUND: `voice/api.rs:33`: the required fourth boolean conflicts with clippy::struct_excessive_bools. No lint bypass was added.
FOUND: `voice/profile/tests.rs:444`: its previous immediate-jump assertion conflicts with B; changed to expect confirmation, retaining the refractory assertion.
FOUND: `voice/profile/tests.rs:259`: sweep still emits nine events and unrelated pitches, including 790.5385 Hz after the C3 pluck.
FOUND: `analysis/score/metrics.rs:216`: settling previously hid provisional errors; repaired with a red/green test.
FOUND: HEAD is 276f4b0, merge-base with the instructed base is 8ed511d. No rebase or branch change was made.
FOUND: The brief's 45 minutes, lockf command and push wording conflict with STACK RULES; used 25 minutes, lane env prefix and no commit/push.
FOUND: Two mouth test literals required the new frame field; source-only updates, no mouth build authorized or run.
FOUND: Cargo reports existing future compiler incompatibilities for block 0.1.6 and proc-macro-error2 2.0.1; dependencies unchanged.

NOT CONFIRMED: All original targets; final whole-crate and stack-tip gates; final complete P06/P07/V03 matrix;
final fused latency and octave tables after score accounting; physical audio behavior; worker's 4.2 ms budget;
all downstream compilation; live profile routing; final continuous-motion rerun.

BLOCKED ON: The specified A/B mechanisms still miss the measured bass and upper-guitar deadlines and produce nine sweep onsets.
No tuning outside the brief was attempted. Clippy requires a representation decision beyond the mandated boolean shape.
The full crate could not complete inside the time box; its partial counts are not a pass.

If this code were used in a hospital, the worst credible failure is a missed or false alarm if these pitch events controlled alarm detection, and it could harm about one monitored person per tracker channel; deployment size is unknown.

Report artifact: briefs/studio/KZ-R04-measurements.md +156/-0.
