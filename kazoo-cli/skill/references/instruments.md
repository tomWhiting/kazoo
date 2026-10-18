# The desk and the instruments

Every instrument is its own terminal app, `kzoo <name>`, with all of its
controls on screen. This page says what each one is and how it's played.
Each app shows its keys on screen, in its footer or with `?`.

**The desk.** Each instrument plugs itself into the desk (`kzoo mix`) when
one is running, and takes the next free strip. The desk then plays it, so
the instrument's own output goes quiet rather than sounding twice. With no
desk, an instrument plays on its own, and its header says which it is
doing.

**On another Mac:** `kzoo mix --listen 0.0.0.0:PORT` takes instruments over
the local network, and the desk shows its addresses. On the other Mac, run
`kzoo --desk host:port <instrument>`.

**The typing-keyboard piano** (cs80, mini, juno, prophet, dx, string, glass,
arp):

- The keys `a w s e d f t g y h u j k o l p ;` run from C to the E an
  octave above it: white keys on the home row, black keys on the row above.
- `z` and `x` move an octave down and up. `a` starts on C3.
- A terminal that can't report key releases holds notes instead:
  - in the cs80 and mini, Backspace lets them go;
  - in the juno and prophet, Space does;
  - in the arp, tap the key again;
  - the dx, string and glass let a note go once the key stops repeating.

**Tempo and play/stop** come from the desk for the 808, 303 and arp, for the
`--phrase` loops of the dx, string and glass, and for `kzoo play --jam`. The
mouth's looper takes the desk's tempo and bars, but you start and stop it
yourself. The cs80, mini, juno and prophet only take notes.

## kzoo mix: the desk

**Strips:** 8, and a master. Each strip runs trim → 3-band EQ (HI, MID, LO)
→ fader and mute → pan → the mix. A post-fader AUX send feeds a reverb,
whose return joins the mix before the master fader. Then come a soft limit
and the output.

**Tempo:** 20–300 BPM, default 120. The desk remembers it.

| Key | Action |
|---|---|
| ←/→ | Strip (past the last is the master) |
| ↑/↓ | Control on the strip |
| `1`–`8`, `9` | Jump to a strip, or the master |
| `+` / `-` | Nudge |
| `}` / `{` | Nudge coarsely |
| `0` | Reset the control |
| `R` | Reset the strip |
| `m` / `s` | Mute / solo |
| Space | Play / stop everything |
| `t` | Tap tempo |
| `[` / `]` | Tempo ∓1 |
| `c` | Metronome |

The fader stops at 0 dB on its way past. The mouse works too: click to
focus, drag a fader, turn a knob with the wheel. `--demo` plugs in a
built-in 808 pattern.

## kzoo 808: TR-808 drum machine

**Ten voices, all synthesised:** kick, snare, closed and open hats (the
closed one chokes the open one), clap, three toms, cowbell and cymbal.

**Patterns:** 16 steps each. A step is normal or accented. The bank starts
with A1, and `n` adds patterns, up to 16. `p` then a key picks one.

**Playing:**

- ↑/↓ choose the voice and ←/→ the step, and Space puts a hit there.
- `a` accents an active step, and `t` auditions the voice.
- Tab moves to the voice's parameters: tune, decay, tone or snappy where
  the voice has them, and level. The kick also has its accent amount, and
  the open hat its own decay (OH Decay).
- Enter plays and stops, `+`/`-` set the BPM, and `[`/`]` the swing. On a
  desk, these ask the desk.

**From the desk:** note numbers 0–9 trigger the voices, from 0 (the kick) to
9 (the cymbal). It is not General MIDI, so an arp aimed at the 808 needs
notes 0–9.

## kzoo 303: TB-303 acid bass

**The voice:** one voice. A saw or square (`w`) goes through accent and
drive, a resonant 4-pole low-pass (ladder-like, not a circuit copy), and the
amp envelope. Accent also opens the filter.

**The pattern:** 16 steps, each with a note (C1–C5), an accent (`a`) and a
slide (`s`).

- **Slide:** a step marked slide is glided into. The step before it holds
  its note, and the pitch slides into the slide step.
- **Notes:** `z`/`x` move a step's note by a semitone.
- **Pattern:** `r` loads the stock acid pattern.
- **Tempo:** Enter plays and stops. BPM is 20–300, default 132.

**Parameters:** cutoff, resonance, env mod, decay, accent, slide (how long
the glide takes) and drive.

