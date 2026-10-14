# KZ-R02 fix run 5 measurements

14 October 2026. Branch `lane/kz-R02`, HEAD
`9764e817bce1a10779c6cacea67e1128a47ee903`, base `8ed511d`.
START: 19:15:28 AEDT, 37 GiB available, load 35.63. Changes remain
uncommitted and unpushed. No dependency or lockfile changes.

The retained change makes `voice_and_line_are_unchanged` finish. Its original
serial matrix ran two identical, synchronous pYIN/onset judge pipelines for
each of ten complete takes and each of two profiles: forty judge passes.
Early settings affect cycle tracking, not those judges. The test now records
the actual delivered verdicts and their sample times, then replays them into
an independent hard-off tracker. That reduces the matrix to twenty judge
passes. Four scoped workers process independent take/profile groups.

Verified by direct source comparison: the instrument acceptance code is byte-for-byte
unchanged from HEAD. Every original per-sample cycle comparison, unjudged frame comparison,
score field and event comparison remains. The replay additionally compares
every fused frame and each floating-point field's bits, and requires all
recorded verdicts and frames to be consumed. Takes, sample counts and
acceptance thresholds are unchanged. Recording exists only under `cfg(test)`
in the offline scoreboard adapter. No production DSP change is retained.

`voice_and_line_are_unchanged` passed in 206.454 s in
`target/kz-logs/run5-replay-and-latency.txt`. This is completion evidence,
not a controlled performance benchmark. Run 4's 494.774 s result was
interrupted and supplies no completed before time. A parallel-only attempt
in this run was also interrupted after 352.357 s when replaced by replay.
Its failure is recorded in `target/kz-logs/run5-unchanged.txt` and is not
counted as passing evidence.

The remaining three acceptance tests are not fixed. Their initial red
receipts are `target/kz-logs/red-early_meets_its_targets-run5.txt`,
`red-a_sweep_is_one_onset_per_note-run5.txt`, and
`red-slides_bends_and_vibrato_are_not_onsets-run5.txt`. Each preserves the
complete initial three-test output: zero passed, three failed, 1167 skipped,
exit 100. `red-voice_and_line_are_unchanged-run5.txt` is explicitly a copy
of run 4's interrupted whole-crate receipt, not a fresh red assertion.

The unchanged initial instrument arrival rows are below. Early off and on
produce the same arrivals. Six rows exceed P07's absolute target.

| take | note Hz | arrival ms | target ms |
|---|---:|---:|---:|
| bass_open | 30.86770 | 46.667 | 50.594 |
| bass_open | 41.20345 | 36.000 | 38.405 |
| bass_open | 55.00000 | 34.667 | 29.273 |
| bass_open | 73.41618 | 20.000 | 22.431 |
| bass_open | 97.99886 | 20.000 | 17.306 |
| guitar_open | 82.40690 | 29.333 | 20.202 |
| guitar_open | 110.00000 | 17.333 | 15.636 |
| guitar_open | 146.83237 | 10.667 | 12.216 |
| guitar_open | 195.99773 | 9.333 | 9.653 |
| guitar_open | 246.94162 | 12.000 | 8.074 |
| guitar_open | 329.62747 | 12.000 | 6.551 |

Sweep emits twelve onsets against six. The large pitch changes in the
polyphonic signal reach the two-frame jump rule in `voice/gate.rs:118`.
The first bass vibrato lock occurs at sample offset 785 from the pluck. The next
64-sample frame is sample 832: 17.333334 ms against 16.615047 ms. At sample
637, the normal mark's best clarity is 0.78147894, below its unchanged 0.80
acceptance floor. At sample 785 it accepts at 0.8259873. Receipt:
`target/kz-logs/run5-acquisition-evidence.txt`. These measurements identify
where the current implementation refuses; they do not prove that every
bounded estimator must refuse.

Rejected changes were removed completely:

- Quarter-period acquisition windows broke the moving E1's 30-cent initial
  accuracy and the handover step guard. Receipt: `run5-quarter-window.txt`.
- A wider normal acquisition climb retained the two P06 failures and six late
  P07 rows. Receipts: `run5-wider-acquisition.txt` and
  `run5-replay-and-latency.txt`.
- A derivative attack detector found fresh sweep plucks, but left nine onsets
  and created an extra onset at the slide's end. Receipt: `run5-pluck.txt`.
- Acquiring at armed zero crossings broke early coverage (0/33) and frame
  smoothness without fixing bass vibrato. Receipt: `run5-zero-crossing.txt`.
