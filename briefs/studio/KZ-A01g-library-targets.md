# KZ-A01g: the wall's console, serve and stop become one `run(Args)` in the library

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Part of the A01 split (see KZ-A01a); shares no files with A01a–f. Checks, code quality and the END block as PLAN.md with `-p kazoo-wall`, plus `cargo test --doc -p kazoo-wall`.

## Outcome

`kazoo-wall` is already a library with a binary; after this lane the binary's argument handling (`console` by default, `serve [--no-audio --no-desk --rate <hz>]`, `stop`) lives in the library as `Args` and `run(Args)`, so `kazoo wall serve` (A06) is one call, and the daemon's self-restart no longer assumes the binary is called `kazoo-wall`.

## The design (decided; build exactly this)

- `kazoo-wall/src/run.rs` (new) with the A01a shape. `pub enum Args { Console, Serve(ServeArgs), Stop }`, `pub struct ServeArgs { pub audio: bool, pub desk: bool, pub rate: Option<u32> }` with today's defaults (audio on, desk on, rate native). `parse_args` accepts exactly what `kazoo-wall/src/main.rs` accepts today (read its loop; keep every error sentence) and refuses anything else by name.
- `run(Args::Console)`, `run(Args::Serve(..))`, `run(Args::Stop)` call the functions `main.rs` calls today, moved into `run.rs` if they live in `main.rs`, otherwise called where they are.
- The daemon's self-restart (`kazoo-wall/src/daemon/mod.rs`, the `current_exe()` call followed by `serve`, about line 649): the arguments it passes become a value the library owns: `pub fn serve_command() -> Vec<OsString>`, which returns `["serve", ...]` when the running executable's file name is `kazoo-wall` and `["wall", "serve", ...]` otherwise (decided: the executable's name is the only thing that tells the two apart, and A06 keeps the file name `kazoo`). The restart passes the running serve's `--no-audio`, `--no-desk` and `--rate` through, as today.
- `main.rs` becomes the crate doc line, `color_eyre::install()?` and `kazoo_wall::cli(std::env::args().skip(1))`.

## Not this lane

The `kazoo` binary and the MCP config (A06). The console's commands, the daemon, the engine: untouched beyond the move.

## Red tests first

- `parse_args` tests: `no_arguments_is_the_console`, `serve_flags_are_parsed_with_their_defaults`, `stop_is_stop`, `an_unknown_argument_is_refused_by_name`, `help_asks_for_the_usage`.
- `serve_command_depends_on_the_executable_name` (pure: a function taking the file name as a parameter, called with `kazoo-wall` and `kazoo`).
- Every existing kazoo-wall test still green (753 at the sweep).

## Definition of done

- [ ] `kazoo-wall/src/lib.rs` exports `Args`, `ServeArgs`, `USAGE`, `parse_args`, `cli`, `run`, `serve_command` with doc comments; `main.rs` is three lines of logic.
- [ ] The live restart path still works: `kazoo-wall serve --no-audio` under `timeout 5`, then the console's restart command (find it) relaunches it (paste the daemon log lines).
- [ ] All red tests then green; checks green.
