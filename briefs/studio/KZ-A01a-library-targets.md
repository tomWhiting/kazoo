# KZ-A01a: cs80, mini and 303 become libraries with `run(Args)`, and each main.rs is the install and one call

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Split: PLAN.md's A01 ("library targets") is split into A01a to A01g because every crate with a run loop needs a `run(Args)` for the `kazoo` binary (A06), not only the six without a `lib.rs`: a = cs80, mini, 303; b = dx, string; c = play, 808; d = arp, juno, prophet; e = glass, mix; f = tui, mcp; g = the wall's console. The seven share no files and may run two at a time. A02 to A05 depend on all seven.

## Outcome

`kazoo-cs80`, `kazoo-mini` and `kazoo-303` can each be started from another crate with one call, `run(Args)`, and behave exactly as they do today from their own binaries. Each `main.rs` is now the error-report install and one call into its library; nothing else lives there. `--help` prints a usage that names the coming `kazoo <name>` form; any other argument is refused with that usage (today all three ignore every argument, which is why a typo starts the synth anyway).

## The design (decided; build exactly this)

The same shape in all three crates, so A06 can wire them mechanically. Every public item carries a doc comment.

```rust
/// What the command line gave: nothing yet for this instrument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args;

/// What `--help` prints.
pub const USAGE: &str = "...";

/// Read the arguments after the program name. `Ok(None)` means help was
/// asked for; `Err` is one sentence saying what was wrong (the caller adds
/// the usage).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String>;

/// Run the instrument until it quits: audio, desk link, terminal, event loop.
/// Error reports must already be installed by the caller
/// (`color_eyre::install`); this never installs them, so one process can
/// run several instruments one after another.
pub fn run(args: Args) -> color_eyre::Result<()>;

/// The standalone binary's entry: parse, print the usage on help, run.
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()>;
```

