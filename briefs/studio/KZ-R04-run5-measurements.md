# KZ-R04 run 5 evidence

The job is incomplete. The cancellation attempt produces six sweep onsets and no release onset, but four pitches miss 50 cents. Seventeen of twenty-two delivery arrivals meet their limits. Tuning stopped after the permitted Lagrange-3 attempt failed. No take, clarity floor, cents bound or arrival limit changed. All work remains uncommitted.

START KZ-R04 base=c511676 branch=lane/kz-R04 head=40d7c778a422bf0edc6c12cf6d81fc7b178d9a8a df=58GiB load=201.69 at 2026-10-15 08:43:34 AEDT

## Retained mechanism

Raw pre-attack history is captured independently of the residual. The exact preceding 64-sample frame supplies key maxima. Maxima below 0.60 are excluded, as are whole multiples or fractions within five cents of a stronger retained member. All remaining members get a comb. Gain is the least-squares per-period decay from the preceding two periods, clamped to [0.9, 1.0]. Fractional delays use Lagrange-3 after linear interpolation failed. Stage histories and search vectors are allocated at construction. The worker primes every stage with pre-attack input, replays the buffered attack samples, and searches the residual for forty milliseconds or until the next attack.

Worker normal readings retain their normal flag and can confirm a provisional reading during that window. The callback retains the worker reading between deliveries. The callback still processes raw input internally and can publish it if no worker reading is available; complete raw-path exclusion is not established (voice/tracker.rs:305-308).

## Allocation guard

No allocating call was found inside the guarded early path. The failure was the tried assertion: normal acquisition could lock before the pending early deadline, and the estimate-is-none condition then prevented that pending deadline from being evaluated. Pending scalar deadlines now finish inside the unchanged guard, without replacing an existing normal lock. Neither guard exclusions nor the test were changed. Both the original guard and the new residual-worker allocation guard pass.

## Sweep on the final source

Before is this run's untouched starting tree. Final values are identical in run5-lagrange.txt and run5-focused.txt. Delivery is independent of the backdated onset stamp.

| pluck | true Hz | before Hz | final Hz | final cents | final delivery ms | unchanged limit ms |
|---:|---:|---:|---:|---:|---:|---:|
| 9600 | 110.00000 | 110.01006 | 110.01023 | 0.161 | 12.020833 | 21.181818 |
| 12960 | 130.81277 | 683.00446 | 136.29094 | 71.023 | 10.145833 | 18.289027 |
| 16320 | 164.81380 | 1309.95117 | 168.55391 | 38.848 | 8.520833 | 15.134906 |
| 19680 | 220.00000 | 365.83273 | 99.76752 | 1369.034 | 13.270833 | 12.090909 |
| 23040 | 261.62555 | 335.03491 | 75.07395 | 2161.343 | 17.187500 | 10.644513 |
| 26400 | 329.62747 | 507.80081 | 86.08637 | 2324.376 | 15.104167 | 9.067455 |

Initial sweep: seven onsets including a release onset at 47801. Linear interpolation: six onsets, four wrong pitches. Lagrange-3: six onsets, four wrong pitches, no release onset. The 50-cent assertion remains unchanged.

## Ringing sets, combs and residual maxima

Periods are in samples; values following them are NSDF. Rows are captured at the first selected worker reading. When fewer than three local maxima exist, all of them are reported.

| pluck | detected attack | ringing P / NSDF | comb P / g | residual top three P / NSDF |
|---:|---:|---|---|---|
| 9600 | 9601 | empty | none | 436.32306 / 0.9997622 |
| 12960 | 12967 | 436.3617 / 0.9999679 | 436.3617 / 0.9941823 | 352.18777 / 0.67525256 |
| 16320 | 16345 | 392.22482 / 0.7776435 | 392.22482 / 0.9 | 284.77536 / 0.7527872 |
| 19680 | 19709 | empty | none | 481.11847 / 0.9945999; 213.5937 / 0.7377095 |
| 23040 | 23065 | empty | none | 639.36957 / 0.61068505; 386.0336 / 0.53809524; 215.78932 / 0.07870559 |
| 26400 | 26421 | empty | none | 559.15094 / 0.973253; 364.7642 / 0.3892831; 180.63107 / 0.29428524 |

C3's first residual reading arrives 480 samples after its detected attack, beyond its 436.3617-sample comb delay. The new string has entered the delayed term by then. This overlap is measured; its contribution to the 71.023-cent error was not isolated. Before the last three plucks, the preceding raw-frame scan finds no eligible ringing maxima. Before E3, its only retained period is 392.22482 samples, describing the mixture rather than either individual string.

## Delivery arrivals

Before values come from the latest completed run 4 table, which that report identifies as preceding its final snapshot correction. This run did not remeasure the untouched starting tree. Final values are native publication times from run5-early-focused.txt; backdated frame stamps are not delivery times. Five limits fail.

