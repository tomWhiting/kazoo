# KZ-R03: the wall, the looper and the moments come clean

Written by Cassio, 14 Oct 2026, 14:0x. Checks, code quality and the END block are as PLAN.md's STACK RULES. Building is allowed in this lane.

Time: about 45 minutes. Crates: every crate except kazoo-core's `analysis/` and `voice/` (KZ-R02 owns those; do not edit them). Depends: main at this lane's base.

## Why

Main now holds the build round (KZ-R01 made it clippy-clean) plus two lanes written without compiling: KZ-V08d (the mouth's looper, kazoo-mouth) and KZ-T04a (the tree knows where the wall is, kazoo-wall). The merge of V08d took V08d's side in `kazoo-mouth/src/controls.rs` and `run.rs`, undoing some of R01's clippy fixes there. And the full run on main before those merges had four kazoo-wall failures:

- `daemon::wall::tests::loops::a_save_that_cannot_wait_says_so` (KZ-L04)
- `engine::tests::glide::gliding_allocates_nothing` (KZ-T05a)
- `tree::rings::tests::a_file_without_a_knob_at_its_default_agrees`, `a_real_walls_log_grows_to_the_wall_it_kept` (KZ-T03)

## What to do

1. Make the workspace compile with V08d and T04a in, then make `cargo clippy --workspace --all-targets --keep-going -- -D warnings` clean, fixing each finding in the code (never an allow, expect, `let _ =`, `.ok()` or an empty error arm; a long function is split by what it does).
2. Fix the four wall failures, and every failure V08d's and T04a's tests show, so each lane's brief holds (`briefs/studio/KZ-L04-takes.md`, `KZ-T05a-...`, `KZ-T03-...`, `KZ-V08d-...`, `KZ-T04a-...`). Never loosen a test to pass; a test that contradicts its brief is fixed to match the brief and named under CHANGED. `gliding_allocates_nothing` guards the audio thread: the fix is in the engine, not the test.
3. Failures in kazoo-core's `analysis::` and `voice::` tests are KZ-R02's; ignore them and do not edit those modules.
4. Do not add features, rename public items or change behaviour beyond what a check, a test or a brief requires.

## Checks

Read `df -g /` and `uptime` before each cargo command and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy --workspace --all-targets --keep-going -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run --workspace --no-fail-fast --build-jobs 4 -j 4 -E 'not (package(kazoo-core) & (test(/^analysis::/) | test(/^voice::/)))'
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc --workspace -j 4
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] clippy clean across the workspace, with its exit code.
- [ ] nextest (as filtered above): every test passes, with the counts.
- [ ] doc, fmt and ast-grep pass, each with its exit code.

## END

```
START KZ-R03 base=<sha> branch=lane/kz-R03 df=<GB> load=<1m> at <time>
END KZ-R03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each failing test found and its cause
TESTS: nextest counts; clippy, doc, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