- `parse_args`: `-h` or `--help` anywhere gives `Ok(None)`; the first other argument gives `Err(format!("unknown argument '{other}'"))`; no arguments gives `Ok(Some(Args))`.
- `cli`: `Ok(None)` writes `USAGE` with `std::io::stdout().lock().write_all(USAGE.as_bytes())` then `flush()`, both with `?` (never `print!`, which panics on a closed pipe; `kazoo-play/src/main.rs:89-94` already does it this way); `Err(why)` becomes `Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}"))` (the pattern at `kazoo-mix/src/main.rs:62`); `Ok(Some(args))` calls `run(args)`.
- `USAGE` for each crate, exactly this shape (fill the name and the one-line description from the crate's `Cargo.toml` `description`):

```
kazoo-cs80: Yamaha CS-80 inspired pad synth, 8-voice polyphonic, dual-layer

usage: kazoo cs80
       kazoo-cs80

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. Keys are on the panel.
```

- Each `main.rs` becomes exactly:

```rust
//! kazoo-cs80 on its own: the same instrument `kazoo cs80` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_cs80::cli(std::env::args().skip(1))
}
```

### kazoo-cs80

- New `kazoo-cs80/src/lib.rs`: the crate doc from `kazoo-cs80/src/main.rs:1-5`; the module list from `main.rs:7-15` with `pub mod modular;` and `pub mod synth;` kept public and the rest private, plus `mod run;`; then `pub use run::{Args, USAGE, cli, parse_args, run};`.
- New `kazoo-cs80/src/run.rs`: everything `main.rs` holds from line 17 to the end except `fn main`: the `use` lines, `TARGET_FPS` (:31), `STREAM_ERROR_BACKLOG` (:35), `UiChannels` (:38-44), the body of `main` (:47-121) as `pub fn run(args: Args)` with line 47 (`color_eyre::install()?`) left out and `let Args = args;` as its first line (the unit struct carries nothing yet; the binding says so and keeps the parameter name uniform across every crate; `Args` is `Copy`, so clippy's `needless_pass_by_value` stays quiet), the event loop `fn run` at :125 renamed `fn event_loop` because the public entry takes its name (the one rename this lane makes), `poll_audio` (:161), `handle_key` (:180), `key_slot` (:258), `send_aftertouch_for_held_notes` (:263), `save_preset` (:272), `load_preset` (:282), `handle_key_release` (:300), and the tests module (:310-406) unchanged apart from the rename. Add `Args`, `USAGE`, `parse_args`, `cli` at the top.
- `kazoo-cs80/src/ui/` reaches `crate::terminal::Tui`, `crate::terminal::KeyReleases`, `crate::app::…`, `crate::command::AudioCommand`, `crate::synth::…` (the modules stay where they are, so these paths still resolve); nothing in `ui/` names an item that lived in `main.rs`.

### kazoo-mini

- New `kazoo-mini/src/lib.rs`: crate doc from `kazoo-mini/src/main.rs:1-8`; modules from `main.rs:10-17` all private, plus `mod run;`; the same `pub use`.
- New `kazoo-mini/src/run.rs`: `main.rs:19-299` except `fn main`: `TICK_RATE` (:36), `STREAM_ERROR_BACKLOG` (:40), `UiChannels` (:43-49), `main`'s body (:52-126, line 52 left out) as `pub fn run(args: Args)`, the loop `fn run` at :130 renamed `fn event_loop`, `poll_audio` (:150), `handle_key_event` (:174), tests (:209-299).
- `kazoo-mini/src/ui/` reaches `crate::terminal::Tui`, `crate::terminal::KeyReleases`, `crate::app::App`, `crate::params::MiniParams`, `crate::command::…`, `crate::synth::…`: all still resolve.

### kazoo-303

- New `kazoo-303/src/lib.rs`: crate doc from `kazoo-303/src/main.rs:1-5`; `mod audio; mod sequencer; mod synth; mod ui; mod run;`; `pub use run::{Args, USAGE, cli, parse_args, run};` and `pub(crate) use run::App;` because `kazoo-303/src/ui.rs` names `crate::App` (one reference).
- New `kazoo-303/src/run.rs`: `main.rs:12-506` except `fn main`: `FRAME_POLL` (:32), `SWING_STEP` (:35), `main`'s body (:38-98, line 38 left out) as `pub fn run(args: Args)`, `setup_terminal`/`undo_setup`/`restore_terminal` (:103-136, untouched here; A04 replaces them), `StreamHealth` (:140), `App` (make it `pub(crate) struct App`), `run_event_loop` (:246), every other private function, and the tests (:361-506). `App::new` and the handlers keep their names.

### What stays as it is

Audio device opening (A02), the hub link (A03), the terminal functions and `terminal.rs` files (A04), the key maps (A05). This lane moves code; it changes no behaviour except the argument handling above.

## Not this lane

- A01b to A01g: the other crates.
- A02 (device open through `kazoo_core::host::audio`), A03 (link), A04 (`kazoo-host` terminal; the two byte-identical `terminal.rs` files in cs80 and mini stay for now), A05 (keys), A06 (the `kazoo` binary and its clap parsing; usage texts are reworded there for the other crates).

## Red tests first

In each crate's `run.rs` tests module (file named per crate above):

- `help_asks_for_the_usage` (cs80, mini, 303): `parse_args(["-h"])` and `parse_args(["--help"])` are `Ok(None)`; `parse_args([])` is `Ok(Some(Args))`.
- `an_unknown_argument_is_refused_by_name` (cs80, mini, 303): `parse_args(["--bogus"])` is `Err` and the text contains `--bogus`.
- `usage_names_both_ways_to_start_it` (cs80, mini, 303): `USAGE` contains `kazoo cs80` (resp. `kazoo mini`, `kazoo 303`) and `kazoo-cs80` (resp. `kazoo-mini`, `kazoo-303`).
- Every test that lived in `main.rs` runs from `run.rs` unchanged: cs80 5 (`key_press_and_release_send_note_on_and_off`, `repeated_press_does_not_retrigger`, `backspace_releases_everything`, `non_ascii_key_is_ignored`, `param_change_is_sent`), mini 3 (`note_key_press_and_release`, `backspace_releases_everything`, `param_edit_sends_params_and_unhandled_key_does_not`), 303 9 (`accepted_edit_updates_mirror` … `quit_keys`).

The three new tests are red because `parse_args`, `Args` and `USAGE` do not exist; write them, see them fail to compile, then build the library.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-cs80 -p kazoo-mini -p kazoo-303
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-cs80 -p kazoo-mini -p kazoo-303 --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-cs80 kazoo-mini kazoo-303
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-cs80 -p kazoo-mini -p kazoo-303
```

(The doc test run is required here: every public item gains a doc comment.)

## Definition of done

- [ ] `kazoo-cs80/src/lib.rs`, `kazoo-mini/src/lib.rs`, `kazoo-303/src/lib.rs` exist and export `Args`, `USAGE`, `parse_args`, `cli`, `run` with the signatures above.
- [ ] Each `main.rs` is the crate doc line, `color_eyre::install()?` and one `cli` call: no `mod`, no `use`, no other function.
- [ ] `nice -n 10 env CARGO_BUILD_JOBS=4 cargo run -p kazoo-303 -- --help` prints the usage and exits 0; `-- --bogus` exits non-zero and the output contains `unknown argument '--bogus'` and the usage (paste both in DONE).
- [ ] nextest counts: cs80 at least 120 + 3, mini at least 64 + 3, 303 at least 33 + 3 (the survey's counts  plus the new tests; paste the real numbers).
- [ ] Every check above green, each with its own exit code read from the tool.
- [ ] No file over 1000 lines (`wc -l` of the three `run.rs` files in DONE).

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else` (ast-grep enforces all five); nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific to this lane: moved code is moved, not rewritten (a diff of `run.rs` against the old `main.rs` should be the moves, the rename of the loop, and the four new items); `main.rs` gains no logic.

## END

```
START KZ-A01a base=<sha> branch=lane/kz-a01a df=<GB> at <time>
END KZ-A01a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
