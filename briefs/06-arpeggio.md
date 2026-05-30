# Arpeggio — Arpeggiator

## What It Is

An arpeggiator with its own built-in oscillator. Feed it a chord, pick a pattern, and it cycles through the notes automatically in time with the clock. Can drive its own sound or output note events for other instruments.

## What It Does

### Arpeggiator Engine

Takes a set of held notes (a chord) and steps through them in a pattern, one note per step, locked to Pulse clock.

**Patterns:**

| Pattern | Description |
|---------|-------------|
| **Up** | Low to high, repeat. |
| **Down** | High to low, repeat. |
| **Up-Down** | Low to high to low (bounce). |
| **Down-Up** | High to low to high (bounce). |
| **Random** | Random note from held set each step. Seed controllable for reproducibility. |
| **Order** | In the order notes were pressed/specified. |
| **Chord** | All notes simultaneously on each step (rhythmic chords). |

**Parameters:**

- **Rate** — Steps per beat: 1/4, 1/8, 1/8T (triplet), 1/16, 1/16T, 1/32.
- **Octave range** — 1 to 4. Extends the pattern across octaves before repeating.
- **Gate length** — 0.1 to 1.0. How long each note sustains as a fraction of the step length.
- **Swing** — From Pulse or local override.
- **Velocity curve** — Static, accent-on-beat, ramp, or random variation.

### Built-in Oscillator

The arpeggiator includes a simple synth voice so it can produce sound directly:

- 2 oscillators (saw, square, triangle, sine) with mix and detune
- SVF filter with envelope modulation
- Amp ADSR envelope
- Optional chorus effect (from `fx` crate when available, or simple built-in detuned doubling)

This is a general-purpose poly/mono synth voice — intentionally versatile. The arpeggiator pattern is what makes it distinctive, not the sound source.

### Dual Output Mode

The arpeggiator can:
1. **Play its own sound** — Full audio output through built-in oscillator.
2. **Output note events** — Send MIDI-like note messages to another instrument (e.g., drive the CS80 with arpeggiated notes).
3. **Both** — Play its own sound AND forward notes.

## JSON Input

```json
{
  "bpm": 130,
  "bars": 4,
  "pattern": "up_down",
  "rate": "1/16",
  "octave_range": 2,
  "gate_length": 0.6,
  "swing": 0.1,
  "chord_sequence": [
    { "beat": 1.0, "notes": [60, 64, 67], "bars": 2 },
    { "beat": 1.0, "bar_offset": 2, "notes": [65, 69, 72], "bars": 2 }
  ],
  "voice": {
    "osc1_wave": "saw",
    "osc2_wave": "square",
    "osc_mix": 0.7,
    "detune_cents": 8,
    "filter_cutoff": 2000,
    "filter_resonance": 0.3,
    "filter_env_amount": 0.4,
    "filter_adsr": [0.01, 0.15, 0.2, 0.1],
    "amp_adsr": [0.005, 0.1, 0.6, 0.15]
  },
  "output": "arp_sequence.wav"
}
```

Notes are MIDI numbers. 60 = C4 (middle C).

## Dependencies

- `dsp` crate (oscillators, envelopes, SVF filter)
- `pulse` crate (clock sync — arp steps quantize to clock subdivisions)
- `serde` + `serde_json`
- `hound` (via dsp)

## Testing

- Up pattern: hold C-E-G, verify output plays C, E, G, C, E, G... in ascending order.
- Octave range: same chord with range=2, verify pattern extends C4-E4-G4-C5-E5-G5 before repeating.
- Gate length: render with gate=0.3 vs gate=0.9, verify note durations differ.
- Rate accuracy: render at 120BPM with 1/16 rate, verify 16 notes per beat in 4 bars.
- Random pattern with fixed seed: render twice, verify identical output.
- Full render: arpeggiate a minor chord, output WAV, verify it sounds musical.
