# Kazoo — Kazoo Synthesizer

## What It Is

A kazoo. Not a sample of a kazoo — a synthesized one. The kazoo is a mirliton: you hum into it, and a membrane vibrates sympathetically, adding a buzzy, nasal timbre to the voice. Deceptively simple physics, surprisingly characterful sound.

## What It Does

### How a Kazoo Works (Physics)

1. The player hums a pitch (the excitation source).
2. The hummed sound enters the kazoo body (a tube).
3. A thin membrane (wax paper, plastic film) vibrates in sympathy with the sound.
4. The membrane adds odd harmonics and a characteristic buzzy resonance.
5. The tube shapes the resonance slightly.

### Synthesis Model

```
Hum Oscillator → Tube Resonance → Membrane Model → Output
                                        ↑
                                  Noise Modulation
```

**Hum Oscillator (Excitation):**
- Base waveform: sine or triangle (smooth, voice-like).
- Slight vibrato (pitch wobble) to simulate natural humming. Rate ~5Hz, depth ~10 cents.
- Optional formant shaping — a couple of resonant peaks to make it sound more "voice" and less "oscillator."

**Membrane Model:**
- Waveshaping (asymmetric soft clip) to add odd harmonics — this is the core of the kazoo buzz.
- Amplitude-dependent: louder input = more buzz. Quiet notes are smoother, loud notes are raspier.
- High-frequency resonance peak around 2-4kHz (the "buzz band").
- Membrane tension parameter controls the resonant frequency and buzz character.

**Tube Resonance:**
- Simple comb filter or short delay with feedback to simulate the tube body.
- Tube length parameter shifts the comb frequency.
- Subtle — the tube adds a slight "hollow" color, not a dominant effect.

**Noise Modulation:**
- A small amount of filtered noise modulates the membrane — simulates air turbulence and membrane rattle.
- Breath noise parameter controls how much air sound bleeds through.

### Parameters

| Parameter | Range | Description |
|-----------|-------|-------------|
| **membrane_tension** | 0.0 - 1.0 | Higher = tighter membrane, brighter buzz, higher resonance peak. |
| **buzz_amount** | 0.0 - 1.0 | Waveshaper drive. 0 = clean hum, 1 = full kazoo rasp. |
| **tube_length** | 0.0 - 1.0 | Comb filter delay. Affects the "body" of the kazoo. |
| **breath_noise** | 0.0 - 0.5 | Air/turbulence noise amount. |
| **vibrato_rate** | 3.0 - 8.0 Hz | Humming wobble speed. |
| **vibrato_depth** | 0.0 - 30.0 cents | Humming wobble amount. |
| **formant_shift** | -1.0 - 1.0 | Shifts the vocal formant peaks. Bigger/smaller "mouth." |
| **amp_adsr** | ADSR | Amplitude envelope per note. |

### Why It's Here

Every studio needs a wildcard. The kazoo is:
- Technically interesting (membrane modeling, nonlinear waveshaping).
- Musically useful as a novelty lead, comedic accent, or lo-fi texture.
- A good test of whether the DSP primitives are flexible enough for unusual instruments.
- Fun.

## JSON Input

```json
{
  "bpm": 120,
  "bars": 4,
  "patch": {
    "membrane_tension": 0.6,
    "buzz_amount": 0.7,
    "tube_length": 0.4,
    "breath_noise": 0.1,
    "vibrato_rate": 5.0,
    "vibrato_depth": 12.0,
    "formant_shift": 0.0,
    "amp_adsr": [0.02, 0.05, 0.8, 0.1]
  },
  "notes": [
    { "beat": 1.0, "note": 72, "velocity": 0.9, "duration": 0.5 },
    { "beat": 1.5, "note": 74, "velocity": 0.7, "duration": 0.25 },
    { "beat": 2.0, "note": 76, "velocity": 1.0, "duration": 0.75 },
    { "beat": 3.0, "note": 79, "velocity": 0.8, "duration": 1.0 },
    { "beat": 4.0, "note": 76, "velocity": 0.6, "duration": 0.5 },
    { "beat": 4.5, "note": 72, "velocity": 0.9, "duration": 0.5 }
  ],
  "output": "kazoo_melody.wav"
}
```

Notes are MIDI numbers. Kazoo range is roughly C4-C6 (60-96) — it sounds silly below that and harsh above.

## Dependencies

- `dsp` crate (oscillators, envelopes, waveshaper, comb filter, noise)
- `pulse` crate (clock sync)
- `serde` + `serde_json`
- `hound` (via dsp)

## Testing

- Render a single sustained note, verify it has the characteristic buzzy harmonic content (odd harmonics stronger than even).
- Buzz amount: compare buzz=0 (clean sine-like hum) vs buzz=1.0 (full rasp). FFT should show dramatically more harmonics at high buzz.
- Membrane tension: compare low (0.1) vs high (0.9), verify resonance peak shifts upward.
- Velocity response: loud note should be buzzier than quiet note.
- Vibrato: render with depth=0 and depth=30, verify pitch modulation in spectrogram.
- Full melody: render a simple tune, output WAV, verify it sounds like a kazoo (or at least something buzzy and charming).
