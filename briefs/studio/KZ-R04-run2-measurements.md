# KZ-R04 run 2 measurements

All five bass notes now meet the unchanged early limits. Guitar E2 remains at 16 ms.
The three new mechanism regressions failed first and pass. The sweep still fails its latency assertion.
Guitar A2, B3 and E4 still exceed their deadlines. This lane is not complete. No target or floor was changed.

START KZ-R04 base=276f4b0 branch=lane/kz-R04 head=6edb0c77a479ca9ea5216142b0283b78e16b94a7 df=67GiB load=24.73 at 2026-10-14 22:20:57 AEDT

## Retained mechanism changes

`analysis/cycle/provisional.rs:47` resets the available correlation history when the worker loses its estimate. The retained history previously included the old string at a new attack. At bass E1's second pluck, sample 81664 selected period 1554.9955, NSDF 0.906585, and published the previous 30.868 Hz note instead of 41.203 Hz. The old provisional reading then blocked another early search until normal acquisition.

`voice/gate.rs` consumes a distinct cycle-reading identity. Repeating a held 64-sample frame cannot confirm itself. Pending jumps expire after two configured judge hops and two distinct readings. Confirmed jumps keep the first reading's stamp.

The gate remembers an amplitude onset already fired. A later jump can use that timestamp within the specified 10 ms window, including while the onset refractory interval is active. This clears the D3/G3 publication delay without a second onset.

`analysis/cycle.rs:677` measures held clarity at the held lag, rather than climbing to another period when deciding whether the unrelated-candidate floor is 0.88 or 0.80. The held floor remains 0.70.

`voice/api.rs:44` groups gate and onset in `VoiceEvents`. Deref and DerefMut preserve flag reads and assignments. Frame literals use the group explicitly. The encoded frame remains 40 bytes with the same four flag bits. The nine wire tests pass, including all 16 flag combinations and the pinned old layout. Two mouth fixture literals were updated; no mouth Cargo check was authorized or run.

## Native latency table

Before is this run's unmodified Run 1 tree. After is the retained amplitude-history tree. These are sample-clock arrival measurements, not CPU benchmarks.

| take | note Hz | before ms | after ms | unchanged limit ms |
|---|---:|---:|---:|---:|
| bass_open | 30.86770 | 41.333 | 41.333 | 50.594 |
| bass_open | 41.20345 | 48.000 | 30.667 | 38.405 |
| bass_open | 55.00000 | 42.667 | 24.000 | 29.273 |
| bass_open | 73.41618 | 38.667 | 17.333 | 22.431 |
| bass_open | 97.99886 | 32.000 | 13.333 | 17.306 |
| guitar_open | 82.40690 | 16.000 | 16.000 | 20.202 |
| guitar_open | 110.00000 | 18.667 | 20.000 | 15.636 |
| guitar_open | 146.83237 | 12.000 | 10.667 | 12.216 |
| guitar_open | 195.99773 | 10.667 | 9.333 | 9.653 |
| guitar_open | 246.94162 | 12.000 | 12.000 | 8.074 |
| guitar_open | 329.62747 | 12.000 | 12.000 | 6.551 |

## Continuous motion

All eight motion rows passed on the retained amplitude-history tree, at the same latencies shown below. Receipt: run2-retained-focused.txt.

| profile and take | before ms | retained after ms |
|---|---:|---:|
| Guitar slide | 12.000 | 12.000 |
| Guitar bend | 8.000 | 8.000 |
| Guitar vibrato | 13.333 | 13.333 |
| Guitar glide | 12.000 | 12.000 |
| Bass slide | 12.000 | 12.000 |
| Bass bend | 8.000 | 8.000 |
| Bass vibrato | 12.000 | 12.000 |
| Bass glide | 12.000 | 12.000 |

## FOUND: per-note candidates and deciding rules

The complete before trace is `target/kz-logs/run2-candidates-before.txt`. The intermediate after trace is in `run2-final-focused.txt`. Large receipts are retained in numbered parts. Each candidate includes lag, NSDF, held status, floor, replacement decision and rule. Worker key maxima include the entire profile range. The diagnostic is test-only; production callbacks do not print.

