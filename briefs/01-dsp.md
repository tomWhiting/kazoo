# DSP — Shared Audio Primitives

## What It Is

The shared foundation crate. Every instrument depends on this. It provides the building blocks: oscillators, envelopes, filters, and basic effects. No instrument logic — just pure DSP math.

## What It Does

### Oscillators

- **Sine** — Pure tone. Band-limited isn't necessary.
- **Saw** — Band-limited (PolyBLEP anti-aliasing). Essential for CS80 and arpeggio.
- **Square/Pulse** — Band-limited, variable pulse width. Sub-bass and leads.
- **Triangle** — Band-limited. Soft leads, sub harmonics.
- **Noise** — White and pink. Hi-hats, snares, texture.
- **Wavetable** — Load arbitrary single-cycle waveforms. CS80 expressiveness.

Each oscillator tracks phase as `f64` for precision and outputs `f32` samples. Supports hard sync (reset phase from another oscillator).

### Envelopes

- **ADSR** — Attack, Decay, Sustain, Release. The workhorse.
- **AD** — Attack, Decay only. For drums (no sustain needed).
- **Multi-stage** — Arbitrary number of segments with curve shapes. CS80 needs this.

Envelope curves: linear, exponential, logarithmic. Configurable per segment.

### Filters

- **State Variable Filter (SVF)** — Low-pass, high-pass, band-pass, notch. Resonance (Q). The core filter for everything.
- **Moog Ladder** — 4-pole (24dB/oct) low-pass with resonance. For that fat sub-bass.
- **One-pole** — Simple smoothing filter for parameter changes (anti-zipper).

All filters process sample-by-sample for modulation compatibility.

### Basic Effects (shared)

- **Waveshaper** — Soft clip, hard clip, tanh saturation. Warmth and grit.
- **Delay line** — Variable-length circular buffer. Building block for chorus, flanger, reverb.
- **DC blocker** — Remove DC offset from oscillator output.

### Utilities

- **Interpolation** — Linear and cubic for wavetable/delay reads.
- **MIDI note to frequency** — `440.0 * 2.0_f64.powf((note - 69.0) / 12.0)`
- **dB to linear / linear to dB** — Gain conversion.
- **Pan law** — Equal-power stereo panning.
- **Buffer operations** — Mix, scale, clear, interleave/deinterleave stereo.
- **WAV writer** — Thin wrapper around `hound` for f32 stereo at 48kHz.

## Interface

```rust
pub trait Oscillator {
    fn set_frequency(&mut self, hz: f64);
    fn next_sample(&mut self) -> f32;
    fn reset_phase(&mut self);
}

pub trait Envelope {
    fn trigger(&mut self);
    fn release(&mut self);
    fn next_sample(&mut self) -> f32;
    fn is_active(&self) -> bool;
}

pub trait Filter {
    fn set_cutoff(&mut self, hz: f32);
    fn set_resonance(&mut self, q: f32);
    fn process(&mut self, input: f32) -> f32;
}
```

## Dependencies

- `hound` (WAV I/O)
- No other external dependencies. Pure math.

## Testing

- Oscillators: Render 1 second of each waveform at 440Hz, FFT to verify fundamental frequency.
- Envelopes: Trigger with known ADSR values, sample at key time points, verify amplitude.
- Filters: Sweep a white noise signal through LP filter at known cutoff, verify frequency rolloff.
- All: Render to WAV for manual listening when needed.
