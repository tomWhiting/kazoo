# KZ-A01b: dx and string become libraries with `run(Args)`, `--help` no longer kills the process, and each main.rs is the install and one call

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Split: PLAN.md's A01 is split into A01a to A01g (see KZ-A01a for the list). This is b = kazoo-dx and kazoo-string. Independent of the other six.

## Outcome

`kazoo-dx` and `kazoo-string` can be started from another crate with `run(Args { phrase })` and behave as today. `--help` returns to the caller instead of calling `std::process::exit(0)` (`kazoo-dx/src/main.rs:219`, `kazoo-string/src/main.rs:222`), which would kill the whole `kazoo` process under A06. Each `main.rs` is the install and one call.

## The design (decided; build exactly this)

The uniform surface of KZ-A01a, with a real `Args`:

```rust
/// What the command line gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// A looped `kazoo-play` phrase to play on the transport, as notation.
    pub phrase: Option<String>,
}
pub const USAGE: &str;
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String>;
pub fn run(args: Args) -> color_eyre::Result<()>;
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()>;
```

`parse_args` keeps the grammar of `kazoo-dx/src/main.rs:203-227`: `--phrase <notation>` and `--phrase=<notation>` (the last wins), `-h`/`--help` anywhere gives `Ok(None)`, a missing notation after `--phrase` gives `Err("--phrase needs notation, e.g. --phrase \"c4/8 e4/8 g4/8 c5/8\"")`, anything else gives `Err(format!("unknown argument '{other}'"))`. `cli` is as in KZ-A01a (`write_all` of `USAGE` on help, `eyre!("{why}\n\n{USAGE}")` on an error, `run` otherwise). No `process::exit` remains in either crate.

`USAGE`, dx:

```
kazoo-dx: four-operator FM synth in the DX/TX81Z tradition

usage: kazoo dx [--phrase "<kazoo-play notation>"]
       kazoo-dx [--phrase "<kazoo-play notation>"]

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. A phrase loops on the transport. Press ? inside for the keys.
```

string: the same with `kazoo-string`, `kazoo string` and the description from `kazoo-string/Cargo.toml`.

Each `main.rs` becomes the three-line form of KZ-A01a (`//!` line, `color_eyre::install()?`, `kazoo_dx::cli(std::env::args().skip(1))`).

### kazoo-dx (four files: `lib.rs` new, `args.rs` new, `run.rs` new, `main.rs`)

- `kazoo-dx/src/lib.rs`: crate doc from `main.rs:1-6`; `mod args; mod audio; mod phrase; mod run; mod synth; mod ui;`; `pub use args::{Args, USAGE, cli, parse_args}; pub use run::run;` and `pub(crate) use run::{…}` for every item `kazoo-dx/src/ui.rs` reaches as `crate::<Item>` (it has one bare `crate::` import: open it and list exactly the names, which are among `App`, `Focus`, `GLOBAL_FIELDS`, `PhraseState`, `HeldKey`).
- `kazoo-dx/src/args.rs`: `Args`, `USAGE`, `parse_args`, `cli`, and the test `args_parse` moved from `main.rs:1008` (adapted: help is `Ok(None)`, not an exit).
- `kazoo-dx/src/run.rs`: `main.rs:13-1019` except `fn main` and `parse_args`: the `use` lines, `REPEAT_HOLD` (:37), `DEFAULT_REPEAT_DELAY` (:39), `main`'s body (:42-122) as `pub fn run(args: Args)` with line 42 left out and line 44 replaced by `let phrase_text = args.phrase;`, `KeyReleases` (:127-133), `setup_terminal`/`undo_setup`/`restore_terminal` (:137-200, untouched; A04c replaces them), `Focus` (:231), `GLOBAL_FIELDS` (:238), `HeldKey` (:241), `PhraseState` (:254), `App` and its impl (:261-~580), `send_command` (:585), `system_repeat_delay` (:598), `parse_initial_key_repeat` (:617), `piano_offset` (:627), `run_event_loop` (:650), `handle_key` (:685) and everything after, and the tests module (:765-1019) minus `args_parse`. It will be about 940 lines: under 1000, and A04c and A05a shrink it.

### kazoo-string (same four files)

- `kazoo-string/src/lib.rs`: crate doc from `main.rs:1-6`; `mod args; mod audio; mod body; mod patch; mod phrase; mod run; mod synth; mod ui; mod waveguide;`; the same `pub use` lines, and `pub(crate) use run::{…}` for what `kazoo-string/src/ui.rs` names via its one bare `crate::` import.
- `kazoo-string/src/args.rs`: as dx, from `main.rs:206-230`, with its `args_parse` test (find it in `main.rs:734-1018`).
- `kazoo-string/src/run.rs`: `main.rs:16-1018` except `fn main` and `parse_args`, in the same way (`run` from :45-~125, the terminal functions, `App`, `system_repeat_delay` at :567, `piano_offset` at :596, `run_event_loop` at :619, the tests).

`phrase.rs` in both crates is byte-identical to `kazoo-glass/src/phrase.rs`; it stays a private module here (its home is decided in a later lane, not this one).

## Not this lane

A01a, c to g; A02 (device); A03 (link); A04 (terminal); A05 (the key map and the macOS repeat probe, which stay in `run.rs` for now); A06.

## Red tests first

In `args.rs` of each crate:

- `help_returns_instead_of_exiting`: `parse_args(["--help"])` and `parse_args(["--phrase", "c4/4", "-h"])` are `Ok(None)`.
- `phrase_is_taken_both_ways`: `parse_args(["--phrase", "c4/8 e4/8"])` and `parse_args(["--phrase=c4/8 e4/8"])` both give `Ok(Some(Args { phrase: Some("c4/8 e4/8".into()) }))`; `parse_args([])` gives `phrase: None`.
- `a_bare_phrase_flag_says_what_it_needs`: `parse_args(["--phrase"])` is `Err` containing `--phrase needs notation`.
- `an_unknown_argument_is_refused_by_name`: `parse_args(["--bogus"])` is `Err` containing `--bogus`.
- `usage_names_both_ways_to_start_it`: `USAGE` contains `kazoo dx` and `kazoo-dx` (resp. string).

Every test from `main.rs` runs from `run.rs` (dx 19 functions in its tests module, of which the `args_parse` one moves to `args.rs`; string likewise). The new ones are red because `parse_args`/`Args`/`USAGE` do not exist yet.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-dx -p kazoo-string
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-dx -p kazoo-string --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-dx kazoo-string
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-dx -p kazoo-string
```

## Definition of done

- [ ] Both `lib.rs` export `Args { phrase }`, `USAGE`, `parse_args`, `cli`, `run`.
- [ ] `grep -rn 'process::exit' kazoo-dx kazoo-string` finds nothing (paste the empty result).
- [ ] Each `main.rs` is the doc line, the install, one `cli` call.
- [ ] `cargo run -p kazoo-dx -- --help` exits 0 with the usage; `-- --phrase` exits non-zero saying what it needs; `-- --phrase "c4/8 e4/8 g4/8 c5/8"` starts the synth with the phrase loaded (run it in a terminal for five seconds and quit with `q`; say so in DONE).
- [ ] nextest counts: dx at least 53 + 5, string at least 81 + 5 (paste the real numbers).
- [ ] `wc -l` of both `run.rs` under 1000.
- [ ] Every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: moved code is moved, not rewritten; the only behaviour change is help returning instead of exiting.

## END

```
START KZ-A01b base=<sha> branch=lane/kz-a01b df=<GB> at <time>
END KZ-A01b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
