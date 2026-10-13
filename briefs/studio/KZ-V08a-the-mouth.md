# KZ-V08a: kazoo-mouth: a voice synth of its own that plays from the voice frame and publishes the voice on the desk

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-mouth (new), kazoo-cli. Time: about 30 to 40 minutes. Depends: KZ-V05 (quantiser), KZ-V07a (`send_voice`); A02 (`kazoo_core::host::audio`), A03 (`kazoo_core::host::link`), A04 (`kazoo_host::terminal`), A06 (`kazoo-cli`) are on main.

Read by Cassio, 13 Oct evening. Changes: the checks take the lane form; the smoke test no longer opens the mic or speakers (the wall plays on them), and the real start and the listen are Cassio's and Tom's after landing; a callback over 4096 frames is rendered in chunks so the 64-frame array always holds. Opens only after V05 and V07a have landed.

Why: Tom: "id still love a dedicated synth, not just on the wall, but i'd like voice to able to drive all the engines we have". The design has always planned this split: "the current broad TUI concept should stop being the whole studio and become the mouth-noise instrument" (`design/terminal-daw/CRATE_SPLIT.md:24`; `CHECKLIST.md:141-149` "kazoo-mouth Split"; `USER_STORIES.md:94-104` "Mouth instrument connects to mixer like any other client"). Today `kazoo mouth` runs all of kazoo-tui (`kazoo-cli/src/main.rs:37`). This lane makes the dedicated program: mic in, the voice tracker (V04) and quantiser (V05) on its callback, its own synth played from each frame, its audio to the desk like any instrument, and every frame published on the desk for everything else to follow.

## Outcome

`kazoo mouth` opens the mic and the speakers, plays a voice synth that follows the voice within two cycles (hertz for hertz, or snapped when quantise is on), plugs into the desk when one is running (playing direct otherwise, as every instrument does), and publishes a voice frame every 64 samples. `kazoo mouth --silent` publishes without making sound (the voice only drives other engines). `kazoo mouth --score a.wav b.wav` prints the scoreboard for those takes and exits. kazoo-tui moves to `kazoo tui`. This lane's screen is a plain status screen; V08b gives it its full screen and controls.

## The design (decided; build exactly this)

### The crate

`kazoo-mouth/` in the workspace members: `Cargo.toml` (`[lints] workspace = true`; deps: `kazoo-core`, `kazoo-host`, `crossbeam-channel`, `crossterm` with `event-stream` as kazoo-mini has it, `ratatui`, `color-eyre`, `ringbuf`; dev: `assert_no_alloc`, `hound`), `src/lib.rs` (`pub fn run(args: Args) -> Result<()>`, `pub struct Args`), `src/main.rs` (three lines, as A01 left every instrument), `src/run.rs` (arguments, start, the event loop), `src/engine.rs` (the audio side), `src/synth.rs` (the voice synth), `src/score.rs` (`--score`).

### Arguments

`kazoo mouth [--input <device>] [--output <device>] [--quantise <scale>:<root>] [--silent] [--score <wav>...]`. `--quantise major:D` turns quantise on (blend 1) with that scale (names as `SCALE_NAMES`, spaces as `-`, e.g. `pentatonic-minor`) and root (`NOTE_NAMES`, `#` allowed); an unknown name prints the list and exits 2. Parsed with clap in kazoo-cli (`MouthArgs`), mirrored into `kazoo_mouth::Args`, the way the other instruments' args are.

### Audio (`engine.rs`)

