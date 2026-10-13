# Kazoo Studio: the plan

Written 12 October 2026 by Cassio from a sweep of the tree  (four survey reports: workspace, desk and hub, wall, instruments). Tom's word, by voice, 12 Oct: one CLI for everything; instruments start up and all connect into the mixer; the mixer feeds a tracker so you see full multi-track; record the audio and the signal; an effects pipeline and a mastering pipeline after that; thousands of modules across a track while delays and reverbs run; "high performance Rust ringing in your ears"; lo-fi sound is fine, the Rust has to be perfect. No code is written by Cassio: every lane is a brief fed to Codex, one at a time, each sized to one mechanism (about 30 minutes of a builder's work), reviewed by an Opus reviewer, then landed under one pooled gate.

## Where we are (12 Oct 2026)

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

### Track V: the voice

Tom's words, 13 Oct 2026, about 15:10 to 15:20: "I would really love to be able to sing the melody into the synth kind of thing or whistle it into the synth ... look at the mouth synthesizer part like I want it to be able to sort of capture like auto tune kind of things" (and then: "When I say auto-tune, obviously, I mean like quantization") "... but also to be able to just drive it direct, like, hertz, you know, hertz for hertz." Then: "id still love a dedicated synth, not just on the wall, but i'd like voice to able to drive all the engines we have".

**Where we are (sweep at `fa3c8d8`).** The voice path is kazoo-tui's engine (`kazoo mouth` runs kazoo-tui today, `kazoo-cli/src/main.rs:37`). Pitch comes from pYIN (`kazoo-core/src/analysis/pitch.rs`) on the analysis thread. It analyses non-overlapping 2048-sample buffers, so there is one estimate every 43 to 46 ms (`pitch.rs:139-154`). It averages the Hz of the five centre-padded sub-frames it gets back, so an octave flip reads as a note in between (`:186-217`). Its range is 60 to 1000 Hz (`:29-30`), so a whistle above 1 kHz is never voiced and the bottom of a low hum is cut off. Its sample rate stays at the default 44,100 (`:31`), because `engine::start` builds the detector from `config.pitch` (`engine/mod.rs:183`) and kazoo-tui passes `EngineConfig::default()` (`kazoo-tui/src/run.rs:85-88`), even though the stream runs at the device rate (`engine/mod.rs:176`). On a 48 kHz device every note therefore reads 147 cents flat (×44.1/48). The path from sound to synth is: input callback → mic ring → output callback → analysis ring → analysis thread, which sleeps 1 ms when the ring is empty (`analysis_thread.rs:95-105`) → `HeapRb<PitchEstimate>` of 32 → `drain_analysis_results` (`processing.rs:865-903`) → `set_pitch` on armed tracks → `PitchTrackedSynth`, with a 20 ms one-pole portamento (`synthesis/pitch_tracked.rs:128`). The onset detector is computed and thrown away (`analysis_thread.rs:125-127`). The desk carries notes (`NoteEventMsg`, 7-bit, no timestamp, `ipc/types.rs:482-540`). It carries no continuous control, and it disconnects an instrument that sends any type it does not know (`kazoo-mix/src/hub/instrument.rs:260`). No instrument handles pitch bend from the desk. The wall drops desk notes (`kazoo-wall/src/engine/mod.rs:1072-1076`) and has no module that takes anything from the desk.

