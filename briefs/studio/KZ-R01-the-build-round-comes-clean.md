# KZ-R01: the build round comes clean

Written by Cassio, 14 Oct 2026, 13:0x, on Tom's word: "can you build and install the latest?". Checks, code quality and the END block are as PLAN.md's STACK RULES, with the whole workspace as the `-p` list.

Time: about 40 minutes. Crates: every crate in the workspace. Depends: main at the head this lane starts from (the build round's merges).

## Why

The write-only round wrote sixteen lanes without compiling a line: V05b, P04, P10, D04, V12, V07a, V11, V08a, V08b, V08c, T03, T05a, L04, L05, P06 and P07. Cassio merged them into main and made the workspace compile (`cargo check --workspace --all-targets` is clean at this lane's base), fixed kazoo-core's clippy findings, and installed the build for Tom. What is left is to make the whole workspace pass every check and every test, without losing what any lane meant.

## What to do

1. Building is allowed in this lane: every cargo command runs under the kazoo cargo lock, niced, with four jobs, as in Checks.
2. `cargo clippy --workspace --all-targets --keep-going -- -D warnings` must be clean. At the base it reports about 80 findings, most in `kazoo-mouth` (pub(crate) inside private modules, missing semicolons, by-reference small arguments, `format!` appended to a String, similar binding names, float equality, a local array over 16 KiB, `bool::then` in `filter_map`, single-arm matches, too-long functions). Fix each one in the code, never with an allow, expect, `let _ =`, `.ok()` or an empty error arm. A function too long is split by what it does.
3. `cargo nextest run --workspace --no-fail-fast` must pass. Expect failures where lanes written apart now meet. One known case: `kazoo-core/src/engine/mod.rs`'s judge tests (`judge_config`'s device-rate tests) were kept on V06's `judge::spawn` framework with a one-window hop of 2048, while KZ-V12 made the judge's hop the voice judge's (512 at 48 kHz, `PitchDetectorConfig::for_rate`). Bring those tests to V12's design (one verdict per completed hop, the 50 to 4000 Hz range, `check_judge_range`) through `judge::spawn`; never weaken V12's design to make an old assertion pass. For every other failure, read the lane's brief in `briefs/studio/` and fix the code or the test so the brief's intent holds; when the two lanes' intents truly conflict, keep both behaviours if they can coexist, otherwise stop and say so under BLOCKED ON.
4. `cargo test --doc --workspace`, `cargo fmt --all --check` and `ast-grep scan` must pass.
5. Do not add features, rename public items or change behaviour beyond what a check or a test requires.

## Checks

Read `df -g /` and `sysctl -n vm.loadavg` before each cargo command and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy --workspace --all-targets --keep-going -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run --workspace --no-fail-fast --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc --workspace -j 4
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] clippy clean across the workspace, with its exit code.
- [ ] nextest: every test passes, with the counts.
- [ ] doc tests, fmt and ast-grep pass, each with its exit code.
- [ ] Every test changed is listed with why, and the brief it serves.

## END

```
START KZ-R01 base=<sha> branch=lane/kz-R01 df=<GB> load=<1m> at <time>
END KZ-R01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: none asked (a clean-up lane); list each failing test found and its cause
TESTS: nextest counts; clippy, doc, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
