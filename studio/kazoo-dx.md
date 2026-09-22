# kazoo-dx — Four-Operator FM Synth

Written at 08:35 on 2026-09-23, before any code, for Tom's 30-minute new-model test.
The "Outcome" section at the bottom is filled in at the end.

## What I'm setting out to do

The studio has subtractive instruments (303, Minimoog, Prophet, Juno, CS-80), drums (808)
and a note scheduler (arp). It has no FM. `kazoo-dx` fills that gap with a Yamaha DX/TX81Z-style
four-operator FM synth. Like its siblings, it is a standalone terminal instrument that also
plugs into the hub.

## Scope (in priority order)

1. **Sound engine (`synth.rs`)**. Pure-math synthesis with no samples.
   - Four sine operators. Each has a frequency ratio, fine detune, output level,
     velocity sensitivity and its own ADSR envelope.
   - Operator 4 has self-feedback, as on the DX7.
   - Eight algorithms, which are the classic 4-op routing set from carrier stack to additive.
   - Eight-voice polyphony, with oldest-voice stealing and a smooth steal fade.
   - A master volume and a NaN/Inf guard on every output sample. It follows the
     `sanitize_sample` / `soft_limit` convention.
   - No allocation after construction. Everything is fixed arrays.
   - A handful of built-in patches (E-piano, bass, bell, brass, organ) to prove the engine.
2. **Audio thread (`main.rs`)**. It uses the same shape as the other instruments.
   - The cpal output callback owns the engine and drains a bounded crossbeam command channel.
   - It processes in fixed chunks, so any device buffer size is fully written.
   - It pushes display snapshots through a bounded channel.
3. **Hub integration (`ipc.rs`)**. A `HubLink` that matches kazoo-mini's.
   - It sends stereo audio to the hub when connected and falls back to standalone.
   - It plays NoteOn/NoteOff events from the hub, so kazoo-arp can drive it.
4. **Playable TUI (`ui.rs`)**. It uses ratatui, like the siblings.
   - The computer keyboard is a piano in the standard tracker layout, with octave shift.
   - Kitty keyboard enhancement gives true key release. Where a terminal cannot report
     releases, notes release automatically so nothing latches.
   - An operator grid shows ratio, level, feedback and ADSR per operator. You can navigate
     it and edit values directly, which follows the "no drawer" task-list rule.
   - An algorithm diagram, a patch selector, a live scope, active-voice lights and a help overlay.
5. **Quality gate**. Unit tests on the engine, `cargo clippy -D warnings` clean,
   `cargo fmt` clean, and it is added to the workspace.

## Stretch, if time allows

- Play a `kazoo-play` notation phrase through the FM engine (`--play "<notation>"`).

## Explicitly out of scope for 30 minutes

- Six-operator DX7 compatibility and SysEx patch loading.
- Saving and loading user patches to disk.

## Outcome

Written at the end of the session (the clock ran 08:35 to 09:05).

### Result

Everything in the scope list shipped, and so did the stretch goal. The crate is registered in the
workspace and has 36 unit tests. Clippy reports nothing for this crate under the workspace's
pedantic and nursery lints. `cargo fmt --check` is clean.

```bash
cargo run -p kazoo-dx --release
cargo run -p kazoo-dx --release -- --phrase "c4/8 e4/8 g4/8 [c5 e5]/4 r/8 a3/8 c4/8 e4/4"
```

### How each goal was met

1. **Sound engine (`synth.rs`)**
   - There are four sine operators per voice. Each operator has ratio, detune, level,
     velocity sensitivity and ADSR.
   - Operator 4 feeds back into itself, using a two-sample average as the DX7 does.
   - There are eight TX81Z-family algorithms, each stored as a modulator bitmask. A test proves
     every modulator has a higher index than its target, so one pass from OP4 down to OP1 is
     always correct.
   - There are eight voices. Voice allocation tries the same note, then a free voice, then the
     oldest released voice, then the oldest held voice. Envelopes restart from their current
     level, so stealing does not click.
   - Carriers are summed at equal power, so the four-carrier organ is not quieter than a
     one-carrier stack. A test checks every factory patch stays between -16 and -4 dBFS for a
     single note, and never exceeds 0 dBFS for a four-note chord.
   - There is one shared LFO for vibrato, with rate and depth kept per patch.
   - Every patch value is sanitised on the way in: NaN, infinities and out-of-range values
     are handled. A voice whose state goes non-finite is reset to silent. Output goes through
     `soft_limit` and then `sanitize_sample`.
   - A zero-sustain decay snaps to exactly zero, so it never creeps into denormal floats.
   - After construction, the engine allocates nothing.
   - There are six factory patches: Tine Piano, Solid Bass, Glass Bell, Brass Section,
     Drawbar Organ and Terminal Lead.
