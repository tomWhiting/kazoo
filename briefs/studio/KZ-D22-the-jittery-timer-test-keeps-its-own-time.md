# KZ-D22: the jittery-timer test keeps its own time, so load cannot fail it

Written by Cassio, 17 Oct 2026, about 12:10. Decided by Cassio; the reasons are given with each decision.

Place: Track D, after KZ-D21. Crate: kazoo-mix (`hub/tests.rs`, and the hub's clock if it needs a seam).

## Why

The 0.2.0 gate (17 Oct 2026) failed `a_paced_source_with_a_jittery_timer_is_placed_once_and_stays_placed` once in 4908 tests: `resyncs` was 2, not 1. The test passed five times in five run alone. It plays the desk in real time on the wall clock, with a 256-frame buffer every 5⅓ ms, against a source that oversleeps 20 to 30 ms every tenth of a second. Under the full suite's load, the desk's own render loop oversleeps too, past what the pacing absorbs, so the test measures the machine and not the hub. Tom's rule is that a test which fails only under nextest is a defect to fix, never to mark serial.

## Outcome

- The test drives the desk's renders and the source's sends from one simulated clock. If the hub or the link reads `Instant::now()` directly, it takes the time from a clock the test owns instead.
- The jitter stays as it is now: a 20 to 30 ms oversleep every 100 ms of simulated time.
- The assertions stand: more than 200 blocks, and exactly one resync.
- It gives the same result under any load. Run it 50 times while the workspace suite runs, and it passes every time.

## Done when

clippy, nextest, fmt and ast-grep pass for kazoo-mix, and an Opus review finds nothing open.
