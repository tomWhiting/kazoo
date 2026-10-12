# KZ-A04: one terminal host for every TUI, in a new `kazoo-host` crate

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-host` plus every crate touched, and `cargo test --doc -p kazoo-host`. Depends: A01a–g.

## Outcome

Raw mode, the alternate screen, the panic-safe restore and the error-report install are set up in one place, once per process. The ten `terminal.rs` files (mini and cs80 are byte-identical) are gone. The code lives in a new crate, `kazoo-host`, because kazoo-core is the DSP library with no UI dependency (CLAUDE.md) and ratatui and crossterm must not enter it. A05 (keys) and A06 (the `kazoo` binary) build on this crate.

## The design (decided; build exactly this)

- New crate `kazoo-host` (added to the workspace members): `[package] name = "kazoo-host"`, edition and rust-version as the other crates, `[lints] workspace = true`, dependencies `crossterm = "0.29.0"`, `ratatui = "0.30.0"`, `color-eyre = "0.6.5"` (the versions already in `Cargo.lock`; nothing new enters the lock).
- `kazoo-host/src/lib.rs`: `pub mod reports; pub mod terminal;` (A05 adds `pub mod keys;`).
- `kazoo-host/src/reports.rs`: `pub fn install() -> color_eyre::Result<()>`: `color_eyre::install` behind a `std::sync::Once`; the second and later calls return `Ok(())` and install nothing, so twelve `run`s under one binary (A06) are safe.
- `kazoo-host/src/terminal.rs`:
  - `pub struct Terminal { inner: ratatui::Terminal<CrosstermBackend<Stdout>> }`; `pub fn enter() -> Result<Terminal, Error>` and `pub fn enter_with(options: Options) -> Result<Terminal, Error>` with `Options { mouse: bool, key_release_events: bool }` (several instruments enable keyboard enhancement flags for key releases: find each `PushKeyboardEnhancementFlags` and keep that behaviour behind the option): enables raw mode, enters the alternate screen, pushes the flags if asked, enables mouse capture if asked, and installs a panic hook that restores the terminal **once** (a `Once` guards the hook install and an `AtomicBool` guards the restore; the survey found tui and mix chaining hooks without a guard) and then calls the previous hook.
  - `impl Drop for Terminal` restores (the same guarded restore). `pub fn draw(&mut self, f: impl FnOnce(&mut Frame)) -> Result<(), Error>`; `pub fn size(&self)`.
  - `Error` names what failed ("raw mode", "alternate screen", "no tty") with the io error inside.
- Replace each crate's `terminal.rs` and the setup/restore blocks in its `run.rs` (after A01): 808, 303, arp, cs80, mini, juno, prophet, dx, string, glass, tui, mix, and the wall console (`kazoo-wall/src/tui`). Delete the files. The `color_eyre::install()?` line in every `main.rs` becomes `kazoo_host::reports::install()?`.

## Not this lane

Keyboard map (A05), the `kazoo` binary (A06), audio and link hosts (A02, A03: those are in kazoo-core because they need only cpal and the ipc module, which kazoo-core already has).

## Red tests first

- `reports::tests::install_twice_is_fine` (second call returns Ok).
- `terminal::tests::restore_runs_once_even_if_asked_twice` (the guarded restore with a counting stand-in behind the same `AtomicBool`).
- `terminal::tests::enter_without_a_tty_is_an_error_in_words` (run under a pipe: the error text contains "tty").
- `terminal::tests::options_default_to_no_mouse_and_no_release_events`.
- Every converted crate's tests still green.

## Definition of done

- [ ] `kazoo-host` builds with the two modules; no `terminal.rs` outside it; `grep -rn "enable_raw_mode\|color_eyre::install" --include=*.rs` matches only `kazoo-host`.
- [ ] Four red tests then green; every converted crate green.
- [ ] Ctrl-C and a panic in one instrument leave the terminal usable (tested by hand on kazoo-808: say what you saw).
