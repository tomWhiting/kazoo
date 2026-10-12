# KZ-A06: one `kazoo` binary with clap subcommands for the desk, the wall, the seat and every instrument

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

From A01g's review: `kazoo_wall::run(Args::Console)` builds its `ServeCommand` from the current executable with `serve`, so the single `kazoo` binary must not call it for the console (the daemon would start as `kazoo serve`). Call `kazoo_wall::tui::run` with `ServeCommand { program: current_exe, args: ["wall", "serve"] }` as this brief says, and add a doc sentence on `kazoo_wall::run` saying so.

Depends on A01a to A01g (every crate exposes `run(Args)`), A02, A03, A04 (`kazoo_host::reports::install` installs error reports once), A05.

## Outcome

`kazoo mix`, `kazoo wall` (the console), `kazoo wall serve`, `kazoo wall stop`, `kazoo mcp --seat Cassio`, `kazoo 808`, `kazoo 303`, `kazoo cs80`, `kazoo mini`, `kazoo juno`, `kazoo prophet`, `kazoo dx --phrase "..."`, `kazoo string`, `kazoo glass --demo x.wav`, `kazoo arp`, `kazoo play ...` and `kazoo mouth` (the voice synth, kazoo-tui) all run from one binary, `target/release/kazoo`, with `kazoo --help` listing them and `kazoo <name> --help` giving each one's flags. The wall's console restarts a missing daemon as `kazoo wall serve`. The fourteen per-crate binaries still build (the mcp tests spawn `kazoo-mcp` as a child process, `kazoo-mcp/tests/seat.rs:115`), but every usage text and README now names the `kazoo` form first. `kazoo studio` is A07's.

## The design (decided; build exactly this)

### The crate: directory `kazoo-cli`, package `kazoo`, binary `kazoo`

`Cargo.toml` adds `"kazoo-cli"` to the workspace members. `kazoo-cli/Cargo.toml`:

```toml
[package]
name = "kazoo"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
description = "The kazoo studio in one command: the desk, the wall, the seat and every instrument"

[lints]
workspace = true

[[bin]]
name = "kazoo"
path = "src/main.rs"

[dependencies]
clap = { version = "4.5", features = ["derive", "env"] }
color-eyre = "0.6.5"
kazoo-host = { path = "../kazoo-host" }
kazoo-mix = { path = "../kazoo-mix" }
kazoo-wall = { path = "../kazoo-wall" }
kazoo-mcp = { path = "../kazoo-mcp" }
kazoo-808 = { path = "../kazoo-808" }
kazoo-303 = { path = "../kazoo-303" }
kazoo-cs80 = { path = "../kazoo-cs80" }
kazoo-mini = { path = "../kazoo-mini" }
kazoo-juno = { path = "../kazoo-juno" }
kazoo-prophet = { path = "../kazoo-prophet" }
kazoo-dx = { path = "../kazoo-dx" }
kazoo-string = { path = "../kazoo-string" }
kazoo-glass = { path = "../kazoo-glass" }
kazoo-arp = { path = "../kazoo-arp" }
kazoo-play = { path = "../kazoo-play" }
kazoo-tui = { path = "../kazoo-tui" }
```

(The directory is `kazoo-cli` because the repository root is already named `kazoo`; the package and the binary are `kazoo`, so `cargo run -p kazoo` and `target/release/kazoo` both read right.)

### `kazoo-cli/src/cli.rs`: the clap 4 derive layout (and nothing else: no I/O)

