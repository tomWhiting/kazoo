# KZ-G03: every program enters the terminal through kazoo-host, so the audit that says so passes

Written by Cassio, 17 Oct 2026, about 10:50. Decided by Cassio; the reasons are given with each decision. Cassio wrote this lane directly, with no builder (memory `do-it-directly`).

Crates:
- kazoo-host: `terminal.rs`
- kazoo-tree: `lib.rs`, `Cargo.toml`
- kazoo-mouth: `run.rs`, `run/terminal.rs` renamed to `run/finish.rs`, `run/tests.rs`
- the `Options` literals in kazoo-arp, cs80, dx, glass, juno, mini, mix, prophet, string and wall

## Why

The full workspace gate of 17 Oct 2026 failed kazoo-host's `programs_use_one_terminal_and_report_host`. That test requires every program to enter and leave the terminal through `kazoo_host::terminal`, so that raw mode, the alternate screen, the mouse and the panic hook are put back the same way everywhere. Earlier gates ran only a few crates, so the failure had not shown. It flagged three things:
- kazoo-tree set up the terminal itself;
- kazoo-mouth had a helper file named `terminal.rs`;
- one of kazoo-mouth's tests wrote `DisableMouseCapture`.

## Outcome

- kazoo-host's `Options` gains `panics: Panics`.
  - `Panics::AnyThread`, the default, keeps today's behaviour: any panic puts the terminal back and is reported.
  - With `Panics::ThisThread`, only a panic on the thread that entered does that. A panic on another thread, while the terminal is entered, is neither restored for nor printed over the screen; the program catches it and says it.
  - That is what kazoo-tree did on its own: its reader thread's panics are caught and shown in the footer while the walk goes on.
  - Every other program passes `Panics::AnyThread`, so nothing changes for them.
- kazoo-tree enters through `kazoo_host::terminal::enter_with`, with the mouse and `Panics::ThisThread`. Its own `Screen`, panic hook and restore are gone. A terminal that refuses the mouse no longer stops the tree: the footer says so, and the keys still work.
- kazoo-mouth's helper is `run/finish.rs`. Its test checks that the terminal is left exactly once on every way out, and that a failure to leave is kept with the run's own failure. Releasing the mouse is now tested where it happens: kazoo-host's `leave_into` test checks that the mouse-off and alternate-screen-off codes are written once.

## Done when

clippy and nextest pass for the workspace, including `programs_use_one_terminal_and_report_host`, and an Opus review finds nothing open.