**From the desk:** notes play it live. A velocity of 100 or more accents,
and a note pressed while another is held slides.

## kzoo cs80: Yamaha CS-80 pads

**The voice:** 8 voices, each with two layers, LAYER I and LAYER II. A layer
is a VCO (saw, pulse or sine) → a high-pass → a low-pass → a VCA. The sine
joins after the filters. Each layer's filter has its own envelope, from IL
(where it starts) to AL (its peak), then decay and release.

**After the layers** comes a ring modulator, and an LFO moves pitch, filter
and level.

**Drift:** each voice drifts in pitch and envelope timing, and voices differ
from each other a little even with Drift at 0. That is the CS-80's life.

**Keys:**

- Tab moves between sections (layer I, layer II, ring mod, LFO, mixer).
- ↑/↓ choose a parameter (arrows only: `j` and `k` are piano keys), and
  ←/→ change it.
- Shift-↑/↓ on held notes is aftertouch: it opens the filter and adds
  level.
- Ctrl-S and Ctrl-L quick-save and quick-load the patch.

## kzoo mini: Minimoog bass and lead

**The voice:** one voice, lowest note first. Three VCOs and noise feed the
mixer, then a 4-pole ladder filter with saturation, then the VCA. Glide is
a rate, in semitones per second.

**Cross-modulation:** osc 3 → osc 2 (FM), and osc 2 → the filter. The mod
wheel sends osc 3 only while osc 3 is in LFO mode: turn that on and raise
the mod wheel (in performance, 0 by default). The mod wheel's amount sets
the depth, up to a semitone of vibrato or the cutoff's own frequency again
on the filter, and Mod Dst picks pitch, filter or both. Osc 3's mixer volume
only decides whether you hear it too.

**Keys:** Tab moves between sections (oscillators, mixer, filter, envelopes,
performance), ↑/↓ choose a parameter and ←/→ change it. The mixer's Ext
input has nothing feeding it yet.

## kzoo juno: Juno-style chorus polysynth

**The voice:** 6 voices. A DCO gives saw, pulse (with LFO PWM), a sub an
octave down, and noise. Then a high-pass → a resonant low-pass → the ADSR →
a modelled BBD-style chorus (off, I, II or I+II).

**Keys:** Tab picks a section, ↑/↓ a parameter, and ←/→ change it. Shift on
a note plays it at full velocity.

## kzoo prophet: Prophet-5

**The voice:** 5 voices. Each has OSC A and OSC B (saw, pulse or triangle),
noise, a pre-filter gain, a resonant 4-pole low-pass, and filter and amp
ADSRs.

**Poly-mod** sends OSC B and the filter envelope to OSC A's pitch and to the
filter. OSC SYNC restarts OSC B each time OSC A starts its cycle. Voice
drift and OSC B drift add life.

**Keys:** as for the Juno.

## kzoo dx: four-operator FM

**The voice:** 8 voices × 4 sine operators, in one of 8 algorithms (drawn
on screen, from `4>3>2>1` to `1 2 3 4`). Operator 4 can feed back into
itself.

**Each operator** has a ratio, detune, level, velocity sensitivity, and an
envelope. There are also global feedback, LFO and vibrato controls.

**Patches:** 1 Tine Piano, 2 Solid Bass, 3 Glass Bell, 4 Brass Section, 5
Drawbar Organ, 6 Terminal Lead. Tab steps through them.

**Keys:** the arrows move around the grid, `-`/`=` change a value, and `c`/`v`
set the velocity.

**`kzoo dx --phrase "c4/8 e4/8 g4/8 r/8"`** loops a phrase (see `kzoo play`)
on the desk's beat. `n` plays and stops it, and `[` `]` `{` `}` set its
tempo.

## kzoo string: plucked string (Karplus-Strong)

**The voice:** 8 waveguide strings with a wooden body, panned across the
keyboard.

**Controls:**

- decay;
- damping (high is dark and woody, low is glassy);
- pick position (bridge to mid-string);
- pick hardness;
- stiffness (piano wire, tine, bell);
- body;
- release;
- volume.

**Patches:** 1 Nylon, 2 Steel, 3 Harp, 4 Pizzicato, 5 Kalimba, 6 Dulcimer.

**Keys:** ↑/↓ choose a control and ←/→ change it. `--phrase` works as for
the dx.

## kzoo glass: bells and glass (modal)

**The voice:** 8 voices × 8 resonant modes, from harmonic (a tine) to
inharmonic (a bell) as metal/glass turns.

