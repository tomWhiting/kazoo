# KZ-A01d: arp, juno and prophet become libraries with `run(Args)`, and each main.rs is the install and one call

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Split: PLAN.md's A01 is split into A01a to A01g (see KZ-A01a). This is d = kazoo-arp, kazoo-juno, kazoo-prophet. Independent of the other six.

## Outcome

`kazoo-arp`, `kazoo-juno` and `kazoo-prophet` can be started from another crate with `run(Args)` and behave as today. Each already has a `lib.rs` holding its DSP (`kazoo-arp/src/lib.rs`: clock and engine; `kazoo-juno/src/lib.rs` and `kazoo-prophet/src/lib.rs`: synth); the app, audio engine, UI and event loop are private to the binary and move under the library. Each `main.rs` is the install and one call; `--help` works and a stray argument is refused (today all three ignore arguments).

## The design (decided; build exactly this)

The uniform surface of KZ-A01a (`Args` unit struct, `USAGE`, `parse_args`, `run`, `cli`), the same `parse_args` and `cli` behaviour, the same three-line `main.rs`. Usage texts in KZ-A01a's shape with the crate's `Cargo.toml` description and `kazoo arp` / `kazoo juno` / `kazoo prophet`.

### kazoo-arp (three files: `lib.rs`, `run.rs` new, `main.rs`)

- `kazoo-arp/src/lib.rs` (exists; keeps its doc, `pub mod clock; pub mod engine;` and the `pub use` lines) gains `mod app; mod audio; mod run; mod ui;` and `pub use run::{Args, USAGE, cli, parse_args, run};`. `app.rs`, `audio.rs`, `ui/` reach `crate::app::…`, `crate::audio::…`, `crate::engine::…` only: no re-export needed.
- `kazoo-arp/src/run.rs`: `main.rs:11-228` except `fn main`: the `use` lines (:11-32; `use kazoo_core::ipc::link::…` and `ringbuf::traits::Split` included), `main`'s body (:40-118, line 40 left out) as `pub fn run(args: Args)` with `let Args = args;`, `setup_terminal`/`enter_screen`/`restore_terminal` (:123-193, untouched; A04b replaces them), `run_event_loop` (:195-228). Note `main.rs:27` imports `crate::app::{App, KeyMode, StreamHealth}`: keep it.

### kazoo-juno and kazoo-prophet (three files each)

The two `main.rs` files differ in 20 lines (name strings and juno's `key_note_map` bookkeeping, `diff` shows it); do the same move in both.

- `kazoo-juno/src/lib.rs` (exists: `pub mod synth; pub use synth::{…}`) gains `mod app; mod audio; mod input; mod run; mod ui;` and the `pub use run::{…}` line. `ui.rs` reaches `crate::app::WAVEFORM_BUF_SIZE` and `crate::app::…` only.
- `kazoo-juno/src/run.rs`: `main.rs:8-479` except `fn main`: the `use` lines (:8-31), `main`'s body (:38-105, line 38 left out) as `pub fn run(args: Args)`, `run_event_loop` (:107), `StreamHealth`/`KeyReleases` and its `note()` method (around :150-168), `setup_terminal`/`undo_setup`/`restore_terminal` (:171-231, untouched; A04b replaces them), `process_event` (:233), `handle_key_press` (around :292), `key_index` (:357), `is_quit_key`, `update_param`, `send_command` and every other private function, and the tests (:382-479).
- `kazoo-prophet/src/lib.rs` and `kazoo-prophet/src/run.rs`: the same, from `kazoo-prophet/src/main.rs` (tests at :364-473).
- `main.rs` of both: the three-line form.

## Not this lane

A01a, b, c, e, f, g; A02; A03; A04 (the terminal functions and the three `KeyReleases`/`KeyMode` enums stay where they land); A05 (`input.rs` of juno and prophet, byte-identical, and the arp's `key_to_note` stay); A06.

## Red tests first

In each crate's `run.rs` tests: the three uniform tests of KZ-A01a (`help_asks_for_the_usage`, `an_unknown_argument_is_refused_by_name`, `usage_names_both_ways_to_start_it` with `kazoo arp`/`kazoo-arp` etc.). Every existing test keeps running: arp 79, juno 22, prophet 24 at e0de0c0 (juno's 8 and prophet's 8 `main.rs` tests among them).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-arp -p kazoo-juno -p kazoo-prophet
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-arp -p kazoo-juno -p kazoo-prophet --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-arp kazoo-juno kazoo-prophet
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-arp -p kazoo-juno -p kazoo-prophet
```

## Definition of done

- [ ] The three `lib.rs` export `Args`, `USAGE`, `parse_args`, `cli`, `run`; the three `main.rs` are the doc line, the install and one call.
- [ ] `cargo run -p kazoo-juno -- --help` exits 0 with the usage; `-- --bogus` exits non-zero naming it (paste both).
- [ ] nextest: arp at least 79 + 3, juno at least 22 + 3, prophet at least 24 + 3 (real numbers).
- [ ] Every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: moved code is moved, not rewritten; juno and prophet stay the same shape as each other.

## END

```
START KZ-A01d base=<sha> branch=lane/kz-a01d df=<GB> at <time>
END KZ-A01d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
