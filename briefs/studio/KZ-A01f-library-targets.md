# KZ-A01f: kazoo-tui and kazoo-mcp gain `run(Args)`, with their tokio runtimes started inside it

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Part of the A01 split (see KZ-A01a); shares no files with A01a–e or A01g. Checks, code quality and the END block as PLAN.md with `-p kazoo-tui -p kazoo-mcp`, plus `cargo test --doc` for both (every public item gains a doc comment).

Shape the reviews of A01a and A01b enforced (match it from the start; read kazoo-cs80/src/run.rs and kazoo-dx/src/args.rs on main): `-h`/`--help` wins wherever it stands, so `--bogus --help` gives help and `--flag -h` never reads `-h` as the flag's value; USAGE's first line is the name, a colon, then the description joined with commas, with no dash and no second colon; `cli` writes the usage, flushes, and returns any write error; main.rs's doc line follows A01a's wording with no dash.

## Outcome

The mouth synth and the MCP seat can each be started from another crate with one call, `run(Args)`, exactly as they run today from their own binaries. Neither uses `#[tokio::main]` any more: each builds its runtime inside `run`, so the `kazoo` binary (A06) can choose a subcommand first and never carries a runtime it does not need.

## The design (decided; build exactly this)

The same public shape as KZ-A01a: `pub struct Args`, `pub const USAGE: &str`, `pub fn parse_args(args) -> Result<Option<Args>, String>`, `pub fn run(args: Args) -> color_eyre::Result<()>`, `pub fn cli(args) -> color_eyre::Result<()>`; `main.rs` becomes the crate doc line, `color_eyre::install()?` and one `cli` call.

### kazoo-tui

- `kazoo-tui/src/main.rs` carries `#[tokio::main]` (find it) and a module list; it has no `lib.rs`. New `kazoo-tui/src/lib.rs` with the module list (private except what `ui/` and tests already reach through `crate::`) and `mod run; pub use run::{Args, USAGE, cli, parse_args, run};`.
- `run.rs`: the body of the old async main becomes `async fn main_loop(args: Args) -> color_eyre::Result<()>`; `pub fn run(args: Args)` builds `tokio::runtime::Builder::new_multi_thread().enable_all().build()?` and `block_on(main_loop(args))`. The runtime is dropped at the end of `run` so a second instrument can run in the same process after it.
- `Args` is a unit struct today (tui takes no arguments: confirm by reading main; if it reads any, carry them as fields and parse them exactly as today).

### kazoo-mcp

- `kazoo-mcp/src/main.rs` parses `--seat`, `--notify-every`, `--socket` by hand (find `cli.rs` or the args loop) and runs under tokio. `Args { seat: String, notify_every: Option<Duration>, socket: Option<PathBuf> }` with the same defaults and the same error words as today. `run` builds the runtime as above and serves stdio until EOF.
- If kazoo-mcp already has a `lib.rs`, add `run.rs` beside it and re-export; otherwise create it.

## Not this lane

A06 (the `kazoo` binary), A04 (terminal host; tui's terminal setup stays where it is).

## Red tests first

- `help_asks_for_the_usage`, `an_unknown_argument_is_refused_by_name`, `usage_names_both_ways_to_start_it` in each crate's `run.rs` tests, as A01a.
- kazoo-mcp: `seat_and_socket_are_parsed_as_today` (the same strings the MCP config uses: `--seat Cassio`).
- `run_builds_its_own_runtime` (tui and mcp): calling `run` twice in one test process with a stand-in that returns at once does not panic ("a runtime is already running" would be the failure today).
- Every existing test in both crates still green.

## Definition of done

- [ ] No `#[tokio::main]` in either crate (`grep -rn "tokio::main"` empty).
- [ ] `main.rs` in both is three lines of logic; `lib.rs` exports the five items with doc comments.
- [ ] `~/.claude.json` is not touched (A06 repoints it); `kazoo-mcp --seat Cassio` still starts from the release binary (run it under `timeout 2` and paste stderr).
- [ ] All red tests then green; checks green.