**Controls:**

- ring time;
- treble damping;
- metal/glass;
- mallet hardness;
- strike position;
- stereo width;
- key damping (how long it rings after release);
- volume.

**Patches:** 1 Wine Glass, 2 Celesta, 3 Temple Bell, 4 Soft Tines, 5 Ice
Chimes, 6 Muted Bronze.

**Keys:** as for string. `--demo out.wav` renders a short tour of the
patches and exits.

## kzoo arp: Jupiter-8 style arpeggiator

**What it does:** it plays the notes you hold (or that the desk sends it) in
a pattern, on the desk's clock. It sends them through the desk to every
instrument, or to one with `--target <id>` (the instrument's 32-digit id:
the desk can't look up names). It has a quiet sine voice so you can hear it
on its own; `--silent` mutes that.

**Settings:**

- tempo (BPM; on a desk, the desk's);
- mode: up (the default), down, up/down, random, as played;
- division;
- swing;
- gate;
- octaves;
- latch.

**Keys:** ←/→ choose a setting and ↑/↓ change it. `1`–`5` pick the mode,
Space latches, and Enter plays and stops the studio.

## kzoo play: notes as text

**A phrase** is tokens separated by spaces, each `BODY/DURATION`. **Every
token needs a duration.**

- **Note:** a letter, an optional `#` or `b`, and the octave. `c4` is middle
  C (MIDI 60). Letters can be either case, and octaves can be negative, as
  long as the note is within MIDI 0–127.
- **Rest:** `r` or `rest`.
- **Chord:** `[c4 e4 g4]/4`.
- **Duration:** `/n` is 4/n beats.
  - `/4` is a quarter and `/8` an eighth.
  - `/6` and `/12` are quarter and eighth triplets.
  - One trailing `.` dots it (× 1.5).

**Ways to play it:**

| Command | What it does |
|---|---|
| `kzoo play 'c4/8 e4/8 g4/4'` | Lists the events |
| `kzoo play --play '...'` | Plays it once, straight to the speakers (not through the desk) |
| `kzoo play --jam --name bass 'c2/8 c2/8 r/4 eb2/8 g2/8 r/4'` | Joins the desk and loops in whole bars. A new line on stdin takes over at the next loop |
| `kzoo play --jam --drive '...'` | Sends the notes through the desk to the other instruments instead of sounding them |

`kzoo play --help` has the rest.

## kzoo mouth: sing into a synth

**The voice:** the microphone feeds a pitch tracker and a gate. Those play
an oscillator (sine, triangle, saw or square) → a filter, with air noise and
a vocoder voice. That goes to the looper, then through an FX chain, then to
the desk.

**Panels** (Tab moves between them):

- **SOURCE:** the synth.
- **PITCH:** quantise to a scale, snap, vibrato, blend.
- **GATE:** the open and close levels, and hold.
- **FX:** the chain.

**The FX chain:** six slots, harmony, megaphone, distortion, delay, echo
and reverb, all bypassed at the start.

- `1`–`6` pick a slot by its place in the chain.
- The slot's first cell picks its variant.
- `b` bypasses or enables it, and `{` `}` move it.

The chain is remembered.

**The looper:**

| Key | Action |
|---|---|
| `r` | Record or overdub |
| `u` | Undo a layer |
| `o` | Play / stop |
| `C` | Clear |
| `l` | Length |
| Shift-W | Save the loop |

`--quantise major:D` snaps to a scale from the start, and `--list-inputs`
names the microphones. Driving the other instruments by voice is not built
yet.

## kzoo tui: the voice tracker

A multitrack recorder driven by the microphone. Its synth modes (`t` cycles
them) are passthrough, pitch-tracked, wavetable, granular, vocoder and phase
vocoder.

**Views:** `1` Mixer, `2` Tracking, `3` Project, `4` Audio I/O.

**Keys:** Space plays, `s` stops, `r` records, `n` adds a track, `a` arms
it, and `?` lists everything.

## kzoo studio: a session

`~/.kazoo/studio/<name>.toml` names the instruments:

```toml
[[instrument]]
name = "dx"
args = ["--phrase", "c4/8 e4/8 g4/8"]
```

- **`kzoo studio up <name>`** starts the desk and up to 8 instruments in
  tmux, one pane each. Without tmux, it prints the commands to run instead.
- **`kzoo studio down <name>`** stops a session that `up` started.
