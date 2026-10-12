# KZ-A01c: play and 808 become libraries with `run(Args)`; play's output is a value a caller can read, not only stdout

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Split: PLAN.md's A01 is split into A01a to A01g (see KZ-A01a). This is c = kazoo-play and kazoo-808. Independent of the other six.

## Outcome

`kazoo-play` can be driven from another crate: `parse_args` gives an `Args`, `render(args)` gives the text it would print, `run(args)` prints it, and the exit codes a shell sees today (0, 1 for a write failure, 2 for anything else) are kept. `kazoo-808` can be started with `run(Args)`. Both `main.rs` files are one call. `kazoo-play --help` exits 0 (today it exits 2 through the error path, `kazoo-play/src/main.rs:133`).

## The design (decided; build exactly this)

### kazoo-play (four files: `lib.rs` new, `args.rs` new, `run.rs` new, `main.rs`)

`kazoo-play` has no color-eyre and keeps none. Its surface:

```rust
/// What the command line gave (the struct at kazoo-play/src/main.rs:29-39,
/// renamed from `Options`; this lane decides the rename).
#[derive(Debug, Clone, PartialEq)]
pub struct Args { pub bpm: f64, pub sample_rate: u32, pub channel: u8, pub format: OutputFormat, pub mode: Mode, pub notation: Option<String>, pub name: String, pub velocity: u8, pub drive: bool }
impl Default for Args  // main.rs:41-55
pub enum OutputFormat { Lines, Tsv }   // main.rs:58, now pub
pub enum Mode { Events, Play, Jam }     // main.rs:64, now pub
pub const USAGE: &str;                  // the text of help() at main.rs:470-475, as a const, with `kazoo play` on the first usage line and `kazoo-play` on a second
/// `Ok(None)` is help; `Err` is one sentence.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String>;
/// Why a run did not deliver, and the exit code a shell should see.
#[derive(Debug)]
pub enum Error {
    /// Bad notation, no device, the desk link, stdin: exit 2.
    Run(String),
    /// The output could not be written: exit 1 (silent when the reader went away).
    Output(std::io::Error),
}
impl Error { pub const fn exit_code(&self) -> u8 }   // Run 2, Output 1
impl fmt::Display for Error; impl std::error::Error for Error;
/// What `run` would print: events as text, or nothing for play and jam (which sound instead).
pub fn render(args: Args) -> Result<String, Error>;
/// Render and write to stdout, flushed.
pub fn run(args: Args) -> Result<(), Error>;
/// The standalone binary's entry.
pub fn cli(args: impl IntoIterator<Item = String>) -> std::process::ExitCode;
```

- `parse_args` is `main.rs:126-189` with two changes: `-h`/`--help` gives `Ok(None)`; the empty-stdin case moves out of parsing (it belongs to `render`). Everything else (every flag, `--`, positional notation joined by spaces, the velocity/bpm/rate checks) is unchanged, and the error strings are unchanged.
- `render` is `main.rs:96-124` (`run`) with `read_stdin` (:203-212) returning `Err(Error::Run("no notation: give it as arguments or on stdin"))` for empty input instead of the help text, and jam errors wrapped in `Error::Run`.
- `run`: `let output = render(args)?; write_output(&mut io::stdout().lock(), &output).map_err(Error::Output)` (`write_output` is `main.rs:91-94`).
- `cli`: `main.rs:70-87` reshaped: help writes `USAGE` with `write_all` + `flush` to stdout and returns `ExitCode::SUCCESS` (a failed write is `ExitCode::from(1)`); a parse error prints `kazoo-play: {why}` to stderr and returns 2; `run` errors: `Output` with `BrokenPipe` is silent and 1, other `Output` prints `kazoo-play: failed to write output: {err}` and 1, `Run` prints `kazoo-play: {why}` and 2.
- `kazoo-play/src/lib.rs`: crate doc from `main.rs:1-5`; `mod args; mod jam; mod run;`; `pub use args::{Args, Error, Mode, OutputFormat, USAGE, cli, parse_args}; pub use run::{render, run};`.
- `kazoo-play/src/args.rs`: the items above plus the tests `parses_play_mode`, `supports_tsv_and_channel` (if it only parses) and the new ones.
- `kazoo-play/src/run.rs`: `render`, `run`, `write_output`, `read_stdin`, `format_notation_error`, `format_events` and the formatting helpers, `play_events` (:218-258), `is_fatal` (:262), `build_stream` (:269), `RenderState` and the rest down to the tests, with the tests that exercise output (`parses_args_and_outputs_lines`, `reports_parse_errors`, `formats_every_event_kind`, `output_write_errors_are_returned`, `only_lost_devices_are_fatal`, `synth_helpers_are_bounded`) adapted to call `render`.
- `kazoo-play/src/main.rs`: `//! kazoo-play on its own: the same tool `kazoo play` runs.` then `fn main() -> std::process::ExitCode { kazoo_play::cli(std::env::args().skip(1)) }`.
- `jam.rs` is untouched except that `run_jam`'s `String` errors are wrapped by the caller.

