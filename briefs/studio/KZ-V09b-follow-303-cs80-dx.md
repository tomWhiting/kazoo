# KZ-V09b: the 303, the CS-80 and the DX follow the voice

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-303, kazoo-cs80, kazoo-dx, kazoo-cli (their `--follow` flags only). Time: about 30 to 40 minutes. Depends: KZ-V09a. May run beside KZ-V09c, V09d and V10. The only file these lanes share is `kazoo-cli/src/cli.rs`, where each adds its own flags: a line-level merge.

Read by Cassio, 13 Oct evening. Changes: the checks take the lane form, and the by-hand listen is Cassio's and Tom's after landing. "May run beside" is limited by the lane rules: at most two lanes build at once. Opens only after V09a has landed.

Why: Tom asked for this: "i'd like voice to able to drive all the engines we have". KZ-V09a decided how an instrument follows the voice and built the mini on it. This lane does the same for one mono instrument and two poly instruments, following V09a's "Decided for every instrument" and "The pattern each instrument follows" exactly: the `Follows` trait, `drain_voice` beside the hub drain, `AudioCommand::Follow`, `i`, `FollowStatus` in the footer, `--follow`, `snapped_hz`, the gain and the 2 ms pitch smoother.

## The poly rule (V09c and V09d use it too)

The voice gets one voice slot through the instrument's normal allocator:
- `follow_start(hz, v)` plays `note_on(nearest MIDI note to hz, v)` through that allocator, so voice stealing, envelopes and unison behave as for a key.
- It records `followed: Option<(index, serial)>`. Each voice gains a `serial: u32` that increments at every note-on.
- From then on, `follow_pitch` sets that voice's pitch override (below), and `follow_stop` releases that voice by index. It never releases by note number, because a key could be holding the same note.
- If the serial no longer matches, the keyboard stole the voice. Treat it as not sounding: drop `followed`, and the next frame with the gate open starts a new voice.
- `follow_gain` sets a per-voice gain target. Each voice gains `gain_target` (1.0 by default, and back to 1.0 at every key note-on) and a smoothed `gain`, which multiplies that voice's output. Keyboard voices are never touched.
- `follow_retrigger` re-triggers that voice's envelopes at the current pitch, without reallocating.
- **Pitch override:** each voice gains `follow_hz: Option<f32>` and a 2 ms per-sample smoother. While it is `Some`, it replaces the note's base frequency wherever the voice computes it. Drift, LFO, detune and unison multiply on top exactly as they do on the note's frequency today, so the instrument keeps its character. A key note-on sets it to `None`.

## The design (decided; build exactly this)

### kazoo-303 (mono)

