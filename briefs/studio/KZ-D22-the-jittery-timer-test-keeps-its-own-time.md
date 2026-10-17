# KZ-D22: tests that time the studio run their timing threads real-time, as the studio does

Written by Cassio, 17 Oct 2026, about 12:10, and revised about 13:30 after the first Opus review: the first plan, a simulated clock, became this one (see Decision).

Place: Track D, after KZ-D21. Crates: kazoo-mix (`hub/tests.rs`, `hub/tests/wake.rs`) and kazoo-core (`ipc/link/tests/wake.rs`), tests only.

## Why

The 0.2.0 gate (17 Oct 2026) failed `a_paced_source_with_a_jittery_timer_is_placed_once_and_stays_placed` once in 4908 tests: `resyncs` was 2, not 1. The test passed five times in five runs alone. It plays the desk in real time on the wall clock, with a 256-frame buffer every 5⅓ ms, against a source that oversleeps 20 to 30 ms every tenth of a second. Both loops ran at normal priority, while the hub thread and the link's worker run real-time. Under the suite's load a normal-priority loop can be held off the CPU, so the test measured the machine as well as the hub. The control below shows the load sensitivity; it does not show which loop overslept. Tom's rule is that a test which fails only under nextest is a defect to fix, never to mark serial.

Three more tests time the studio from a thread at normal priority, with deadlines a lost scheduling quantum (about 10 ms on macOS) can break:
- the link's `a_ui_request_is_carried_without_waiting_for_the_tick` (each round trip under 1 ms);
- the link's `an_idle_link_still_reads_the_hubs_messages_within_a_tick` (within two 5 ms link ticks, waiting in a spin);
- the hub's `a_transport_anchor_still_reaches_an_instrument_within_the_tick_budget` (within 50 ms, waiting in a yield loop).

## Outcome

- **Each timing thread at real-time priority, as its studio counterpart is.** A test that times the studio schedules its own timing thread real-time, through `kazoo_core::realtime::RealTime`:
  - the jittery-timer test's source, with the wall render thread's own period: four 256-frame blocks a wake, 21⅓ ms at 48 kHz;
  - its desk loop, with one buffer's period, 5⅓ ms;
  - the instrument's or the hub's side in the other three.
- **Promotion is required.**
  - On macOS any process may have the time-constraint policy.
  - On Linux the process needs `RLIMIT_RTPRIO` above 0 or `CAP_SYS_NICE`. Without it, the test stops and says so rather than measure the scheduler. A test that ran unpromoted would bring the load sensitivity back.
- **Nothing allocates while promoted,** as `RealTime`'s contract asks:
  - the source's block is made before it promotes;
  - the desk loop renders into a buffer made beforehand (`Rig::render_into`).
- **The promoted loops wait in short sleeps,** not spins or yields, so none uses more of its period than macOS allows.
- **The source always stops.** A guard stops it however the test ends, a panic included, so it never keeps polling at real-time priority.
- **The jitter test is otherwise unchanged:** its deliberate 20 to 30 ms oversleep every 100 ms stays, and so do its assertions, more than 200 blocks and exactly one resync. The other three keep their deadlines.

## Decision

The first plan was a simulated clock. That would need a seam through the hub's poll loop, the link's worker and its socket, three threads and two crates, to fake what production does with priority. The failure was a normal-priority loop under load, so the fix gives each timing thread the priority its production counterpart has:
- the wall's render thread (`kazoo-wall/src/daemon/audio.rs`);
- the hub thread;
- the link's worker;
- the desk's CoreAudio callback.

## Proof (17 Oct 2026, about 13:35 to 13:55, on Tom's Mac)

Each test ran 50 times while a workspace nextest ran beside it. That suite holds dozens of real-time hub and link threads, and finished 4924 of 4926, with only the R04 pair failing. Each run was the test binary alone, `--exact`.

| Test | This lane | Main (control) |
|---|---|---|
| jittery timer | 50/50 | 49/50 |
| transport anchor | 50/50 | 50/50 |
| UI request | 50/50 | 50/50 |
| idle link | 50/50 | 50/50 |

The jitter test's control failed as the gate did. The other three's controls did not fail in this run, so for them the change is the same fix applied before they fail, not a failure reproduced and cured.

An earlier, weaker run (about 13:20) used 10 cores held busy by normal-priority load. The jitter test passed 20 of 20 alone and 20 of 20 under that load, against 17 of 20 for the old test.

## Done when

clippy, nextest, fmt and ast-grep pass for kazoo-mix, and an Opus review finds nothing open.