2. **Audio thread (`main.rs`)**
   - This follows the 303 and Minimoog shape. The cpal callback owns the engine and drains a
     bounded crossbeam channel. Every command is `Copy`, so sending never allocates.
   - The callback renders in fixed 1,024-frame chunks, so device buffers of any size are fully
     written. Sibling crates only process up to their scratch-buffer size.
   - Display snapshots are sent about 60 times a second over a bounded channel with
     `try_send`.
3. **Hub integration (`ipc.rs`)**
   - This uses the same `HubLink` as kazoo-mini. It connects to the hub with a 500 ms timeout
     and falls back to standalone mode.
   - When connected, it sends stereo audio to the hub.
   - Hub note-on and note-off events play the synth, so kazoo-arp can drive it.
   - The phrase follows the hub transport: its tempo, play, stop and pause.
   - **Verified live.** With kazoo-tui running, its header showed `●kazoo-dx`, kazoo-dx showed
     `● HUB`, and the hub's master L/R meters moved when notes were played.
4. **Playable TUI (`ui.rs`)**
   - The computer keyboard is a piano in tracker layout: `a` to `;` are white keys and
     `w e t y u o p` are black keys. `z`/`x` change octave and `c`/`v` change velocity.
   - With the kitty keyboard protocol, notes hold until the key is released, and Repeat events
     never retrigger.
   - Other terminals only send presses. There, the timing tells a held key from a fresh tap. It
     reads the macOS `InitialKeyRepeat` delay, with 250 ms as the fallback.
   - An event arriving near that delay counts as auto-repeat and keeps the note alive. A quicker
     second tap retriggers the note.
   - Automatic release comes at twice the delay after the first press, or 140 ms after the last
     repeat.
   - The operator grid is the control surface. Arrow keys move the cursor and `- / =` edit the
     value, with Shift for large steps. The header row holds the global fields: algorithm,
     feedback, LFO rate, vibrato and volume. There is no drawer, following task-list item 2.
   - The screen shows each operator's role (heard or modulating) and a live routing diagram.
   - The screen also has an ADSR sketch per operator and a decibel level meter.
   - There is a zero-crossing-triggered scope with auto-zoom, voice lights (held or releasing),
     a patch-modified marker and a help overlay.
   - The layout was checked in a real terminal at 100 and 150 columns.
5. **Stretch: phrase playback (`phrase.rs`)**
   - `--phrase` takes `kazoo-play` notation, parsed with `kazoo_core::notation`.
   - It loops sample-accurately inside the audio callback. Trailing rests count toward the
     loop length.
   - Tempo follows the hub by scaling a fractional playhead, so the phrase is never
     re-parsed on the audio thread.

### Review round (Opus, per CLAUDE.md)

An Opus reviewer read the whole crate at 08:52. It found no allocation, lock, panic or NaN
issue in the audio path. It raised five defects, and all five were fixed with regression tests:

1. **Stuck note.** A key released while Ctrl was held could leave its note ringing. Releases
   are now processed before the modifier check.
2. **Lost fast taps.** Without release reporting, a quick second tap was treated as
   auto-repeat and did not retrigger. It is now classified against the OS repeat delay.
3. **Stutter on long repeat delays.** A long OS repeat delay could retrigger a held key. The
   hold limit now scales with the real delay, and a silent held key ignores its repeats.
4. **Tempo drift.** The phrase loop drifted against the hub tempo, because each wrap dropped
   the fractional overshoot. The overshoot is now carried over.
5. **Phrase cutting other notes.** A loop wrap released keyboard and hub notes of the same
   pitch. Voices now record their source (player or phrase), and the phrase only releases
   its own voices.

### Decisions and trade-offs

- **Local output while on the hub.** kazoo-dx keeps playing locally while it also sends audio
  to the hub, as kazoo-mini and kazoo-808 do. Changing that belongs in one hub-wide decision,
  not in a single instrument.
- **Voice liveness.** A voice stays alive while any operator envelope is running, not just the
  current carriers. Changing algorithm mid-note therefore never cuts a ringing voice.

### Found along the way (not changed)

- **Hanging kazoo-tui test.** `cargo test --workspace` hangs in kazoo-tui at
  `input::tests::backtab_cycles_focus_backward`. It hangs with no hub socket present, and
  kazoo-tui does not depend on kazoo-dx. Every other crate in the workspace passes.
- **Clippy on kazoo-core.** `cargo clippy --workspace -- -D warnings` now fails in kazoo-core
  with the installed Rust 1.97 clippy. That is 16 lints, mostly `suboptimal_flops`, and all
  of them predate this work.

### Not done (out of scope, as planned)

- Six-operator DX7 compatibility and SysEx loading.
- Saving user patches to disk. Edits live until you change patch, and the `*` marker shows
  when a patch is modified.
