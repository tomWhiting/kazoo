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
  - With `Panics::ThisThread`, only a panic on the thread that entered does that. For a panic on another thread, from entering until the terminal is wholly put back (raw mode off), the hook neither restores nor reports anything; the program must catch it on that thread and say it.
  - The owner is a token: each such entry takes a fresh one, and the entering thread keeps it in a thread-local. The hook takes no lock. The owner is cleared at the end of the restore, so a reader that panics while the screen is being left still writes nothing over it.
  - The hook's decision is a function of its own (`on_panic`), with its own test.
  - That is what kazoo-tree did on its own: its reader thread's panics are caught and shown in the footer while the walk goes on.
  - Every other program passes `Panics::AnyThread`, so nothing changes for them.
- kazoo-tree enters through `kazoo_host::terminal::enter_with`, with the mouse and `Panics::ThisThread`, and `walk` takes kazoo-host's `Terminal`. Its own `Screen`, panic hook and restore are gone.
  - A terminal that refuses the mouse no longer stops the tree: the footer says so, and the keys still work.
  - A capture that fails part way is still undone on leave. The mouse is marked as taken before capture is asked for, and turning off modes that were never on is harmless.
  - Both of the tree's other threads catch their panics and say them: the reader in the footer, as before, and now the go thread too, as the go's answer.
- kazoo-mouth's helper is now `leave` in `run.rs`, and the `run/terminal.rs` file is gone.
  - When two failures are combined, each keeps its whole chain of causes (`{:#}`).
  - Its test checks that the terminal is left exactly once on every way out, and that each failure's cause survives.
- Releasing the mouse and the keyboard flags is tested where it happens. kazoo-host's `leave_into` test checks that each is let go once and that the alternate screen is left on every call.
- **Versions.** Adding `panics` to `Options`, a struct callers build directly, breaks callers of the published kazoo-host 0.1.0. The pyin port's new public fields do the same to kazoo-core. So the next release, made by the release lane (`lane/kz-rel`) before anything is published from main, is 0.2.0 for every crate. npm is published at that version, never at 0.1.0 from a main that no longer matches tag v0.1.0.

## Done when

clippy and nextest pass for the workspace, including `programs_use_one_terminal_and_report_host`, and an Opus review finds nothing open.
