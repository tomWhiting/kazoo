# Kazoo Studio: the plan

Written 12 October 2026 by Cassio from a sweep of the tree at e0de0c0 (four survey reports: workspace, desk and hub, wall, instruments). Tom's word, by voice, 12 Oct: one CLI for everything; instruments start up and all connect into the mixer; the mixer feeds a tracker so you see full multi-track; record the audio and the signal; an effects pipeline and a mastering pipeline after that; thousands of modules across a track while delays and reverbs run; "high performance Rust ringing in your ears"; lo-fi sound is fine, the Rust has to be perfect. No code is written by Cassio: every lane is a brief fed to Codex, one at a time, each sized to one mechanism (about 30 minutes of a builder's work), reviewed by an Opus reviewer, then landed under one pooled gate.

## Where we are (12 Oct 2026, e0de0c0)

- 20 crates, about 224k lines of Rust, about 3,300 tests. Lints: `unsafe_code = forbid`, clippy all + pedantic + nursery; ast-grep forbids `let _ =`, `.ok()`, `#[allow]`/`#[expect]`, empty error arms, `if let Ok` without `else`. Tests run under nextest only; doc tests under `cargo test --doc`. There is no Justfile, no CI and no benchmark anywhere.
- **The desk (kazoo-mix)** is a live 8-strip console: trim, 3-band EQ, one aux send, pan, fader, mute, solo, meters, clip light, master fader, Freeverb return, click, soft limiter, tempo and play/stop. It has no recording (a record request just plays, `hub.rs:1115`), no timeline, no clips, no session save, no undo, no insert effects, no automation, no groups, no loop or locate, and a fixed 8 strips with one aux.
- **Instruments reach the desk** through `hub_link` (`kazoo-core/src/ipc/link.rs:409`): audio goes into a lock-free ring, a link thread polls it every 500 µs and sends it over a Unix socket (9-byte header, 64 KiB payload cap), the hub thread polls every 500 µs, gives the instrument the first free strip, and hands blocks to the audio callback. Audio is copied four times on the way. Mismatched sample rate is refused; more than 2 channels is refused; one link is one strip.
- **The wall (kazoo-wall)** renders in 32-frame blocks on one thread: pages of 64 slots, each a `Box<dyn Module>` with 24 knobs and 4 output buffers; cables are a table by slot and jack; render order is a topological sort rebuilt on every change (`tables.rs:251`); every cable has a 2048-sample delay line; the load guard refuses adds above 0.8 and allows them again below 0.7. Every `out` sums into one stereo master (`engine/mod.rs:1107`), so the desk gets exactly one strip for the whole wall. The expensive modules are the circuit models: screamer (16× oversampled, one Newton solve per oversampled sample), fold (16×, Lambert W by Newton), amp (16×), klon (4×, 8×8 matrix plus a diode solve), fuzz (8×), the tape hysteresis models (8 substeps), and the reverbs (32-point interpolators; spring has about 312 allpasses).
- **The instruments** (808, 303, arp, cs80, mini, juno, prophet, dx, string, glass, play, tui): 15 binaries, every CLI a hand-written `env::args` loop, no clap. Audio device setup is copied into all ten; hub link setup, terminal setup and restore, the keyboard-to-note map (5 incompatible layouts) and whole files (`phrase.rs` in dx, glass and string) are duplicated. cs80, mini, dx, string, 303 and play have no `lib.rs`. No instrument accepts `ParameterChange` from another process. The arp never sends notes to the hub.
- Stale docs: CLAUDE.md pulls in two plan files that do not exist, says kazoo-tui is the hub and tells you to run `cargo test`; README documents the removed synth drawer; `briefs/00–11` are old briefs with names that do not match the crates; `kazoo-sampler` is unused by any crate.

## The shape of the finished thing

```
kazoo mix                 the desk: strips for every instrument stream, inserts, buses, master chain,
                          record, timeline with waveform lanes, session save, undo
kazoo wall serve          the modular synth; every `out` module is its own strip on the desk
kazoo 808 | 303 | cs80 | mini | juno | prophet | dx | string | glass | arp | play | mouth
kazoo mcp                 the Claude seat on the wall
kazoo studio up <name>    starts the desk and every instrument in a session file, each in its own pane,
                          all linked
kazoo master <session>    renders a session through the master chain to a WAV
```

