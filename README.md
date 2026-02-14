# Kazoo

A voice-driven synthesizer that transforms vocal input into synthesized instrument sounds.

Hum a melody and hear it as a synth lead. Beatbox a rhythm and hear it as electronic drums. Make mouth noises and hear them as evolving textures. Kazoo uses genuine synthesis techniques — granular, wavetable, vocoder, phase vocoder, and pitch-tracked resynthesis — to turn your voice into music.

## Building

```bash
# Prerequisites: Rust 1.85+
cargo build --workspace

# Run the TUI (release mode recommended for real-time audio)
cargo run -p kazoo-tui --release
```

## Architecture

Two-crate Rust workspace:

- **kazoo-core** -- Audio engine library with zero UI dependencies. Any frontend consumes this API.
- **kazoo-tui** -- Ratatui terminal frontend.

```
┌─────────────────────────────────────────────────────────┐
│  kazoo-tui (Ratatui terminal frontend)                  │
│  ┌─────────┐ ┌──────────┐ ┌──────────┐ ┌────────────┐  │
│  │Transport│ │ Waveform │ │ Spectrum │ │  Effects   │  │
│  │ Bar     │ │ (Braille)│ │ Analyzer │ │  Inspector │  │
│  ├─────────┤ ├──────────┤ ├──────────┤ ├────────────┤  │
│  │ Track   │ │ VU       │ │ Mixer    │ │  Synth     │  │
│  │ List    │ │ Meters   │ │ View     │ │  Drawer    │  │
│  └─────────┘ └──────────┘ └──────────┘ └────────────┘  │
│         │ commands (channel)    ▲ display state (ringbuf)│
└─────────┼───────────────────────┼───────────────────────┘
          ▼                       │
┌─────────────────────────────────────────────────────────┐
│  kazoo-core (audio engine library)                      │
│                                                         │
│  ┌──────────┐  ┌───────────┐  ┌──────────────────────┐ │
│  │ Engine   │──│ Mixer     │──│ Tracks               │ │
│  │ (threads)│  │ (pan/vol) │  │ ┌──────┐ ┌────────┐  │ │
│  └──────────┘  └───────────┘  │ │Layers│→│Effects │  │ │
│       │                       │ │(x4)  │ │Chain   │  │ │
│       ▼                       │ └──────┘ └────────┘  │ │
│  ┌──────────┐  ┌───────────┐  └──────────────────────┘ │
│  │ Analysis │  │ Transport │  ┌──────────────────────┐ │
│  │ (pitch,  │  │ (play,    │  │ I/O                  │ │
│  │  FFT,    │  │  record,  │  │ (mic, speaker, WAV,  │ │
│  │  onset,  │  │  loop,    │  │  file import,        │ │
│  │  formant)│  │  metron.) │  │  sample rate conv.)  │ │
│  └──────────┘  └───────────┘  └──────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

### Thread Architecture

1. **cpal input callback** (OS-managed) -- writes mic samples to ring buffer. ZERO processing.
2. **cpal output callback** (OS-managed) -- the main audio workhorse. Reads mic from ring buffer, drains commands, runs the mixer (synth layers + effects), applies soft limiter, writes directly to output buffer, pushes display state. All processing state is owned by this callback's closure.
3. **Analysis thread** -- pitch detection, FFT spectrum, formant extraction, onset detection.
4. **Disk I/O thread** -- writes recorded audio to WAV files.

Communication is entirely lock-free: `ringbuf` (SPSC) between input and output callbacks for mic samples, `crossbeam-channel` (bounded MPSC) for UI-to-engine commands, and `ringbuf` for display state snapshots back to the UI.

## Synthesis Modes

Each track has one or more synth layers (up to 4). Each layer can use a different synthesis mode:

| Mode | Description |
|------|-------------|
| **Passthrough** | Raw microphone signal with no synthesis processing. The signal still passes through the track's effect chain, volume, and pan. Use this to monitor your voice directly or to apply effects without synthesis. |
| **Pitch Tracked** | Voice pitch drives band-limited oscillators (saw, square, sine, triangle). Parameters: shape, detune, cutoff, filter Q, portamento, envelope sensitivity. |
| **Wavetable** | Single-cycle waveforms extracted from voice, played back with wavetable morphing. |
| **Granular** | Voice buffer decomposed into grains and reassembled as clouds with control over grain size, scatter, and density. |
| **Vocoder** | Voice spectral envelope applied to a synthesized carrier signal. |
| **Phase Vocoder** | STFT-based time stretching and pitch shifting of the voice signal. |

## Effects

Each track has an effect chain with up to 8 slots. Available effects:

| Effect | Description |
|--------|-------------|
| **Biquad Filter** | Low-pass, high-pass, band-pass, and notch filters with adjustable cutoff and Q. |
| **Delay** | Tempo-syncable delay with feedback and mix control. |
| **Reverb** | Algorithmic reverb with room size, damping, and wet/dry mix. |
| **Chorus** | Modulated delay-based chorus with rate and depth control. |
| **Distortion** | Waveshaping distortion with drive and tone control. |
| **Formant Shift** | Shifts vocal formant frequencies up or down. |

## Multi-Synth Layering

Tracks support up to 4 simultaneous synth layers, each with its own synthesis mode, gain, and parameter set. Layers are mixed together before the effect chain. This lets you combine, for example, a pitch-tracked saw lead with a granular texture and a vocoder pad -- all driven by the same vocal input.

Layer management is done in the synth control drawer (see below):

- **n** -- Add a new layer (cycles through synth modes)
- **x** -- Remove the selected layer (layer 1 cannot be removed)
- **e** -- Toggle layer on/off
- **[/]** -- Select layer (when in drawer)
- **+/-** -- Adjust layer gain

## Synth Control Drawer

Press **d** to open a full-width bottom panel that shows the selected track's synth parameters with visual slider bars. The drawer provides a more spacious editing environment than the side inspector.

```
+-- Transport (3 rows) -------------------------------------------+
+-- Tracks (26c) --+-- Waveform (compressed) ---------------------+
|                  +-- Synth Drawer ------------------------------+
|                  | Track 1 — Pitch Tracked Synth                |
|                  |   Synth | Params | Effects                   |
|                  |                                               |
|                  | > Shape       [████████████░░░░] Saw          |
|                  |   Detune      [░░░░░░░░██░░░░░░] 0 cents     |
|                  |   Cutoff      [██████████████░░] 5000 Hz      |
|                  |   Filter Q    [████░░░░░░░░░░░░] 0.71         |
|                  |   Portamento  [██░░░░░░░░░░░░░░] 20 ms        |
|                  |   Env Sens    [██████████░░░░░░] 0.50          |
|                  |                                               |
|                  | ↑↓ select  ←→ adjust  t cycle  Esc close      |
+------------------+-----------------------------------------------+
```

The drawer has three tabbed sections (cycle with **Tab**):

- **Synth** -- Layer list (when multiple layers exist) and synth mode selector
- **Params** -- Parameter sliders for the selected layer
- **Effects** -- Effect chain overview with bypass indicators

When a track has multiple layers, the drawer shows a layer list above the parameter sliders:

```
  Layers:
  > [1] Pitch Tracked     0dB  [ON]
    [2] Wavetable         -6dB  [OFF]
    [+] Add layer

  Parameters (Layer 1):
    Shape    [████████████░░░░] Saw
    ...
