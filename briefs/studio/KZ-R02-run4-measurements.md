# KZ-R02 fix run 4 measurements

14 October 2026. Branch `lane/kz-R02`, HEAD `7eb9327529a456ab1ab8ee02cb441989eec50eca`,
base `8ed511d`. Changes are uncommitted. START: 14 GiB available,
load 32.60, 18:48:40 AEDT. No dependency or lockfile changes.

The retained attack detector applies only to instruments with the longer peak
release: the bass profile. A six-dB rise in the sample difference releases the
previous pitch and clears its marks before the old tracking filter delays the
new note. It adds one preallocated scalar; processing performs no allocation,
lock or I/O. Voice and line retain their cycle measurement paths.

Applying that detector to guitar too improved some arrivals but delayed D3 from
10.667 to 13.333 ms, over its 12.216 ms limit. That variant was removed.
`a_guitar_repluck_keeps_the_normal_path_latency` failed first, then passed
with the bass-only restriction. The original acceptance thresholds are unchanged.

The gate now applies its 60 ms refractory rule to reopening as well as level,
flux and pitch-jump onsets. This correction applies to every profile.
The reopening regression failed first; it and the voice tests then passed.
Sweep still produces 12 onsets, against exactly six. The initial count was 13.

Test 6 implements the amendment in `KZ-P07-low-notes-speak-sooner.md:115`.
It compares the early-enabled and normal sample clocks, requires a provisional
lock only when early wins, and retains the 10-cent handover limit. When normal
wins, the first two locked periods also retain the 10-cent frame-step limit.
No other acceptance target was relaxed. The normal-first E1 case passes.

Red receipts:
- `target/kz-logs/red-a_vibrato_note_stays_smooth_through_the_handover-run4.txt`:
  zero handovers against one before the amendment.
- `target/kz-logs/red-the_bass_profile_hears_low_b-run4.txt`:
  bass A1 42.667 ms, against 37.364 ms; the final bass acquisition test passes.
- `target/kz-logs/red-a_sweep_is_one_onset_per_note-run4.txt`: 13 against six.
- `target/kz-logs/red-slides_bends_and_vibrato_are_not_onsets-run4.txt`:
  bass vibrato onset remains 17.333334 ms, against 16.615047 ms.
- `target/kz-logs/red-early_meets_its_targets-run4.txt`: eight late note rows.
- `target/kz-logs/red-a_gate_reopening_respects_the_onset_refractory_time.txt`:
  reopening retriggered inside the refractory interval; now passes.
- `target/kz-logs/red-a_new_instrument_pluck_reopens_the_search.txt`:
  previous pitch retained after a fresh pluck; now passes.
- `target/kz-logs/red-a_guitar_repluck_keeps_the_normal_path_latency.txt`:
  the rejected all-instrument variant delayed D3; final restriction passes.

The early-search diagnostic is in `target/kz-logs/run4-diagnostics.txt`.
B0's first negative lobe proposes 149.694 samples against a 1555.02-sample
period and spends the one-shot search. At the guitar E2 deadline, true-period
half-window NSDF is 0.6702, below the fixed early floor of 0.90; even an early
search seeded with the true period refuses. The floor and one-shot cost bound
were retained. These measurements explain refusal; they do not prove that every
possible bounded estimator must refuse.

V12 retains B1. The previous measured cycle splice_800 gross error was 2.56%;
this run's `high_splices_hold` passes at 0%, with zero settled octave-error steps
in every row. Current full tables are in `target/kz-logs/kz-v12/`.
B2 was not built. No callback timing, judge benchmark or live audio check ran.
The 4.2 ms judge budget, physical low-B response and held mic notes remain
unverified.

The whole-crate run started before the bass-only restriction and final guitar
regression. It is evidence for that earlier implementation only. Final focused
receipts, counts and exit codes follow below; no whole-crate pass is claimed.

Check receipts:

| receipt in target/kz-logs | run | passed | failed | skipped | exit |
|---|---:|---:|---:|---:|---:|
| red-run4.txt | 5 | 0 | 5 | 1162 | 100 |
| run4-diagnostics.txt | 2 | 2 | 0 | 1166 | 0 |
| red-a_gate_reopening_respects_the_onset_refractory_time.txt | 2 | 1 | 1 | 1167 | 100 |
| run4-refractory.txt | 26 | 25 | 1 | 1143 | 100 |
| red-a_new_instrument_pluck_reopens_the_search.txt | 2 | 1 | 1 | 1168 | 100 |
| run4-pluck.txt | 37 | 33 | 4 | 1133 | 100 |
| run4-pluck-level.txt | 1 | 0 | 1 | 1169 | 100 |
| run4-attack.txt | 37 | 35 | 2 | 1133 | 100 |
| run4-final-measurements.txt | 5 | 4 | 1 | 1164 | 100 |
| red-a_guitar_repluck_keeps_the_normal_path_latency.txt | 1 | 0 | 1 | 1169 | 100 |
| run4-nextest-all.txt, interrupted earlier implementation | 1169 | 1165 | 4 | 0 | 100 |

The whole-crate run was interrupted after 720.448 s. Three acceptance failures
completed; `voice_and_line_are_unchanged` received SIGINT after 494.774 s.
Eight passing tests were slow. Both original voice scoreboards and every high
scoreboard passed in that run. It does not verify the final whole crate.

Clippy receipts `run4-clippy.txt`, `run4-clippy-final.txt` and
`run4-clippy-last.txt` each exited 0, including all targets with `-D warnings`.
Formatter runs and their checks (`run4-fmt*`) each exited 0. Ast-grep receipts
`run4-ast-grep*` each exited 0. Both doc runs (`run4-doc.txt` and
`run4-doc-last.txt`) exited 0: two passed, zero failed each. `git diff --check`
exited 0. The named crate was kazoo-core; no workspace build, test or lint ran.

Final production verification: `run4-final-nextest.txt` exited 100 after
134.348 s: 39 run, 36 passed, three failed, 1131 skipped. Nine of the twelve
original acceptance tests have passing receipts. Remaining failures are
`early_meets_its_targets`, `a_sweep_is_one_onset_per_note` and
`slides_bends_and_vibrato_are_not_onsets`. Allocation, nonsense, hum, motion,
original cycle and all three added regression guards pass in this focused run.
This subset does not prove a whole-crate pass.

Final instrument table, early off and on:

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| early off | bass_open | 46.67 | 46.67 | 0 | 27.33 | 36.00 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early on | bass_open | 46.67 | 46.67 | 0 | 27.33 | 36.00 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early off | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |
| early on | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |

Final note-arrival milliseconds (off and on are equal): bass B0 46.667,
E1 36.000, A1 34.667, D2 20.000, G2 20.000; guitar E2 29.333, A2 17.333,
D3 10.667, G3 9.333, B3 12.000, E4 12.000. The six P07 late rows remain
A1, G2, E2, A2, B3 and E4. Bass A1 is within the separate P06 37.364 ms limit.
No wall-clock judge or early-search cost benchmark ran. The final whole crate,
physical bass response and live singing/whistling remain unverified.