Audio reaches the desk without sleep-polling, with two copies instead of four, with named streams on one link, resampled at the hub if the rates differ. The desk records any armed strip to WAV and records the note and parameter signal next to it, sample-accurately against the transport, and plays takes back on the timeline. The wall renders only what reaches an `out`, pays nothing for unplugged jacks, keeps its tables incrementally, spreads the graph across a pool of real-time threads, and runs the circuit models from tables and warm starts instead of cold Newton solves. Everything is measured by benchmarks that exist before the work and are re-run after it.

## Tracks and lanes

Each lane is one brief in this directory, `KZ-<track><nn>-<slug>.md`. A lane is one mechanism, one or two crates, under ten files, with its own red tests, and runs only its own crates' checks. The full gate runs once per landing. "Depends" names lanes that must have landed on main first. Lanes in different tracks that share no files may run two at a time; lanes in the same track run in order.

### Track A: one CLI

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| A01 library targets | cs80, mini, dx, string, 303 and play each gain `lib.rs` with `pub fn run(args: Args) -> Result<()>`; `main.rs` becomes three lines | those six | — |
| A02 one audio host | one `kazoo_core::host::audio` opens the output (and input) device, and all ten instruments plus tui and mix call it; the ten private copies go | kazoo-core + callers | A01 |
| A03 one hub link setup | one `kazoo_core::host::link` does link open, the audio-callback `try_recv` drain, and teardown; every instrument calls it | kazoo-core + callers | A02 |
| A04 one terminal host | one `kazoo_core::host::terminal` does raw mode, alternate screen, panic-safe restore, `color_eyre` once; the ten `terminal.rs` go | kazoo-core + callers | A01 |
| A05 one keyboard map | one keyboard-to-note layout with octave shift in kazoo-core, used by every instrument; the macOS key-repeat probe once | kazoo-core + callers | A04 |
| A06 the `kazoo` binary | new crate `kazoo` with clap subcommands dispatching to each crate's `run`; runtimes start inside the subcommand; wall daemon restarts as `kazoo wall serve`; MCP config and usage texts updated | kazoo (new), kazoo-wall | A01–A05 |
| A07 `kazoo studio` | session file `~/.kazoo/studio/<name>.toml` lists instruments; `kazoo studio up` starts the desk then each instrument in its own tmux pane, all linked; `down` stops them | kazoo | A06 |
| A08 Justfile and docs | `just gate` (fmt, clippy -D warnings, ast-grep, nextest, doc tests), `just lane <crates>`; CLAUDE.md rewritten to the current truth; README rewritten; old `briefs/00–11` moved to `briefs/old/` | docs | A06 |

### Track B: audio into the desk, properly

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| B01 no sleep-polling | the link thread is woken by `Thread::unpark` from the audio callback (constant time, no lock) instead of a 500 µs sleep; the hub reads sockets with `poll(2)` through the `polling` crate instead of a 500 µs sleep; measured wake latency in a test | kazoo-core, kazoo-mix | — |
| B02 two copies, bigger frames | interleaved f32 blocks go ring → socket write and socket read → desk ring; frame limit raised to 1 MiB; backlog bounded by time (ms) not bytes; stuck detection by time | kazoo-core, kazoo-mix | B01 |
| B03 named streams on one link | `Register` carries a stream list; `Audio` frames carry a stream id; the hub gives each stream its own strip named `<instrument>/<stream>`; streams can be added and removed live | kazoo-core, kazoo-mix | B02 |
| B04 wall outs as streams | every `out` module on the wall is a stream on the wall's link, named by its module id or label; add and remove follow the wall; the stereo master is the default stream when there is no `out` | kazoo-wall | B03 |
| B05 resample at the hub | an instrument at another sample rate is accepted and resampled (rubato) on the hub thread, never on the callback | kazoo-core, kazoo-mix | B02 |
| B06 ParameterChange is real | every instrument applies `ParameterChange`; the wall maps it to a knob; the desk can send one; a test per instrument | kazoo-core + instruments | A03 |
| B07 the arp drives | the arp sends its notes to the hub, so it can play any synth | kazoo-arp | — |

