# Studio Architecture

## Overview

A terminal-based music production system built as a collection of Rust crates. Each instrument is an independent binary that communicates through a central clock/sync bus. Every instrument accepts JSON input and renders to WAV for automated testing. The system is designed for agents to build, test, and iterate without requiring a human to plug in headphones.

## Design Principles

- **Fast and lightweight.** No frameworks, no bloat. Raw audio math in f32 buffers.
- **Each instrument is standalone.** Own binary, own crate, own brief. Can be developed in parallel by different agents.
- **JSON in, WAV out.** Every instrument can render a performance from a JSON description to a .wav file. This is the primary testing interface.
- **Central clock, loose coupling.** The `pulse` crate broadcasts tempo and transport. Instruments subscribe. They can lock to the grid or drift.
- **Shared DSP primitives.** Common oscillators, envelopes, filters, and effects live in a shared `dsp` crate. Instruments compose from these.

## Audio Format

| Parameter | Value |
|-----------|-------|
| Sample rate | 48000 Hz |
| Bit depth | 32-bit float (f32) |
| Channels | Stereo (2ch) |
| Buffer size | 512 samples (10.67ms at 48kHz) |
| Output format | WAV (via `hound` crate) |

## Crate Map

```
studio/
  crates/
    dsp/          Shared DSP: oscillators, envelopes, filters, effects
    pulse/        Clock, transport, tempo sync bus
    mixbus/       Mixer, master output, WAV render
    drum808/      808 drum machine (synthesized)
    subbass/      Sub-bass synth
    arpeggio/     Arpeggiator with built-in oscillator
    cs80/         CS80-style polysynth (Blade Runner pads)
    kazoo/        Kazoo synth (lo-fi lead/melody)
    fx/           Effects rack (reverb, delay, chorus, saturation)
    sampler/      WAV sample playback and slicing
    tape/         Tape machine (saturation, wow/flutter, looping)
```

## Message Bus

Instruments communicate with `pulse` and `mixbus` over a lightweight message protocol. In-process: crossbeam channels. Cross-process: Unix domain sockets with msgpack framing.

### Clock Messages (pulse -> instruments)

```json
{
  "type": "tick",
  "beat": 1.0,
  "bar": 1,
  "bpm": 120.0,
  "sample_position": 0,
  "transport": "playing"
}
```

### Audio Messages (instruments -> mixbus)

Each instrument renders a buffer of f32 stereo samples per tick and sends it to the mixbus. The mixbus sums, applies master effects, and writes output.

### Command Messages (JSON input for testing)

Each instrument defines its own JSON command schema. Example for drum808:

```json
{
  "bpm": 120,
  "bars": 4,
  "pattern": {
    "kick":  [1,0,0,0, 1,0,0,0, 1,0,0,0, 1,0,0,0],
    "snare": [0,0,0,0, 1,0,0,0, 0,0,0,0, 1,0,0,0],
    "hihat": [1,1,1,1, 1,1,1,1, 1,1,1,1, 1,1,1,1]
  },
  "output": "test_beat.wav"
}
```

## Shared Dependencies

| Crate | Purpose |
|-------|---------|
| `hound` | WAV read/write |
| `serde` + `serde_json` | JSON command parsing |
| `crossbeam-channel` | In-process message passing |
| `clap` | CLI argument parsing |

No external audio device I/O required for core functionality. Device output (via `cpal`) is optional and only needed for live playback — not for testing.

## Testing Strategy

Every instrument ships with:

1. **Render tests** — JSON input -> WAV output -> verify file exists, correct duration, non-silent
2. **Frequency tests** — Render a known pitch, FFT the output, verify dominant frequency matches expected
3. **Sync tests** — Two instruments render the same tempo, verify beat alignment in mixed output
4. **Snapshot tests** — Render a reference WAV, compare future renders against it (regression detection)

Agents can also render recognizable melodies and ask a human to verify: "Does this sound like the Blade Runner opening?"

## Live Playback (Optional)

For interactive use, `mixbus` can open a `cpal` output stream and play the mix in real-time. But this is not required for development — WAV rendering is the primary output path.