| guitar note | best generated true-period candidate before deadline | deciding rule | first normal correct acceptance ms |
|---|---|---|---:|
| E2 | provisional period 582.4736, NSDF 0.682665 | long-period 0.60 provisional accepted | 16.000 provisional |
| A2 | period 436.3705, NSDF 0.700541 at 12.625 ms | below 0.80; held estimate prevents provisional search | 16.667 |
| D3 | period 326.8928, NSDF 0.804702 at 9.771 ms | 0.80 acquisition; recent amplitude onset now confirms publication | 9.771 |
| G3 | period 244.8998, NSDF 0.923107 at 8.896 ms | 0.80 acquisition; recent amplitude onset now confirms publication | 8.896 |
| B3 | period 194.3833, NSDF 0.785418 at 7.896 ms | below 0.80; period below 400 excluded from A | 11.729 |
| E4 | no generated true-period candidate before 6.551 ms | period below 400 excluded from A | 11.104 |

These measurements concern the existing candidate generator. They are not an impossibility proof for other pitch estimators. Expanding A to short periods or lowering acquisition floors would change the decided design, so neither was done.

| bass note | stale provisional period | stale NSDF | deciding rule before repair |
|---|---:|---:|---|
| B0, first | 1555.0234 | 0.918890 | correct provisional |
| E1 | 1554.9955 | 0.906585 | old history selected previous B0 |
| A1 | 1164.9574 | 0.846891 | old history selected previous E1 |
| D2 | 872.6713 | 0.741292 | old history selected previous A1 |
| G2 | 654.1966 | 0.775680 | old history selected previous D2 |

## FOUND: all nine baseline sweep onsets

| onset stamp | selected period | NSDF | fired rule |
|---:|---:|---:|---|
| 10176 | 436.3671 | 0.761239 | initial gate rise, correct A2 provisional |
| 14464 | 60.7181 | 0.837445 | unrelated 0.80 acquisition after unlock; gate reopened |
| 18304 | 73.4315 | 0.805937 | unrelated 0.80 acquisition after unlock; gate reopened |
| 21120 | 436.4302 | 0.615663 | old-string provisional; held frame falsely confirmed next 64-sample frame |
| 27264 | 53.6310 | 0.944634 | unrelated 0.80 acquisition after unlock; gate reopened |
| 35840 | 384.7939 | 0.875967 | held frame falsely confirmed at 35904 |
| 38656 | 59.1389 | 0.882390 | held frame falsely confirmed at 38720 |
| 41792 | 54.9103 | 0.947951 | unrelated acquisition and amplitude/gate rise |
| 44800 | 156.3275 | 0.814839 | held frame falsely confirmed at 44864 |

The final sweep has six events, but the C3 event is still the wrong 790.5385 Hz at 31.333 ms, exceeding 18.289 ms. Six events do not prove one timely onset for each of six notes. When the cycle estimate has unlocked, design B's held floor cannot reject an unrelated candidate at 0.80.

## Red receipts

| test | failing output | receipt |
|---|---|---|
| early_meets_its_targets | bass replucks and upper guitar late; fused bass octave figure 2.0873% | red-early_meets_its_targets-run2.txt |
| a_sweep_is_one_onset_per_note | nine onsets against six | red-a_sweep_is_one_onset_per_note-run2.txt |
| a_repluck_discards_the_previous_search_history | sample 81663, 30.868258 instead of 41.20345 Hz, error 499.96924 cents | red-a_repluck_discards_the_previous_search_history.txt |
| a_held_wrong_reading_cannot_confirm_itself | held reading confirmed at hop 66 | red-a_held_wrong_reading_cannot_confirm_itself.txt |
| a_recent_amplitude_onset_confirms_a_jump | published Some(110.0), expected Some(196.0) | red-a_recent_amplitude_onset_confirms_a_jump.txt |
| a_provisional_low_e_is_right, inherited Run 1 receipt | guitar_open published no provisional readings | red-a_provisional_low_e_is_right.txt |
| a_lone_wrong_reading_is_no_onset, inherited Run 1 receipt | injected unrelated reading fired onset | red-a_lone_wrong_reading_is_no_onset.txt |

