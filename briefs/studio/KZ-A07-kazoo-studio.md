# KZ-A07: `kazoo studio up <name>` starts the desk and every instrument in a session, linked

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo`. Depends: KZ-A06.

## Outcome

Tom writes one file naming the instruments he wants, runs `kazoo studio up <name>`, and the desk comes up with every instrument in its own pane, all on the desk's strips. `kazoo studio down <name>` stops them.

## The design (decided; build exactly this)

- Session file `~/.kazoo/studio/<name>.toml`: `[desk] rate = 48000` (optional), `[[instrument]] name = "808"`, `args = ["--phrase", "x"]` (optional), repeated. Parsed with serde + toml (both already in the lockfile) into `StudioFile { desk: Desk, instrument: Vec<Instrument> }`; unknown keys are an error naming the key and line.
- `kazoo/src/studio.rs`: `up` requires tmux on PATH (decided: tmux because every instrument is a TUI needing its own terminal; if tmux is absent, print the exact commands one per line and exit 2). It runs `tmux new-session -d -s kazoo-<name> 'kazoo mix'`, waits up to 5 s for the desk socket (the path from `kazoo_core::ipc` default socket) to accept, then one `tmux split-window -t kazoo-<name> 'kazoo <instrument> <args>'` per instrument with `select-layout tiled`, then `tmux attach -t kazoo-<name>`. `down` runs `tmux kill-session -t kazoo-<name>`; the desk gets SIGTERM first so it closes links cleanly.
- Every child is the same `kazoo` binary (`std::env::current_exe()`), never a crate binary name.

## Not this lane

The subcommands themselves (KZ-A06). Session save/load inside the desk (KZ-C08).

## Red tests first

- `studio::tests::file_with_two_instruments_parses` and `unknown_key_is_an_error_naming_it`.
- `studio::tests::commands_are_built_in_order_desk_first` (the tmux command list is built as data and asserted; tmux is not run in tests).
- `studio::tests::no_tmux_prints_the_commands_and_exits_two` (PATH set to an empty tempdir).

## Definition of done

- [ ] Three red tests then green. `kazoo studio up demo` with 808 and mini listed shows both on desk strips (by hand; say what you saw).
- [ ] `down` leaves no `kazoo` processes (checked with `pgrep`).
