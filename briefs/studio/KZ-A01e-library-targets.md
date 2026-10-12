# KZ-A01e: glass and mix become libraries with `run(Args)`; glass's `--help` no longer kills the process

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Split: PLAN.md's A01 is split into A01a to A01g (see KZ-A01a). This is e = kazoo-glass and kazoo-mix. Independent of the other six.

## Outcome

`kazoo-glass` can be started from another crate with `run(Args { demo, phrase })`, where `demo` renders the preview WAV offline without a device or desk, as `--demo <wav>` does today (`kazoo-glass/src/main.rs:50-59`). `kazoo-mix` can be started with `run(Args { demo })`. Both `main.rs` files are the install and one call. Glass's `--help` returns instead of `std::process::exit(0)` (`main.rs:236`).

## The design (decided; build exactly this)

### kazoo-glass (four files: `lib.rs`, `args.rs` new, `run.rs` new, `main.rs`)

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// Render the six-vignette preview to this WAV and exit: no device, no desk.
    pub demo: Option<std::path::PathBuf>,
    /// A looped `kazoo-play` phrase to play on the transport, as notation.
    pub phrase: Option<String>,
}
```

- `parse_args`: `--demo <wav>` anywhere (today it must be first and alone, `main.rs:50-59`: this lane relaxes that, since clap in A06 will not care about order; `--demo` without a path is `Err("--demo needs a WAV path")`), `--phrase <notation>` / `--phrase=<notation>` as in KZ-A01b, `-h`/`--help` gives `Ok(None)`, anything else `Err(format!("unknown argument '{other}'"))`. `demo` and `phrase` together is allowed: `run` renders the demo and ignores the phrase, as the old code did by exiting before parsing it; say so in the `demo` doc comment.
- `run(args)`: if `args.demo` is `Some(path)`, `demo::render(&path)` and return; else the body of `main.rs:62-131` with `let phrase_text = args.phrase;`.
- `USAGE`:

```
kazoo-glass: polyphonic modal bell synth, tuned glass, chimes and metallic keys

