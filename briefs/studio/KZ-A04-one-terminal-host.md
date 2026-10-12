# KZ-A04: one terminal host for every TUI

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-core` plus every crate touched.

## Outcome

Raw mode, the alternate screen, the panic-safe restore and `color_eyre` are set up in one place, once per process. The ten `terminal.rs` files (mini and cs80 are byte-identical) are gone.

## The design (decided; build exactly this)

- `kazoo-core/src/host/terminal.rs`:
  - `pub struct Terminal { inner: ratatui::Terminal<CrosstermBackend<Stdout>> }` with `pub fn enter() -> Result<Terminal, Error>`: enables raw mode, enters the alternate screen, enables mouse capture only if asked (`enter_with(Options { mouse: bool })`), installs a panic hook that restores the terminal **once** (guarded by a `std::sync::Once`; the survey found tui and mix chain hooks without a guard) and then calls the previous hook.
  - `impl Drop for Terminal` restores. `pub fn draw(&mut self, f: impl FnOnce(&mut Frame))`.
  - `pub fn install_reports()`: `color_eyre::install` behind a `Once`, so twelve mains calling it under one binary (KZ-A06) is safe.
- Replace each crate's `terminal.rs` and the setup/restore blocks in main: 808, 303, arp, cs80, mini, juno, prophet, dx, string, glass, tui, mix, and the wall console. Delete the files.

## Not this lane

Keyboard map (KZ-A05), the `kazoo` binary (KZ-A06).

## Red tests first

- `host::terminal::tests::install_reports_twice_is_fine` (second call returns Ok and installs nothing).
- `host::terminal::tests::panic_hook_restores_once` (a fake restore counter behind the same `Once`, triggered twice).
- `host::terminal::tests::enter_without_a_tty_is_an_error_in_words` (run under a pipe).

## Definition of done

- [ ] No `terminal.rs` outside kazoo-core; `grep -rn "enable_raw_mode" --include=*.rs` matches only kazoo-core.
- [ ] Three red tests then green; every converted crate green.
- [ ] Ctrl-C and a panic in any TUI leave the terminal usable (tested by hand on one instrument; say which).