**Decisions for the whole track** (each is argued in its brief):
- **A fast tracker with a slow judge.** A period-synchronous cycle tracker runs on the output callback and updates once per cycle of the note. It works the way a hardware pitch-to-voltage converter does: a Schmitt trigger on a filtered signal marks each cycle, and a normalised autocorrelation over exactly one period checks each candidate period. pYIN stays on a thread as the judge. It moves to overlapping 2048-sample windows with a 512-sample hop, takes the newest frame instead of an average, and runs at the device rate. The fusion takes octave and voicing from the judge and Hz from the tracker.
- **Latency budgets.** The first pitch arrives within 2 periods + 1 ms of the note's start: 2.3 ms for a 1.5 kHz whistle, 10 ms for A3, 31 ms for a C2 hum. A change of note lands within 2 periods of the new note + 1 ms. A voice frame goes out every 64 samples (1.3 ms at 48 kHz). The desk passes a frame from the mouth to a follower within 2 ms at p95 on a quiet machine. Taken together, from sound in to a following instrument's sound out is about 2 periods + 8 ms at 128-frame buffers.
- **The voice stream is its own message, not audio and not notes.** A batch of 40-byte frames goes out as `MSG_VOICE` (0x31). A follower asks for it with `MSG_VOICE_FOLLOW` (0x32), and the desk says it can carry it with `MSG_DESK_FEATURES` (0x24, hub → instrument). Three other routes were weighed and turned down:
  - B03's named streams are audio. Each takes a desk strip, goes into the mix and will be resampled by B05.
  - Notes plus `ParameterChange` are 7-bit and discrete. They would need every instrument's parameter table (B06), and 750 messages a second would fill the 64-entry request ring that notes share (`ipc/link.rs:88`).
  - The protocol version stays 3: a new desk tells an instrument it carries the voice, and an instrument built before this track never asks.
- **One way to follow, the same everywhere.** `i` toggles follow-the-voice on every pitched instrument, and `--follow` starts one following. `i` is in A05's list of keys left free for commands and is bound by no instrument today. `m` is the wall console's key for hearing the wall, so it is not used here. A mono instrument plays legato with continuous pitch. A poly instrument gives one voice to the voice, and keyboard notes keep the other voices.
- **Excluded from following, with reasons:**
  - kazoo-808: its notes choose a drum, not a pitch.
  - kazoo-arp: it makes notes for other instruments. Its only sound is a preview sine, and a voice would fight its pattern.
  - kazoo-play: it plays text phrases, and its jam engine is a 16-sine monitor.
  - kazoo-tui: the voice drives its armed tracks directly through its own engine (V06).
  - kazoo-perc, kazoo-sampler, kazoo-speech: libraries, not programs.
  - kazoo-mix: the desk.
  - The wall gets modules instead (V10).

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| V01 the judge at the right rate | pYIN runs at the device rate on overlapping windows (2048 window, 512 hop at 48 kHz, scaled for 88.2 kHz and up), reads the newest frame instead of an average, and covers 50 to 4000 Hz; a bench for one judge run | kazoo-core | — |
| V02 the scoreboard | `analysis::score`: synthetic takes (sines, a low hum, steps, vibrato, a glide, octave bait, a vowel, noise bursts, a whistle, re-articulation) with exact truth, plus WAVs dropped into `kazoo-core/tests/fixtures/voice/` scored against offline pYIN; latency, cents, octave, gross, missed and false-voiced figures; the `voice_score` example; baseline in BENCH.md | kazoo-core | V01 |
| V03 the cycle tracker | `analysis::cycle::CycleTracker`: a pitch per cycle on the callback, allocation-free, meeting the latency budget on the scoreboard | kazoo-core | V02 |
| V04 the voice frame | `kazoo_core::voice`: `VoiceFrame` every 64 samples (Hz, 1 V/oct with 0 = C4, gate, onset, loudness, breath, confidence); the judge thread (pYIN plus the onset detector, which is no longer thrown away); the fusion that folds octaves and vetoes false voicing | kazoo-core | V03 |
| V05 the quantiser | `voice::quantise`: scale and root, snap speed 0 to 400 ms, keep-vibrato, hysteresis, blend; the wall's scale tables as the one source | kazoo-core | V04 |
| V06 kazoo-tui plays Hz for Hz | the tui engine's armed tracks follow the voice frame, not the 46 ms pYIN; the judge thread replaces pitch and onset on the analysis thread; portamento default 3 ms | kazoo-core | V04 |
| V07a voice on the wire | `MSG_VOICE`, `MSG_VOICE_FOLLOW` and `MSG_DESK_FEATURES`; `HubLinkAudio::send_voice` and `drain_voice` on rings of their own; `HubLink::follow_voice` | kazoo-core | V04, B03a |
| V07b the desk carries the voice | the hub announces the feature, forwards voice only to followers, keeps one voice at a time, and the strip heads say `voice` and `follows` | kazoo-mix | V07a, B03b |
| V08a the mouth | new crate kazoo-mouth: mic in, the tracker, the quantiser, its own synth (tone plus breath "air"), the stream published on the desk; `kazoo mouth` runs it, kazoo-tui moves to `kazoo tui`; `kazoo mouth --score` | kazoo-mouth (new), kazoo-cli | V05, V07a |
| V08b the mouth's screen | the pitch readout, a 4-second trail (direct and snapped), meters, and every quantiser and synth control on keys | kazoo-mouth | V08a |
| V09a following: the shared part and the mini | `voice::follow` (`Follower`, the `Follows` trait, `FollowStatus`), `FOLLOW_VOICE = 'i'` in `kazoo_host::keys`, `--follow`; the mini follows legato | kazoo-core, kazoo-host, kazoo-mini, kazoo-cli | V07b, V08a, A05 |
| V09b following: 303, cs80, dx | the 303 follows legato, with its sequencer resting; one voice of the cs80 and of the dx follows, and keyboard notes keep the rest | those three, kazoo-cli | V09a |
| V09c following: juno, prophet | one voice of each follows | those two, kazoo-cli | V09a |
| V09d following: string and glass | the plucked and struck voices are retuned at frame rate while they ring | kazoo-string, kazoo-glass, kazoo-cli | V09a |
| V10 the voice on the wall | `voice` (pitch, snap, gate, level) and `breath` (breath, onset, sure) modules fed from the desk's stream; the wall follows while one is on it; the wall's `quant` and the mouth share one scale table | kazoo-wall | V07b, V05 |