```

### Drawer Controls

| Key | Action |
|-----|--------|
| d | Open drawer |
| Esc | Close drawer |
| Up/Down | Navigate parameters |
| Left/Right | Adjust selected parameter |
| t | Cycle synth mode |
| Tab | Cycle section (Synth / Params / Effects) |
| n | Add synth layer |
| x | Remove selected layer |
| e | Toggle layer enabled |
| [/] | Select prev/next layer |

## Metronome

Press **M** to toggle the metronome. When enabled, you'll hear click sounds synced to the current BPM:

- Downbeat clicks (beat 1) play at 1000 Hz
- Normal beat clicks play at 800 Hz
- Both are short sine bursts with exponential decay (~10ms)

The transport bar shows visual beat dots when the metronome is enabled:

```
 PLAY | 00:05.123 | 3.2.000 | 120.0 BPM | LOOP MET ● ○ ○ ○
```

The filled circle indicates the current beat. Metronome audio is mixed into the speaker output but is **not** included in recordings.

### BPM Control

When the Transport panel is focused:

| Key | Action |
|-----|--------|
| = | BPM +1 |
| - | BPM -1 |
| + (Shift+=) | BPM +10 |
| _ (Shift+-) | BPM -10 |

## Recording

### Free Recording

Press **r** to start recording on all armed tracks. Audio is captured from the mic and saved as clips on the timeline. Press **s** to stop. Recorded clips appear on the timeline view.

### Count-In Recording

Press **R** (Shift+R) to record with a count-in. The metronome plays for a configurable number of bars before recording begins, then optionally auto-stops after a set number of bars. This provides tempo-locked recording without needing to start exactly on the beat.

The transport bar shows the count-in progress:

```
 COUNT 2/4 | 00:03.456 | 2.1.000 | 120.0 BPM | MET ○ ● ○ ○
