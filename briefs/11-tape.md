# Tape — Tape Machine Emulator

## What It Is

A virtual reel-to-reel tape recorder. Records audio to a virtual tape, plays it back with tape-characteristic artifacts: saturation, wow and flutter, head bump, hiss, and speed manipulation. Can function as a mastering effect (print to tape for warmth) or as a creative tool (half-speed, reverse, varispeed).

## What It Does

### Tape Recording Model

Audio is "recorded" onto a virtual tape buffer — a large circular buffer representing the tape medium. The recording process applies tape saturation characteristics. Playback reads from this buffer with its own set of artifacts.

```
Input → Record Head (saturation + bias) → Tape Buffer → Playback Head (EQ + artifacts) → Output
                                                ↑
                                          Wow & Flutter (speed modulation)
```

### Tape Saturation

Magnetic tape has a nonlinear transfer curve. Quiet signals pass cleanly; loud signals compress and add harmonics. Modeled as:

- **Hysteresis curve** — Simplified Jiles-Atherton model (or a lookup table approximation). Produces asymmetric saturation with stronger odd harmonics.
- **Input gain** — Drive into the saturation curve. Higher = more warmth/distortion.
- **Bias** — High-frequency bias signal mixed before recording. Affects linearity and high-frequency response. Modeled as a parameter that shifts the operating point on the hysteresis curve.

### Wow and Flutter

Slow pitch wobble (wow) and fast pitch wobble (flutter) caused by mechanical imperfections in the tape transport.

| Parameter | Range | Description |
|-----------|-------|-------------|
| **wow_rate** | 0.1 - 3.0 Hz | Slow speed variation. Capstan eccentricity. |
| **wow_depth** | 0.0 - 1.0 | Amount of slow wobble (in cents). |
| **flutter_rate** | 3.0 - 20.0 Hz | Fast speed variation. Motor irregularity. |
| **flutter_depth** | 0.0 - 1.0 | Amount of fast wobble. |

Implemented as modulated read position on the tape buffer — the playback head moves at a slightly varying speed.

### Head Bump

Real tape machines boost low frequencies around 60-100Hz due to the physical geometry of the playback head. A subtle resonant peak in the low end that adds weight and warmth. Modeled as a gentle resonant low shelf.

| Parameter | Range | Description |
|-----------|-------|-------------|
| **bump_freq** | 40 - 150 Hz | Center frequency of the bump. |
| **bump_gain_db** | 0 - 6 | Amount of low-end boost. |

### Tape Hiss

Inherent noise floor of magnetic tape. Band-limited white noise shaped to the tape noise spectrum (slightly more energy in the highs).

| Parameter | Range | Description |
|-----------|-------|-------------|
| **hiss_level** | 0.0 - 0.1 | Hiss amplitude. 0 = clean digital. 0.05 = subtle tape feel. 0.1 = old cassette. |

### Speed Control

| Mode | Description |
|------|-------------|
| **Normal** | 15 ips (inches per second). Standard playback. |
| **Half-speed** | 7.5 ips. Everything down one octave, double length. |
| **Double-speed** | 30 ips. Everything up one octave, half length. |
| **Varispeed** | Arbitrary playback rate multiplier. 0.5 = half speed, 2.0 = double. |
| **Reverse** | Play the tape buffer backwards. |

Speed changes affect both pitch and duration — this is tape behavior, not time-stretching.

### Tape Stop / Start

Emulates the physical deceleration when stopping a tape machine and acceleration when starting. The "DJ tape stop" effect.

- **Stop time** — How long the deceleration takes (50ms to 2s).
- **Start time** — How long the acceleration takes (50ms to 1s).

During stop/start, the playback rate smoothly ramps from current speed to zero (or zero to target speed), producing the characteristic pitch dive/rise.

## JSON Input

### As Mastering Effect (process existing WAV)

```json
{
  "input": "dry_mix.wav",
  "tape_speed": "normal",
  "saturation": {
    "input_gain": 0.6,
    "bias": 0.5
  },
  "wow_rate": 0.5,
  "wow_depth": 0.15,
  "flutter_rate": 6.0,
  "flutter_depth": 0.08,
  "head_bump": {
    "freq": 80,
    "gain_db": 2.5
  },
  "hiss_level": 0.02,
  "output": "tape_master.wav"
}
```

### As Creative Tool (speed manipulation)

```json
{
  "input": "vocal_take.wav",
  "tape_speed": "half",
  "saturation": {
    "input_gain": 0.3,
    "bias": 0.5
  },
  "wow_rate": 0.3,
  "wow_depth": 0.3,
  "flutter_rate": 5.0,
  "flutter_depth": 0.1,
  "hiss_level": 0.04,
  "tape_stop": {
    "at_beat": 16.0,
    "stop_time_ms": 800
  },
  "output": "slowed_vocal.wav",
  "bpm": 120
}
```

## Dependencies

- `dsp` crate (delay lines for tape buffer, filters for head bump/hiss shaping, waveshaper for saturation)
- `pulse` crate (optional, for beat-synced tape stop)
- `hound` (WAV I/O)
- `serde` + `serde_json`

## Testing

- **Saturation:** Process a sine wave with increasing input gain, verify harmonics increase progressively.
- **Wow and flutter:** Process a steady tone, verify pitch variation in spectrogram. Measure modulation rate matches parameter.
- **Head bump:** Process pink noise, verify low-frequency boost at specified frequency in FFT.
- **Hiss:** Process silence with hiss enabled, verify noise floor is present and spectrally shaped.
- **Half-speed:** Process a 1-second 440Hz tone at half speed, verify output is 2 seconds at ~220Hz.
- **Reverse:** Process a known signal, verify output is time-reversed.
- **Tape stop:** Process a signal with tape stop at a known position, verify pitch dive at that point.
- **Full print:** Take a mix, run through tape with moderate settings, output WAV. A/B with original for tonal character.