### kazoo-808 (three files: `lib.rs`, `run.rs` new, `main.rs`)

- `kazoo-808/src/lib.rs` (exists: `pub mod sequencer; pub mod synth;`) gains `mod app; mod audio; mod run; mod ui;` and `pub use run::{Args, USAGE, cli, parse_args, run};`. Nothing under `app.rs`, `audio.rs`, `ui/` names a crate-root item from `main.rs` (they reach `crate::app::…`, `crate::audio::…`, `crate::synth::…`), so no re-export is needed.
- `kazoo-808/src/run.rs`: `main.rs:11-172` except `fn main`: `FRAME_POLL` (:28), `main`'s body (:31-93, line 31 left out) as `pub fn run(args: Args)` with `let Args = args;` first, `Tui` (:96), `setup_terminal`/`enter_screen`/`restore_terminal` (:100-134, untouched; A04b replaces them), `run_event_loop` (:137-172). Plus the uniform `Args` (unit), `USAGE`, `parse_args`, `cli` exactly as KZ-A01a specifies, with the usage text:

```
kazoo-808: TR-808 drum machine, all synthesis, no samples

usage: kazoo 808
       kazoo-808

It plugs into the kazoo-mix desk whenever one is running and follows its
tempo and play/stop; alone, it keeps its own. Press ? inside for the keys.
```

- `kazoo-808/src/main.rs`: the three-line form (install, `kazoo_808::cli(...)`).

## Not this lane

A01a, b, d to g; A02 (the device code in `play_events`, `jam::open_output` and 808's `main`); A03 (link); A04 (terminal); A06 (clap; A06 builds its own clap structs and converts them into these `Args`).

## Red tests first

`kazoo-play/src/args.rs`:

- `help_is_not_an_error`: `parse_args(["--help"])` and `parse_args(["-h"])` are `Ok(None)`.
- `defaults_are_the_documented_ones`: `parse_args([])` is `Ok(Some(Args::default()))` with bpm 120, rate 48000, channel 0, `Lines`, `Events`, name `kazoo-play`, velocity 100, drive false.
- `every_flag_lands`: `["--jam", "--drive", "--name", "bass", "--velocity", "90", "--bpm", "96", "--rate", "44100", "-c", "2", "--format", "tsv", "--", "c4/4", "e4/4"]` fills every field and joins the notation with a space.
- `exit_codes_follow_the_error_kind`: `Error::Run(..).exit_code() == 2`, `Error::Output(..).exit_code() == 1`.

`kazoo-play/src/run.rs`:

- `render_gives_the_event_lines` (the body of today's `parses_args_and_outputs_lines`, through `render`).
- `empty_notation_is_a_run_error_naming_stdin`: `render(Args { notation: None, .. })` with stdin not a terminal is awkward to drive; instead make `read_stdin` take `impl Read` and test it with `std::io::empty()`: `Err(Error::Run(..))` whose text contains `stdin`.

`kazoo-808/src/run.rs`: the three uniform tests of KZ-A01a (`help_asks_for_the_usage`, `an_unknown_argument_is_refused_by_name`, `usage_names_both_ways_to_start_it`, with `kazoo 808` and `kazoo-808`).

All of play's existing tests keep running (10 ), 808's 57 too.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-play -p kazoo-808
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-play -p kazoo-808 --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-play kazoo-808
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-play -p kazoo-808
```

## Definition of done

- [ ] `kazoo_play::{Args, Error, Mode, OutputFormat, USAGE, parse_args, render, run, cli}` and `kazoo_808::{Args, USAGE, parse_args, run, cli}` exist with the signatures above.
- [ ] `echo 'c4/4' | cargo run -p kazoo-play` prints the two event lines and exits 0; `cargo run -p kazoo-play -- --help` exits 0; `cargo run -p kazoo-play -- --bogus` exits 2; `cargo run -p kazoo-play | head -0` exits 1 silently (paste each exit code).
- [ ] Both `main.rs` are one call (808 with the install line).
- [ ] nextest: play at least 10 + 6, 808 at least 57 + 3 (real numbers).
- [ ] Every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: `print!`/`println!` never appear in play's library (a closed pipe must not panic); every stdout write is `write_all` + `flush` with the error returned.

## END

```
START KZ-A01c base=<sha> branch=lane/kz-a01c df=<GB> at <time>
END KZ-A01c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