```

Count-in recordings start at exact bar boundaries for precise timeline alignment.

### Recording Workflows

Three recording workflows are available, configured in the Transport panel:

| Workflow | Description |
|----------|-------------|
| **Count-In** (default) | Count-in for N bars, then record for M bars (0 = unlimited). Shift+R starts the count-in, metronome plays during count-in, recording begins automatically at the bar boundary. |
| **Fixed Length** | Record exactly N bars, then auto-stop. No count-in period. |
| **Free Record** | Standard recording (same as pressing r). No count-in, no auto-stop. Clip boundaries are quantized to bar boundaries on stop. |

### Workflow Configuration (Transport Panel)

When the Transport panel is focused:

| Key | Action |
|-----|--------|
| w | Cycle workflow (Count-In / Fixed Length) |
| ] | Increase record bars +1 |
| [ | Decrease record bars -1 |

The current workflow is shown in the transport bar:

```
 PLAY | 00:00.000 | 1.1.000 | 120.0 BPM | LOOP MET | CI:1/4b
```

`CI:1/4b` = Count-In, 1 bar count-in, 4 bars recording. `FIX:4b` = Fixed Length, 4 bars.

## Timeline and Clips

When recordings or imported audio files exist, the center area switches from the oscilloscope waveform view to a multi-track timeline:

- Clips are shown as blocks on each track's timeline lane with waveform overviews
- The playhead position is indicated by a vertical cursor
- Recording clips appear in real-time as they're being captured

### Clip Operations

| Key | Action |
|-----|--------|
| , / . | Select prev/next clip |
| < / > | Move clip left/right on timeline |
| Ctrl+d | Duplicate clip |
| Ctrl+s | Split clip at playhead |
| Ctrl+x | Delete clip |

### Audio File Import

Press **o** to open the file browser. Navigate to audio files (WAV, FLAC, MP3, OGG, AAC) and press Enter to import them as clips on the selected track. Files are automatically resampled to match the engine's sample rate if needed.

## UI Layout

The TUI is divided into panels. Press **Tab** to cycle focus between them:

```
+-- Transport (3 rows) -----------------------------------------------+
+-- Tracks (26c) --+-- Waveform / Timeline ---+-- Inspector (36c) ----+
|                  |  (top 60%)               |  Effects / Mixer      |
|                  +-- Spectrum ----+-- VU ---+                       |
|                  |  (bottom 70%)  | (30%)   |                       |
+------------------+----------------+---------+-----------------------+
```

### Panels

| Panel | Description |
|-------|-------------|
| **Transport** | Play/record state, time position (MM:SS.mmm and Bar.Beat.Tick), BPM, loop/metronome toggles, beat dots, pitch detection, input level, CPU load. |
| **Tracks** | Track list with name, synth mode, mute/solo/arm indicators. Tracks are color-coded. |
| **Waveform** | Braille-character oscilloscope showing the live audio waveform. Switches to timeline view when clips exist. |
| **Spectrum** | FFT spectrum analyzer drawn with Braille characters. Shows magnitude vs. frequency. |
| **Meters** | VU meters for each track and the master bus. Shows peak and RMS levels. |
| **Inspector** | Right-side panel showing either the effects chain or mixer strip depending on focus. |

## Global Key Bindings

| Key | Action |
|-----|--------|
| Space | Play / Pause |
| s | Stop |
| r | Record (free) |
| R | Record with count-in |
| q | Quit |
| ? | Toggle help overlay |
| Tab | Next panel |
| Shift+Tab | Previous panel |
| j / k | Select next/prev track |
| 1-9 | Select track by number |
| m | Mute selected track |
| S | Solo selected track |
| a | Arm track for recording |
| n | Add new track |
| x | Delete selected track |
| t | Cycle synth mode |
| d | Open synth drawer |
| o | Open file browser |
| L | Toggle loop |
| M | Toggle metronome |
| h / l | Pan left/right or cycle parameters |
| +/- | Adjust volume or parameter |
| [/] | Zoom waveform |
| Esc | Close overlay / cancel |

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| cpal | 0.17 | Cross-platform audio I/O (mic input, speaker output) |
| rustfft | 6.4 | FFT/IFFT for spectral processing |
| fundsp | 0.23 | DSP primitives and synthesis nodes |
| pyin | 1.2 | Vocal pitch detection (PYIN algorithm) |
| hound | 3.5 | WAV file read/write |
| symphonia | 0.5 | Multi-format audio file decoding |
| rubato | 1.0 | Sample rate conversion for file import |
| ringbuf | 0.4 | Lock-free SPSC ring buffers |
| crossbeam-channel | 0.5 | Multi-producer command channels |
| ratatui | 0.30 | TUI framework |
| crossterm | 0.29 | Terminal event handling |

## License

TBD
