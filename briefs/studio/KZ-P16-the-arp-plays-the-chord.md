# KZ-P16: the arp plays the chord: with `--follow-chord` (or `h`), the arp's held notes are the chord Tom is playing, so a strum is arpeggiated on whatever synth the arp drives

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 16. It is one of the two chord consumers; P17 (the wall) is the other, and the two may run side by side.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: A2, C3 and E3 are notes 45, 48 and 52; keeping a note while it is within 0.80 semitone changes it 30 cents past the crossing; the arp's semitone steps are stated honestly, and the continuous paths stay on the wall and the voice. Accepted as written (the desk key in the hand check is now `C`, P15).

Time: about 30 minutes. Crates: kazoo-arp, kazoo-cli.

Depends: KZ-P15 (the desk forwards chords) must have landed before this lane opens, and with it P14.

Files (6): `kazoo-arp/src/chord.rs` (new), `kazoo-arp/src/chord/tests.rs` (new), `kazoo-arp/src/app.rs` (held notes from the chord; the key), `kazoo-arp/src/audio.rs` (drain the chord on the audio side), `kazoo-arp/src/run.rs` (`follow_chord` on the link), `kazoo-cli/src/cli.rs` (`--follow-chord`).

Why: Tom: "strum a chord and the synth arpeggiates it". The arp already turns held notes into patterns and sends them to any synth (B07; `--target`, `kazoo-cli/src/cli.rs:148`). Its held notes come only from its own keys (`app.rs:131`, `held: [bool; 128]`). This lane lets the played chord hold them.

## What is honest about pitch here

The arp's output is note events, and desk notes are 7-bit semitones with no bend (`kazoo-core/src/ipc/types.rs:482-540`). The arp therefore plays the chord's tones as the nearest semitones, and a slid chord moves the arpeggio a semitone at a time. Every other path in Track P carries continuous pitch: the chord's tones glide on the wall (P17), and Tom's own notes glide through the voice (P08). A note format with bend for every instrument is a protocol change across eleven programs, not part of this lane. The arp's help says this in one line.

Sweeps: "pick it in a sweep and it follows note by note" is the voice (P08, with P06's jump onsets). With both on, the arp arpeggiates the chord the sweep spells while a follower plays each swept note.

## Outcome

`kazoo arp --follow-chord` (or `h` in the arp) asks the desk to follow the chord. While a chord sounds, the arp's held notes are its tones, in their played octaves, and the arp's pattern, octave span, swing and latch apply as to keyed notes. A new chord replaces the held notes at the arp's next step, so the pattern never stumbles mid-step. Vibrato never changes a held note. A slide changes each tone's note once per semitone, 30 cents past the halfway point. When the chord stops sounding for 150 ms the notes release, unless latch is on. `h` again, or the flag off, gives the keys back.

## The design (decided; build exactly this)

### The held set (`chord.rs`)

```rust
pub struct ChordHold { notes: [Option<u8>; 4], silent_since: Option<u64> }
impl ChordHold {
    /// Fold one frame in; returns whether the held set changed.
    pub fn take(&mut self, frame: &ChordFrame, now_samples: u64, rate: u32) -> bool;
    pub fn notes(&self) -> impl Iterator<Item = u8> + '_;
}
```

- **Each tone to a note.** `n* = 69 + 12·log2(hz/440)`, with `hz` from `hz_from_volts(volts)` (V04). If the tone's slot already holds a note `m` and `|n* − m| < 0.5 + HYSTERESIS` (`HYSTERESIS = 0.30` semitone), keep `m`; otherwise take `round(n*)`, clamped to 0..=127. Why 0.30: vibrato of ±30 cents never reaches 0.80 semitone from its centre, and a slide changes note 30 cents after the true crossing, which is 3 ms at the steepest slide (16 cents a frame), too short to hear.
- Tones beyond `count` are `None`.
- **Silence.** A frame not sounding starts `silent_since`, and 150 ms of it clears the held notes. Why 150 ms: P13 drops a decaying chord under −50 dBFS, and a short gap between strums must not stop the pattern.

### Into the arp (`app.rs`, `audio.rs`, `run.rs`)

- The audio side drains `drain_chord` (P14) every callback into `ChordHold::take`. When the set changed, it publishes the new set through the arp's existing path from its key handling to its engine. Find how a key press reaches `engine.note_on` and `note_off`, and use that path, never a second one. It is applied at the next step boundary: the arp's own pattern clock decides, and the chord only changes what is held.
- In chord mode, key presses still play the arp's keyboard notes into the held set, alongside the chord (a player may add a note). Leaving chord mode releases the chord's notes and leaves keyed notes as they are.
- `run.rs`: `hub.follow_chord(true)` on the flag or the key. `Err(Unsupported)` puts "this desk does not carry chords" on the arp's status line, and the mode stays off. `NotConnected` keeps the wish (P14 sends it when the desk appears).
- The status line shows `chord: A min (following)`, `chord: waiting for a chord`, or nothing when off.

### Keys and flags

`--follow-chord` (kazoo-cli `ArpArgs`, mirrored into `kazoo_arp::Args`). The key `h`, if no binding of the arp's has it in A05's `kazoo_host::keys` map. If `h` is taken, the first free of `y` and `u`, named under CHANGED. `i` is the voice's (V09) and is not used.

## Not this lane

Pitch bend on desk notes. The wall (P17). Any change to the arp's patterns.

## Red tests first

`kazoo-arp/src/chord/tests.rs`, plus an app test where the held set reaches the engine. Red for all: none of this exists.

1. `a_chord_becomes_held_notes_in_their_octaves`: an A minor frame with tones at A2, C3 and E3 holds 45, 48 and 52.
2. `vibrato_never_changes_a_held_note`: frames of that chord with ±30 cents at 6 Hz on each tone for 2 s: the held set never changes.
3. `a_slide_changes_each_note_once_per_semitone`: frames gliding up a perfect fourth over 2 s (P13's sliding take, rendered to frames): each tone's note rises by exactly 5, one step at a time, each step 30 cents (±1 frame) past the half-semitone crossing. This is the arp's stepping, said in its help.
4. `a_short_gap_keeps_the_chord_and_a_long_one_releases_it`: 100 ms of not sounding keeps; 200 ms releases; with latch on, it never releases.
5. `the_chord_changes_at_the_next_step`: a chord change between two steps: the step in flight finishes with the old notes, and the next uses the new ones.
6. `keys_add_to_the_chord_and_leaving_releases_only_the_chord`.
7. `a_desk_without_chords_is_said_plainly`: a stand-in link answering `Unsupported`: the status line says so and the mode is off.
8. `the_audio_side_never_allocates_in_chord_mode`: 2 s of frames through the drain and `take` inside `assert_no_alloc`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-arp -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-arp -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-arp/src kazoo-cli/src
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-arp and kazoo-cli test passes.
- [ ] The arp's help has the one line on semitone stepping.
- [ ] Not the builder's: after landing, Cassio with Tom runs the desk with a guitar input (`C` on), `kazoo arp --follow-chord --target <a synth>` and that synth; Tom strums chords and slides one. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. `ChordHold::take` and the drain run on the arp's audio side: fixed arrays, no allocation, lock, format or I/O. Every frame's floats were sanitised on decode (P14), and `take` clamps again. No `as` casts clippy pedantic flags (notes through a checked conversion written once). Files under 1000 lines.

## END

```
START KZ-P16 base=<sha> branch=lane/kz-P16 df=<GB> at <time>
END KZ-P16 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