### Track C: the desk becomes a DAW

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| C01 strips without limit | strips are a `Vec`, added when a stream arrives and freed when it leaves; the desk view scrolls; buses (groups) with their own fader, pan, mute, solo | kazoo-mix | B03 |
| C02 inserts: engine | each strip and bus has an insert chain from kazoo-fx, pre-allocated, bypassable, with latency compensation | kazoo-mix, kazoo-fx | C01 |
| C03 inserts: view | add, remove, reorder and tweak inserts from the strip, with the wall's catalogue names | kazoo-mix | C02 |
| C04 record audio | arm a strip; on record, its post-fader audio and the master are written as WAV takes by the disk thread, starting on a transport frame; `~/.kazoo/sessions/<name>/takes/` | kazoo-mix, kazoo-core | C01 |
| C05 record the signal | note and parameter events per strip are written next to the audio take with stream-frame timestamps; played back to the instrument on the same frames | kazoo-mix, kazoo-core | C04, B06 |
| C06 takes play back | a take player per strip plays recorded takes through the strip on the transport; loop and locate | kazoo-mix | C04 |
| C07 timeline view | per-strip clip lanes, real time scale, zoom, waveform from peak files computed on the disk thread; the kazoo-core clip model reused | kazoo-mix | C06 |
| C08 session save, load, undo | strips, inserts, takes, clips, transport in `session.json`; autosave on change; undo for every desk change | kazoo-mix | C07 |
| C09 master chain | EQ, compressor, limiter, dither on the master bus from kazoo-fx; `kazoo master <session>` renders offline | kazoo-mix, kazoo-fx | C02 |
| C10 automation | parameter changes recorded against the timeline and played back | kazoo-mix | C05, C08 |

### Track D: high performance

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| D01 benchmarks | criterion benches: wall render per module kind and per cable count (10, 100, 1k, 10k); desk callback at 8, 64, 512 strips; each circuit model; the link path. Numbers recorded in `briefs/studio/BENCH.md` | kazoo-wall, kazoo-mix, kazoo-fx | — |
| D02 no cost for the unplugged | gathers only for plugged jacks; knob glides only while moving; the finiteness scan one pass per block; `out` sum from a list, not a room scan (`engine/mod.rs:1107–1230`) | kazoo-wall | D01 |
| D03 cull what nobody hears | modules that reach no `out`, scope, readout or record are left out of the render order | kazoo-wall | D02 |
| D04 cables without lines | a cable's delay line exists only where latency compensation or a cycle needs it; other cables pass by index | kazoo-wall | D02 |
| D05 incremental tables | a change updates the order, latencies and cable table for the slots it touches; `send_tables` (`wall.rs:1495`) is no longer O(N²); `patch.rs` scans become indexed maps | kazoo-wall | D04 |
| D06 render in parallel | the sorted graph is split into independent chains rendered by a pool of real-time threads with a per-block barrier; one thread when the pool is 1 | kazoo-wall | D05 |
| D07 screamer and amp | adaptive oversampling (4× unless the drive asks for more), table-driven diode, warm-started solve; null test against the old model under a stated threshold | kazoo-fx or kazoo-wall | D01 |
| D08 fold and klon | Lambert W from a table with interpolation; klon diode solve tabulated per gain; same null tests | kazoo-fx or kazoo-wall | D01 |
| D09 fuzz and tape | fuzz oversampling and f64 to f32 where the null test allows; tape substeps adaptive | kazoo-fx or kazoo-wall | D01 |
| D10 reverbs and pitch | 4-point interpolation where modulation is slow; spring allpass count bounded; pitch and shimmer searches bounded per block | kazoo-fx or kazoo-wall | D01 |
| D11 SIMD blocks | block mixes, gains, sums and gathers through the `wide` crate (safe) | kazoo-wall, kazoo-mix | D02 |
| D12 desk callback | EQ coefficients ramped from precomputed tables not trig on the callback; aux reverb skipped when every send is zero; denormal flush in delay and chorus feedback | kazoo-mix, kazoo-fx | D01 |
| D13 saves and looks | the wall saves only on change, atomically; `look` and the console paginate | kazoo-wall | D05 |

