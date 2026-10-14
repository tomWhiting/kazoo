# KZ-R02 fix run 3 measurements

14 October 2026. Branch `lane/kz-R02`, HEAD `00b27fec716cc5cd9aae4e86cdcfbde14b7f9755`, base `8ed511d`. Changes are uncommitted. Start: 18 GiB available, load 106.87, 18:21:26 AEDT.

The amendment in `KZ-P07-low-notes-speak-sooner.md:108` changes tests 1 and 5. Test 1 compares early-off arrival against early-on arrival and checks the first frame is provisional if the early path wins. Test 5 accepts the normal path winning acquisition and retains the original 30-cent accuracy and frame-step limits.

Instrument NSDF windows now remove their own mean from raw input, while cycle marks still use the configured DC blocker and tracking filter. The recursive blocker's onset transient previously biased partial-cycle correlation: guitar glide readings alternated between 110.001, 110.898 and 109.930 Hz while truth rose from 110.219 to 110.375 Hz. Resampling the timestamped readings one period behind the sample clock removes the switch between held readings and extrapolation. Voice and line retain the original measurement path.

The early search now refits its local parabola twice and refuses an unbracketed local maximum. Five coarse evaluations, three local triplets and the half-period triplet total at most seventeen lag evaluations, below eighteen. No clarity or accuracy target changed.

Window-local DC rejection also applies to the gate's periodic level. The regression `dc_does_not_open_a_quiet_instrument` failed first with 676 gated frames on a -55 dBFS tone plus 0.2 DC. Receipt: `target/kz-logs/red-dc_does_not_open_a_quiet_instrument.txt`.

The initial twelve-test run completed: 3 passed, 9 failed, 1154 skipped, nextest exit 100. Receipt: `target/kz-logs/red-run3.txt`. Separate current-run red receipts use `red-<test>-run3.txt`; the earlier two runs' red receipts are retained.

The last focused instrument/cycle run before the gate-level correction completed: 34 run, 30 passed, 4 failed, 1133 skipped, exit 100. Receipt: `target/kz-logs/run3-centred.txt`. It verified the amended low-B and mid-glide tests, slide continuity, frame smoothness, refused-candidate behavior, hum rejection, allocation guards and the original cycle guards. It does not establish a crate-wide pass.

| measurement | before | focused result | unchanged target |
|---|---:|---:|---:|
| isolated B0 first frame, early on / off | 46.667 / 46.667 ms | 46.667 / 46.667 ms | early on <= 50.6 ms; off >= on |
| isolated E1 first frame, early on / off | initial run stopped at B0 assertion | 36 / 36 ms | early on <= 38.4 ms; off >= on |
| guitar glide frame error at sample 10560 | 9.069 cents | frame smoothness test passed | every voiced frame <= 5 cents within one period |
| guitar slide frame error at sample 57216 | 10.198 cents | continuity test passed | every voiced frame <= 10 cents within one period |
| sweep onset count | 10 | 13 | exactly 6 |
| bass A1 first correct frame | 42.667 ms | 42.667 ms | <= 37.364 ms (P06); <= 29.273 ms (P07) |
| bass vibrato onset | 17.333 ms | 17.333 ms | <= 16.615 ms |

The shaped E1 vibrato's normal path locks at sample 11341 (36.29 ms after its pluck), before the one-shot early deadline at sample 11438.59 (38.30 ms). The provisional path only runs while unlocked. Test 6 still requires exactly one provisional handover; the amendment changes only tests 1 and 5. No normal-path delay or test-6 relaxation was introduced. Receipt: `target/kz-logs/red-dc_does_not_open_a_quiet_instrument.txt`, diagnostic output.

The open-note P07 latency rows before this run's fixes are below. The full run repeated all eleven latency rows unchanged; its early scoreboard test completed and failed. Final instrument rows are in `target/kz-logs/run3-instrument-table.md`.

| take | note Hz | early off / on ms | target ms |
|---|---:|---:|---:|
| bass_open | 30.86770 | 46.667 / 46.667 | 50.594 |
| bass_open | 41.20345 | 48 / 48 | 38.405 |
| bass_open | 55 | 42.667 / 42.667 | 29.273 |
| bass_open | 73.41618 | 49.333 / 49.333 | 22.431 |
| bass_open | 97.99886 | 30.667 / 30.667 | 17.306 |
| guitar_open | 82.40690 | 29.333 / 29.333 | 20.202 |
| guitar_open | 110 | 17.333 / 17.333 | 15.636 |
| guitar_open | 146.83237 | 10.667 / 10.667 | 12.216 |
| guitar_open | 195.99773 | 9.333 / 9.333 | 9.653 |
| guitar_open | 246.94162 | 12 / 12 | 8.074 |
| guitar_open | 329.62747 | 12 / 12 | 6.551 |

V12 retains B1. This run's initial `high_splices_hold` passed. The B1 decision and prior measured cycle `splice_800` gross error, 2.56% before versus 0% after, are recorded in `BENCH.md:948`. This run changes neither the full-range voice cycle path nor fusion. B2 was not built.

No wall-clock judge benchmark or live audio test was run in this fix run. The earlier load-affected judge measurement does not prove the 4.2 ms budget. Low-B following on a physical bass and held singing/whistling into the mouth remain unverified.

Final production-code verification uses `target/kz-logs/run3-nextest-all.txt`. After it started, two test-only expressions were changed for clippy (`get_or_insert_with` and `mul_add`); the adjusted low-B and DC regression tests were rerun in `run3-final-lint-test-check.txt`. The production implementation was unchanged after the full run started.

Clippy `--all-targets -- -D warnings` exited 0 (`run3-clippy-final.txt`). Doc tests exited 0: two passed, none failed (`run3-doc.txt`). Formatter and ast-grep exited 0 (`run3-fmt-final.txt`, `run3-ast-grep-final.txt`). The first clippy run exited 101 on two test-expression lints; both were corrected without lint suppression.

The full run's V12 splice table was refreshed in `target/kz-logs/kz-v12/high_splices_hold.md`: cycle/voice median cents are 3.33/3.32 at 800 Hz, 3.16/3.20 at 1100 Hz, 2.78/2.72 at 1500 Hz, and 2.54/2.54 at 2200 Hz. Gross error is 0/0%, 0/0%, 0.16/0.18%, and 0.11/0.13%, respectively. Every row has 0% octave errors and zero settled octave-error steps. `high_splices_hold` passed in the full run.

Final instrument score rows (early off and on):

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| early off | bass_open | 46.67 | 46.67 | 0 | 45.33 | 49.33 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.11 | 0.00 | - |
| early on | bass_open | 46.67 | 46.67 | 0 | 45.33 | 49.33 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.11 | 0.00 | - |
| early off | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |
| early on | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |

Remaining failures: E1 handover (zero provisional handovers against one), P07 early latency (all final latencies above repeat the initial rows), sweep (13 onsets against six), bass vibrato onset (17.333 ms against 16.615 ms), and bass A1 acquisition (42.667 ms against 37.364 ms). The sweep count worsened from ten to thirteen; this is recorded, not accepted as satisfying P06.

At the time-box boundary, the remaining `voice_and_line_are_unchanged` process was interrupted through its owned exec session. Nextest returned exit 100: 1167 run, 1161 passed (9 slow), 6 failed, 0 skipped. Five are acceptance failures; the sixth is SIGINT after 591.760 seconds. This is not a completed whole-crate verification. All other tests finished, including both original scoreboards, B1 guards, every high-note row and allocation guards. The full implementation run preceded two test-only lint edits; both adjusted tests passed their final focused rerun.
