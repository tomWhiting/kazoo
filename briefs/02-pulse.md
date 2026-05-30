# Pulse — Clock, Transport & Sync

## What It Is

The heartbeat. Every instrument syncs to Pulse. It manages tempo, beat position, bar counting, and transport state (play/stop). It's not an instrument — it's the metronome that everything locks to.

## What It Does

- Maintains a high-resolution clock tied to sample position (not wall-clock time — sample-accurate).
- Broadcasts tick messages at a configurable resolution (default: 24 ticks per quarter note, same as MIDI clock).
- Tracks transport state: `Stopped`, `Playing`, `Paused`.
- Supports tempo changes (instant or ramped over N beats).
- Supports swing (shuffle) — shifts even ticks by a percentage.
- Supports "free" mode where instruments run without sync (tasteful desync).

## Clock Resolution

24 PPQN (pulses per quarter note). At 120 BPM, that's 48 ticks per second. Each tick carries:

```rust
pub struct Tick {
    /// Absolute sample position since transport start
    pub sample_pos: u64,
    /// Current beat (1-indexed within bar, fractional)
    pub beat: f64,
    /// Current bar number (1-indexed)
    pub bar: u32,
    /// Tempo in BPM
    pub bpm: f64,
    /// Time signature numerator (beats per bar)
    pub beats_per_bar: u8,
    /// Transport state
    pub transport: Transport,
    /// Swing amount (0.0 = straight, 0.5 = triplet feel)
    pub swing: f64,
}

pub enum Transport {
    Stopped,
    Playing,
    Paused,
}
```

## Sync Modes

| Mode | Behaviour |
|------|-----------|
| **Locked** | Instrument quantizes all events to the nearest tick. Tight. |
| **Loose** | Instrument receives ticks but applies its own micro-timing (humanise). |
| **Free** | Instrument ignores clock entirely. Runs at its own pace. |

## JSON Input (for testing)

```json
{
  "bpm": 120,
  "time_signature": [4, 4],
  "swing": 0.0,
  "bars": 8,
  "tempo_changes": [
    { "at_bar": 5, "bpm": 140, "ramp_beats": 4 }
  ]
}
```

Pulse doesn't render audio — it generates a sequence of `Tick` events that other instruments consume.

## Interface

```rust
pub struct PulseClock {
    bpm: f64,
    sample_rate: u32,
    ppqn: u32,
    // ...
}

impl PulseClock {
    pub fn new(bpm: f64, sample_rate: u32) -> Self;
    pub fn set_bpm(&mut self, bpm: f64);
    pub fn play(&mut self);
    pub fn stop(&mut self);
    pub fn next_tick(&mut self) -> Option<Tick>;
    /// Advance by N samples, yielding any ticks that fall within
    pub fn advance(&mut self, num_samples: u32) -> Vec<Tick>;
}
```

## Integration

In-process: instruments call `pulse.advance(buffer_size)` each render cycle and get back any ticks that occurred within that buffer window. They use tick positions to place events sample-accurately within the buffer.

Cross-process: Pulse runs as a lightweight server, broadcasting ticks over a Unix domain socket.

## Dependencies

- `serde` + `serde_json` (tick serialization)
- `crossbeam-channel` (in-process broadcast)
- No audio dependencies — Pulse is pure timing.

## Testing

- Verify tick count per bar matches PPQN * beats_per_bar.
- Verify tempo change ramp produces smooth BPM transition.
- Verify swing shifts alternate ticks correctly.
- Verify sample_pos advances correctly across multiple buffers.
