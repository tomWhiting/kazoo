# kazoo-mini — Bass / Lead Synth

Separate TUI crate. Monophonic. Moog Minimoog Model D inspired. Three VCOs, 24 dB/oct ladder filter with nonlinear saturation, rate-based glide. The fattest bass in synthesis. Connects to kazoo-tui hub via IPC.

## What Makes Minimoog Bass Fat

1. **Three oscillators slightly detuned.** Even 1-3 cents of spread between Osc 1, 2, 3 creates beating and chorusing that fills the spectrum.
2. **24 dB/oct ladder filter.** Steeper than CS-80's 12 dB. More dramatic filter sweeps. More closed sound below cutoff.
3. **Transistor saturation in the filter stages.** At high signal levels, each filter stage soft-clips (tanh). This is warm distortion built into the filter itself. A linear biquad cannot replicate this.
4. **Resonance steals low-end.** As Q increases, bass content decreases. This is correct — it's why Minimoog bass cuts through a mix instead of just booming.
5. **Rate-based glide.** Logarithmic pitch slide — faster at the start, decelerates approaching target. An octave takes 12x longer than a semitone at the same rate.

All sample-by-sample. Zero lookahead. Latency = buffer size only.

## Oscillator Section

Three VCOs:

**Waveforms (per oscillator):** triangle, sawtooth, square, narrow pulse, wide pulse. All from a single core oscillator via waveshaping.

**Octave range:** 32', 16', 8', 4' per oscillator.

**Fine tune:** Osc 2 and Osc 3 each have independent fine tune. This is the primary source of thickness — detune them slightly and the beating does the rest.

**Osc 3 as LFO:** Switchable to low-frequency range. Disconnected from keyboard tracking. Feeds the modulation wheel routing:
- Destinations: Osc 1+2 pitch, filter cutoff, or both
- This is the Minimoog's only LFO — there is no dedicated LFO module

**Cross-Modulation (X-Mod):**
- Osc 3 can FM Osc 2's frequency — produces dirty, aggressive FM timbres
- Osc 2 can modulate filter cutoff
- These are the Minimoog's secret weapons for aggressive leads

**Mixer:** Each oscillator has a level control. External input and noise generator also available in the mixer. The mix feeds the filter.

## The Ladder Filter

4-pole (24 dB/oct) transistor ladder low-pass filter.

### Architecture

Four cascaded 1-pole RC filter stages. Each stage = 6 dB/oct. Together = 24 dB/oct.

Resonance is a feedback path from filter output back to input. At maximum resonance, the filter self-oscillates — producing a sine wave at the cutoff frequency. This is usable as a fourth oscillator.

### Nonlinear Saturation (Critical)

The transistors in each stage operate in their exponential region. The voltage-to-current relationship is exponential, not linear. This introduces soft saturation at high signal levels — each stage gently clips.

**This MUST be modeled.** A naive linear discretization (bilinear transform) does not capture the saturation character. Implementations:

1. **tanh per stage:** Apply `tanh(signal * drive)` within each filter stage. This approximates the transistor transfer curve.
2. **Zero-Delay Feedback (ZDF):** The feedback path has inherent one-sample delay in naive implementations. This makes the resonance character wrong (less sharp, differently pitched self-oscillation). Use Zavalishin's ZDF method or Huovilainen's model to eliminate feedback delay.

The recommended approach: Huovilainen's nonlinear Moog ladder model or Zavalishin's "The Art of VA Filter Design" ZDF implementation. Both are well-documented and proven.

### Resonance Behavior

- Self-oscillation at max Q: usable sine oscillator at cutoff frequency
- **Low-end loss with resonance:** As Q increases, bass energy decreases. Do NOT compensate for this — it's correct behavior and it's what makes Minimoog bass cut through
- Cutoff tracks keyboard at 1V/oct for musical filter sweeps

### Filter Frequency Range

10 Hz to 32 kHz (DC to well above audio).

## Envelopes

**Filter Contour:** ADSR with Amount control.
- Amount scales how much the envelope modulates cutoff (0 to full range)
- Attack: 1ms to ~10s
- Standard ADSR shape

**Loudness Contour:** ADS (Attack, Decay, Sustain).
- Release is implicit / minimal in the original
- Some implementations add adjustable release