```rust
/// The kazoo studio: the desk, the wall, the seat and every instrument, in one command.
#[derive(Debug, Parser)]
#[command(name = "kazoo", version, about, long_about = None, propagate_version = true)]
pub struct Cli { #[command(subcommand)] pub command: Command }

#[derive(Debug, Subcommand, PartialEq)]
pub enum Command {
    /// The desk: every instrument plugs into a strip and follows its tempo and play/stop.
    Mix(MixArgs),
    /// The wall: the shared modular synth that never stops (the console, unless told otherwise).
    Wall { #[command(subcommand)] command: Option<WallCommand> },
    /// The wall for a Claude seat: an MCP server on stdio.
    Mcp(McpArgs),
    /// TR-808 drum machine.
    #[command(name = "808")] EightOhEight,
    /// TB-303 acid bass.
    #[command(name = "303")] ThreeOhThree,
    /// Yamaha CS-80 pad synth.
    Cs80,
    /// Minimoog bass and lead.
    Mini,
    /// Juno chorus polysynth.
    Juno,
    /// Prophet-5 polysynth.
    Prophet,
    /// Four-operator FM synth.
    Dx(PhraseArgs),
    /// Karplus-Strong plucked string.
    String(PhraseArgs),
    /// Modal bell and glass synth.
    Glass(GlassArgs),
    /// Jupiter-8 style arpeggiator.
    Arp,
    /// Text notation to events or sound.
    Play(PlayArgs),
    /// The mouth: the voice-driven synth (kazoo-tui).
    Mouth,
}

#[derive(Debug, Args, PartialEq)] pub struct MixArgs { /// Put a built-in 808 pattern on strip 1. #[arg(long)] pub demo: bool }
#[derive(Debug, Subcommand, PartialEq)] pub enum WallCommand {
    /// Run the wall until stopped (Ctrl-C, SIGTERM, or a console).
    Serve(ServeArgs),
    /// Ask the running wall to stop.
    Stop,
    /// Open the console, starting the wall if it is not playing (the default).
    Console,
}
#[derive(Debug, Args, PartialEq)] pub struct ServeArgs {
    /// Render on a timer with no audio device (the desk still hears the wall).
    #[arg(long)] pub no_audio: bool,
    /// Never join the kazoo-mix desk.
    #[arg(long)] pub no_desk: bool,
    /// Sample rate: with a device, switch it to this; with --no-audio, the rate to render at (default 48000).
    #[arg(long, value_parser = clap::value_parser!(u32).range(8_000..=384_000))] pub rate: Option<u32>,
}
#[derive(Debug, Args, PartialEq)] pub struct McpArgs {
    /// Who this seat is on the wall: 1 to 24 of A-Z a-z 0-9 space _ . -
    #[arg(long, env = "KAZOO_SEAT")] pub seat: String,
    /// News of other seats' changes comes at most once per this many seconds.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u64).range(1..=3_600))] pub notify_every: u64,
    /// The wall's control socket (default: kazoo-wall.sock in kazoo's runtime directory).
    #[arg(long)] pub socket: Option<PathBuf>,
}
#[derive(Debug, Args, PartialEq)] pub struct PhraseArgs { /// A looped kazoo-play phrase, as notation. #[arg(long)] pub phrase: Option<String> }
#[derive(Debug, Args, PartialEq)] pub struct GlassArgs { /// Render the preview to this WAV and exit. #[arg(long)] pub demo: Option<PathBuf>, #[arg(long)] pub phrase: Option<String> }
#[derive(Debug, Args, PartialEq)] pub struct PlayArgs {
    #[arg(long, group = "mode")] pub play: bool, #[arg(long, group = "mode")] pub events: bool, #[arg(long, group = "mode")] pub jam: bool,
    #[arg(long)] pub drive: bool,
    #[arg(long, default_value = "kazoo-play")] pub name: String,
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u8).range(0..=127))] pub velocity: u8,
    #[arg(long, default_value_t = 120.0)] pub bpm: f64,
    #[arg(long = "sample-rate", visible_alias = "rate", default_value_t = 48_000)] pub sample_rate: u32,
    #[arg(short = 'c', long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=15))] pub channel: u8,
    #[arg(long, value_enum, default_value_t = Format::Lines)] pub format: Format,
    /// Notation; with none, stdin is read.
    #[arg(trailing_var_arg = true)] pub notation: Vec<String>,
}
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)] pub enum Format { Lines, Tsv }
```

Conversions, in `cli.rs`, each a plain `impl From<…> for <crate>::Args` (or a `fn` where the target is not a struct): `MixArgs` to `kazoo_mix::Args { demo }`; `PhraseArgs` to `kazoo_dx::Args`/`kazoo_string::Args`; `GlassArgs` to `kazoo_glass::Args`; `PlayArgs` to `kazoo_play::Args` (mode: `jam` then `play` else `Events`; notation `None` when the vector is empty, else joined by one space; bpm checked finite and positive here with a clap `value_parser` function, as `kazoo-play/src/main.rs:152` does); `McpArgs` to `kazoo_mcp::cli::Options` (seat validated with `kazoo_wall::protocol::valid_name`, as `kazoo-mcp/src/cli.rs:100` does, the refusal in the same words; socket defaulted with `kazoo_wall::paths::socket_path`); `ServeArgs` to `kazoo_wall::daemon::DaemonConfig` exactly as `kazoo-wall/src/main.rs:60-95` builds it (`DaemonConfig::standard()?`, `desk = DeskMode::Off` on `--no-desk`, `audio = Headless { sample_rate: rate.unwrap_or(48_000) }` or `Device { sample_rate: rate }`). The unit `Args` crates need no conversion.