The three new regression failures were saved before their respective production fixes. All three now pass. The two original acceptance targets remain unfulfilled.

## Commands and bounds

Every Cargo command used nice -n 10, CARGO_BUILD_JOBS=4, and lane-local NEXTEST_STATE_DIR. Every nextest run used -p kazoo-core -j 4 --no-fail-fast. No lockf, CARGO_TARGET_DIR, cargo clean, dependency change, install, commit, push, merge or branch switch was used. Formatter execution and workspace formatter checks were explicitly allowed.

| receipt | full summary or result | exit |
|---|---|---:|
| run2-red-baseline | 4 run, 2 passed, 2 failed, 1170 skipped | 100 |
| run2-candidates-before | 1 run, 1 passed, 1174 skipped | 0 |
| run2-red-regressions | 2 run, 0 passed, 2 failed, 1175 skipped | 100 |
| run2-after-focused | 6 run, 5 passed, 1 failed, 1171 skipped | 100 |
| red-a_recent_amplitude_onset_confirms_a_jump | 1 run, 0 passed, 1 failed, 1177 skipped | 100 |
| run2-full-nextest, stopped for new behavior | 88/1177 run, 84 passed, 4 SIGINT failures, 0 skipped; 1089 unrun | 100 |
| run2-red-clippy | struct_excessive_bools and diagnostic visibility errors | 101 |
| run2-clippy | frame 106 lines | 101 |
| run2-clippy-final | frame 101 lines | 101 |
| run2-clippy-complete | clean before amplitude history | 0 |
| run2-clippy-retained | gate method 102 lines | 101 |
| run2-clippy-last | clean after shared amplitude predicate | 0 |
| run2-fmt, run2-fmt-final, run2-fmt-complete | formatter execution | 0 |
| run2-fmt-check, run2-fmt-retained-check | formatter check | 0 |
| run2-ast-grep, run2-ast-grep-retained | kazoo-core and mouth literal fixtures | 0 |
| run2-doc | 2 doc tests passed, 0 failed | 0 |

FOUND: The Run 2 instruction to merge main conflicts with the overriding stack prohibition on merges and git metadata writes. No merge was attempted. origin/main read back as ef1c757; lane HEAD stayed 6edb0c7, base 276f4b0.

FOUND: The brief's 45 minutes, lockf commands, and push wording conflict with stack rules. This run follows the 25-minute limit, nice/env prefix, no commit and no push.

FOUND: Cargo retains future incompatibility warnings for block 0.1.6 and proc-macro-error2 2.0.1. Cargo.lock is unchanged.

NOT CONFIRMED: Both original targets together; a complete retained-tree kazoo-core pass; the pooled stack-tip gate; live audio devices; physical guitar/bass response; judge CPU budget; downstream mouth compilation.

BLOCKED ON: A2/B3/E4 candidate evidence does not meet the unchanged deadlines within A/B. Full-crate verification is subject to the explicit time box. No floor, take, budget or assertion was loosened.

If this code were used in a hospital, the worst credible failure is a missed or false alarm if these pitch events drove alarm detection, and it could harm about one monitored person per tracker channel; deployment size is unknown.

## Final checks and time-box stop

run2-final-focused: Summary [ 664.337s] 14 tests run: 12 passed (2 slow), 2 failed, 1163 skipped; nextest exit 100.
run2-retained-focused: Summary [ 377.604s] 18 tests run: 16 passed (1 slow), 2 failed, 1160 skipped; nextest exit 100.
run2-retained-full-nextest: Summary [ 377.227s] 80/1178 tests run: 76 passed (1 slow), 4 failed, 0 skipped; nextest exit 100.
run2-last-regressions: Summary [ 208.628s] 8 tests run: 7 passed (1 slow), 1 failed, 1170 skipped; nextest exit 100.

The retained full run ended with 76 passes and four interrupted tests, not four assertion failures. 1098 tests did not run. Its interrupted names were high_notes_hold_at_96k, high_notes_hold_through_the_engine_judge, high_splices_hold, and high_sung_thirds_hold. The retained focused early acceptance test was interrupted after 377.597 seconds. Its fused final table is not verified. The earlier focused early test was also interrupted; its summary is above.