usage: kazoo glass [--phrase "<kazoo-play notation>"]
       kazoo glass --demo <preview.wav>
       kazoo-glass [--phrase ...] | --demo <preview.wav>

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. --demo renders a preview and exits. Press ? inside for keys.
```

- `kazoo-glass/src/lib.rs` (exists: doc, `pub mod patch; pub mod synth;`, the test allocator) gains `mod args; mod audio; mod demo; mod phrase; mod run; mod ui;`, the `pub use args::{Args, USAGE, cli, parse_args}; pub use run::run;` lines, and `pub(crate) use run::{…}` for what `ui.rs` names via `crate::` (it names `crate::KeyReleases::Reported`, `crate::audio::AudioStats::default` and one bare `crate::` import: list the exact items). The duplicate `#[global_allocator]` in `main.rs:15-17` goes (the one in `lib.rs:9-11` covers the crate's tests; a binary cannot have two).
- `kazoo-glass/src/demo.rs` and `main.rs`'s `use kazoo_glass::patch; use kazoo_glass::synth;` (:10, :12): inside the library a crate cannot name itself, so `kazoo_glass::` becomes `crate::` in `demo.rs:3-4` and in `run.rs`.
- `kazoo-glass/src/args.rs`: `Args`, `USAGE`, `parse_args`, `cli`, its tests (including the moved `args_parse` from `main.rs`'s tests, adapted).
- `kazoo-glass/src/run.rs`: `main.rs:19-1032` except `fn main`, `parse_args` and the allocator: constants (:43, :45), `run`, `KeyReleases`, the terminal functions (:~138-216, untouched; A04c), `App` and its impl, `system_repeat_delay` (:581), `piano_offset` (:610), `run_event_loop` (:633), `handle_key`, the tests (:748-1032 minus `args_parse`). About 950 lines; under 1000.
- `main.rs`: the three-line form.

### kazoo-mix (three files: `lib.rs`, `run.rs` new, `main.rs`)

- `kazoo-mix/src/lib.rs` (exists; every module public) gains `mod run;` and `pub use run::{Args, USAGE, cli, parse_args, run};`.
- `kazoo-mix/src/run.rs`: `main.rs:9-366` except `fn main`: the `use` lines (switch `kazoo_mix::` to `crate::`), `UI_TICK` (:30), `NEWS_SECONDS` (:33), `USAGE` (:35-47, made `pub`, text unchanged: A06 rewords it), `Options` (:51-53) renamed `Args` (this lane's rename; derive `Default` too), `parse_args` (:56-66) returning `Result<Option<Args>, String>` with `Err(format!("unknown option `{other}`"))` and no usage inside the error (uniform with the others; `cli` adds it), `main`'s body (:70-105, line 69 left out) as `pub fn run(args: Args)`, `run_desk` (:108), `HubNews` (:174-231), `hub_priority` (:235), `demo_line` (:246), `MixerAudio` (:261-300), `build_output_stream` (:302-324), the tests (:341-366, `Options` to `Args`, the error assertion kept).
- `cli` as in KZ-A01a. `main.rs`: the three-line form (`kazoo_mix::cli(...)`).

## Not this lane

A01a to d, f, g; A02 (`MixerAudio::start` and glass's device code stay); A03; A04 (`kazoo-mix/src/terminal.rs` and glass's terminal functions stay); A05; A06 (usage rewording, clap).

## Red tests first

`kazoo-glass/src/args.rs`:

- `help_returns_instead_of_exiting`: `["--help"]` and `["--demo", "/tmp/x.wav", "-h"]` give `Ok(None)`.
- `demo_is_a_path_anywhere`: `["--demo", "/tmp/p.wav"]` and `["--phrase", "c4/4", "--demo", "/tmp/p.wav"]` give `demo: Some("/tmp/p.wav".into())`; `["--demo"]` is `Err` containing `--demo needs a WAV path`.
- `phrase_is_taken_both_ways`, `an_unknown_argument_is_refused_by_name`, `usage_names_both_ways_to_start_it` (`kazoo glass`, `kazoo-glass`): as KZ-A01b.

`kazoo-glass/src/run.rs`:

- `demo_renders_without_a_device`: `run(Args { demo: Some(tempdir.join("p.wav")), phrase: None })` writes a WAV; `hound::WavReader::open` reads it with 2 channels at 48000 and more than zero samples (use `std::env::temp_dir()` with the process id in the name, as the tree's other tests do; remove the file at the end).

`kazoo-mix/src/run.rs`:

- `arguments_choose_the_demo_or_help` adapted to `Args` and the `String` error; `usage_names_the_desk` (`USAGE` contains `kazoo-mix`).

Every existing test keeps running: glass 54, mix 181 at e0de0c0.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-glass -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-glass -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-glass kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-glass -p kazoo-mix
```

## Definition of done

- [ ] `kazoo_glass::{Args, USAGE, parse_args, cli, run}` and `kazoo_mix::{Args, USAGE, parse_args, cli, run}` exist as above; both `main.rs` are the three-line form.
- [ ] `grep -rn 'process::exit\|kazoo_glass::' kazoo-glass/src` finds nothing.
- [ ] `cargo run -p kazoo-glass -- --demo /tmp/kazoo-glass-a01e.wav` writes the file and exits 0 (`ls -l` it); `cargo run -p kazoo-mix -- --help` exits 0 with the usage.
- [ ] nextest: glass at least 54 + 6, mix at least 181 + 1 (real numbers).
- [ ] `wc -l kazoo-glass/src/run.rs` under 1000.
- [ ] Every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: moved code is moved, not rewritten; the only behaviour changes are the two argument-handling ones named above.

## END

```
START KZ-A01e base=<sha> branch=lane/kz-a01e df=<GB> at <time>
END KZ-A01e head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