**Envelope Retriggering:**
- **Legato mode (default):** New note while a key is held does NOT retrigger envelopes. Pitch changes, amplitude stays in sustain phase. This is how the hardware works.
- Optional retrigger mode for staccato playing

## Keyboard Behavior

**Monophonic.** One note at a time.

**Note Priority:** Lowest note (original hardware default). When multiple keys are held, the lowest pitch plays. On note-off, reassign to next-lowest remaining held note.

**Glide (Portamento):**
- **Rate-based, not time-based.** The pitch slides at a fixed rate (volts/second, or semitones/second in digital).
- An octave interval takes 12x longer than a semitone at the same glide rate.
- This is the analog RC slew characteristic — logarithmic curve that decelerates approaching target.
- Glide only activates in legato (new note while previous held). Does not glide from silence.

## TUI Layout

```
+-- MINIMOOG MODEL D -----------------------------------------+
|                                                              |
| OSC 1          OSC 2          OSC 3 [LFO]                   |
| Wave: Saw      Wave: Saw      Wave: Tri                     |
| Oct:  8'       Oct:  8'       Oct:  Lo                      |
| Tune: 0c       Tune: +2c      Tune: -1c                    |
| Level: 80%     Level: 75%     Level: 0%                     |
|                                                              |
| MIXER              FILTER               ENVELOPES            |
| Osc1: [====]       Cutoff: 2.4kHz      FILTER CONTOUR       |
| Osc2: [====]       Resonance: 40%       A: 5ms              |
| Osc3: [==  ]       Amount: 70%          D: 200ms            |
| Noise:[=   ]       Key Track: On        S: 30%              |
| Ext:  [    ]                             R: 150ms           |
|                    X-MOD                                     |
|                    Osc3->Osc2: Off      LOUDNESS CONTOUR     |
|                    Osc2->Filt: Off       A: 2ms              |
|                                          D: 300ms            |
| GLIDE: 40ms/st    LEGATO: On            S: 60%              |
|                                          R: 100ms            |
|                                                              |
| [====== waveform / spectrum ======]                          |
+--------------------------------------------------------------+
```

Knob-per-function layout. Everything visible, everything directly editable. No menus, no drawers, no hidden panels. You see it, you turn it.

- Tab between sections (Oscillators / Mixer / Filter / Envelopes / Performance)
- j/k to select parameter within section
- +/- to adjust value
- Mouse: click on a parameter area, drag up/down to adjust
- QWERTY keyboard mapped to chromatic notes

## Crate Structure

```
kazoo-mini/
  Cargo.toml
  src/
    main.rs           # TUI event loop, IPC connection
    app.rs            # Application state, presets
    synth/
      mod.rs          # Monophonic voice engine, note priority, glide
      oscillator.rs   # Multi-waveform VCO with LFO mode
      ladder.rs       # 4-pole Moog ladder filter (ZDF, tanh per stage)
      envelope.rs     # ADSR with amount control
      mixer.rs        # Oscillator + noise + external mix bus
      glide.rs        # Rate-based portamento (RC slew)
      xmod.rs         # Cross-modulation routing
    ui/
      mod.rs          # Layout
      oscillators.rs  # Three-column oscillator editor
      filter.rs       # Filter section with cutoff/resonance/amount
      envelopes.rs    # ADSR editors
      performance.rs  # Glide, legato, note priority
      monitor.rs      # Waveform / spectrum display
    input.rs          # Keyboard-to-note, mouse zones
    ipc.rs            # Hub connection
```

## Dependencies on kazoo-core

Uses from `kazoo-core`:
- `Processor` trait
- `sanitize_sample`, `soft_limit`
- `Db`, `Pan` types
- `analysis::SpectrumAnalyzer` for display

New DSP that needs building:
- **Moog ladder filter (ZDF with tanh saturation)** — this is the big one. The existing `BiquadFilter` in kazoo-core is a standard biquad, not a ladder. The ladder needs: 4 cascaded 1-pole stages, nonlinear saturation per stage, delay-free feedback. Reference: Zavalishin "The Art of VA Filter Design" Chapter 6, or Huovilainen 2004/2006 papers.
- **Rate-based glide** — RC slew limiter on pitch CV. Not a standard portamento (which is time-based).
- **Cross-modulation routing** — FM between oscillators and filter modulation
- **Lowest-note priority voice manager** — monophonic note stack with priority reassignment on note-off
