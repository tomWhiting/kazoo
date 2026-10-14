# KZ-R04 run 4 evidence

The job is incomplete. No limit or take was loosened. All changes remain uncommitted.

START KZ-R04 base=c511676 branch=lane/kz-R04 head=a31209e df=73GiB load=189.70 at 2026-10-15 08:14:34 AEDT

## Latest completed arrival measurements

Receipt: run4-final-measurements.txt. These precede the final history snapshot and initial-gate corrections. They are not final-tree proof. Delivery time is measured independently of the backdated onset stamp.

| take | note | Hz | delivery ms | limit ms | first provisional sample | NSDF |
|---|---:|---:|---:|---:|---:|---:|
| bass_open | 0 | 30.86770 | 42.020832 | 50.59448 | 11585 | 0.99997365 |
| bass_open | 1 | 41.20345 | 32.687500 | 38.40472 | 83137 | 0.9999448 |
| bass_open | 2 | 55.00000 | 24.687500 | 29.27273 | 154753 | 0.99988395 |
| bass_open | 3 | 73.41618 | 18.041666 | 22.43146 | 226466 | 0.99985844 |
| bass_open | 4 | 97.99886 | 14.020833 | 17.30630 | 298273 | 0.99988586 |
| guitar_open | 0 | 82.40690 | 15.354167 | 20.20236 | 10337 | 0.99976224 |
| guitar_open | 1 | 110.00000 | 15.354167 | 15.63636 | 57729 | 0.99454737 |
| guitar_open | 2 | 146.83237 | 8.041667 | 12.21573 | 106018 | 0.99993694 |
| guitar_open | 3 | 195.99773 | 6.687500 | 9.65315 | 153921 | 0.99989843 |
| guitar_open | 4 | 246.94162 | 4.708334 | 8.07431 | 201858 | 0.99908364 |
| guitar_open | 5 | 329.62747 | 4.020834 | 6.55059 | 249793 | 0.99789035 |
| bass_open_noise | 0 | 30.86770 | 42.020832 | 82.99080 | 11585 | 0.9188074 |
| bass_open_noise | 1 | 41.20345 | 31.354166 | 62.67453 | 83073 | 0.921116 |
| bass_open_noise | 2 | 55.00000 | 24.687500 | 47.45454 | 154753 | 0.7960422 |
| bass_open_noise | 3 | 73.41618 | 17.354166 | 36.05244 | 226433 | 0.88844967 |
| bass_open_noise | 4 | 97.99886 | 13.354167 | 27.51050 | 298241 | 0.86486584 |
| guitar_open_noise | 0 | 82.40690 | 15.354167 | 32.33727 | 10337 | 0.6951467 |
| guitar_open_noise | 1 | 110.00000 | 12.020833 | 24.72727 | 58177 | 0.7984796 |
| guitar_open_noise | 2 | 146.83237 | 8.687500 | 19.02622 | 106017 | 0.91570836 |
| guitar_open_noise | 3 | 195.99773 | 6.687500 | 14.75525 | 153921 | 0.91266304 |
| guitar_open_noise | 4 | 246.94162 | 9.354167 | 12.12385 | 201665 | 0.659374 |
| guitar_open_noise | 5 | 329.62747 | 4.020834 | 9.58432 | 249793 | 0.80795306 |

The native frame stamp can precede publication. It must not be interpreted as delivery latency. The last completed table has 22 of 22 delivery arrivals within limit; it does not prove the retained tree.

## Latest completed sweep

Receipt: run4-final-measurements.txt. The pitch assertion failed. This precedes the final exact snapshot fix. No onset was published for pluck 16320; a further onset appeared at 47801.

| pluck | true Hz | published Hz | cents error |
|---:|---:|---:|---:|
| 9600 | 110.00000 | 110.01006 | 0.158 |
| 12960 | 130.81277 | 683.00446 | 2861.270 |
| 16320 | 164.81380 | missing | missing |
| 19680 | 220.00000 | 365.83273 | 880.417 |
| 23040 | 261.62555 | 335.03490 | 428.170 |
| 26400 | 329.62747 | 507.80080 | 748.111 |

## Motion

| profile | take | historical baseline ms | latest completed delivery ms |
|---|---|---:|---:|
| Guitar | slide | 12.000000 | 12.020833 (pre-fix) |
| Guitar | bend | 8.000000 | not verified |
| Guitar | vibrato | 13.333333 | not verified |
| Guitar | glide | 12.000000 | not verified |
| Bass | slide | 12.000000 | not verified |
| Bass | bend | 8.000000 | not verified |
| Bass | vibrato | 12.000000 | not verified |
| Bass | glide | 12.000000 | not verified |

Baselines come from briefs/studio/KZ-R04-run2-measurements.md. The latest completed motion run failed on zero Guitar bend onsets and stopped before the other six rows. The initial normal-reading path was then repaired, but the motion matrix did not rerun on that retained tree.

## Judge cost

At 48 kHz the normal 64-sample cadence is 750 steps/s. Sixteen disjoint 20 ms attack windows occupy 32% of a second. The bounded cadence becomes 990 steps/s, +240 steps/s or +32%. Total CPU time is not measured: reconstruction also adds one complete previous-frame NSDF scan per attack. No timing benchmark or live-device check ran.

