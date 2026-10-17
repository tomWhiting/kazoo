# KZ-D22: the jittery-timer test keeps its own time, so load cannot fail it

Written by Cassio, 17 Oct 2026, about 12:10. Decided by Cassio; the reasons are given with each decision.

Place: Track D, after KZ-D21. Crate: kazoo-mix (`hub/tests.rs`, and the hub's clock if it needs a seam).

## Why

The 0.2.0 gate (17 Oct 2026) failed `a_paced_source_with_a_jittery_timer_is_placed_once_and_stays_placed` once in 4908 tests: `resyncs` was 2, not 1. The test passed five times in five run alone. It plays the desk in real time on the wall clock, with a 256-frame buffer every 5⅓ ms, against a source that oversleeps 20 to 30 ms every tenth of a second. Under the full suite's load, the desk's own render loop oversleeps too, past what the pacing absorbs, so the test measures the machine and not the hub. Tom's rule is that a test which fails only under nextest is a defect to fix, never to mark serial.

## Outcome

- The test's two timing loops run at real-time priority, through `kazoo_core::realtime::RealTime`:
  - the source, which stands in for the wall's render thread;
  - the desk loop, which stands in for the desk's audio callback.

  In the studio both of those run real-time, and the hub thread and the link's worker already ask for it.
- The jitter stays as it is now: a deliberate 20 to 30 ms oversleep every 100 ms. Load from the rest of the machine no longer adds to it, so the test measures the pacing and not the scheduler.
- The assertions stand: more than 200 blocks, and exactly one resync.

## Decision

The first plan was a simulated clock. That would need a seam through the hub's poll loop, the link's worker and its socket, three threads and two crates, to fake what production does with priority. The failure was a normal-priority loop starved by the suite's load, so the fix gives the test's loops the priority their production counterparts have.

## Proof (17 Oct 2026, about 13:20, on Tom's Mac)

- **Alone:** 20 of 20 passed.
- **Under load,** with all 10 cores held busy (`yes` on each): 20 of 20 passed.
- **Control:** under the same load, the test as it was on main (8ce4fd5) passed 17 of 20.

## Done when

clippy, nextest, fmt and ast-grep pass for kazoo-mix, and an Opus review finds nothing open.
