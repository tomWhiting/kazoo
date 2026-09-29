# kazoo-string — Plucked-String Physical Model

The studio had subtractive synths (303, Minimoog, Prophet, Juno, CS-80), FM (dx) and drums (808),
but no physical modelling. `kazoo-string` fills that gap: each note is a real vibrating string
computed as a Karplus-Strong digital waveguide. Like its siblings it is a standalone terminal
instrument that also plugs into the hub.

## How a note is made

- **Delay line and loop.** A delay line one period long feeds itself through a two-point
  averager (treble dies as the string rings), an optional pair of allpass dispersion stages
  (stiffness: upper partials run sharp, as in a piano wire, kalimba tine or bell) and a
  fractional-delay allpass for tuning.
- **Exact pitch.** Every delay in the loop is measured as a phase delay at the fundamental, so
  a note lands on pitch whatever the damping or stiffness. Ring time (60 dB) is honoured at the
  fundamental too. The loop gain is capped below unity, so it cannot grow at any frequency.
- **The pluck.** Noise, softened by pick hardness and scaled by velocity, is injected for one
  period. A second, inverted copy is injected behind it by the pick position, which cuts a comb
  into the spectrum: near the bridge is nasal, mid-string is hollow.
- **Release.** Letting go swaps the loop to the release gain, so a string is stopped rather
  than cut off.
- **Body.** Three fixed resonances (air, top plate, back) ring alongside the strings.
- **Polyphony.** Eight strings. A stolen string is handed to one of four ghost strings that
  mute in 25 ms, so stealing never clicks.

## Controls (all 0 to 1, edited in place on the panel)

decay, damping, pick position, pick hardness, stiffness, body, release, plus master volume.
Six factory patches: Nylon, Steel, Harp, Pizzicato, Kalimba, Dulcimer. Editing decay or release
also reaches strings that are already ringing; they keep their pitch and pluck.

## Layout

- `waveguide.rs` — the string: tuning solver, loop, exciter, pickup.
- `body.rs` — body resonances.
- `patch.rs` — controls, parameter mappings, factory patches.
- `synth.rs` — polyphony, voice allocation, ghost strings, master, scope.
- `audio.rs`, `phrase.rs`, `main.rs`, `ui.rs` — the same audio thread, hub link, `--phrase`
  player, keyboard handling and ratatui front panel as `kazoo-dx`.

```bash
cargo run -p kazoo-string --release
cargo run -p kazoo-string --release -- --phrase "c4/8 e4/8 g4/8 [c5 e5]/4 r/8 a3/8 c4/8 e4/4"
```

## Outcome (2026-09-29)

### Verified, on this Mac, with cargo

- `cargo test -p kazoo-string`: **81 tests pass**. That covers the DSP (pitch within 4 cents on
  notes 28 to 105, pick-position comb, ring time, release, mute, extreme and hostile input),
  levels (every patch between -24 and -3 dBFS for one note, an eight-note chord never clips),
  voice stealing without a step discontinuity, equal-power stereo panning, and the audio thread,
  hub link, transport following, phrase player and keyboard handling.
- `cargo clippy -p kazoo-string --all-targets -- -D warnings` is clean under the workspace's
  pedantic and nursery lints. `cargo fmt --check` and `ast-grep scan kazoo-string` are clean.
- The binary builds and its `--help` and argument errors work.

### Stereo

Notes pan across the keyboard with equal-power gains (middle C is centred at unity, the ends sit
0.55 of the way out), and a stolen string keeps its pan while it fades. The body hears the centre
of the mix and rings equally on both sides. Devices with any channel count are handled.

### Not verified

- The sound has not been listened to and the TUI has not been driven in a real terminal; those
  need someone at the keyboard.

### Not built

- Pitch bend and vibrato (pitch changes would need an interpolated delay read) and hub
  `ParameterChange` mapping.
- Bowed strings, sympathetic resonance and saving user patches.
