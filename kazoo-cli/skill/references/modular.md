# Modular synthesis, for playing the wall

A modular synth is a set of small machines joined by cables. Each one makes,
shapes or steers a signal, and nothing sounds until something is patched
into the output. The wall is one of these, and it never stops. This page
covers the ideas, then the patches every player should know, then how to
play the wall well with other people.

## Signals

Three kinds of signal travel on the cables. The catalogue says which kind
each port carries.

- **Audio:** what you hear. It swings fast, many times a second.
- **CV (control voltage):** a slow number that moves a knob. On the wall,
  pitch CV is **1.0 per octave**, with 0 = C4: 1.0 is C5, −1.0 is C3, and a
  semitone is 1/12 ≈ 0.0833.
- **Gate:** on or off; high above 0.5. A gate starts an envelope, steps a
  sequencer or clocks a sample and hold. A short gate is a **trigger**.

**Every knob is also a jack.** A cable into `vcf1.cutoff` moves the cutoff
around wherever the knob sits. A cable's **amount** (−1 to 1) scales it, and
a negative amount turns it upside down. That is an attenuverter on every
cable.

## The modules, by job

- **Sources:** `vco` (an oscillator: the pitched voice), `noise` (every
  frequency at once: hats, wind, randomness), and `loop` (a tape loop that
  records what is patched into it and plays it back in bars).
- **Shapers:** `vcf` (a filter: takes away brightness or picks out a band)
  and `vca` (an amplifier: how loud, opened and closed by a CV).
- **Movers:** `lfo` (a slow wave for wobble, sweep and tremolo), `env` (an
  envelope: attack, decay, sustain, release, started by a gate) and `slew`
  (smooths jumps into glides: portamento).
- **Time and order:** `clock` (a steady pulse), `divide` (slower pulses
  from a fast one), `seq` (a sequencer: a row of steps played in turn), `sh`
  (sample and hold: grabs a value at each gate, so noise into an `sh` gives
  random notes) and `quant` (snaps a CV to the notes of a scale).
- **Logic:** `compare`, `logic`, `branch`, `switch`, `select` and `math` make
  patches decide things: "when this is above that, open the gate", "every
  third step take the other path".
- **Drums and rhythm:** voices from `kick` to `modal` bars and bells,
  `metal` and `noiseperc`, and `euclid`, `prob`, `grids`, `poly` and
  `burst` to play them.
- **Effects:** drive (`fuzz`, `fold`, `crush`…), time (`tape`, `digital`,
  `granular`…), space (`plate`, `hall`, `shimmer`, `resonator`) and lo-fi
  (`cassette`, `vinyl`, `radio`…).
- **The end of the line:** `mix` (several signals into one), `out` (to the
  speakers), and `scope` and `readout` (measure a point: `wall_probe` reads
  them).

`wall_catalogue` has every kind with its exact knobs, ranges and ports, and
[wall.md](wall.md) lists them all.

## The patches to know

**The subtractive voice (East Coast; Moog).** `vco` → `vcf` → `vca` →
`out`. A `clock` into `seq1.clock` steps a `seq`, which sends pitch to the
`vco` and gates to an `env`. The `env`
opens the `vca`, and the same `env` at a smaller amount opens the `vcf`, so
each note starts bright and darkens. Most basslines, leads and plucks are
this, with the knobs set differently.

**The pluck.** The same voice with a very short decay, no sustain, and
filter resonance up a little. Shorter is clickier, longer is rounder.

**The pad.** Two `vco`s a few cents apart (`tune` is in semitones, so 0.05
is 5 cents), a slow attack and long release, a slow `lfo` on the cutoff,
and `plate` or `hall` at the end.

**Generative melody.** A `sh` clocked by the `clock` (with nothing in its
`in`, it samples its own noise) → `quant` → the `vco`'s pitch. The patch writes its own tune, always in key. Slow the
clock for a meditation and speed it up for a solo. A `slew` after the
`quant` makes it sing between notes.

**The groove.** A `clock` into `euclid` (16 steps, 5 pulses) into a
`kick`, and `prob` into a `hat`. Rotate the euclid now and then, and drift
`prob`'s density with a slow `lfo`.

**The drone.** One or two `vco`s held open, the filter moved by two `lfo`s at
rates that hardly ever line up (0.13 Hz and 0.071 Hz meet again only after
about 1,000 seconds), or by one `lfo` on smooth random. It changes all the
time and seems never to repeat.

**FM.** One `vco`'s `out` into another's `fm` input, so its `pitch` cable
(from the sequencer) stays where it is, and `fm_depth` sets how much.

- Send the same pitch cable to both VCOs, so their ratio holds from note
  to note.
- A simple ratio between them (an `octave` apart; 3:2, a fifth, is `tune`
  7.02) stays musical. Other ratios ring like bells and metal.
- The wall's FM is linear and gentle: at most about index 1 at 1:1. Depth
  alone gives growl and warmth; ratio is what makes it metallic.

**West Coast (Buchla).** It shapes by adding instead of taking away: a
simple wave folded or driven into brightness, and a gate that closes volume
and brightness together like a struck object. The drive effects (`fold`,
a Buchla or Serge wavefolder, and `fuzz`, `screamer` and the rest) and a
`vcf` and `vca` opened by the same fast `env` get you there.

**Feedback and self-playing (the Krell patch).** Each note's envelope ends
by starting the next one, with its length and pitch taken from random
values. On the wall:

1. The voice: `vco1.out` → `vca1.in`, `env1.out` → `vca1.cv`, and
   `vca1.out` → `out1.left`. Give `env1` a quick attack (0.005 s or less)
   and a release near 1 s.
2. The loop: `env1.out` → `compare1.in` at amount −1, leaving
   `compare1.threshold` at 0 for now. Patch `compare1.trig` to `env1.gate`
   (the next note starts) and to `sh1.trig` and `sh2.trig`.
3. `sh1.out` → `quant1.in` → `vco1.pitch` gives each note a new pitch.
4. `sh2.out` → `env1.release` at amount 0.1 gives each note its own length.
5. Last, turn `compare1.threshold` to −0.02. As it passes −0.01 the first
   note starts. From then on, each time the envelope dies away below about
   0.01, `compare1.trig` fires and the next note starts. (The envelope
   arrives upside down, so this is 0.02 less half the default `hysteresis`
   of 0.02.)

Turning the threshold before the loop is patched fires the one trigger
into nothing, and the loop never starts. Inside one `wall_batch` the order
does not matter, since the wall hears the whole batch at once.

Once it starts, nobody needs to touch it: let it play. If it sits silent,
turn `compare1.threshold` up to 0.1 and back to −0.02 to start a note.

## Playing well, together

**Look first.** Call `wall_look` before changing anything: someone else may
be mid-gesture.

**Glide, don't jump.** `wall_turn` moves a knob over beats. A change over
eight or sixteen beats is weather; a jump is a crash.

**Change one thing, then listen** (`wall_listen`, or a `scope`). Big
changes are many small ones.

**Mark the good bits** with `wall_mark`. The tree keeps every moment, and a
mark is how you find one again.

**Branch to experiment.** `wall_go` back to a moment, `wall_look` (every
module is new after a go), then change it, and a new branch grows from
there. Nothing is ever lost.

**Leave room.** If the wall is busy, take something away before adding
something. Space is a part too.