### `kazoo-cli/src/main.rs`: dispatch, no runtime at the top

```rust
fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(err) = kazoo_host::reports::install() { eprintln!("kazoo: {err}"); return ExitCode::FAILURE; }
    match cli.command { ... }
}
```

Each arm calls the crate's `run` and maps its result: a `color_eyre::Result<()>` error prints `kazoo: {report:?}` to stderr and returns `ExitCode::FAILURE`; `kazoo_play::Error` prints as `kazoo-play` does and returns `ExitCode::from(err.exit_code())`; `kazoo_mcp::run`'s `String` error prints `kazoo: {why}` and returns `FAILURE`; `kazoo_wall::daemon::serve` and `stop_daemon` map as `kazoo-wall/src/main.rs:96-116` do; `Wall { command: None | Some(Console) }` calls `kazoo_wall::tui::run(serve)` with `serve = ServeCommand { program: std::env::current_exe()?, args: ["wall", "serve"] }` (the console's launch value from A01g: `kazoo_wall::daemon::ServeCommand`, which `launch_detached` spawns as `program args…`;  the console ran `current_exe() serve` at `kazoo-wall/src/daemon/mod.rs:649` and `kazoo-wall/src/tui/mod.rs:159`). The tokio runtimes start inside `kazoo_tui::run` (mouth) and `kazoo_mcp::run` (mcp) only (A01f); `main` is synchronous and never builds one, so the two cannot nest.

Stdout discipline: `kazoo mcp` speaks MCP on stdout, so the dispatcher prints nothing to stdout itself (clap's help and version go to stdout only when asked for).

### Usage texts and documents that name separate binaries

- `kazoo-mix` `USAGE` (`kazoo-mix/src/run.rs`, from `main.rs:35-47`) and the header line at `main.rs:210-213` ("run kazoo-tui, kazoo-808, …"): say `kazoo 808`, `kazoo mini`, `kazoo cs80`, `kazoo dx`, `kazoo arp`, `kazoo mouth`.
- `kazoo-mcp/src/cli.rs:19-33` `USAGE`: `usage: kazoo mcp --seat <Name> …` first, `kazoo-mcp` second; last line `It never starts the wall: run `kazoo wall` for that.`; `kazoo-mcp/tests/seat.rs:28` `NOT_RUNNING` and the matching text in `kazoo-mcp/src/link.rs` (find `start it with`): ``start it with `kazoo wall` ``.
- `kazoo-wall/src/main.rs:20-38` `USAGE`: a `kazoo wall [serve|stop]` form first; `kazoo-wall/src/tui/mod.rs:132-134` (``kazoo-wall` opens it again, `kazoo-wall stop` stops it`): `kazoo wall` / `kazoo wall stop`; `kazoo-wall/src/tui/worker.rs:342` (``run `kazoo-wall serve``): `kazoo wall serve`; `kazoo-wall/src/daemon/mod.rs:604` client name `"kazoo-wall stop"` stays (it is a protocol name, not a usage).
- `kazoo-mix/README.md:9-22` and `kazoo-mcp/README.md:9-20`: the `kazoo …` commands, built once with `cargo build --release -p kazoo`.
- Every per-crate `USAGE` from A01 already shows `kazoo <name>` first.

### After landing (Cassio, in the main checkout; not the builder, who works only in the lane checkout)

1. `nice -n 10 env CARGO_BUILD_JOBS=4 cargo build --release -p kazoo`.
2. `ln -sfn ../../release/kazoo target/tom/release/kazoo` next to the existing `target/tom/release/kazoo-mcp -> ../../release/kazoo-mcp`.
3. In `~/.claude.json`, the kazoo project's `mcpServers.kazoo`: `"command": "/Users/tom/Developer/projects/deno_rust/kazoo/target/tom/release/kazoo"`, `"args": ["mcp", "--seat", "Cassio"]`.

The builder writes these three lines, verbatim, under NOT CONFIRMED.

## Not this lane

A07 (`kazoo studio`), A08 (Justfile, CLAUDE.md, the top README); removing the per-crate binaries (they stay); any change to what an instrument does.

## Red tests first

`kazoo-cli/src/cli.rs` tests (clap parsing is pure; use `Cli::try_parse_from`):

- `every_instrument_is_a_subcommand`: for each of `["808", "303", "cs80", "mini", "juno", "prophet", "dx", "string", "glass", "arp", "play", "mouth", "mix", "wall", "mcp"]`, `try_parse_from(["kazoo", name, …minimal args…])` is `Ok` with the matching `Command` variant (mcp with `--seat Tom`).
- `wall_without_a_subcommand_is_the_console`: `["kazoo", "wall"]` gives `Wall { command: None }`; `["kazoo", "wall", "serve", "--no-audio", "--rate", "44100"]` converts to a `DaemonConfig` with `AudioMode::Headless { sample_rate: 44100 }`; `--rate 100` is refused.
- `play_flags_become_play_args`: `["kazoo", "play", "--jam", "--drive", "--name", "bass", "--velocity", "90", "--bpm", "96", "--rate", "44100", "-c", "2", "--format", "tsv", "c4/4", "e4/4"]` converts to the same `kazoo_play::Args` that `kazoo_play::parse_args` gives for the equivalent flags; `--play --jam` together is refused.
- `mcp_seat_comes_from_the_flag_or_the_environment_and_is_checked`: `--seat Waffles` gives `Options.seat == "Waffles"` with `notify_every` 20 s and the default socket; `--seat "bad/name"` is refused with the words `valid seat name`; `--notify-every 0` is refused.
- `dx_and_glass_take_their_phrases`: `["kazoo", "dx", "--phrase", "c4/8"]` converts to `kazoo_dx::Args { phrase: Some("c4/8") }`; `["kazoo", "glass", "--demo", "/tmp/p.wav"]` converts to `kazoo_glass::Args { demo: Some(…), phrase: None }`.
- `help_names_every_command`: `Cli::command().render_long_help().to_string()` contains each subcommand name once.
- `the_console_restarts_the_wall_as_kazoo_wall_serve`: the `ServeCommand` the dispatcher builds has `args == ["wall", "serve"]` (make the builder a pure `fn console_serve(program: PathBuf) -> ServeCommand` and test it).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo -p kazoo-wall -p kazoo-mcp -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo -p kazoo-wall -p kazoo-mcp -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-cli kazoo-wall kazoo-mcp kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo
```

## Definition of done

- [ ] `nice -n 10 env CARGO_BUILD_JOBS=4 cargo build --release -p kazoo` produces `target/release/kazoo`; `target/release/kazoo --help` lists the sixteen subcommands (paste it).
- [ ] `target/release/kazoo play 'c4/4'` prints the two event lines; `target/release/kazoo mcp --seat Tom --help` exits 0; `KAZOO_SEAT=Tom target/release/kazoo mcp` starts and says `seat Tom` on stderr (Ctrl-C it).
- [ ] `target/release/kazoo wall serve --no-audio --no-desk` runs headless (another terminal: `target/release/kazoo wall stop` stops it and says so).
- [ ] With no daemon running, `target/release/kazoo wall` starts one: `ps` shows a `kazoo wall serve` process; `~/.kazoo/wall/daemon.log` has its start line. Quit the console; `kazoo wall stop`.
- [ ] `target/release/kazoo mix` and, in another terminal, `target/release/kazoo 808`: the 808 appears on strip 1 (say so).
- [ ] `grep -rn 'kazoo-wall`\|run kazoo-tui\|kazoo-808 --release' kazoo-mix kazoo-mcp kazoo-wall/src` finds no usage text naming a separate binary first (paste the result; protocol names such as `"kazoo-wall stop"` are allowed and listed).
- [ ] The three after-landing lines under NOT CONFIRMED, verbatim.
- [ ] nextest counts; every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: `cli.rs` is pure (parsing and conversion, testable without a device); every exit code is explicit; `std::process::exit` appears nowhere; `main` never builds a tokio runtime.

## END

```
START KZ-A06 base=<sha> branch=lane/kz-a06 df=<GB> at <time>
END KZ-A06 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