- Opening the bass search filter to 1500 Hz passed motion onsets, but delayed
  B0 to 444 ms and E1 to 90.667 ms. Its A1/G2 arrivals were 28/16 ms. Those
  latency trades are rejected. Receipt: `run5-open-bass-filter.txt`.
- Search cutoffs at 750 and 1000 Hz did not fix bass vibrato: 17.333 and
  18.667 ms respectively. Receipts: `run5-bass-cutoff.txt` and
  `run5-bass-double-cutoff.txt`.

Temporary diagnostic tests were removed. Their complete outputs remain in
`run5-acquisition-evidence.txt` and `run5-window-evidence.txt`.

Every cargo invocation used nice, four build jobs, the lane-local nextest
state directory and the existing target directory. Disk/load checks preceded
cargo invocations. No workspace build, lint, test, installation, live audio
check or benchmark ran. Doc tests did not run because no doc comments changed.

| receipt in target/kz-logs | run | passed | failed | skipped | exit |
|---|---:|---:|---:|---:|---:|
| red-run5.txt | 3 | 0 | 3 | 1167 | 100 |
| run5-unchanged.txt, interrupted replaced attempt | 1 | 0 | 1 | 1169 | 100 |
| run5-quarter-window.txt | 41 | 37 | 4 | 1129 | 100 |
| run5-wider-acquisition.txt | 41 | 39 | 2 | 1129 | 100 |
| run5-replay-and-latency.txt | 2 | 1 | 1 | 1168 | 100 |
| run5-pluck.txt | 3 | 1 | 2 | 1167 | 100 |
| run5-zero-crossing.txt | 41 | 37 | 4 | 1129 | 100 |
| run5-acquisition-evidence.txt | 1 | 1 | 0 | 1170 | 0 |
| run5-window-evidence.txt | 1 | 1 | 0 | 1170 | 0 |
| run5-open-bass-filter.txt | 42 | 39 | 3 | 1129 | 100 |
| run5-bass-cutoff.txt | 38 | 36 | 2 | 1132 | 100 |
| run5-bass-double-cutoff.txt | 38 | 36 | 2 | 1132 | 100 |

Clippy `-p kazoo-core --all-targets -- -D warnings` exited 0 in
`run5-clippy-final.txt`. The first clippy run exited 101 on two signed casts
in a temporary diagnostic test; that diagnostic module was removed.
Formatter execution (`run5-fmt.txt`), formatter check (`run5-fmt-check.txt`),
ast-grep scan of `kazoo-core` (`run5-ast-grep.txt`) and `git diff --check`
each exited 0.

V12 retains B1. Its prior measured cycle `splice_800` gross error was 2.56%
before versus 0% after, as recorded in `BENCH.md:948`. B2 was not built.
The final whole-crate run refreshes the high-note tables under
`target/kz-logs/kz-v12/`.

Not confirmed: all twelve acceptance tests passing; the 4.2 ms judge budget;
the early search's callback cost; physical low-B response; held singing and
whistling with the actual input/output devices. The final whole-crate result
is recorded below after the tool exits.

Initial instrument scoreboard, before the rejected experiments. The retained
production DSP is identical to this run. These are score measurements, not
wall-clock benchmarks. Receipt: `target/kz-logs/red-run5.txt`.

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| early off | bass_open | 46.67 | 46.67 | 0 | 27.33 | 36.00 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early on | bass_open | 46.67 | 46.67 | 0 | 27.33 | 36.00 | 4 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 1.19 | 0.00 | - |
| early off | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |
| early on | guitar_open | 29.33 | 29.33 | 0 | 12.00 | 17.33 | 5 | 0 | 0.01 | 0.04 | 0.34 | 0.26 | 0.93 | 0.00 | - |

Final whole-crate run was interrupted at the time-box boundary through its
owned exec session. Nextest exit 100. This is not completed crate verification.

Summary [ 622.696s] 99/1170 tests run: 95 passed (7 slow), 4 failed, 0 skipped

      SIGINT [  38.006s] (  96/1170) kazoo-core analysis::score::tests::the_judge_holds_its_floors
      SIGINT [ 323.240s] (  97/1170) kazoo-core analysis::score::tests::early::early_meets_its_targets
      SIGINT [ 309.574s] (  98/1170) kazoo-core analysis::score::tests::early::voice_and_line_are_unchanged
      SIGINT [  63.248s] (  99/1170) kazoo-core analysis::score::tests::the_fixtures_folder_may_be_empty
