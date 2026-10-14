# KZ-R02: the tracker meets its targets

Written by Cassio, 14 Oct 2026, 14:0x. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-core`. Building is allowed in this lane.

Time: about 45 minutes. Crates: kazoo-core (`analysis/cycle*`, `analysis/score*`, `voice/*`). Depends: main at this lane's base (the build round, KZ-R01 and the looper and moments merges).

## Why

KZ-P06 (profiles tune the tracker), KZ-P07 (low notes speak sooner) and KZ-V12 (high notes hold) were written in a write-only round and never run. On main after the build round, the full run (`target/kz-logs/build-round-nextest-1.log` in the main checkout) has these twelve kazoo-core failures, each one an acceptance test of those briefs:

- `analysis::cycle::tests::a_refused_candidate_costs_nothing`, `a_low_b_speaks_in_a_period_and_a_half`, `a_note_that_starts_mid_glide`, `a_vibrato_note_stays_smooth_through_the_handover` (P07)
- `analysis::score::tests::early::early_meets_its_targets` (P07)
- `voice::profile::tests::hum_is_not_a_note`, `a_sweep_is_one_onset_per_note`, `slides_are_continuous`, `slides_bends_and_vibrato_are_not_onsets`, `the_bass_profile_hears_low_b`, `the_frames_follow_a_glide_and_a_vibrato_without_steps` (P06)
- `analysis::score::high_notes::high_splices_hold` (V12)

## What to do

1. Read `briefs/studio/KZ-P06-profiles-tune-the-tracker.md`, `KZ-P07-low-notes-speak-sooner.md` and `KZ-V12-high-notes-hold.md` first. Their designs and their numbers are decided; this lane makes the code meet them.
2. For each failure, find the cause in the code (run the test, read its output, measure) and fix the code so the brief's target holds. Never loosen a test's threshold, shorten its take, or change what it asserts to make it pass. A test that itself contradicts its brief is fixed to match the brief, and named under CHANGED with the brief's line.
3. V12: R01 tried V12's B1 mechanism and it brought settled octave errors at 2200 Hz in the full score, so it was removed. Follow V12's decision rule between B1 and B2 with measured numbers, and say which one you built and why.
4. Where two briefs' targets cannot both hold, stop on that pair and report both measured numbers under BLOCKED ON; do not trade one away silently.
5. Every other kazoo-core test stays green; `processing_never_allocates` and the score tables still pass.

## Checks

Read `df -g /` and `uptime` before each cargo command and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --no-fail-fast --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4
cargo fmt --all --check
ast-grep scan kazoo-core
```

While iterating, filter nextest to the failing tests; run the whole of kazoo-core at the end.

## Definition of done

- [ ] The twelve tests pass, each with its output.
- [ ] Every kazoo-core test passes, with the counts; clippy, doc, fmt and ast-grep each with an exit code.
- [ ] V12's B1 or B2 choice with its measured numbers.

## END

```
START KZ-R02 base=<sha> branch=lane/kz-R02 df=<GB> load=<1m> at <time>
END KZ-R02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each of the twelve tests' failing output at the base, and its cause
TESTS: nextest counts; clippy, doc, fmt and ast-grep each with their own exit code
BENCH: the score rows the briefs ask for, measured
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
