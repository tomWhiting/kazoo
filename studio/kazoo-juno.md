# kazoo-juno — Procedural DCO Chorus Polysynth

Separate TUI crate. Six-voice polyphonic. Roland Juno-60/Juno-106 inspired, but fully original and procedural: no sample playback, no captured waveforms, no borrowed sounds. Every audible component is generated from math in the audio callback.

## What Makes the Juno Sound

1. **Stable DCO tone.** The oscillators are intentionally steadier than a VCO synth. A little per-voice drift is present, but the character is clean and locked-in rather than woolly.
2. **One oscillator plus sub.** The classic fullness comes from saw/pulse mixed with a square sub-oscillator one octave down, not from two detuned VCOs.
3. **Simple signal path.** DCO mixer -> high-pass -> resonant low-pass -> VCA. Fewer moving pieces makes the instrument immediate.
4. **PWM.** A slow LFO modulates pulse width for motion without needing oscillator detune.
5. **The chorus is the instrument.** Two short modulated delay lines plus tiny generated hiss create the wide, swimmy 1980s ensemble effect.

## Current Implementation

```text
DCO saw + pulse + sub + white noise
    -> one-pole high-pass
    -> two-pole resonant state-variable low-pass
    -> ADSR VCA
    -> procedural BBD-style chorus
    -> soft limiter
```

### DCO

Generated per voice:

- Saw ramp from phase accumulator
- Variable-width pulse
- Sub square one octave below
- White noise from deterministic integer PRNG
- Per-voice phase offsets
- Small deterministic voice drift
- LFO-driven PWM

### Filter

- Static HPF amount removes low-end weight like the original front-panel HPF.
- Resonant low-pass uses a topology-preserving two-pole state-variable design.
- Cutoff responds to envelope and keyboard tracking.
- Nonlinear `tanh` saturation keeps hot resonance bounded and adds analog-ish edge.

### Chorus

- Modes: Off, I, II, I+II
- Two modulated delay taps
- Linear interpolation delay reads
- Procedural BBD hiss from PRNG
- Mono-compatible output for the existing audio plumbing

## TUI Controls

- Tab / Shift-Tab: section select
- j/k or Up/Down: parameter select
- h/l or Left/Right: adjust
- Space: all notes off
- Ctrl-Q or Esc: quit
- QWERTY keyboard: chromatic notes
- Shift while pressing a note: higher velocity

## Why This Crate Exists

Kazoo already has heavier flagship analog voices: Minimoog, Prophet, CS-80, and 303. The Juno lane is different: simple, stable, immediate, chorus-soaked polyphony for pads, basses, plucks, and synth-pop chords.
