# Kazoo

A voice-driven synthesizer that transforms vocal input into synthesized instrument sounds.

Hum a melody and hear it as a synth lead. Beatbox a rhythm and hear it as electronic drums. Make mouth noises and hear them as evolving textures. Kazoo uses genuine synthesis techniques — granular, wavetable, vocoder, phase vocoder, and pitch-tracked resynthesis — to turn your voice into music.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  kazoo-tui (Ratatui terminal frontend)                  │
│  ┌─────────┐ ┌──────────┐ ┌──────────┐ ┌────────────┐  │
│  │Transport│ │ Waveform │ │ Spectrum │ │  Effects   │  │
│  │ Bar     │ │ (Braille)│ │ Analyzer │ │  Inspector │  │
│  ├─────────┤ ├──────────┤ ├──────────┤ ├────────────┤  │
│  │ Track   │ │ VU       │ │ Mixer    │ │  Help      │  │
│  │ List    │ │ Meters   │ │ View     │ │  Overlay   │  │
│  └─────────┘ └──────────┘ └──────────┘ └────────────┘  │
│         │ commands (channel)    ▲ display state (ringbuf)│
└─────────┼───────────────────────┼───────────────────────┘
          ▼                       │
┌─────────────────────────────────────────────────────────┐
│  kazoo-core (audio engine library)                      │
│                                                         │
│  ┌──────────┐  ┌───────────┐  ┌──────────────────────┐ │
│  │ Engine   │──│ Mixer     │──│ Tracks               │ │
│  │ (threads)│  │ (pan/vol) │  │ ┌─────┐ ┌─────────┐ │ │
│  └──────────┘  └───────────┘  │ │Synth│→│Effects  │ │ │
│       │                       │ └─────┘ └─────────┘ │ │
│       ▼                       └──────────────────────┘ │
│  ┌──────────┐  ┌───────────┐  ┌──────────────────────┐ │
│  │ Analysis │  │ Transport │  │ I/O                  │ │
│  │ (pitch,  │  │ (play,    │  │ (mic, speaker, WAV)  │ │
│  │  FFT,    │  │  record,  │  │                      │ │
│  │  onset)  │  │  loop)    │  │                      │ │
│  └──────────┘  └───────────┘  └──────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

## Synthesis Modes

| Mode | Description |
|------|-------------|
| **Pitch Tracked** | Voice pitch drives oscillators (saw, square, sine, triangle) |
| **Wavetable** | Single-cycle waveforms extracted from voice, played as wavetable |
| **Granular** | Voice buffer decomposed into grains, reassembled as clouds |
| **Vocoder** | Voice spectral envelope applied to synth carrier signal |
| **Phase Vocoder** | STFT-based time stretching and pitch shifting |

## Building

```bash
# Prerequisites: Rust 1.85+
cargo build --workspace

# Run the TUI (release mode recommended for real-time audio)
cargo run -p kazoo-tui --release
```

## TUI Key Bindings

| Key | Action |
|-----|--------|
| Space | Play / Pause |
| r | Record |
| s | Stop |
| j / k | Navigate tracks |
| h / l | Navigate parameters |
| 1-9 | Select track |
| Tab | Cycle panel focus |
| m | Mute track |
| S | Solo track |
| a | Arm track for recording |
| +/- | Adjust parameter |
| Enter | Edit parameter |
| L | Toggle loop |
| M | Mixer view |
| E | Effects view |
| [ / ] | Zoom waveform |
| ? | Help |
| q | Quit |

## Dependencies

| Crate | Purpose |
|-------|---------|
| cpal | Cross-platform audio I/O |
| rustfft | FFT for spectral analysis |
| fundsp | DSP synthesis primitives |
| pyin | Probabilistic pitch detection |
| hound | WAV file I/O |
| symphonia | Multi-format audio decoding |
| rubato | Sample rate conversion |
| ringbuf | Lock-free ring buffers |
| ratatui | Terminal UI framework |
| crossterm | Terminal event handling |

## License

TBD
