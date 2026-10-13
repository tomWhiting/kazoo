# KZ-A03c: a desk link that would not stop is said on the way out, where it can be seen, and the 808's badge that could never light goes

Line numbers are from `main` at the time of writing and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place: Track A, a follow-up to A03b (its review's finding 4). Written by Cassio, 13 Oct 2026, 23:4x.

Time: about 20 minutes. Crates: kazoo-core, and the ten programs that call `connect_or_direct`: kazoo-303, 808, arp, cs80, dx, glass, juno, mini, prophet, string (each a three-line edit).

Depends: nothing in flight. If a lane editing one of these `run.rs` files is open, merge line by line.

Files (12): `kazoo-core/src/host/link.rs` (one function), `kazoo-808/src/ui/mod.rs` (the badge and its tests go), and the ten `run.rs` files above. That is over the ten-file guide: ten of them are the same three lines. Say so under CHANGED.

## Why

`kazoo_core::ipc::link::stuck_shutdowns()` counts desk links whose shutdown gave up after 500 ms with the thread still running, and `undelivered_completions()` counts those that finished later with no one to tell (`ipc/link/shutdown.rs`). A link only shuts down when its `HubLink` drops, which in every instrument is after the screen has gone (`kazoo-808/src/run.rs`: `terminal.leave()`, then `drop(stream)`, then `drop(hub)`). The 808's footer badge (`ui/mod.rs`, `shutdown_span`) reads the count while the screen is up, when it is always 0, so it can never show. The count is only true after the drop, and only standard error can carry it then. The wall daemon already logs both (`daemon/control/link_diagnostics.rs`) and is unchanged.

## The design (decided; build exactly this)

- `kazoo_core::host::link::exit_report() -> Option<String>`, beside `connect_or_direct`: `None` when both counts are 0; otherwise `"kazoo: <n> desk link(s) did not stop within 500 ms; <m> finished later"`, with the plural right for 1, and the second clause only when `m > 0`. Its doc says it is read after every `HubLink` this process made has dropped, and why.
- Each of the ten `run.rs`: after the line that drops the hub link (after the terminal is left), print `exit_report()` to standard error when it is `Some`. Where a `run.rs` does not drop the link explicitly after leaving the terminal, add the explicit `drop` in that order (leave the terminal, stop the stream, drop the link, report) and say which under CHANGED.
- `kazoo-808/src/ui/mod.rs`: `shutdown_span`, its call and `mod shutdown_tests` are removed.

## Not this lane

The link's shutdown itself, its 500 ms, and the wall daemon's diagnostics.

## Red tests first

1. `kazoo-core` `host::link` tests: `exit_report_is_silent_when_every_link_stopped`; `exit_report_counts_stuck_and_late_links` (using the existing test path that makes a stuck link, `ipc/link/tests/shutdown.rs`, and reading the report's text for 1 and for 2). The counts are process-wide and other tests may add to them, so assert on the difference from a reading taken first, through a helper that formats from given counts (`exit_report_from(stuck, late)`), which `exit_report` calls.
2. For one instrument (the 808), a test that `run`'s teardown order is leave, stream, link, report: extract the teardown into a function taking closures for each step, called by `run`, and assert the order with a recording stand-in.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core -p kazoo-303 -p kazoo-808 -p kazoo-arp -p kazoo-cs80 -p kazoo-dx -p kazoo-glass -p kazoo-juno -p kazoo-mini -p kazoo-prophet -p kazoo-string --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core -p kazoo-303 -p kazoo-808 -p kazoo-arp -p kazoo-cs80 -p kazoo-dx -p kazoo-glass -p kazoo-juno -p kazoo-mini -p kazoo-prophet -p kazoo-string --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] The red tests fail first (paste them), then every test in those crates passes.
- [ ] `rg 'exit_report' kazoo-*/src` shows the function, its tests and ten callers; `rg 'shutdown_span'` shows nothing.

## Code quality

As PLAN.md's STACK RULES. No `let _ =`, no `.ok()`: the report is printed or not, and nothing is discarded. Nothing here runs on an audio thread.

## END

```
START KZ-A03c base=<sha> branch=lane/kz-A03c df=<GB> at <time>
END KZ-A03c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
