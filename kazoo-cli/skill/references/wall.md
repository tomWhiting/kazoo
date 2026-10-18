# The wall in detail

`wall_catalogue` is the exact reference: every kind with every knob's
range, unit and default, and every port. `wall_catalogue {kind: "vcf"}`
narrows it to one kind, or to one family: synth, fx, perc, rhythm or
speech. This page is the map.

## Conventions

- **Levels:** audio and CV are nominally ±1.
- **Pitch** is 1.0 per octave, with 0 = C4 (261.6 Hz).
- **Gates** are high above 0.5.
- **Every knob is a jack.** A cable into a knob adds cv × half the knob's
  range × the cable's amount (−1 to 1), held to the range. Two exceptions:
  - `vcf1.cutoff`'s jack moves it 5 octaves per 1.0.
  - The `select` jack of `switch` and `select` adds whole positions: cv ×
    the number of positions.
- **Stepped knobs** come in two sorts in `wall_turn`:
  - Knobs with named positions (a `clock`'s `division`, a `quant`'s
    `scale`) take the number `wall_catalogue` shows beside each name on
    its `positions:` line. It is usually 0 for the first, but not always:
    the spring reverb's `springs` is `2=2 springs, 3=3 springs`.
  - Counting knobs take the count itself: `divide`'s `division` (1–32),
    the `steps` of a `seq` or `euclid`, a `loop`'s `bars`, a `switch`'s
    `positions`, and a `poly`'s `a` and `b`.
- **Glide:** a knob glides to its target over 0–64 beats, 2 by default.
- **Sound:** the wall starts silent, and only a console's `m` lets it out:
  to the speakers, and to the desk. Silent, it still plays, and
  `wall_record` still records it.
- **The desk:** while a desk is running, the wall joins it as `kazoo-wall`
  and follows its tempo and beat. Otherwise it keeps its own tempo.

**Where things are:**

| What | Where |
|---|---|
| State | `~/.kazoo/wall/` |
| Tree | `~/.kazoo/tree` |
| Recordings | `~/Music/kazoo-wall` |

**The seed patch** (92 BPM) is a starting point:

1. A clock drives an 8-step `seq`, through a `quant` (pentatonic minor in A).
2. Two VCOs a fifth apart go through a `mix` into a `vcf` (900 Hz).
3. Then a `vca` with an envelope (which also nudges the filter's
   resonance), then a tape echo, a plate reverb, and out.
4. A slow LFO sweeps the filter, and a smooth-random LFO moves vco2's width
   and the sequence's chance.

## The wall's own modules (family synth)

| Kind | What it is | In | Out |
|---|---|---|---|
| vco | Oscillator: octave, tune (semitones), shape (sine → triangle → saw → square), width, level, fm_depth | pitch, fm | out |
| lfo | Slow wave: rate, shape (sine, triangle, saw, square, random steps, smooth random), depth, offset, sync to bars | reset | out |
| noise | White, pink or brown noise | — | out |
| vcf | Filter: cutoff, resonance (capped below self-oscillation), mode (LP → BP → HP), drive | in | out |
| vca | Amplifier: bias + gain × cv (plain gain with no cv) | in, cv | out |
| env | ADSR envelope, 0 to 1 | gate | out |
| clock | Steady gates at a division of the beat, with swing | reset | out, beat |
| seq | Up to 16 steps of pitch (±24 semitones), a gate length and a chance | clock, reset | pitch, gate |
| sh | Sample and hold, with slew; samples its own noise when `in` is unplugged | in, trig | out |
| quant | Snaps a pitch to a scale and root | in | out |
| slew | Rise and fall times: portamento | in | out |
| mix | Four inputs, a level each | a, b, c, d | out |
| out | To the speakers: level (capped) and pan | left, right | — |
| compare | Above a threshold (or inside a window), as a gate, its inverse, and a trigger | in | gate, inverse, trig |
| logic | and, or, xor, nand | a, b | and, or, xor, nand |
| branch | Sends each gate to `a` or `b` by chance | in | a, b |
| switch | One input to one of up to 4 outputs | in, step, reset | out1..out4 |
| select | One of 4 inputs to the output | in1..in4 | out |
| divide | One gate out per N in | in, reset | out, count |
| math | Scale and offset two signals: sum, min, max, abs | a, b | sum, min, max, abs |
| scope | Measures two signals (passed on); `wall_probe` reads it | a, b | a, b |
| readout | Measures one signal (passed on) | in, reset | out |
| loop | A tape loop of whole bars, recording what is patched into it | left, right, rec | left, right, pass, phase |