## Red

red-a_sweep_is_one_onset_per_note-run4.txt: nextest exit 100; 1 run, 0 passed, 1 failed, 1197 skipped. Pluck 12960: published 109.96763 Hz against 130.81277 Hz, 300.509 cents. There is no green sweep receipt.

## Commands and complete results

Every cargo command used nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=/Users/tom/Developer/projects/deno_rust/kazoo/.lanes/R04/target/kz-logs/nextest cargo. Commands were sequential. df -g / and uptime were read before each cargo launch. No target override, clean, full-workspace build/test/clippy, dependency edit, commit or push ran.

| receipt | command/result | exit |
|---|---|---:|
| red-a_sweep_is_one_onset_per_note-run4.txt | Summary [   5.661s] 1 test run: 0 passed, 1 failed, 1197 skipped | 100 |
| run4-focused.txt | Compilation failed; no tests ran. | 101 |
| run4-focused-02.txt | Summary [ 207.392s] 6 tests run: 4 passed (1 slow), 2 failed, 1192 skipped | 100 |
| run4-measurements.txt | Summary [ 108.222s] 10 tests run: 8 passed (2 slow), 2 failed, 1188 skipped | 100 |
| run4-final-measurements.txt | Summary [  94.215s] 4 tests run: 1 passed (1 slow), 3 failed, 1194 skipped | 100 |
| run4-mouth-full.txt | Summary [ 223.015s] 133 tests run: 130 passed (2 slow), 3 failed, 0 skipped | 100 |
| run4-fmt.txt | cargo fmt --all | 0 |
| run4-fmt-02.txt | cargo fmt --all | 0 |
| run4-fmt-03.txt | cargo fmt --all | 0 |
| run4-fmt-final.txt | cargo fmt --all | 0 |
| run4-fmt-check.txt | cargo fmt --all --check | 0 |
| run4-fmt-retained.txt | cargo fmt --all | 0 |
| run4-fmt-last.txt | cargo fmt --all | 0 |
| run4-clippy.txt | core + mouth all-target clippy: private visibility, extra boolean and missing semicolon errors; fixed | 101 |
| run4-clippy-final.txt | core + mouth all-target clippy before final clock correction | 0 |
| run4-clippy-retained.txt | core + mouth all-target clippy including exact snapshot buffer | 0 |
| run4-doc.txt | core 2 doctests passed; mouth 0; before last snapshot correction | 0 |
| run4-ast-grep-last.txt | ast-grep scan kazoo-core | 0 |

No interrupted suite is a pass. Mouth interrupted: engine::fx::tests::render_with_every_slot_on_never_allocates; score::tests::score_reads_a_take; score::tests::unreadable_takes_report_an_error_and_scoring_continues.

## Found and not confirmed

- The strongest novel key maximum can still be unrelated to the new string in a short post-attack window; the sweep pitch test exposes it.
- The final history snapshot, first normal-reading gate repair, allocation guards, final motion matrix, complete core suite, complete mouth suite and final fused score are not proven by a complete runtime gate.
- Existing future compiler incompatibility warnings name block 0.1.6 and proc-macro-error2 2.0.1 (Cargo.lock:287 and :2635). No dependencies changed.
- STACK RULES override standing commit/push, 45-minute duration, lockf and file-splitting instructions. No forbidden git write or branch change occurred.

If this code were used in a hospital, the worst credible failure is a missed or false pitch-triggered audio alarm, and it could harm about one monitored person per tracker channel; deployment size is unknown.

## Final core result

Summary [  48.804s] 77/1198 tests run: 72 passed, 5 failed, 0 skipped; exit 100 (interrupted).

      SIGINT [  12.575s] (  74/1198) kazoo-core analysis::score::high_notes::high_notes_hold_through_the_engine_judge
      SIGINT [  22.034s] (  75/1198) kazoo-core analysis::pitch::tests::estimates_come_every_hop
      SIGINT [  17.725s] (  76/1198) kazoo-core analysis::pitch::tests::reset_forgets_the_voice
      SIGINT [  14.109s] (  77/1198) kazoo-core analysis::score::high_notes::high_notes_hold_at_96k

END KZ-R04 head=a31209ed0c657e1f5f4ae5009e59eb0b106e8901 base=c511676 main=bb69a16ff612c5abe87233ad4da3af32b2c8d92c pushed=no at 2026-10-15 08:39:34 AEDT minutes=25.00

## Required handback corrections

The retained core tree has one assertion failure: analysis::cycle::tests::the_early_path_never_allocates failed at kazoo-core/src/analysis/cycle/tests.rs:659 with "the early deadline was evaluated inside the allocation guard". This is not an allocation violation claim. Baseline status for this guard was not verified; preservation of all Run 3 green behavior is not confirmed.

Core: 77/1198 run, 72 passed, 1 assertion failed, 4 SIGINT, 1121 unrun, 0 skipped; exit 100. Mouth: 133 run, 130 passed, 3 SIGINT, 0 skipped; exit 100. Neither suite passed.

run4-fmt-check-retained.txt: cargo fmt --all --check, exit 0. Ast-grep and git diff --check also exited 0 on the retained tree. No running cargo command remains. Final receipts were scanned for secret-like strings; no files matched.
