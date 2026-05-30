# Mixbus — Mixer & Master Output

## What It Is

The mixing desk. Receives audio from every instrument, mixes them with per-channel volume/pan/mute/solo, applies master effects, and writes the final output to WAV. Also handles the render-to-file pipeline for testing.

## What It Does

### Channel Strip (per instrument)

- **Volume** — Linear gain, 0.0 to 2.0 (default 1.0).
- **Pan** — -1.0 (hard left) to 1.0 (hard right), equal-power pan law.
- **Mute / Solo** — Solo overrides: if any channel is soloed, only soloed channels pass through.
- **Send levels** — Per-channel send to effect buses (reverb send, delay send).

### Master Section

- **Master volume** — Final output gain.
- **Limiter** — Brick-wall limiter at 0dBFS to prevent clipping. Simple lookahead design.
- **Metering** — Peak and RMS levels per channel and master. Updated per buffer.

### Render Pipeline

1. Collect one buffer of audio from each active instrument.
2. Apply channel strip (volume, pan, mute/solo).
3. Sum into master bus.
4. Send to effect returns (fx crate processes sends, returns mixed audio).
5. Apply master volume and limiter.
6. Write to WAV file or send to cpal output.

## JSON Input (for testing)

Mixbus takes a "session" JSON that describes which instruments to instantiate, their configurations, and the render length:

```json
{
  "bpm": 120,
  "bars": 8,
  "master_volume": 0.8,
  "channels": [
    {
      "instrument": "drum808",
      "config": { "...instrument-specific JSON..." },
      "volume": 0.7,
      "pan": 0.0
    },
    {
      "instrument": "subbass",
      "config": { "...instrument-specific JSON..." },
      "volume": 0.9,
      "pan": 0.0
    },
    {
      "instrument": "cs80",
      "config": { "...instrument-specific JSON..." },
      "volume": 0.5,
      "pan": -0.2,
      "sends": { "reverb": 0.4 }
    }
  ],
  "output": "session_render.wav"
}
```

Mixbus orchestrates the full render: creates a PulseClock, instantiates each instrument, runs the render loop buffer-by-buffer, and writes the final WAV.

## Interface

```rust
pub struct Channel {
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub soloed: bool,
    pub sends: HashMap<String, f32>,
}

pub struct Mixbus {
    channels: Vec<Channel>,
    master_volume: f32,
    limiter: Limiter,
    sample_rate: u32,
}

impl Mixbus {
    pub fn add_channel(&mut self, channel: Channel) -> usize;
    pub fn process(&mut self, inputs: &[&[f32; 2]], output: &mut [f32; 2]);
    pub fn peak_level(&self, channel: usize) -> (f32, f32);
    pub fn master_peak(&self) -> (f32, f32);
}
```

## Metering (for TUI)

Mixbus exposes real-time peak and RMS values for each channel and the master bus. The TUI (future) reads these to draw VU meters and level bars.

```rust
pub struct MeterReading {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
}
```

## Dependencies

- `dsp` crate (limiter, pan law, buffer ops)
- `pulse` crate (clock for render orchestration)
- `hound` (WAV output)
- `serde` + `serde_json` (session JSON)
- `cpal` (optional, for live playback)

## Testing

- Mix two sine waves at different frequencies, verify both present in output FFT.
- Pan a mono source hard left, verify right channel is silent.
- Mute a channel, verify its audio is absent from output.
- Solo a channel, verify only its audio is present.
- Limiter: feed a hot signal, verify output never exceeds 0dBFS.
- Full session render: load a session JSON with multiple instruments, render 8 bars, verify WAV is correct duration and non-silent.