| take | Hz | run 4 latest ms | final delivery ms | limit ms | result |
|---|---:|---:|---:|---:|---|
| bass_open | 30.86770 | 42.020832 | 42.020832 | 50.59448 | pass |
| bass_open | 41.20345 | 32.687500 | 51.354168 | 38.40472 | FAIL |
| bass_open | 55.00000 | 24.687500 | 50.020832 | 29.27273 | FAIL |
| bass_open | 73.41618 | 18.041666 | 17.375000 | 22.43146 | pass |
| bass_open | 97.99886 | 14.020833 | 32.687500 | 17.30630 | FAIL |
| guitar_open | 82.40690 | 15.354167 | 15.354167 | 20.20236 | pass |
| guitar_open | 110.00000 | 15.354167 | 12.020833 | 15.63636 | pass |
| guitar_open | 146.83237 | 8.041667 | 17.375000 | 12.21573 | FAIL |
| guitar_open | 195.99773 | 6.687500 | 14.020833 | 9.65315 | FAIL |
| guitar_open | 246.94162 | 4.708334 | 4.708334 | 8.07431 | pass |
| guitar_open | 329.62747 | 4.020834 | 5.354166 | 6.55059 | pass |
| bass_open_noise | 30.86770 | 42.020832 | 42.020832 | 82.99080 | pass |
| bass_open_noise | 41.20345 | 31.354166 | 31.354166 | 62.67453 | pass |
| bass_open_noise | 55.00000 | 24.687500 | 24.687500 | 47.45454 | pass |
| bass_open_noise | 73.41618 | 17.354166 | 17.354166 | 36.05244 | pass |
| bass_open_noise | 97.99886 | 13.354167 | 13.354167 | 27.51050 | pass |
| guitar_open_noise | 82.40690 | 15.354167 | 15.354167 | 32.33727 | pass |
| guitar_open_noise | 110.00000 | 12.020833 | 12.020833 | 24.72727 | pass |
| guitar_open_noise | 146.83237 | 8.687500 | 8.687500 | 19.02622 | pass |
| guitar_open_noise | 195.99773 | 6.687500 | 6.687500 | 14.75525 | pass |
| guitar_open_noise | 246.94162 | 9.354167 | 11.354167 | 12.12385 | pass |
| guitar_open_noise | 329.62747 | 4.020834 | 4.020834 | 9.58432 | pass |

## Motion following

These rows measure pitch-following delay over moving segments after 200 ms, independently of onset stamps. A search over 0-50 ms in 32-sample steps minimizes squared cents error against delayed continuous truth. The reference is an early-disabled replay on this tree. Every row matches that reference. Each take produces one initial onset and no motion onset. The historical run 2 rows used initial onset latency; they are not a same-definition pitch-following reference. Historical following-delay preservation is therefore not verified.

| profile | take | early-disabled reference ms | final following ms |
|---|---|---:|---:|
| Guitar | slide | 3.333333 | 3.333333 |
| Guitar | bend | 2.000000 | 2.000000 |
| Guitar | vibrato | 3.333333 | 3.333333 |
| Guitar | glide | 7.333333 | 7.333333 |
| Bass | slide | 2.000000 | 2.000000 |
| Bass | bend | 0.666667 | 0.666667 |
| Bass | vibrato | 2.000000 | 2.000000 |
| Bass | glide | 7.333333 | 7.333333 |

The full early vibrato trajectory still fails: frames 10273 and 10305 change from 147.5684 to 147.8582 Hz beyond the allowed per-frame motion bound (voice/profile/tests.rs:389). The moving-segment table above does not override that failure.

The full arrival and motion tables were measured before the final equivalent extraction of worker_frame for the function-length lint. Those two matrices were not rerun after that extraction; the retained-tree sweep and both allocation guards were rerun.

## Commands and complete results

Every Cargo invocation used nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=/Users/tom/Developer/projects/deno_rust/kazoo/.lanes/R04/target/kz-logs/nextest cargo. All nextest commands ran sequentially. Final fmt-check was started while final clippy was still finishing, contrary to the sequential-Cargo rule. df -g / and uptime were read before each Cargo invocation. No target override, clean, full-core/full-mouth/workspace gate, dependency edit, installation, commit or push ran.

| receipt | command/result | exit |
|---|---|---:|
| red-a_sweep_is_one_onset_per_note-run5.txt | nextest: 2 run, 0 passed, 2 failed, 1196 skipped | 100 |
| red-the_early_path_never_allocates-run5.txt | same complete failing receipt, saved before edits | 100 |
| run5-first-green.txt | nextest compilation failed on Vec/Box history swap; fixed | 101 |
| run5-cancellation.txt | nextest linear attempt: 2 run, 1 passed, 1 failed, 1196 skipped | 100 |
| run5-lagrange.txt | nextest: 4 run, 3 passed, 1 failed, 1196 skipped | 100 |
| run5-focused.txt | nextest: 105 run, 102 passed (2 slow), 3 failed, 1096 skipped | 100 |
| run5-early-focused.txt | nextest: 6 run, 4 passed (3 slow), 1 assertion failed, 1 SIGINT, 1195 skipped | 100 |
| run5-fmt.txt | cargo fmt --all | 0 |
| run5-ast-grep.txt | ast-grep scan kazoo-core | 0 |