- Open the output with `kazoo_core::host::audio::open_output` (stereo, native rate, the named device or default); then the input with `open_input` at `RateChoice::Hz(<output rate>)`, 1 channel asked, the named device or default. An input that cannot run at the output's rate fails start with the host's error plus "the mic must run at the speakers' rate (<rate> Hz)". Why the same rate: one clock for tracker, synth and desk, with no resampler on the callback.
- Input callback: push channel 0 of each frame (`OutputInfo::channels` gives the stride) into `HeapRb<f32>` of `rate / 10` samples (100 ms); what does not fit is counted (`mic_dropped`).
- Output callback, per call (the `MouthEngine::render(&mut self, out: &mut [f32], channels: usize)` method, which owns everything):
  1. Drain the command channel (`crossbeam_channel::bounded(64)`): `SetQuantise(QuantiseSettings)`, `SetSynth(SynthSettings)`, `SetSilent(bool)`.
  2. Mic: if more than two blocks are waiting, skip the oldest so one block is left (as `processing.rs:788-806` does: the newest audio, not a backlog); pop up to the block's frames into a pre-allocated `mic` buffer, zero the rest.
  3. `judge_feed.push(mic)`; drain judge results into `tracker.judge(..)`.
  4. `tracker.process(mic, |frame| ..)`: for each frame, `quantiser.process(&mut frame)`, `synth.control(&frame)`, `hub.send_voice(&frame)`, and push it to the display ring (`HeapRb<VoiceFrame>`, 1024 frames: 1.4 s, for V08b's trail; full → oldest kept, newest counted). Frames are collected into a fixed array of 64 first (a 4096-frame callback makes 64; a longer callback is rendered in chunks of at most 4096 frames, so 64 always holds) and handled after `process` returns, so the closure borrows nothing else.
  5. `synth.render(mic, &mut mono)` into a pre-allocated mono buffer; unless silent, write it to every output channel; when the desk link is connected, send the stereo block with `send_audio` and silence the local output (the A03 rule every instrument follows; copy it from the instrument that does it most simply); when not connected, play locally.
- Judge: `voice::judge::spawn(PitchDetectorConfig::for_rate(rate), block, OnsetConfig::default())`.
- Link: `kazoo_core::host::link::connect_or_direct` (A03) with `LinkConfig::new("kazoo-mouth", 2, rate, block)`.

### The voice synth (`synth.rs`)

A dedicated, simple and complete voice synth, every number decided:

- **Tone**: `kazoo_core::synthesis::PitchTrackedSynth` at the stream rate, fed the mic block as its input (its envelope follows the voice, `pitch_tracked.rs:293`), its target frequency set to `frame.snapped_hz` on every frame with `voiced` (held otherwise). Settings: shape saw, detune 0, cutoff 2400 Hz, Q 0.9, portamento 3 ms, envelope sensitivity 0.5 (set through `set_param` with its `PARAM_*` indices).
- **Air** (the breath in the voice, so a breathy note sounds breathy): white noise (a xorshift32 here, seeded with a constant) through `kazoo_core::effects::BiquadFilter` band-pass at `clamp(2 · snapped_hz, 200, 8000)` Hz, Q 1.5 (coefficient update per frame, not per sample), scaled by `frame.breath · frame.loudness · air` with `air` default 0.25.
- **Gate**: a gain that rises to 1 in 3 ms when `frame.gate` and falls to 0 in 60 ms when not (one-pole per sample): the tone and air stop when the voice stops, with a short tail.
- **Out**: `(tone + air) · gate · volume` (volume default 0.7), then `x / (1 + |x|)` soft clip, then `sanitize_sample`.
- `SynthSettings { shape, cutoff_hz, air, volume }`, `Copy`, held to ranges (cutoff 100..=12,000; air and volume 0..=1).

### Scoring (`score.rs`)

`--score <wav>...`: no audio opened; for each path, `analysis::score::read_take`, then score `voice` (`FusedUnderTest`) and `pyin` on it, print `analysis::score::table(..)` to stdout and the gate table for `voice`; an unreadable file prints its error and the run exits 1 after scoring the rest. This is how Tom scores his own takes without cargo.

### The status screen (V08b replaces it)

`kazoo_host::terminal::enter`, one line each, redrawn at 30 Hz: the newest frame's note name, cents and Hz (direct and snapped), gate on/off, loudness, breath and confidence as numbers; quantise on/off with scale and root; the link state ("on the desk, strip N", "playing direct: no desk", "the desk is too old to carry the voice" when `desk_carries_voice()` is false while connected); `mic_dropped` and `voice_stats().dropped` when not zero. Keys: Space toggles quantise (blend 0 ↔ 1), Esc and Ctrl-C quit.

### The CLI (`kazoo-cli`)

- `Command::Mouth(MouthArgs)` runs `kazoo_mouth::run`; its doc becomes "the mouth: sing or whistle into a synth, and drive every other instrument with your voice".
- New `Command::Tui` runs `kazoo_tui::run(kazoo_tui::Args)` with the doc "the voice tracker DAW (kazoo-tui)"; kazoo-tui's usage text (`kazoo-tui/src/run.rs:13-20`) says `kazoo tui`.
- The A07 session file format (if it names instruments by subcommand) accepts `mouth` for the new program; update any test that lists subcommands.

## Not this lane

The full screen and controls (V08b). Instruments following (V09). Saving the mouth's settings between runs: decided not to — the defaults are hertz for hertz, and quantise is four keys away; nothing else in the mouth is worth a settings file yet. Mouth-noise modes other than the voice synth (granular, vocoder: they stay in kazoo-tui).

## Red tests first

Red for all: the crate does not exist. `MouthEngine` is built in tests with a fake hub that records `send_voice`/`send_audio` (a small trait `DeskOut { fn send_voice(&mut self, f: &VoiceFrame) -> bool; fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool; fn is_connected(&self) -> bool; }`, implemented for `HubLinkAudio` and for the fake), and with the mic fed by pushing into its ring.

- `a_frame_is_published_every_64_samples`: 1 s of a 220 Hz sine in 128-frame callbacks at 48 kHz: 750 frames sent, `at` from 64 to 48,000.
- `quantise_reaches_the_published_frame`: 30 cents flat A4 with `SetQuantise` (chromatic, snap 0, blend 1): published `snapped_hz` is 440 within 0.1 cents and `hz` stays flat.
- `the_synth_plays_the_voice_hertz_for_hertz`: sing 220 Hz then 330 Hz: the synth's output, measured by zero crossings over 50 ms windows, is 220 then 330 within 5 cents, and changes within `2/330 s + 1 ms + one block` of the change.
- `silence_in_is_silence_out`: after a note ends, the output falls below −80 dBFS within 300 ms (gate tail plus the tone's envelope).
- `silent_mode_publishes_but_plays_nothing`: `SetSilent(true)`: frames still sent, output all zeros.
- `plugged_in_the_desk_gets_the_audio_and_the_speakers_are_silent`: fake connected: `send_audio` called each callback, local output zero.
- `nonsense_mic_input_is_survived`: NaN, ±Inf: no panic, output finite, frames finite.
- `render_never_allocates`: 10 s of `vowel_ah` (from `score::suite`) through `render` inside `assert_no_alloc`.
- `score_reads_a_take`: a generated 1 s 220 Hz WAV in `target/kz-logs/mouth-score-test/`: `--score` output contains a `voice` row and a `pyin` row for it.
- `args_parse` (kazoo-cli): `kazoo mouth --quantise major:D --silent` parses to quantise on, major, root 2, silent; `--quantise lydian:H` is an error naming the choices; `kazoo tui` parses to `Command::Tui`.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mouth -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mouth -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mouth kazoo-cli
```

Doc tests if doc comments were written: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-mouth -j 4`. Read `df -h /System/Volumes/Data` before each command and put it in START and END.

## Definition of done

- [ ] All red tests red first (paste), then green; every existing kazoo-cli test green.
- [ ] Smoke without the mic or speakers: `kazoo mouth --help`, `kazoo tui --help`, and `kazoo mouth --score` on the test's generated WAV each exit 0 (paste the output). The builder never opens Tom's mic or speakers: the wall plays on them. Starting `kazoo mouth` and `kazoo tui` for real is Cassio's after landing.
- [ ] Not the builder's: after landing, Cassio installs and asks Tom to sing a slide and a whistle into `kazoo mouth`, then press Space and sing slightly flat to hear it snap. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The output callback (`MouthEngine::render`) never allocates, frees, locks, formats or does I/O: every buffer is sized at start for the largest callback the host gives (`OutputInfo::frames`, plus the A02 rule for longer callbacks: render in blocks of that size); `send_voice` and the judge feed's `unpark` are the only system calls. NaN/Inf defended at the mic (sanitised on entry) and at the output. Files under 1000 lines.

## END

```
START KZ-V08a base=<sha> branch=lane/kz-V08a df=<GB> at <time>
END KZ-V08a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
