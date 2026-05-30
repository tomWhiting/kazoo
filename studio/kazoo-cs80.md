# kazoo-cs80 — Pad Synth

Separate TUI crate. 8-voice polyphonic. Two independent layers per voice. Yamaha CS-80 inspired. Also the home for generative/modular synthesis via a node-graph patching system. Connects to kazoo-tui hub via IPC.

## What Makes CS-80 Pads Sound Like That

1. **Per-voice analog drift.** No two of the 8 voices have identical tuning or envelope timing. Random detuning offsets (0-10 cents) per voice. Per-voice envelope timing jitter. This is not optional — it IS the sound.
2. **Two independent layers per voice.** Layer I: bright, fast attack. Layer II: slow, evolving pad. Mixed together under independent control. A single-layer synth cannot replicate this.
3. **12 dB/oct filters.** Softer than Moog's 24 dB. Never harsh. Resonance deliberately limited — no self-oscillation.
4. **IL/AL filter envelope.** The filter doesn't start from silence. It starts at some defined brightness (Initial Level) and sweeps to the Attack Level. A standard ADSR starting from zero can't do this.
5. **Ring modulator with its own envelope.** Metallic sheen on the attack, fading as the ring mod depth decays.

All sample-by-sample. Zero lookahead. Latency = buffer size only.

## Voice Architecture

8 voices. Each voice = 2 complete synthesis chains mixed together.

### Per Layer (x2 per voice)

```
VCO ──> HPF ──> LPF ──> VCA ──> Mix
              (12dB)  (12dB)
         + Sine injected post-filter (sine has no harmonics to filter)
```

**VCO:**
- Waveforms: sawtooth, variable-width pulse (PWM 50-90%), sine
- Sine output is post-filter — intentional, filtering a sine does nothing
- Octave ranges: 32', 16', 8', 4'

**HPF:** 2-pole (12 dB/oct) state-variable filter. Own resonance control.

**LPF:** 2-pole (12 dB/oct) state-variable filter. Own resonance control.

Per voice = 4 filters total (HPF + LPF for each of 2 layers).

**VCA Envelope:** Standard ADSR (Attack, Decay, Sustain, Release).

### Filter Envelope (NOT standard ADSR)

Parameters:
- **IL** (Initial Level): where the filter cutoff starts at note-on
- **AL** (Attack Level): where the attack phase peaks
- **Attack time:** how long from IL to AL
- **Decay time:** how long from AL back toward IL
- **Release time:** how long from current level to IL after note-off

The attack phase runs from IL to AL. This allows the filter to start open and close, or start closed and open, or anything in between. A standard ADSR always starts from zero. This envelope is why CS-80 pads have that characteristic non-zero starting brightness before evolving.

### Ring Modulator

- Carrier: internal sine wave
- Modulates: voice signal (multiplication)
- Own attack-decay envelope on modulation depth
- Creates metallic attack transients on bright patches

### LFO

- Waveforms: sine, sawtooth, ramp, pulse, noise
- Destinations (independent depth per destination): VCO pitch, VCF cutoff, VCA level
- Rate range goes into audio rate for FM effects
- Separate depth controls allow tremolo + vibrato independently

### Keyboard and Performance

- Velocity sensitive: affects volume and filter brightness at note-on
- **Polyphonic aftertouch:** each held note responds to pressure independently. Routes to pitch, filter, VCA volume
- Chord memory: store a chord, single keys play the full chord transposed
- Ribbon controller: horizontal strip routed to pitch or modulation

## Generative / Modular Extension

The CS-80 architecture is already semi-modular (two parallel signal paths, ring mod, LFO routing). Extend with a node graph view:

- Each module (VCO, VCF, VCA, ENV, LFO, Ring Mod, Noise, Mixer) is a node
- Nodes have typed inputs (audio, control-rate, trigger) and outputs
- Drag to connect outputs to inputs
- Unlimited patching: any source to any destination
- Save/load patches as presets

