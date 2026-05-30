# kazoo-808 — TR-808 Drum Machine

Separate TUI crate. All sounds synthesized, no samples. Connects to kazoo-tui hub via IPC.

## Synthesis Architecture

Every voice is a triggered decaying oscillator or noise source with envelope. Sample-by-sample, zero lookahead. Latency = buffer size only.

### Kick (Bass Drum)

Bridged-T bandpass filter excited into self-oscillation by a 1ms trigger pulse.

- Base resonant frequency: ~49 Hz (G1)
- At trigger, pitch is pushed up to ~130 Hz for approximately 6ms via a parallel circuit — this is the punch
- After 6ms, oscillator retriggers at base frequency and decays exponentially
- Decay range: 50ms (min) to 800ms (max), center ~300ms
- Subtle downward pitch drift throughout the decay (voltage leakage characteristic)
- Accent: higher trigger intensity, extends pitch burst duration and amplitude

**DSP implementation:** High-Q bandpass filter (or direct sine oscillator) with pitch-swept frequency envelope and exponentially decaying amplitude. The punch is the 6ms pitch sweep from 130 Hz down to 49 Hz, not a separate click.

### Snare

Two bridged-T oscillators (tonal body) summed with HP-filtered white noise (snare rattle).

- Oscillator 1: 476 Hz
- Oscillator 2: 238 Hz (one octave below — mild inharmonic beating)
- Noise path: white noise through HP filter, independent decay control
- **Tone knob:** ratio between the two tonal oscillators
- **Snappy knob:** noise decay time, independent from tonal body

**DSP implementation:** Two decaying sine generators summed with HP-filtered noise under a separate exponential envelope. Three controls: level, tone balance, snappy.

### Hi-Hats (Closed / Open)

Six square-wave oscillators at inharmonic frequencies, summed and bandpass filtered.

**Oscillator frequencies:**
1. 800 Hz
2. 540 Hz
3. 523 Hz
4. 370 Hz
5. 304 Hz
6. 205 Hz

The ratios are inharmonic (not integer multiples) — this is what makes the sound metallic, not tonal.

**Filter chain:** Sum of oscillators -> BPF at 7100 Hz -> BPF at 3440 Hz -> VCA

- Closed hat: 50ms fixed decay
- Open hat: 90–600ms adjustable decay

**DSP implementation:** Six square wave oscillators summed, two bandpass filters in series, exponential decay envelope. Frequency accuracy is not critical (+-20% still sounds right). The inharmonic ratio structure matters more than absolute pitch.

### Clap

Bandpass-filtered white noise through a multi-burst envelope.

- Noise filtered at ~1000 Hz bandpass
- Envelope: 3 rapid 10ms sawtooth bursts, then a 20ms release, then a 100ms exponential tail
- Total initial burst window: ~30ms
- The multi-burst simulates multiple hands clapping slightly out of sync

**DSP implementation:** White noise -> BPF at ~1 kHz -> VCA driven by multi-segment envelope.

### Toms (High / Mid / Low)

Bridged-T oscillators, same architecture as kick at different frequencies.

- High Tom: ~200 Hz, 100ms decay
- Mid Tom: ~150 Hz, 130ms decay
- Low Tom: ~100 Hz, 200ms decay
- Subtle downward pitch sweep during decay
- Optional pink noise blend for texture

### Cowbell

Two square oscillators (800 Hz + 540 Hz) through bandpass at 880 Hz. The beat frequency between 800 and 540 creates the tone. Two-stage envelope: 50ms transient, 500ms sustain.

### Cymbal

Same six-oscillator metallic source as hi-hats. Longer decay: 350–1200ms adjustable. Different filter balance emphasizing lower metallic frequencies.

## TUI Layout

```
+-- 808 DRUM MACHINE ------------------------------------------+
|                                                               |
|  KICK  SNARE  CH   OH   CLAP  TOM1 TOM2 TOM3 COWB  CYM     |
|  [===] [===]  [==] [==] [===] [==] [==] [==]  [==] [==]     |
|                                                               |
|  Step Sequencer (16 steps)                                    |
|  KCK  [ ][ ][X][ ][ ][ ][X][ ][ ][ ][X][ ][ ][ ][X][ ]     |
|  SNR  [ ][ ][ ][ ][X][ ][ ][ ][ ][ ][ ][ ][X][ ][ ][ ]     |
|  CHH  [X][ ][X][ ][X][ ][X][ ][X][ ][X][ ][X][ ][X][ ]     |
|  OHH  [ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][X]     |
|  CLP  [ ][ ][ ][ ][X][ ][ ][ ][ ][ ][ ][ ][X][ ][ ][ ]     |
|  ...                                                          |
|                                                               |
|  Pattern: A1    Swing: 55%    BPM: [synced to hub]            |
+---------------------------------------------------------------+
```

- Number keys (1-9, 0) select voice
- Arrow keys navigate grid
- Space toggles step on/off
- +/- adjust per-step velocity or accent
- Tab cycles between grid / voice parameters / pattern settings
- Pattern chaining: multiple 16-step patterns linked sequentially

## Voice Parameter Controls

Each voice has a parameter panel accessible when selected:

- **Kick:** Tune, Decay, Level, Accent amount
- **Snare:** Tune, Tone, Snappy, Decay, Level
- **Hi-Hats:** Tune (shared metal oscillators), Closed Decay, Open Decay, Level
- **Clap:** Tune (filter center), Decay, Level
- **Toms:** Tune, Decay, Level (per tom)
- **Cowbell:** Tune, Decay, Level
- **Cymbal:** Tune, Decay, Level

## Crate Structure

```
kazoo-808/
  Cargo.toml          # depends on kazoo-core, ratatui, crossterm
  src/
    main.rs           # TUI event loop, IPC connection to hub
    app.rs            # Application state, pattern storage
    synth/
      mod.rs          # Voice trait, trigger dispatch
      kick.rs         # Bridged-T kick synthesis
      snare.rs        # Dual-osc + noise snare
      hihat.rs        # Six-osc metallic synthesis (shared by hat/cymbal/cowbell)
      clap.rs         # Multi-burst noise clap
      tom.rs          # Bridged-T tom (parameterized by freq/decay)
      cowbell.rs      # Two-osc cowbell
    sequencer/
      mod.rs          # Step sequencer engine, pattern storage
      clock.rs        # Clock division, swing, hub sync
    ui/
      mod.rs          # Layout
      grid.rs         # Step sequencer grid renderer
      params.rs       # Voice parameter editor
    ipc.rs            # Hub connection, audio output, transport sync
```

## Dependencies on kazoo-core

Uses from `kazoo-core`:
- `effects::BiquadFilter` — for the bandpass filters in hat/cymbal/cowbell voices
- `Processor` trait — voice interface
- `sanitize_sample` / `soft_limit` — NaN defense and output limiting
- Audio I/O primitives if running standalone (for testing without hub)

New DSP that needs building:
- Bridged-T oscillator (resonant bandpass driven into self-oscillation) — does not exist in kazoo-core yet
- Multi-segment envelope generator (for clap burst pattern) — the existing ADSR is insufficient
- Square wave oscillator bank (for metallic voices) — fundsp may have this
