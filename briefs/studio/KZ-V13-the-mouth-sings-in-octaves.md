# KZ-V13: the mouth sings in octaves and harmonies: a sixth effect, `harmony`, adds up to three shifted copies of the mouth's sound (an octave down, an octave up, a fifth, a third, or any interval) beside the dry sound, and the effects panel says its own keys

Line numbers are from main at `fe32b20` and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-mouth`.

Written by Cassio, 14 Oct 2026, 23:5x, from Tom's words below. Decided by Cassio; each decision's reason is beside it.

Place: Track V (the voice), after V08c and V08d (both landed). Time: about 45 minutes. Crates: kazoo-mouth only. kazoo-fx is read, never changed.

Files (8): `kazoo-mouth/src/engine/fx.rs`, `src/engine/fx/tests.rs`, `src/engine.rs` (`FxOrder` carries six), `src/controls.rs`, `src/controls/settings.rs`, `src/controls/tests.rs`, `src/ui/fx.rs`, `src/ui/tests.rs`.

## Why

Tom, 14 Oct 2026, about 23:50, playing `kazoo mouth`: "We'd also really love uh, like pitch shifting, like octave up, octave down, add the octave, you know, add octaves up and down and whatnot, or other kind of like, you know, harmonics and stuff like that to the voice."

And a minute before: "I can't figure out how to get to the different effects. Like I've just figured out how to turn them on and off... I can't figure out how to move it from megaphone to something else." The keys exist (`[` `]` move between effects, `controls.rs:793`), but the panel never says so; only the help card does (`ui/board.rs:326`).

kazoo-fx already has a clean pitch shifter for voices: `pitch` (`kazoo-fx/src/time/pitch.rs`), knobs `shift` (semitones, ±24), `fine` (cents), `formant` (`move` or `keep`), `mix` (equal-power dry to shifted). It plays one shifted copy. Octaves "added" to a voice are the dry voice plus copies, so this lane builds the harmony from up to three `pitch` instances run side by side on the same input, at `mix` 1 each, summed with the dry at levels the panel sets. No DSP is written beyond gains and ramps, as in V08c.

The polyphonic octave pedal (KZ-P11, `octaves`, octaves on chords without finding a note) is not finished (P11b came back with failing acceptance tests). When it lands, a follow-on lane adds it as this slot's second kind. Not in this lane.

## Outcome

1. **A sixth slot, `Slot::Harmony`**, named `harmony`, first in `Slot::ALL` and in a fresh chain's order (pitch before drive and space is how a pedalboard is laid out: a shifted copy then goes through the megaphone, the delays and the reverb with the dry). Its one kind is the mouth's own harmony over `pitch`; `ids()` answers `&["pitch"]`.
2. **Three voices, pre-allocated.** The harmony slot owns three `pitch` instances, built at chain construction for the running rate and block size, never in the callback. Each voice has a shift (semitones, −24 to +24, integer steps) and a level (−60 to +6 dB, and off at the bottom). `dry` sets the dry's level (−60 to +6 dB, off at the bottom). `formant`, shared by the voices: `keep` by default (Decision: on a voice, `keep` is what stops the octave up being a chipmunk, `pitch.rs`'s own advice), `move` the other choice. A voice that is off is not processed (no cost) and its instance is reset when it comes back on, so it never plays stale audio. Every level ramps over `RAMP_SECONDS` as the other slots' gains do.
3. **Presets, on the slot's chip, like the megaphone's models.** Row 0 of the slot cycles a preset with Left and Right. A preset sets the three voices' shifts and levels and `dry`, nothing else:
   - `octave down`: −12 at 0 dB; dry 0 dB.
   - `octave up`: +12 at −3 dB; dry 0 dB.
   - `octaves`: −12 at 0 dB, +12 at −3 dB; dry 0 dB.
   - `two down`: −12 at 0 dB, −24 at −3 dB; dry 0 dB.
   - `fifth`: +7 at −3 dB; dry 0 dB.
   - `power`: −12 at 0 dB, +7 at −3 dB; dry 0 dB.
   - `major third`: +4 at −3 dB, +7 at −6 dB; dry 0 dB.
   - `minor third`: +3 at −3 dB, +7 at −6 dB; dry 0 dB.
   - `custom`: what the knobs were last set to. Turning any voice knob by hand makes the chip read `custom`.
   The default preset is `octaves` (Tom's own words: "add octaves up and down"). Levels below the dry keep the voice on top (Decision: an equal-level octave up masks the voice's own pitch).
4. **The rows under the chip**, in this order: `dry`, `voice 1 shift`, `voice 1 level`, `voice 2 shift`, `voice 2 level`, `voice 3 shift`, `voice 3 level`, `formant`. Left and Right turn them, `d` puts one back to the current preset's value (or its default under `custom`).
5. **Five becomes six everywhere.** Every place fixed at five slots takes `Slot::ALL.len()`: `fx_input` and `fx_output` (`controls.rs:309-310`, `:369-370`), the `[` `]` wrap (`:795-797`), the `{` `}` bound (`:820`), `FxOrder` (`engine.rs:78`), the settings' `order` (`settings.rs:98`). No literal 5 or 4 that means "the slots" is left.
6. **Old settings files still load.** A `~/.kazoo/mouth/settings.json` written before this lane has five names in `order` and no harmony entry. It loads as before, with `harmony` put first and its defaults (`octaves`, off), and no note is shown for it (it is not a problem; it is a new effect). A file with six names loads them as written; one with a name twice, or an unknown name, keeps today's handling (`settings.rs:288-299`). The harmony's preset, voices, levels and `formant` are saved and loaded with the rest.
7. **The panel says its keys.** The effects panel's last line, always shown while it has focus: `[ ] effect · ↑↓ row · ←→ change · b on/off · { } move · d default`. If the panel is too narrow for it, the shortest form that fits: `[ ] effect · ←→ change · b on/off`, then `[ ] effect`. Never cut mid-word.
8. **It is new, so it starts off.** The harmony slot is off in a fresh chain and in an old file, so nobody's sound changes until they press `b` on it.

## Tests (write them first; each fails before the change)

- `engine/fx/tests.rs`:
  - A 220 Hz sine through `octave down` alone (dry off, voice 1 at 0 dB) comes out with its strongest partial within 1% of 110 Hz once settled; through `octave up` alone, 440 Hz; `fifth` alone, 329.6 Hz (±1%).
  - `octaves` with dry on: the spectrum holds 110, 220 and 440 Hz, each within 3 dB of the level the preset sets for it.
  - A voice switched off costs nothing: its instance's `process` is not called (count the calls through a test hook or by timing over 1,000 blocks: the off voice adds under 5%).
  - No allocation in the callback with the harmony on, all three voices on (the crate's existing `assert_no_alloc` guard).
  - Turning a level or switching presets mid-tone never steps: the 2 ms RMS moves by no more than the ramp allows (no jump over 1 dB within any 1 ms).
  - NaN and Inf in give silence out, and the next finite input plays.
- `controls/tests.rs`:
  - `[` and `]` reach all six slots and wrap; `{` and `}` move harmony to the last place and back.
  - Left and Right on row 0 cycle the nine presets and wrap; turning `voice 2 shift` makes the chip read `custom`.
  - An old five-name settings file loads with harmony first and off, with no note; a six-name file round-trips exactly.
- `ui/tests.rs`: the fx panel shows the key line at 80 columns, the shorter forms at narrower widths, and never cuts a word.

## Measure and report

- The harmony's cost per block at 48 kHz, 128 frames, with one, two and three voices on (release build, `cargo nextest run --release` with a timing test, or the crate's bench if it has one), against the whole chain's. Report it; no limit is set here, but say if three voices take more than a quarter of the block's time.
- The latency each preset adds to the shifted copies (from `pitch`'s own figures: 13 to 25 ms within an octave, up to 50 ms two octaves up): say it in the panel's chip help text, so the player knows the octave up arrives a little after the voice.

## Not in this lane

- The polyphonic `octaves` kind (KZ-P11): a follow-on lane once it lands.
- Pitch-tracked synth layers an octave from the mouth's own synth (cheaper and exact for the synth, but not for the dry voice): a separate decision, not made here.
- Anything on the wall or the desk.
