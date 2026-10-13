# KZ-V09d: the string and the glass follow the voice: plucked and struck voices retuned while they ring

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-string, kazoo-glass, kazoo-cli (their `--follow` flags only). Time: about 30 to 40 minutes. Depends: KZ-V09a. May run beside KZ-V09b, V09c and V10. The only file they share is `kazoo-cli/src/cli.rs`, and the merge there is line by line.

Read by Cassio, 13 Oct evening. Changes: the checks take the lane form, and the by-hand listen is Cassio's and Tom's after landing. At most two lanes build at once. Opens only after V09a has landed.

Why: Tom said "i'd like voice to able to drive all the engines we have". These two are the hard ones. A Karplus-Strong string fixes its delay line at the pluck (`kazoo-string/src/synth.rs:163-171`), and the modal glass fixes its mode rotations at the strike (`kazoo-glass/src/synth.rs:202`, `:224`). For them, following means a slide guitar and a bowed glass: the voice plucks or strikes, and its pitch retunes the ringing voice live. Follow V09a's pattern and V09b's poly rule exactly, except for what this brief changes.

## The design (decided; build exactly this)

### What is different from V09b's poly rule

- **`follow_start(hz, v)`** plucks or strikes one voice through the normal allocator. The string uses `StringSynth::pick_voice` (`synth.rs:249`). The glass uses `GlassSynth::pick_voice` (`synth.rs:250`), with `Source::Hub` (`:18-22`), the source hub notes use.
- **`follow_pitch(hz)`** retunes the followed voice whenever the smoothed Hz has moved more than 0.5 cent since its last retune. Under 0.5 cent nobody hears a difference, and the threshold keeps a held note from re-solving every frame. Retuning happens at most once per frame (750 times a second), never per sample.
  - **String:** call `StringTuning::solve` (`synth.rs:163-171`) for the new Hz and update that voice's delay length and fractional tuning in place. The delay line's contents are kept, which is what makes it a slide and not a re-pluck.
    - The delay buffer is sized at construction for the lowest note the string plays. If a retune asks for a longer delay than that buffer holds, clamp at the buffer's limit and count it in a `retune_clamped` counter, shown in the string's status line when it is non-zero.
    - If `solve` allocates, or is not bounded in time, write that under FOUND. Then add a `solve_into(&mut self, hz)` that reuses the voice's storage, and use it here.
  - **Glass:** recompute the followed voice's per-mode rotation coefficients (`cos` and `sin` of `2π · mode_ratio · hz / rate`, as the strike does at `synth.rs:202`, `:224`), and keep each mode's current state (amplitude and phase). Then the ringing bowl glides in pitch, and its decay and brightness carry on.
- **`follow_retrigger`** re-plucks or re-strikes the same voice at the current Hz, at velocity `v`.
- **`follow_stop`** damps the followed voice as a key release does on each instrument. The string damps; the glass lets its ring decay with the release the glass uses for a key release.
- **`follow_gain`** is a smoothed per-voice multiplier, as in V09b. It shapes a plucked string's sustain only by level. That is right for this instrument, because the string's own decay is its sound.

### Plumbing

| | Hub drain | `AudioCommand` | Keys (`i` free) |
|---|---|---|---|
| kazoo-string | `drain_hub`, `audio.rs:282-302` | `audio.rs:35` | `run.rs:462-544` |
| kazoo-glass | `drain_hub`, `audio.rs:282-304` | `audio.rs:35` | `run.rs` (A05's lane `:545-628`) |

`c`, `v`, `n` and `q` are unchanged on both. The phrase loop, when running, keeps the other voices.

## Not this lane

The other instruments (V09b, V09c). A bowed or sustained string model; the voice retunes the plucked string that exists.

## Red tests first

Red, per crate: today they ignore the voice.

- `a_slide_retunes_the_ringing_voice`: pluck or strike at 220 Hz, then slide the frames to 330 Hz over 300 ms with the gate held open. The output's pitch, measured by zero crossings (string) or by the strongest partial's frequency by FFT over 40 ms windows (glass), tracks the slide within 15 cents and at most 4 ms behind. Exactly one pluck or strike happened: the onset count, read through a test hook counting plucks or strikes, is 1.
- `a_held_note_is_not_retuned_every_frame`: a steady 220 Hz with ±0.2 cent jitter causes no retune after the first. The test hook counts retunes.
- `a_re_sung_syllable_plucks_again`: an onset with the gate open gives a second pluck or strike on the same voice index.
- `one_voice_follows_and_the_keys_keep_the_rest`, and `a_stolen_voice_is_let_go_and_taken_again` (as V09b).
- `string: a_retune_below_the_lowest_note_is_clamped_and_counted`.
- `the_render_never_allocates_while_following`.
- `kazoo-cli`: `follow_flags_parse` for `string` and `glass`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-string -p kazoo-glass -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-string -p kazoo-glass -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-string kazoo-glass kazoo-cli
```

## Definition of done

- [ ] All red tests are red first (paste the failures), then green. Every existing test stays green.
- [ ] Not the builder's: after landing, Cassio starts `kazoo string --follow` and `kazoo glass --follow` with the desk and the mouth, and asks Tom to sing a slide (the string should pluck once and slide) and to hum (the glass should ring and bend). The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES and V09a's. Retuning runs on the audio thread at frame rate: no allocation, lock or panic, and bounded work (one `solve`, or one set of mode coefficients, per frame). Each `Follows` impl goes in its own `follow.rs`. Files stay under 1000 lines.

## END

```
START KZ-V09d base=<sha> branch=lane/kz-V09d df=<GB> at <time>
END KZ-V09d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
