# kazoo-glass — Modal Bells and Glass

Glass adds tuned modal percussion to the Kazoo studio: eight decaying resonances per strike,
eight-note polyphony, velocity-sensitive mallets, stereo resonance spread and four short
stolen-note tails. There are no recordings or samples. Its public Rust library exposes
`GlassSynth`, `Patch` and `Source` for other frontends.

```sh
cargo run -p kazoo-glass --release
cargo run -p kazoo-glass --release -- --phrase "c4/8 g4/8 c5/4 [e4 g4]/4 r/4"
cargo run -p kazoo-glass -- --demo /tmp/kazoo-glass.wav
```

The demo renders 24 seconds of stereo float WAV at 48 kHz without opening a device or
connecting to a live hub. Each four-second segment plays one factory patch, in panel order:
Wine Glass, Celesta, Temple Bell, Soft Tines, Ice Chimes, Muted Bronze.

## Playing and editing

- `a s d f g h j k l ;` are white keys; `w e t y u o p` are black keys.
- `z / x` change octave; `c / v` change velocity.
- Up/down select a control; left/right or `- / =` edit it; Shift takes larger steps.
- Tab / Shift-Tab cycle presets; `1` through `6` select directly.
- `n` starts/stops the supplied phrase; `[ / ]` change tempo.
- Space quickly damps every voice, including released and stolen tails.
- `?` opens help; `q` or Escape quits.

Terminals that report key releases hold notes until release. Others use the existing Kazoo
key-repeat timing fallback and release notes automatically. The panel shows the current
patch, edited-state marker, controls, resonance ratios, waveform, voices, level and link status.

## Sound controls

| Control | Effect |
| --- | --- |
| Ring time | Fundamental decay to -60 dB: 0.2–16 seconds |
| Treble damping | Shortens upper-mode decay independently of the fundamental |
| Metal / glass | Morphs ratios from harmonic integers to bell-inspired inharmonic modes |
| Mallet hardness | Excites more upper modes; velocity also changes brightness and level |
| Strike position | Changes modal excitation according to the strike location |
| Stereo width | Alternates resonances across the stereo image; zero produces mono |
| Key damping | Release decay to -60 dB: 0.04–8 seconds, capped by each mode's held decay |
| Volume | Smoothed master gain |

Patch edits voice the next strike. Ringing notes retain their material, tuning and decay;
release uses the current key-damping control. This keeps material changes from retuning a tail.
Modes above 45% of the sample rate are omitted. Very high notes on a low-rate device may be
silent when even the fundamental lies beyond that limit. The final mix is softly limited.

## Studio integration

The executable reuses `kazoo-string`'s established audio, keyboard, phrase and front-panel
structure. It registers as `kazoo-glass` through `kazoo-core::ipc::link::hub_link`, automatically
joins/rejoins `kazoo-mix`, accepts hub note events (including `kazoo-arp`), and routes stereo
blocks to its mixer strip. Local output is muted while the desk routes it, avoiding doubled
sound. Standalone operation resumes when the link is absent.

A `--phrase` uses `kazoo-play` notation and the desk's scheduled transport frames; local play
and tempo controls become desk requests while connected. Keyboard, phrase and hub voices
have distinct ownership, so one source's release cannot cut another source's same-pitch note.

The audio callback preallocates its scratch buffers. Modal voices and stolen tails use fixed
arrays. Rendering performs no allocation, locks, socket calls or disk writes. WAV rendering
runs separately on the main thread. Pitch bend, hub parameter mapping and patch persistence
are not implemented.

## Benchmark verification — 30 September 2026

- Dean's laptop: `cargo test -p kazoo-glass -j 2` passed **54 tests** (12 library,
  42 executable). This includes allocator assertions for strikes, edits, voice stealing,
  phrase playback, command handling, snapshots and callback rendering; acoustic tuning
  within 0.2 cents on the tested notes at 44.1/48/96 kHz; real Unix-socket hub note and
  scheduled-transport tests; and full/small terminal render tests.
- Dean's laptop: `cargo clippy -p kazoo-glass --all-targets -j 2 -- -D warnings` passed.
- Local: `rustfmt --edition 2024 --check kazoo-glass/src/*.rs`, `ast-grep scan kazoo-glass`
  and `git diff --check` passed.
- The binary built remotely, ran locally through CoreAudio and rendered a 24-second demo.
  The local terminal session exercised phrase playback, preset selection, control editing,
  a keyboard note, panic and clean exit. This checks functioning playback; it is not a
  subjective listening assessment.
- The installed running mixer was not exercised; hub integration was checked with private
  protocol sockets. No workspace-wide gate or Jev review was run for this benchmark.

Ready-to-run local binary: `target/benchmark/kazoo-glass`.
Audio preview: `/private/tmp/kazoo-glass-demo.wav`.
