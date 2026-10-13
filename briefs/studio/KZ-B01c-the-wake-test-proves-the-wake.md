# KZ-B01c: the link's wake test proves the wake itself, not a wall-clock figure the Mac's load decides

Line numbers are from `main` at the time of writing and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place: Track B, a defect after B01 and B01b. Written by Cassio, 13 Oct 2026, 23:3x.

Time: about 20 minutes. Crates: kazoo-core.

Depends: nothing in flight. If a lane editing `kazoo-core/src/ipc/link.rs` is open (L06, V07a, P14), merge line by line; this lane adds two counters and one classification after the park, nothing else.

Files (4): `kazoo-core/src/ipc/link.rs` (the counters and where the connected loop parks), `ipc/link/audio.rs` (the test-only switch beside `waker.unpark()`), `ipc/link/tests/wake.rs`, and the file that holds `LinkState` and `LinkConfig` (find them by name).

## Why

`a_queued_block_wakes_the_link_thread_at_once` (`ipc/link/tests/wake.rs`) asserts that the p50 time from `send_audio` to the frame arriving is under 200 µs. That figure is the scheduler's at the moment, not the link's: at load about 90 on this Mac it fails, and at load 125 it passed at 158.6 µs (B01b). By rule 8 a test that fails under load is a defect, never marked serial to hide it. What B01 must prove is the mechanism: a queued block unparks the link thread, and the thread serves it at once rather than at its next `LINK_TICK` (5 ms, `link.rs:88`). A send served by the tick waits 2.5 ms on average, so the two are told apart by how the park ended, not by a microsecond bound.

## The design (decided; build exactly this)

- `LinkState` gains `woken: AtomicU64` and `ticked: AtomicU64`, `Relaxed`, never reset while the link lives.
- In the connected branch of the link thread's loop (the `thread::park_timeout(LINK_TICK)` after `serve`), take `Instant::now()` before the park. After it returns, if the time parked is less than `LINK_TICK − WAKE_SLACK` (`WAKE_SLACK = 500 µs`, its reason in the doc: `park_timeout` never returns more than a scheduling delay early on a timeout, and half a millisecond is far past any measured one), add one to `woken`; otherwise add one to `ticked`. The idle branch (`IDLE_INTERVAL`) is not counted.
- `HubLink::wake_counts(&self) -> (u64, u64)` returns `(woken, ticked)`. `LinkStatus` does not gain them: they are for tests and diagnostics, and its doc comment says so.
- The test: the same 10 000 blocks. Before the loop, read the counts; after, read them again. Assert that at least 99% of the sends were followed by a woken park, that is `woken_after − woken_before ≥ 0.99 × BLOCKS`, and that `ticked_after − ticked_before ≤ 1% of BLOCKS + 2` (the two allow for a tick that falls during set-up and tear-down). Print p50, p99 and the counts in the test's `eprintln!` as the BENCH line, as today; the builder pairs each with the load average read beside the run. The 200 µs assertion is removed: its figure lives on as a printed measurement, not a pass mark.
- A red test first: with the audio side's unpark skipped through a test-only switch (`LinkConfig::for_test_without_wake()`, and a `bool` on the audio side's waker that exists only under `#[cfg(test)]`), the new assertion fails with the counts in the message. This is the proof that the test can see a lost wake.

## Not this lane

Any change to the wake mechanism, the link's priority (B01b) or `LINK_TICK`. The other three tests in `wake.rs` keep their bounds: each is against `LINK_TICK` or 1 ms, a tick's fraction, and none has failed under load. If one does during this lane's runs, name it under FOUND with the load average.

## Red tests first

1. `a_lost_wake_is_seen`: with `for_test_without_wake`, 200 blocks: the counts show at most 1% woken, and the new assertion's helper returns its failure.
2. `a_queued_block_wakes_the_link_thread_at_once`, rewritten as above, red before the counters exist (it does not compile), then green.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` and `sysctl -n vm.loadavg` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc
```

Run the wake test three times on its own (`cargo nextest run -p kazoo-core -E 'test(wake)'`), and paste each BENCH line with its load average.

## Definition of done

- [ ] The red test fails first (paste it), then every test passes. Every existing kazoo-core test passes.
- [ ] Three runs of the wake tests, each with its load average, all green.
- [ ] `WAKE_SLACK` has its reason in its doc comment.

## Code quality

As PLAN.md's STACK RULES. The two counters are plain atomics on the link thread; nothing is added to the audio side's production build (`send_audio` compiles to what it is today outside `#[cfg(test)]`). The test-only constructor exists only under `#[cfg(test)]`. `link.rs` is long: add only what is named.

## END

```
START KZ-B01c base=<sha> branch=lane/kz-B01c df=<GB> load=<1m> at <time>
END KZ-B01c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the three wake BENCH lines with load averages
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