**For Tom by hand (needed from V02 on, not blocking):** four takes into `kazoo-core/tests/fixtures/voice/`, each 10 to 30 s, mono or stereo, any rate, on the laptop mic at your normal distance: `hum-low.wav` (a slow low hum, the lowest you can), `vibrato-phrase.wav` (a sung phrase with vibrato), `slides.wav` (slides up and down between notes), `whistle.wav` (a whistled tune). `cargo run --release -p kazoo-core --example voice_score` scores them, and from V08a on so does `kazoo mouth --score kazoo-core/tests/fixtures/voice/*.wav`.

**Follow-up for C05a (recorded where C05a is next written, not built here):** C05a's `.kzev` record has kinds 0 to 4. Recording the voice stream beside a take needs kind 5 "voice frame", with the 40-byte frame as its body.

### Track L: loops and tape

Tom's words, 13 Oct 2026, about 15:25 to 15:35:
- On loops: "for that [the voice] to work alongside the wall ... we also really need like the tracking, like the loop stuff, because like I'd record something as a loop and have it work in and out over time ... I don't know if that's on the wall or in the tracker or both."
- On the look: "I love the aesthetic ... recreating sort of old analog stuff in the terminal ... we could really lean into that aesthetic." He also named Teenage Engineering-style interfaces, and: "Try to create the effect of like a spinning reel to reel tape thing. I had an Ampex ... an eight track. It had like the eight big preamps that were like as big as a VCR, kind of mounted on the top. The thing was taller than me ... I think a standard tracking view should be good, but I also think that'd be a great thing ... a loom-style concept, this idea of things being sort of woven together ... something we can do with the interface for a looping thing with reel-to-reel tape."
- The binding rule: "the dressing should never overpower function, the tracking view is why im saying that, we need to be careful vu meters and such dont take over."
- Then: "lets get them briefed and ready, and move through it as best we can."

**Where we are (sweep at `247ca07`).**
- The wall has no recorder of its own material. `wall_record` writes the master to a WAV (`daemon/record.rs`), and nothing plays a recording back.
- The wall takes no sound from the desk: `drain_hub` drops notes and parameters (`engine/mod.rs:1061`). Track V brings the voice to the wall as control frames (V10), not as sound.
- The mouth (V08a) sends only its synth to the desk.
- The wall's save is text with a hard size bound (`MAX_WALL_FILE`, `store.rs:67`), and its change log is stamped with UTC, not frames. KZ-W02 (frame-stamped replay) is not written yet.
- The desk has no recording, takes or timeline until Track C (C04, C06, C07).
- D03 culls any module that reaches no `out`, `scope` or `readout`.
- `tape` and `reel` are already kazoo-fx effect ids, so the new module is `loop`.
- For drawing, the wall console has its own braille grid (`tui/rack/screen.rs`'s layered dots, `tui/rack/dial.rs`'s `braille_bit`, `braille` and `dial::small`). The desk draws its VU needles with ratatui's `Canvas` and `Marker::Braille` (`kazoo-mix/src/ui.rs:963`).