Final source allocation checks passed: cycle processing, voice processing, quantiser processing, and every input profile. The earlier early-path allocation guard passed in the full run. Final continuous motion and all nine wire tests passed.

Clippy at end: run2-clippy-end.txt, exit 0. Formatter at end: run2-fmt-end-check.txt, exit 0. Ast-grep at end: run2-ast-grep-end.txt, exit 0. Git diff whitespace check: exit 0. No audio-device failure was observed among completed tests.

## Retained sweep latency table

Rows pair the six events with the six expected plucks exactly as the unchanged acceptance test does. Wrong pitches and late events are retained in the measurements. Before is the first later baseline event for each pluck; after is the corresponding retained event. Neither table establishes correct pitch arrival.

| truth Hz | before first-later ms | retained paired ms | retained reported Hz | limit ms |
|---:|---:|---:|---:|---:|
| 110.00000 | 12.000 | 12.000 | 109.99912 | 21.182 |
| 130.81277 | 31.333 | 31.333 | 790.53850 | 18.289 |
| 164.81380 | 41.333 | 41.333 | 653.67004 | 15.135 |
| 220.00000 | 30.000 | 35.333 | 110.06913 | 12.091 |
| 261.62555 | 88.000 | 85.333 | 109.89461 | 10.645 |
| 329.62747 | 18.000 | 352.667 | 110.31853 | 9.067 |

## Changed files

| added | removed | file |
|---:|---:|---|
| 31 | 3 | kazoo-core/src/analysis/cycle.rs |
| 23 | 0 | kazoo-core/src/analysis/cycle/provisional.rs |
| 39 | 0 | kazoo-core/src/analysis/score/early_measurements.rs |
| 4 | 2 | kazoo-core/src/engine/processing/voice.rs |
| 8 | 4 | kazoo-core/src/ipc/client/voice_tests.rs |
| 4 | 2 | kazoo-core/src/ipc/link/tests/voice.rs |
| 4 | 2 | kazoo-core/src/ipc/voice.rs |
| 8 | 4 | kazoo-core/src/ipc/voice/tests.rs |
| 23 | 4 | kazoo-core/src/voice/api.rs |
| 40 | 13 | kazoo-core/src/voice/gate.rs |
| 29 | 0 | kazoo-core/src/voice/gate/tests.rs |
| 3 | 1 | kazoo-core/src/voice/mod.rs |
| 1 | 0 | kazoo-core/src/voice/profile/tests.rs |
| 1 | 2 | kazoo-core/src/voice/quantise/tests.rs |
| 13 | 5 | kazoo-core/src/voice/tracker.rs |
| 4 | 2 | kazoo-mouth/src/synth/tests.rs |
| 4 | 2 | kazoo-mouth/src/ui/tests.rs |
| 13 | 0 | kazoo-core/src/analysis/cycle/diagnostics.rs |
| 29 | 0 | kazoo-core/src/analysis/cycle/provisional_tests.rs |

## Receipt parts

Receipts over 900 lines were divided into consecutive numbered parts. The original filename is part 1; all lines, including full summaries, are preserved.

run2-candidates-before.txt: 11939 lines preserved in 14 parts.
run2-final-focused.txt: 13319 lines preserved in 15 parts.

END KZ-R04 head=6edb0c77a479ca9ea5216142b0283b78e16b94a7 base=276f4b0 pushed=no df=52GiB load=190.37 at 2026-10-14 22:45:40 AEDT minutes=24.73
DONE: Definition 1 not met: sweep latency still fails; final early/fused acceptance and full crate are not verified. Three added regressions are red then green; inherited required new tests pass. Clippy, fmt and ast-grep exit 0.
DONE: Definition 2 native before/after tables cover all eight takes and both motion profiles. Final fused open-note score table was interrupted and is not verified.
BLOCKED ON: The remaining A2/B3/E4 and sweep deadlines require a mechanism beyond the specified A/B repair. The full crate and final early score did not finish within the time box. No uncommitted work was stashed, committed or pushed.