## Effects (family fx)

Each effect takes `left` and `right` (right follows left when unplugged) and
gives `left`, `right`, `wet_left` and `wet_right`.

| Group | Effects |
|---|---|
| Drive | `klon`, `screamer` (overdrives); `fuzz`; `amp` (fender or marshall, cabinet on or off); `fold` (Buchla or Serge wavefolder); `crush`; `ringmod`; `comp`; `gate` |
| Time | `tape` echo; `bbd`; `digital` delay; `chorus`; `flanger`; `phaser`; `trem`; `shift` (frequency shifter); `granular`; `pitch` |
| Space | `plate`, `hall`, `spring` and `shimmer` reverbs; `resonator` (tuned to a note or a chord); `eq` |
| Lo-fi | `vhs`, `cassette`, `reel`, `vinyl`, `genloss` (generation loss), `radio`, `sampler12` |
| Shape | `sidestep` (turns the input's own cycles into a synth) |

`tape`, `digital` and `trem` can sync to note values.

## Drums (family perc)

Each voice is struck by a gate on `trigger`: how far the gate rises above
0.5 sets the velocity, and 1 or more is full. A gate high on `accent` as it
is struck accents it, and `choke` cuts it off. Each gives `out`.

- `kick` (808 or 909)
- `snare`
- `clap`
- `hat`
- `cymbal` (ride or crash)
- `tom`
- `conga` (bongo to tumba)
- `rim` (rimshot or clave)
- `cowbell`
- `shaker`
- `membrane` (a physical drum head)
- `modal` (marimba, vibraphone, xylophone, wood, glass, bell, steelpan,
  chime)
- `metal` (tuned FM metal)
- `noiseperc` (a noise sweep)

## Rhythm (family rhythm)

Each takes `clock` and `reset`.

| Kind | What it is | Out |
|---|---|---|
| euclid | Euclidean rhythm: steps, pulses, rotate | gate, accent |
| prob | A chance for each of 16 steps, with ratchets | gate, accent |
| grids | A drum map: x, y, and a density each for `kick`, `snare` and `hat`, plus chaos | kick, snare, hat, accent |
| poly | Two rhythms, `a` and `b` steps long | a, b, both, either |
| burst | Ratchets and bouncing balls | gate, level |

## Speech (family speech)

- **`vocoder`:** takes `carrier_left`, `carrier_right` (it follows the left
  when unplugged) and `modulator`, and gives `left` and `right`.
- **`speak`:** takes `gate` and gives `out`. It says the words `wall_speak`
  gives it.

## The console (`kzoo wall`)

`?` (or `:keys`) lists every key and command. The ones to know:

| Key | Action |
|---|---|
| Arrows or `hjkl` | Module and knob |
| `=` / `-` | Turn the knob (`+` / `_` finely) |
| Enter | Type a value |
| `[` / `]` | Shorter / longer glide |
| `a` | Add a module |
| `p` | Patch |
| `u` | Unplug |
| `x` | Remove |
| `z` | Undo the latest change |
| `v` | List or rack view |
| `m` | Hear the wall, or silence it |
| `r` | Record, or stop |
| `b` | Mark the moment |
| `B` | The tree |
| `:` | Commands: add, patch, turn, mark, go… |
| `q` | Leave (the wall plays on) |
| `Q` | Stop the wall for everyone (it asks first) |

## The tree (`kzoo tree`)

`?` lists every key. The ones to know:

| Key | Action |
|---|---|
| ↑/↓ | One change at a time |
| → / ← | Into a branch / back to where it grew |
| Tab | The next mark |
| `.` | Where the wall is |
| `hjkl` or a drag | Pan |
| `+` / `-` | Zoom |
| `0` | The whole tree |
| Enter | Put the wall at the selected moment |
| `n` | A batch's notes |
| `q` | Quit |

**The picture:**

- ◉ is where the wall is.
- ◆ is a mark, and ▪ a save's mark.
- ❖ is a batch, one knot for its changes.
- The bright limb is the wall's own line.
- A limb's thickness is how many modules are alive. Its colour comes from
  them and their settings; plumbing (out, mix, vca, scope, readout) adds
  none.
- Where a moment kept its sound, lighter is louder.
