# Kazoo - Voice-Driven Synthesizer

## Project Overview

Kazoo transforms vocal input (humming, mouth noises, beatboxing) into synthesized instrument sounds through genuine synthesis techniques. This is NOT effects processing over voice — it is real synthesis driven by vocal analysis.

## Architecture

Two-crate Rust workspace:
- **kazoo-core**: Audio library with zero UI dependencies. Any frontend consumes this API.
- **kazoo-tui**: Ratatui terminal frontend.

### Thread Architecture (kazoo-core)
1. **cpal input callback** (OS-managed) — writes mic samples to ring buffer. ZERO processing.
2. **cpal output callback** (OS-managed) — the main audio workhorse. Reads mic from ring buffer, drains commands, runs mixer (synth + effects), applies soft limiter, writes directly to output buffer, pushes display state. All processing state is owned by this callback's closure.
3. **Analysis thread** — pitch detection, FFT spectrum, formant extraction, onset detection. Allowed to allocate.
4. **Disk I/O thread** — writes recorded audio to WAV files.

### Communication
- UI → Engine: `crossbeam-channel` (bounded MPSC) for commands
- Engine → UI: `ringbuf` (lock-free SPSC) for display state snapshots
- Input callback → Output callback: `ringbuf` for mic samples

## Coding Standards

This codebase runs mission-critical infrastructure for financial, legal, and healthcare settings.

**REQUIREMENTS:**
- **NO LAZY CODE:** Every implementation must be complete and robust
- **NO SHORTCUTS:** Handle all edge cases, no partial implementations
- **NO DEVIATING FROM PLAN:** Follow agreed approach; raise concerns before changing direction
- **NO DEFERRED WORK:** Set work is NOT optional, you must not defer tasks "for later"
- **PRODUCTION READY:** All code deployable immediately
- **STABLE:** All error cases handled, inputs validated
- **PERFORMANT:** Consider memory, complexity, efficiency

**The standard:** Would you trust this code with patient records, financial transactions, or legal documents? If not, it's not ready.

## Critical Architectural Rules

1. **Output callback is the processing engine:** No allocations, no locks, no file I/O, no panics. Pre-allocated state is moved into the callback at engine start.
2. **Pre-allocate everything:** All buffers allocated at engine creation, reused throughout lifetime.
3. **`unsafe_code = "forbid"`** across the entire workspace. No exceptions.
4. **Lock-free communication only:** Ring buffers between real-time threads, channels for command dispatch.
5. **NaN/Inf defense:** All audio processing must handle NaN/Inf inputs gracefully (output silence).

## Module Dependency Graph

```
lib.rs (Processor trait, Error, Db, Pan, TimePosition, constants)
  │
  ├── transport/    (TransportClock — no other internal deps)
  ├── analysis/     (PitchDetector, SpectrumAnalyzer, OnsetDetector, FormantExtractor, EnvelopeFollower)
  ├── io/           (device enumeration, file I/O, disk recorder)
  ├── effects/      (BiquadFilter, Delay, Reverb, Chorus, Distortion, FormantShift, EffectChain)
  ├── synthesis/    (PitchTracked, Wavetable, Granular, Vocoder, PhaseVocoder — uses analysis types)
  ├── mixer/        (Track, Mixer — uses effects + synthesis)
  └── engine/       (EngineHandle, threading, ring buffers — uses ALL modules)
```

## Build Commands

```bash
# Check everything compiles
cargo check --workspace

# Run all tests
cargo test --workspace

# Run clippy (pedantic + nursery lints enabled)
cargo clippy --workspace

# Check formatting
cargo fmt --check

# Run the TUI
cargo run -p kazoo-tui

# Run the TUI in release mode (recommended for audio)
cargo run -p kazoo-tui --release
```

## Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| cpal | 0.17 | Audio I/O (mic input, speaker output) |
| rustfft | 6.4 | FFT/IFFT for spectral processing |
| fundsp | 0.23 | DSP primitives and synthesis nodes |
| pyin | 1.2 | Vocal pitch detection (PYIN algorithm) |
| hound | 3.5 | WAV file read/write |
| symphonia | 0.5 | Multi-format audio file decoding |
| rubato | 1.0 | Sample rate conversion |
| ringbuf | 0.4 | Lock-free SPSC ring buffer |
| crossbeam-channel | 0.5 | Multi-producer channels |
| ratatui | 0.30 | TUI framework |
| crossterm | 0.29 | Terminal event handling |

## Code Review Standards

Reviews must use the Opus model. There is no such thing as a minor issue. Everything needs to be dealt with. Nothing can be skipped, nothing can be deferred, nothing can be of any standard other than the highest.

When requesting a review, provide: the plan (if any), original task intent, relevant files/folders. The reviewer must explore beyond those files to their satisfaction and give robust, constructive feedback. Every issue must be addressed before shipping.

## Vision: Terminal Core

This is real music software for real musicians. David Hirschfelder, Sam Hirschfelder — this is their calibre. Built in Melbourne where there are more roasteries than people and you can't walk down a street without passing a world-class musician. The tone already sounds good. Glitchy pops and cracks are part of the character. But it has to do what it's asked to do.

### Multi-Crate Instrument Architecture (Future)

Keep the current single TUI as the main workspace for mixing, tracking, and moving through a session. But split instruments into separate crates — each one its own terminal app:

- **`kazoo-808`** — TR-808 drum machine. All-synthesis, no samples. Step sequencer TUI.
- **`kazoo-cs80`** — Yamaha CS-80 pad synth. 8-voice poly, two layers per voice, per-voice drift. Also the home for generative/modular synthesis (node graph patching).
- **`kazoo-mini`** — Moog Minimoog bass/lead. Monophonic. 3 VCOs, ladder filter with nonlinear saturation (ZDF implementation), rate-based glide.
- **`kazoo-arp`** — Jupiter-8 style arpeggiator. Note scheduler that drives any synth. Up/Down/Up-Down/Random/As-Played, latch, swing, octave spanning.

All instruments connect into a central server (Unix domain sockets or shared-memory ring buffers — whichever benchmarks faster). Each gets its own terminal window. The genre is **Terminal Core**.

Full specs: `studio/INSTRUMENTS.md`

Do NOT throw away the current single-app TUI. It stays as the hub.

## Current Task List

These are active requirements. Do not defer, reorder, or skip any.

1. **Mixer redesign** — faders on the right side of the tracking view. Per track: gain slider, L/R pan, level indicator (bouncing meter) on either side of the slider. Master L/R out with VU meters.

2. **Synth: kill the drawer, direct editing** — no duplicate controls. Synths are separate modules you put down and control directly. No drawer that duplicates what's already visible.

3. **Effects: make accessible** — currently unreachable in a sidebar. Must be able to add/remove/tweak without fighting the UI.

4. **Tracking: per-track waveforms** — each track gets its own waveform lane like every DAW. Not one shared waveform.

5. **Timeline: real time scale** — show actual time values (not all 0:00). Support zoom in/out.

6. **Header: add tempo + metronome** — BPM display and metronome toggle in the persistent header bar.

7. **Track names: no "Track" prefix** — don't label tracks as "Track". It's obvious. Just show the name or number.

8. **Header cleanup** — the header layout is broken. Fix it.

9. **Kill the Synth view** — it duplicates controls. Synth settings live inline where you use them.

10. **Latency: native sample rate** — done. Uses device native rate to avoid Core Audio resampler.

@.claude/plans/playful-rolling-prism.md
@.claude/plans/output-callback-refactor.md