**Decisions for the whole track** (each is argued in its brief):
- **One recording, two places.** The loop machine lives on the wall: a `loop` module (L02) whose level, like every knob, any cable moves. So a loop works in and out over time under the wall's own modulation. Every take it finishes is one reel in a shared store, and the same reel reaches the desk as a loop clip to place, repeat, mix and record (L10, L11).
- **Reels by hash, not in the save.** `~/.kazoo/reels/<sha256>.wav` is 32-bit float stereo, hashed over its samples. The wall's save and the desk's session name reels; they never hold audio.
  - A reel is kept while any pin list names it, and swept a day after nothing does.
  - Checkpoints are held to 4 GiB.
  - The sweep deletes nothing while any pin list is unreadable (L01, L04).
- **Loops in bars, in the wall's time.**
  - Length is `bars` × 4 + `beats`, read when blank tape is spliced, up to 96 s (4,608,000 frames, 36.9 MB).
  - Punches land on the loop's own bar lines.
  - The head is a pure function of the wall's beat, so a save, a load and a rebuild need no head state. A tempo change varispeeds the tape.
  - There are at most eight loops on a wall, the eight-track: 295 MB at 48 kHz.
  - Unequal lengths drift (Eno's *Music for Airports*). `look` says when they meet again (L05), and the loom shows it (L13).
- **Tape-like loss is optional.** `keep` (overdub feedback, default 1) and `wear` (3 dB and a quarter-octave a pass at full, read-time, bit-exact at 0). The seam is an 8 ms diagonal splice from a recorded tail, and so are the punch fades.
- **Exactness.**
  - The loop has no randomness, so the same save renders the same samples.
  - A take is a copy-on-write snapshot at a stated frame, logged by `kazoo-wall` as an undoable `What::Take` with its reel, so KZ-W02 can install it at that frame on replay. Between a punch-in and its take, replay plays what the wall alone would record.
  - A save waits up to 5 s for recording loops; a load says what it did not keep.
  - Checkpoints every minute bound a crash's loss.
- **Sources.** A loop records what is patched into it (any wall signal, the wall's own mix included), or a desk strip named by `from`.
  - The desk sends that strip back down the wall's link ("returns", L06, L07), each block stamped with the wall's own stream frame at which the desk played it.
  - The loop writes it 0.5 s behind its head, at the positions it was heard (L09). Tom's voice reaches the desk dry as `mouth/dry` (L08).
  - A return is not offered as a live wall module: heard live it would flam by the desk's lead, and only a loop, heard one pass later, can put it back on the beat.
- **Views, function first.**
  - The desk's timeline (C07, the standard tracking view) gets a loop clip drawn as one run with `↻` and seam ticks, and nothing else (L11).
  - The dressing lives in the wall console, reached on purpose with `T` and `W`, never the default:
    - the tape view: eight channel strips with braille VU needles, two reels turning one turn a bar with packs that grow as the loop fills, and the tape's bars under the heads (L12);
    - the loom: outs as warp, loops as weft, over where the loop is louder, past above the line and the predicted future below (L13).
  - Every mark is listed with the reading it carries.
  - The caps are 30 fps (tape) and 10 fps (loom), 1 ms a rebuild. An ASCII fallback below 100 × 30, or on `G`, keeps every reading.

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| L01 the reel store | content-addressed f32 WAV reels in `~/.kazoo/reels`, checked on read, pinned by holders, swept after a day's grace, budget constant | kazoo-core | — |
| L02 the loop module | `loop`: records on bar lines, plays in the wall's time with a spliced seam, overdubs with `keep`, wears with `wear`, copy-on-write snapshots, `pass` and `phase` outputs, never culled into silence by a fault | kazoo-wall | W01a (landed) |
| L03 the wall keeps its loops | loops in the save by reel, put back on load, undo, restore and rebuild by a tape thread; `erase` and `from` requests; eight at most; a loop is a D03 sink | kazoo-wall | L01, L02, D03 (landed), W01a (landed) |
| L04 takes | punch-outs become reels and undoable `What::Take` changes; checkpoints a minute apart; a save waits for its loops; pins, the sweep and the 4 GiB checkpoint budget | kazoo-wall | L03 |
| L05 loops for seats | `look` shows each loop's state, length, place, pass, wear and take; when unequal loops meet again; `wall_loop`; one line on the console's loop panel | kazoo-wall, kazoo-mcp | L04 |
| L06 returns on the wire | `MSG_RETURN_ASK/READY/AUDIO/DROP` (0x15 to 0x18) and `MSG_LOOP_TAKE` (0x50), gated by desk features; the link's return rings and `pop_return` | kazoo-core | B03a, V07a |
| L07 the desk sends returns | taps on any strip, group or the master, forwarded by the hub stamped in the asker's stream frames; `ret` on the strip head | kazoo-mix | L06, B03b, C01, C04, V07b |
| L08 the mouth sends its dry voice | `mouth/dry`, mono, sample-aligned with `mouth/main` | kazoo-mouth | V08a, V08b, B03a, B03b |
| L09 a loop records a desk return | the engine's return lines; a loop with `from` writes the return 0.5 s behind its head at the beats it was heard, with `nudge`; asks again every 5 s | kazoo-wall | L06, L03, L05, L02 |
| L10 loop takes reach the desk | the wall sends each punch take; the desk loads the reel with its seam baked, puts it on a `loops` strip's take list, places repeats, saves and pins it in the session | kazoo-wall, kazoo-mix | L04, L06, L07, L01, C06, C08a |
| L11 loop clips on the timeline | a loop clip drawn as one run with `↻` and seam ticks, moved and deleted whole; the tracking view stays plain | kazoo-mix | L10, C07a, C07b |
| L12 the tape view | `T`: eight channel strips with braille needles, reels turning with the beat whose packs grow with the take, the tape's bars, the counter; 30 fps cap, ASCII fallback | kazoo-wall | L05 |
| L13 the loom view | `W`: per-out levels from the engine; outs as warp, loops as weft, over/under by loudness, pass diamonds, past and predicted future; 10 fps cap, ASCII fallback | kazoo-wall | L12, L05 |

### Track T: the tree

Tom's words, 13 Oct 2026, about 19:20 to 20:10:
- "What happens if you don't have different songs? You just have the wall, and the wall branches, and it changes over time. But you can go back to any moment in time and skip timelines ... different branches, and the music just grows as a tree. And ... when we move from one branch to another, we transition the patches."
- "We're actually making the most of it being not just digital, but effectively text-based. And you start your song, and your song stays with you forever. And you can't ever change it, because you can't change what's happening in your life. But you can change the future."
- "Can we just use Git to version it? ... a little bit more like JJ ... contributions become commits ... and then we just replay the commits ... and if we store a time and all the settings and all that kind of stuff with them."
- "Tracks is just a complete misnomer for what this is ... It's all one journey that branches and forks ... come back to a bookmark and the second you change one dial ... it's another branch, like a little bud comes out. And if you leave it at that, you leave it at that ... the tree only grows as you make changes."
- On the thought experiments: "that solves a problem that I didn't even realize that we had ... with the birthmarks, I feel like if every patch, every module, every change all has a timestamp and every module has a birthmark ... that helps us just get the order of things right ... it might need to be built in early on."
- On the view: "The bevy stuff I'm less concerned about. I'd be happy with just some other kind of thing to begin with ... not required to play the music. But I do like the idea of a tree visualization of some kind, even if it's just a little one." Later, a Bevy tree you explore (thicker branches where more modules lived, dive in to see the graph, pull strings to play, "set the tree on fire ... all the smoke that comes off it is the music"). Reference: his Bevy project Meridian Life.
- On lanterns (Norn's design, `ablative/stack/norn/docs/design/norn-memory/DESIGN.md` and `MEMORY-AND-LANTERNS.md`; memory as songlines): "if that was tagged, you could go back and you could talk to yourself from then ... talk to the ancestors ... how was Tom feeling at that? How did that make him react?" Tom's definition (8 Aug): "A memory is a way of storing a note against something ... A lantern comes with an annotation like the note, but is a pathway back to have a conversation with the previous self through a fork."
- "Let's get all this stuff written down and done."

**Where we are (sweep at main after W01b, W01c and V01).**
- Every change is already logged, in order, append-only: `~/.kazoo/wall/log.jsonl`, 6,373 entries since 26 Sep (5,055 turns, 642 patches, 364 adds, 206 removes, 63 unpatches, 26 records, 11 tempos, 3 loads, 2 migrates, 1 speak). Each has `seq`, UTC `at`, `seat`, `what` and a summary. Not stamped with beats or frames (KZ-W02 is not written).
- A save is one JSON file (`~/.kazoo/wall/saves/<name>.json`, `MAX_WALL_FILE`). Loading replaces the whole wall at once; there is no transition.
- Module ids (`vcf9`) are unique on one wall but reused across walls and loads. Nothing gives a module a lasting identity.
- `toml` 0.9 is already a workspace dependency (kazoo-cli). No git crate is.

**Decisions for the whole track** (each is argued in its brief):
- **Real git, worked the jj way.** The tree is a git repository at `~/.kazoo/tree`, readable with `git log` and `jj log`, clonable and forkable. Nothing in it is ever rewritten: no reset of a branch backwards, no force, no merges. The tree only grows.
- **One text file per module.** `modules/<id>.toml`: kind, name, every knob one line, its birth mark, and its inputs (a cable lives in the file of the module it feeds, because an input takes one cable). `wall.toml` holds tempo, seed and rows. Each file round-trips exactly with the save format.
- **Birth marks, built in first.** Every module gets a birth mark when it is born: the seq and UTC instant of the change that added it plus the wall's fingerprint, unique across every tree. It never changes while the module lives, survives save and load, and is how a crossing matches modules (not by id). Every change and every commit carries its UTC time and the wall's beat.
- **A commit for every change.** Author is the seat, date is the change's UTC time, message is the log summary, trailers carry `Kazoo-Seq`, `Kazoo-Beat`, `Kazoo-Tempo` and the change as JSON. A glide is one commit (target and glide beats), not its steps. Commits are written by a tree thread off the control thread, never the audio thread, from a bounded queue; a full queue or a git failure is shown on the console and logged, and never stops the wall.
- **Moments, marks and buds.** Any commit is a moment. A mark is a named moment (a lightweight tag, `mark/<name>`); a branch is a line of growth. Going to a moment puts the wall there; the first change after that grows a bud, a new branch named for its moment and time. Leave it and it stays a bud.
- **The crossing.** Going to another moment is a transition, not a cut: matched modules (by birth) glide their knobs, modules only the destination has fade in, modules it lacks fade out and are then removed, changed cables crossfade. Default 16 beats; 0 is a cut.
- **The fire.** Replay walks a branch forward from a moment, applying each commit at its beat offset, at any speed. A change during the fire grows a bud at the commit the fire had reached; the past is never changed.
- **The first rings.** The existing log since 26 Sep is imported as the trunk, as far as it can be reconstructed exactly; any gap is a commit that says so.
- **Lanterns.** A note on a moment is a git note (`refs/notes/lanterns`), so notes grow later without changing the music. A lantern is a note plus a way back: the seat's Claude session id and message at that moment (resumed with `claude --resume <id> --fork-session`) and the span of Tom's spoken words around it from Dot's transcript, so anyone can ask the moment how it felt.
- **Views.** First a small tree in the wall console (branches as lines, thickness by modules alive, marks and lanterns on them, the wall's place). The Bevy tree, explorable and maybe VR, comes later and is not needed to play.

| Lane | Outcome | Crates | Depends |
|---|---|---|---|
| T01 the tree's files and birth marks | `modules/<id>.toml` and `wall.toml`, exact round trip with the save; every module born with a birth mark that survives save, load and undo | kazoo-wall | — |
| T02 a commit for every change | the tree thread writes one git commit per logged change, seat as author, UTC date, beat and seq trailers; bounded queue, failures shown, the wall never waits | kazoo-wall | T01 |
| T03 the first rings | the log since 26 Sep imported as the trunk, exact where it can be, gaps stated | kazoo-wall (tool) | T02 |
| T04 moments, marks and buds | `wall_mark`, `wall_go` (a cut), buds on the first change, `wall_tree`; console keys | kazoo-wall, kazoo-mcp | T02 |
| T05 the crossing | going to a moment glides, fades and crossfades by birth mark over N beats | kazoo-wall | T04 |
| T06 the fire | replay of a branch at its beats and any speed; a change mid-fire grows a bud | kazoo-wall, kazoo-mcp | T05 |
| T07a notes and lanterns in the tree | the note format (JSON lines, append only) in `refs/notes/lanterns`, kept out of every copy; writing, listing, visits, dimming by changes; readers for Claude and Dot transcripts (where Tom's words are, never what). Built from Tom's Norn lantern design (`MEMORY-AND-LANTERNS.md`, his 8 Aug definition); Tom, 13 Oct 20:2x: "the design for that has already basically happened now, so we're all good" | kazoo-wall | T04a, T04b |
| T07b the wall keeps lanterns | `lantern`, `note`, `visit`, `lanterns` requests, three a minute per seat, `Event::Lit`, `no_lantern` | kazoo-wall, kazoo-mcp (one arm each) | T07a |
| T07c lanterns for the seats | `wall_lantern`, `wall_note`, `wall_lanterns`, `wall_ask`; the server finds the seat's own conversation and the exact call; kazoo returns the fork command and never runs it | kazoo-mcp | T07b, T04c |
| T07d lanterns on the console | `n` lights a lantern; `B` lists lanterns and notes; Enter on a lantern shows its way back | kazoo-wall (tui) | T07b, T04d |
| T08a the tree's shape | a `shape` request: every limb grown from its parent, modules alive per commit, marks, lanterns and the wall placed | kazoo-wall | T07b |
| T08b the little tree | `Y` draws the tree in braille (plain fallback), thicker where more modules lived; every mark a reading | kazoo-wall (tui) | T08a, T07d, L12 (or a small lane split from L12 for `Dots` and plain glyphs) |
| T09 the Bevy tree | an explorable tree (native and wasm), after Meridian Life | new crate | T08 |

## Order of play

1. D01 (benchmarks first, so every later number is against a baseline) and A01 side by side.
2. A02, A03, A04, A05, A06, A07, A08 in order. Track A touches every `main.rs`, so nothing else that edits a main runs beside it.
3. B01, B02, B03, B04 in order, with B07 and D02 beside them.
4. C01 through C10 in order, with D03 through D13 beside them (different crates).
5. B05, B06, E01, E02 fill gaps.
6. Track V: V01, V02, V03, V04 in order (each scores on the last one's scoreboard). Then V05 and V06 side by side, since they share no files. V07a waits for B03a and V07b waits for B03b, because they edit the same wire files. V08a follows V05 and V07a. Then V08b, V09a and V10 run side by side. After V09a, V09b, V09c and V09d run side by side; they share only `kazoo-cli/src/cli.rs`, where each adds its own `--follow` flags, so the merge is line by line.
7. Track L: L01 and L02 side by side (different crates), then L03, L04 and L05 in order (each edits `daemon/loops/`). L06 waits for B03a and V07a, because they edit the same wire files, and L07 waits for L06, C04 and V07b. L08 runs once V08b has landed. L09 follows L05 and L06. L12 can run beside L06 to L09 (console files only), then L13. L10 waits for L07 and C08a, then L11 after C07b. The first thing Tom can play is after L05: a loop on the wall recording any wall signal, saved and undoable. Singing into it needs L06 to L09.
8. Track T: T01, T02a, T02b, T03 in order, then T04a, T04b, then T04c and T04d side by side (T02 and T04 were split by the ten-file rule; T03 and T04a both edit the tree thread, so they run in order), then T05, T06 in order. T07a, T07b in order, then T07c and T07d side by side (no shared file); then T08a, then T08b. T07 and T08 can run beside T05 and T06, two lanes at most at once. T09 waits for T08b. T01 goes first among new wall work because birth marks must be in before anything else saves modules.

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