### Track E: the wall's workflow

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| E01 chains | `:chain save <name>` keeps a selected run of modules and cables; `:chain add <name>` puts it down | kazoo-wall, kazoo-mcp | — |
| E02 folding groups | a group folds to one tile on the rack and unfolds | kazoo-wall | — |

## Order of play

1. D01 (benchmarks first, so every later number is against a baseline) and A01 side by side.
2. A02, A03, A04, A05, A06, A07, A08 in order. Track A touches every `main.rs`, so nothing else that edits a main runs beside it.
3. B01, B02, B03, B04 in order, with B07 and D02 beside them.
4. C01 through C10 in order, with D03 through D13 beside them (different crates).
5. B05, B06, E01, E02 fill gaps.

## How a lane runs

- Two lane checkouts: `~/Developer/projects/deno_rust/kazoo-lanes/a` and `/b`, full clones of this repo. A lane is a branch `lane/kz-<id>` from a named base sha, built and checked in its checkout, pushed to origin. Never main. The target dir of each checkout is held under `~/.aion/reaper-hold` before the first build and `cargo clean`ed the moment its lane is reviewed. Keep 60 GB free (59 GB today). At most two kazoo builds at once beside the other seats'.
- Dispatch: `CODEX_COMPANION_SANDBOX=danger-full-access node ~/.claude/plugins/cache/openai-codex/codex/1.0.6/scripts/codex-companion.mjs task --background --write --model gpt-6.1-sol --effort xhigh --cwd <checkout> --prompt-file <absolute path>`. The prompt file is the STACK RULES header below plus the brief. A resume note is written at dispatch time.
- Review: an Opus reviewer in the same checkout, from the template in `briefs/studio/REVIEW.md`, reads the brief and the whole diff against the merge-base, compares every fake against its real counterpart, hunts swallowed errors, unbounded buffers and wire-order faults, and fixes red-first in its own commits.
- Landing: one Codex job merges the reviewed lanes `--no-ff` in a stated order with the conflict resolutions written out, runs the pooled gate (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `ast-grep scan`, `cargo nextest run --workspace`, `cargo test --doc --workspace`), pushes main, rebuilds the release wall and mcp.
- Records: `briefs/studio/QUEUE.md` holds every START, END and REVIEWED line.

## STACK RULES header (prepended to every brief)

```
STACK RULES (these override the brief wherever they conflict). You work only in this directory,
<checkout>, a clone of the kazoo repository; create branch lane/kz-<id> from <base sha> (main may
have moved; your merge-base is <base sha>). Commit on this branch and push it to origin
(git push -u origin HEAD). Do NOT merge to main, do NOT push main, do NOT touch any other checkout,
do NOT install anything, do NOT edit Cargo.lock beyond what adding a named dependency needs.
Every cargo command runs as: nice -n 10 env CARGO_BUILD_JOBS=4 cargo <...>; never set
CARGO_TARGET_DIR. Do NOT run any full-workspace build, clippy or test; the pooled gate runs once at
the stack tip. Your checks are exactly: nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p <crates>;
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p <crates> --all-targets -- -D warnings;
cargo fmt --all --check; ast-grep scan <paths>. Doc tests only if you changed doc comments:
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p <crates>.
Rules of the tree: unsafe_code is forbidden; no #[allow] or #[expect]; no let _ =; no .ok() discards;
no empty Err arms; no if let Ok without else; nothing allocates, locks, formats or does I/O on an
audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green,
one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs.
Names may have changed since the brief was written: read the tree, find each item by what it does,
never rename what you find. Write logs under <log dir>. Ask no questions; every decision is in the
brief; a conflict with any standing rule goes under FOUND and you continue. Finish with the complete
END block, every field, as your last message.
```

## END block (every brief ends with it)

```
START KZ-<id> base=<sha> branch=lane/kz-<id> df=<GB> at <time>
END KZ-<id> head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