Mouse interaction: click node to select, drag between ports to connect, scroll on a parameter to adjust. Ratatui mouse support is limited but sufficient for zone-based click detection and vertical drag for value adjustment.

## TUI Layout

```
+-- CS-80 PAD SYNTH -------------------------------------------+
|                                                               |
| LAYER I                    | LAYER II                         |
| VCO: Saw  8'  Tune: +2c   | VCO: Pulse 16' Tune: -3c        |
| HPF: 120Hz  Q:0.3         | HPF: 80Hz   Q:0.2               |
| LPF: 2.4kHz Q:0.5         | LPF: 800Hz  Q:0.4              |
| ENV: IL:40 AL:80 A:50ms   | ENV: IL:20 AL:60 A:200ms        |
|      D:300ms  R:500ms     |      D:800ms  R:1.2s            |
| VCA: A:10ms D:200ms       | VCA: A:80ms D:500ms             |
|      S:0.7  R:400ms       |      S:0.8  R:800ms             |
|                            |                                  |
| RING MOD: Depth:40 A:5ms D:200ms                              |
| LFO: Sine 2.5Hz  Pitch:5c  Filter:20%  VCA:0%               |
| MIX: I [====|====] II     DRIFT: 6c                          |
|                                                               |
| VOICE MONITOR: 1[~] 2[~] 3[.] 4[.] 5[~] 6[.] 7[.] 8[.]    |
| [======== spectrum/waveform ========]                         |
+---------------------------------------------------------------+
```

- Two-column layer editor (Layer I / Layer II side by side)
- Per-voice drift visualization (which voices are active, their detuning)
- Real-time spectrum/waveform display
- Tab between sections, j/k navigate parameters, +/- adjust
- QWERTY keyboard mapped to chromatic notes for playing
- Aftertouch simulation: hold key + modifier or scroll for per-note pressure

## Crate Structure

```
kazoo-cs80/
  Cargo.toml
  src/
    main.rs            # TUI event loop, IPC connection
    app.rs             # Application state, voice allocation, presets
    synth/
      mod.rs           # Voice allocator (8 voices), note assignment
      voice.rs         # Single voice: two layers + ring mod + LFO
      layer.rs         # One layer: VCO -> HPF -> LPF -> VCA
      oscillator.rs    # Saw, pulse (with PWM), sine
      filter.rs        # 12 dB/oct state-variable (HPF + LPF)
      envelope.rs      # IL/AL filter envelope + standard ADSR for VCA
      ring_mod.rs      # Sine carrier * voice signal, own envelope
      lfo.rs           # Multi-waveform, multi-destination
      drift.rs         # Per-voice random detuning and timing jitter
    modular/
      mod.rs           # Node graph engine
      node.rs          # Node trait, typed ports
      graph.rs         # Connection routing, topological sort for processing order
      nodes/           # Concrete node implementations (reuse synth/ modules)
    ui/
      mod.rs           # Layout
      layers.rs        # Dual-layer editor
      modular_view.rs  # Node graph renderer
      monitor.rs       # Voice activity, spectrum, waveform
    input.rs           # Keyboard-to-note mapping, aftertouch, mouse zones
    ipc.rs             # Hub connection
```

## Dependencies on kazoo-core

Uses from `kazoo-core`:
- `effects::BiquadFilter` — may be adaptable for the SVF filters, but CS-80 uses state-variable (not biquad). May need a dedicated SVF implementation.
- `analysis::SpectrumAnalyzer` — for the waveform/spectrum display
- `Processor` trait, `sanitize_sample`, `soft_limit`
- `Db`, `Pan` types

New DSP that needs building:
- State-variable filter (12 dB/oct, simultaneous HPF + LPF output) — different topology from existing BiquadFilter
- IL/AL filter envelope — not a standard ADSR, needs its own implementation
- Ring modulator with envelope — multiply + internal sine + AD envelope
- Voice allocator with per-voice drift — allocation + random detuning per voice
- Node graph engine (for modular extension) — topological sort, typed signal routing