- **Voice:** one `AcidSynth` with `target_freq`/`current_freq` in Hz, slewed every sample (`synth.rs:74-75`, `:154`, slew at `:205-209`). Hub notes are drained at `audio.rs:291-313` (`hub_note_on`/`off` at `:317-335`), and the sequencer plays at `render_chunk` `:371`.
- **`Follows`:**
  - `follow_start(hz, v)` is `note_on` non-legato, with accent when `v ≥ 0.8` (the hub path's rule: velocity ≥ 100 of 127, `audio.rs:317-335`). The pitch comes from a follow smoother (`FOLLOW_SMOOTH_MS`) that bypasses the slide slew, because the slew's shortest setting is a 15 ms time constant (`synth.rs:205-209`), too slow for a vibrato.
  - `follow_pitch` sets the smoother's target.
  - `follow_retrigger` is a non-legato `note_on` at the current pitch, with accent by the same rule.
  - `follow_stop` is `note_off`.
  - `follow_gain` is a smoothed multiplier before the 303's filter. The 303's character is its filter envelope, and gain after the filter would only turn the volume down.
- **The sequencer while following:** while following is on, sequencer steps do not play notes. The step position keeps advancing with the transport, so turning following off brings the pattern back in time. Why: the 303 is mono, and its line is now the voice.
- **Key:** `i` is free in the 303's handler (`run.rs:280-347`, which has no piano). The status goes in its footer.

### kazoo-cs80 (poly, 8 voices)

- **Allocator:** `Cs80Synth::note_on` (`synth/mod.rs:206-222`). Make it return the voice index it used. Its callers ignore the value.
- **Voice:** the base frequency is `frequency` (`synth/voice.rs:86`), multiplied by drift and LFO in `tick()` (`voice.rs:165-172`). The override replaces `frequency` in that product.
- **Plumbing:** the hub drain is the closure at `audio.rs:188-190`, which calls `apply_hub_message` (`:143-154`). `AudioCommand` is at `command.rs:25`, sent through `CommandLink`. The keys are at `run.rs:236-319`.
- **The CS-80's two layers:** both layers of the followed voice follow, because one voice is both layers on this instrument.

### kazoo-dx (poly, 8 voices)

- **Allocator:** `FmSynth::pick_voice` (`synth.rs:~600`) through `note_on`, with source `Source::Player` (`:374`).
- **Voice:** `voice.note_hz` (`synth.rs:571`) is read every sample (`:716`). The override replaces `note_hz` there, and every operator ratio multiplies on top as it does now.
- **Phrase mode:** when the dx is playing its `--phrase` loop, following still takes one voice. The phrase keeps the others.
- **Plumbing:** the hub drain is `drain_hub` (`audio.rs:272-292`), `AudioCommand` is at `audio.rs:34`, and the keys are at `run.rs:496-578`. `i` is not bound there, and `c`/`v`/`n`/`q` are unchanged.

## Not this lane

juno and prophet (V09c), string and glass (V09d), the wall (V10).

## Red tests first

In each crate, drive the audio render directly with frames, as V09a's mini tests do. Red for all of them: the instruments ignore the voice today.

- `kazoo-303`:
  - `the_303_follows_a_voice_legato`: a slide from 110 to 165 Hz over 200 ms. The output's pitch tracks it within 10 cents, at most 3 ms behind.
  - `the_sequencer_rests_while_the_303_follows`: no sequencer note sounds while following, and after `Follow(false)` the next step plays on its beat.
  - `a_loud_voice_accents`: a start at loudness 1.0 accents, and one at 0.45 does not.
- `kazoo-cs80` and `kazoo-dx`, each:
  - `one_voice_follows_and_the_keys_keep_the_rest`: hold two keys, then start the voice. Exactly one more voice sounds, at the voice's pitch within 10 cents. The two key voices keep their pitch and gain. The voice stops, and only its own voice releases.
  - `a_stolen_voice_is_let_go_and_taken_again`: fill all 8 voices from the keyboard so the followed voice is stolen. The next gate-open frame starts a new followed voice, and no key voice is changed by `follow_pitch`.
  - `following_keeps_the_instruments_character`: with drift/LFO (cs80) or an operator ratio of 2 (dx), the followed voice's spectrum moves with the voice's Hz exactly as a key note's does. Assert the ratio of the output's fundamental to the voice's Hz equals the key note's ratio within 5 cents.
  - `the_render_never_allocates_while_following`: `assert_no_alloc`.
- `kazoo-cli`: `follow_flags_parse` for `303`, `cs80` and `dx`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-303 -p kazoo-cs80 -p kazoo-dx -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-303 -p kazoo-cs80 -p kazoo-dx -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-303 kazoo-cs80 kazoo-dx kazoo-cli
```

## Definition of done

- [ ] All red tests red first (paste the output), then green. Every existing test green.
- [ ] Not the builder's: after landing, Cassio starts the desk, the mouth and each of the three with `--follow`, checks that each footer says "following the voice", and asks Tom to sing to each. The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES and V09a's. Per-voice `gain`, `serial` and `follow_hz` are plain fields set at construction. No allocation is added, and nothing locks on the audio thread. No file grows past 1000 lines: put each `Follows` impl in its own `follow.rs`.

## END

```
START KZ-V09b base=<sha> branch=lane/kz-V09b df=<GB> at <time>
END KZ-V09b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
