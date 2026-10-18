# Playing like Aphex Twin

Richard D. James (Aphex Twin) is the reference for kazoo's spirit. He is
known for modifying his gear and treating a machine as something to argue
with. He puts brutal, detailed rhythm under melodies as plain as a
lullaby. What follows is a set of ways of working taken from his records,
turned into moves on the wall. It is not a style to copy.

The records worth knowing, for what they teach:

- **Selected Ambient Works 85–92** (1992): warm, hissy, tape-worn analogue
  loops. Simple patches, left to breathe. The imperfection is the point.
- **Selected Ambient Works Volume II** (1994): largely beatless, slow,
  drowned in reverb. Sound as weather, not as song.
- **Richard D. James Album** (1996): drums edited down to tiny fragments,
  rolls and stutters faster than a hand plays, under string melodies and
  toy-like leads.
- **Windowlicker** (1999): the single's second track, known as "Equation"
  or "Formula", hides his face in its spectrogram. The sound is also a
  picture if you look at it the right way.
- **Drukqs** (2001): fierce, densely edited drums next to quiet pieces for
  a computer-driven piano (a Yamaha Disklavier).
- **Syro** (2014), and the **Cheetah** (2016), **Collapse** (2018) and
  **Blackbox Life Recorder 21f / in a room7 F760** (2023) EPs: dense, hardware-heavy, with
  squelching and an ear for microtuning.

He has also put tools out. **Samplebrain** (made with Dave Griffiths,
open-sourced 2022) rebuilds one sound out of the pieces of another.

## Moves on the wall

**Lullaby over the machine.** Keep one voice almost childishly simple: a
slow sequence of four or five notes on a soft wave. Put the busy, complex
detail in the drums. The contrast is the music.

**Edit time, don't just play it.** Play 3s against 5s: a `poly` (a 3, b 5)
on a fast clock gives two rhythms that meet every 15 steps. Set a clock's
`division` to 1/16 (position 0) and patch a slow square `lfo` (at its full
depth) into it at amount 0.33: it jumps up to 1/8 and back, like a drummer
losing their mind. A
`burst` gives ratchets that bounce. Short envelopes (decay well under 100
ms) make each hit a click, not a note.

**Tune it on purpose, then put it off.** Detune two oscillators by a few
cents for width: a `vco`'s `tune` is in semitones, so 0.05 is 5 cents. Then
pull one a quarter-tone away (`tune` 0.5) and let the melody sit in
between. Small pitch offsets read as character, not as mistakes.

**Wear the tape out.** A slow `lfo` (at its full depth) into a `vco`'s
`tune` at a tiny amount, about 0.004, wobbles it about 5 cents either way:
that is wow. `cassette` and `genloss` wobble the whole sound with their own
`wow` and `flutter` (flutter is the fast one), `vinyl` with `wow`, and
`reel` with `flutter`; `tape` is an echo,
so its wobble is in the echoes. Low-pass the top end, add a little noise
under it, and the patch sounds as if it came off a cassette found in a
drawer.

**Hide something in it.** Not every detail has to be heard on first
listening. A voice far down in the mix, panned hard, that changes once a
minute rewards whoever walks the tree later.

**Let the machine answer.** Feed a signal back into itself, set a sample
and hold off a noise source into a pitch, or let one envelope's output open
another's gate. Then listen instead of steering. When it does something
good, mark the moment (`wall_mark` keeps the knobs and cables) and record
it (`wall_record` keeps the sound: the random notes themselves are not
kept).

**Silence and room.** SAW II is mostly space. Turn things down. Take the
drums away entirely for a while. A long reverb tail on one held note can be
the whole piece.

## On a terminal

Kazoo is played from a terminal, and that suits this way of working. Text
is exact: a `tune` of 0.5 is exactly a quarter-tone, every time. Each
change is logged and can be undone. The tree keeps every moment, so a happy
accident is never lost. Work fast and small, listen, mark what's good, and
branch when you want to try a wild idea without losing the good one.