The first filter selected voice:: and analysis::cycle::. The early tests actually live under analysis::score::tests::early::; the second command selected that namespace. The first filter also selected IPC voice tests. Their union runs 111 distinct tests: 106 passed, four assertions failed, one interrupted. Neither focused gate passed.

Failed assertions: a_sweep_is_one_onset_per_note; the_frames_follow_a_glide_and_a_vibrato_without_steps; the_bass_profile_hears_low_b (settled 468.73685 Hz against 30.867702 Hz at frame 60725); a_provisional_low_e_is_right (provisional note truth missing at early_tests.rs:224). early_meets_its_targets was interrupted after 685.119 seconds. voice_and_line_are_unchanged passed in 459.571 seconds.

No doc comments changed, so doc tests did not run. No timing benchmark or physical audio-device test ran. The motion and arrival measurement tests print evidence; their own pass does not establish acceptance.

## Findings and limits

- The prescribed ringing set is empty at the last three sweep attacks; there is then no comb cancellation. No cap, threshold reduction or cents relaxation was applied (cycle/cancellation.rs:29).
- The callback raw estimate remains available before a worker reading arrives (voice/tracker.rs:307). Full residual-only publication before that first reading is not established.
- The current worker sends normal readings only after selecting a provisional reading (cycle/provisional.rs:119). The retained attempt is not a proof of every normal-acquisition path required by the decision.
- Existing dependency warnings name block 0.1.6 and proc-macro-error2 2.0.1 (Cargo.lock:287, :2635). Cargo.lock is unchanged.
- STACK RULES override commit/push, lockf, whole-crate gates, the older duration and conflicting file-splitting requirements. No git metadata write or branch change occurred.
- A read-only ps probe was denied with Operation not permitted. The owned nextest session was interrupted successfully through its tool handle. No process was killed by name.

NOT CONFIRMED: sweep acceptance; all arrival limits; the complete early acceptance result; historical same-definition motion delays; full raw-input exclusion; normal acquisition before provisional selection; complete core, mouth or stack-tip gates; real-time cost, physical audio behavior or performance budgets. The four assertion failures are failures, not unverified passes.

BLOCKED ON: the unchanged sweep pitch target remains red after both permitted fractional-delay methods. Tuning stopped under the explicit decision. No permission or product question is pending.

If this code were used in a hospital, the worst credible failure is a missed or false pitch-triggered audio alarm, and it could harm about one monitored person per tracker channel; deployment size is unknown.

## Final static and retained checks

Summary [   6.255s] 3 tests run: 2 passed, 1 failed, 1198 skipped; nextest exit 100.

Clippy first exited 101 on frame length; extracting only the new worker-reading branch fixed it. Final clippy exit 0; final formatter execution exit 0; fmt-check exit 0; final ast-grep exit 0. No lint bypass was added. Final receipt names: run5-clippy-final.txt, run5-fmt-final.txt, run5-fmt-check.txt, run5-ast-grep-final.txt, run5-retained.txt.

START KZ-R04 base=c511676 branch=lane/kz-R04 df=58GiB load=201.69 at 2026-10-15 08:43:34 AEDT
END KZ-R04 head=40d7c778a422bf0edc6c12cf6d81fc7b178d9a8a base=c511676 main=bb69a16ff612c5abe87233ad4da3af32b2c8d92c pushed=no at 2026-10-15 09:08:33 AEDT minutes=24.98
DONE: incomplete; six sweep onsets, four wrong pitches; 17/22 arrivals within limit; eight following rows equal early-disabled reference; both allocation guards and static checks pass. Historical following-delay preservation is not verified.
RED: red-a_sweep_is_one_onset_per_note-run5.txt: seven against six, wrong C3 683.00446 Hz; remains red. red-the_early_path_never_allocates-run5.txt: deadline tried assertion failed; unchanged guard is now green.
TESTS: complete counts and individual exits above; early_meets_its_targets interrupted, not passed.
BENCH: no benchmark; full native before/final sweep, 22 arrivals and eight motion rows above.
CHANGED:
kazoo-core/src/analysis/cycle.rs +3/-2
kazoo-core/src/analysis/cycle/early.rs +3/-0
kazoo-core/src/analysis/cycle/provisional.rs +84/-59
kazoo-core/src/analysis/cycle/provisional_tests.rs +45/-0
kazoo-core/src/analysis/score/early_measurements.rs +64/-0
kazoo-core/src/voice/tracker.rs +15/-3
kazoo-core/src/analysis/cycle/cancellation.rs +96/-0
briefs/studio/KZ-R04-run5-measurements.md +156/-0
FOUND: final fmt-check overlapped final clippy; ps denied; normal-before-provisional publication and raw fallback gaps are identified above; unchanged dependency warnings.
NOT CONFIRMED: complete early acceptance, historical same-definition motion comparison, post-helper arrival/motion reruns, complete residual-only normal publication, full suites, live audio and real-time cost.
BLOCKED ON: unchanged sweep pitch requirement after both permitted delay interpolations; stopped tuning. No Cargo process remains.
If this code were used in a hospital, the worst credible failure is a missed or false audio alarm, and it could harm about one monitored person per tracker channel; deployment size is unknown.
