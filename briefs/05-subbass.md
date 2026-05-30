# Subbass — Sub-Bass Synthesizer

## What It Is

A monophonic bass synthesizer designed for deep, chest-rattling low end. Think 808 sub-bass, UK garage, dubstep, trap. One note at a time, optimised for frequencies below 120Hz. Simple but heavy.

## What It Does

### Signal Path

```
Osc 1 (sine/triangle/square) ──┐
                                ├── Mix ── Moog Ladder Filter ── Waveshaper ── Amp Envelope ── Output
Osc 2 (sine/sub-octave)     ──┘
```

### Oscillators

- **Osc 1** — Sine, triangle, or square. Main tone.
- **Osc 2** — Sub-octave oscillator. Always one octave below Osc 1. Sine or triangle only. Adds weight.
- **Mix** — Blend between Osc 1 and Osc 2 (0.0 = only Osc 1, 1.0 = only Osc 2).
- **Detune** — Fine detune between oscillators for thickness. Subtle — cents, not semitones.

### Filter

Moog ladder filter (24dB/oct low-pass). Even for sub-bass, the filter shapes the tone — rolling off harmonics from square/triangle waves, controlling the darkness.

- **Cutoff** — 20Hz to 2000Hz.
- **Resonance** — 0 to self-oscillation.
- **Envelope amount** — How much the filter envelope modulates cutoff.
- **Filter envelope** — Separate ADSR for filter modulation.

### Amp Envelope

- **Attack** — 0ms to 2s.
- **Decay** — 0ms to 5s.
- **Sustain** — 0 to 1.
- **Release** — 0ms to 5s.

### Waveshaper (Saturation)

Soft-clip distortion after the filter. Adds harmonics that make the bass audible on small speakers. Controllable drive amount.

### Glide (Portamento)

Smooth pitch slide between notes. Configurable time. Essential for bass slides.

## JSON Input

```json
{
  "bpm": 140,
  "bars": 4,
  "patch": {
    "osc1_wave": "sine",
    "osc2_wave": "sine",
    "osc_mix": 0.3,
    "detune_cents": 5,
    "filter_cutoff": 400,
    "filter_resonance": 0.2,
    "filter_env_amount": 0.5,
    "filter_adsr": [0.01, 0.2, 0.3, 0.1],
    "amp_adsr": [0.005, 0.1, 0.8, 0.3],
    "drive": 0.2,
    "glide_time": 0.05
  },
  "notes": [
    { "beat": 1.0, "note": 36, "velocity": 1.0, "duration": 0.5 },
    { "beat": 1.5, "note": 36, "velocity": 0.8, "duration": 0.25 },
    { "beat": 2.0, "note": 38, "velocity": 1.0, "duration": 1.0 },
    { "beat": 3.0, "note": 36, "velocity": 1.0, "duration": 0.5 },
    { "beat": 3.5, "note": 43, "velocity": 0.9, "duration": 0.5 }
  ],
  "output": "subbass_line.wav"
}
```

Notes use MIDI numbers. 36 = C2 (~65Hz), 24 = C1 (~33Hz).

## Dependencies

- `dsp` crate (oscillators, envelopes, Moog filter, waveshaper)
- `pulse` crate (clock sync)
- `serde` + `serde_json`
- `hound` (via dsp)

## Testing

- Render a single C2 sine, FFT verify ~65Hz dominant frequency.
- Glide test: two notes a fifth apart with glide, verify smooth frequency transition in spectrogram.
- Filter sweep: render with high envelope amount, verify frequency content changes over note duration.
- Saturation: compare clean (drive=0) vs driven (drive=0.8), verify additional harmonics present in driven version.
- Render a simple bass line, output WAV for listening.
