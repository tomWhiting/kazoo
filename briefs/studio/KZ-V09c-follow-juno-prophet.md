# KZ-V09c: the Juno and the Prophet follow the voice

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-juno, kazoo-prophet, kazoo-cli (their `--follow` flags only). Time: about 30 minutes. Depends: KZ-V09a. May run beside KZ-V09b, V09d and V10 (shared file: `kazoo-cli/src/cli.rs` only, a line-level merge).

Read by Cassio, 13 Oct 23:5x. Changes: the checks take the lane form, and the by-hand listen is Cassio's and Tom's after landing. At most two lanes build at once. Opens only after V09a has landed.

Why: Tom: "i'd like voice to able to drive all the engines we have". The two analogue polys get V09a's pattern and V09b's poly rule exactly ("The poly rule": one voice through the normal allocator, `(index, serial)` with a steal guard, a per-voice `follow_hz` override with the 2 ms smoother under drift and detune, a per-voice smoothed gain, release by index, retrigger without reallocating). Read both briefs first; this one only names where each piece goes.

## The design (decided; build exactly this)

### kazoo-juno (poly, 6 voices)

- Allocator: `JunoSynth::note_on` (`synth/engine.rs:36-45`: free, releasing, oldest); make it return the index.
- Voice: `frequency` in Hz with drift (`synth/voice.rs:95`), used every sample (`:161`): the override replaces `frequency` in that use; drift multiplies on top.
- The chorus and the DCO's sub-oscillator follow automatically (they derive from the voice's frequency); say in a test that the sub stays an octave down.
- Plumbing: hub drain in `render` (`audio.rs:123-125`) → `apply_hub_message` (`:146-164`); `AudioCommand` `audio.rs:37`; keys `handle_key_press` `run.rs:282-345` (`i` free); footer status.

### kazoo-prophet (poly, 5 voices)

- Allocator: `ProphetSynth::note_on` (`synth/engine.rs:41-51`); make it return the index.
- Voice: holds `note: Option<u8>` and converts it to Hz every sample in `process` (`synth/voice.rs:99`). Add `follow_hz: Option<f32>` (smoothed): when `Some`, it is the base Hz instead of the note's; both oscillators' `fine_tune_cents` (`synth/oscillator.rs:43`, `:80`), the per-voice drift detune (`synth/engine.rs:50`) and the poly-mod path apply on top as they do to the note's Hz.
- Plumbing: as juno (`audio.rs:123-125`, `:146-164`, `AudioCommand` `audio.rs:37`, keys `run.rs:281-336`).

## Not this lane

303, cs80, dx (V09b); string, glass (V09d).

## Red tests first

Per crate (red: they ignore the voice today):
- `one_voice_follows_and_the_keys_keep_the_rest` (as V09b).
- `a_stolen_voice_is_let_go_and_taken_again` (all 6 or 5 voices from keys).
- `following_keeps_the_instruments_character`: juno: the sub-oscillator (mixed at `synth/voice.rs:142`) one octave under the voice within 5 cents; prophet: with oscillator B's `fine_tune_cents` at +7 (`synth/oscillator.rs:43`), the frequency the followed voice computes for oscillator B is the voice's Hz × 2^(7/1200) within 0.1 cent (read through a `#[cfg(test)]` accessor on the voice), exactly as for a key note.
- `the_render_never_allocates_while_following`.
- `kazoo-cli`: `follow_flags_parse` for `juno`, `prophet`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-juno -p kazoo-prophet -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-juno -p kazoo-prophet -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-juno kazoo-prophet kazoo-cli
```

## Definition of done

- [ ] All red tests red first (paste), then green; every existing test green.
- [ ] Not the builder's: after landing, Cassio starts `kazoo juno --follow` and `kazoo prophet --follow` with the desk and the mouth, and asks Tom to sing beside held keys. The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES and V09a's. Each `Follows` impl in its own `follow.rs`. Files under 1000 lines.

## END

```
START KZ-V09c base=<sha> branch=lane/kz-V09c df=<GB> at <time>
END KZ-V09c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
