# Sampler — WAV Sample Playback Engine

## What It Is

A sample-based instrument. Load WAV files, map them to MIDI notes, play them back with pitch shifting, envelopes, and basic processing. Covers everything that synthesis can't: real drums, vocal chops, foley, orchestral hits, found sound.

## What It Does

### Sample Loading

- Reads WAV files (mono or stereo, any sample rate — resampled to engine rate on load).
- Samples stored in memory as f32 buffers.
- Supports loading a single sample or a sample map (multiple samples across the keyboard).

### Sample Map

A sample map assigns WAV files to MIDI note ranges. Each zone has:

| Field | Description |
|-------|-------------|
| **file** | Path to WAV file. |
| **root_note** | The MIDI note where the sample plays at original pitch. |
| **low_note** | Lowest MIDI note this zone responds to. |
| **high_note** | Highest MIDI note this zone responds to. |
| **low_vel** | Lowest velocity this zone responds to (0-127). |
| **high_vel** | Highest velocity this zone responds to (0-127). |

When a note is played, the sampler finds the matching zone(s) and triggers them. Notes outside the root are pitch-shifted.

### Pitch Shifting

Linear interpolation for sample playback at non-native pitches. The playback rate is calculated from the interval between the played note and the root note:

```
rate = 2^((played_note - root_note) / 12)
```

Rate > 1 = faster/higher. Rate < 1 = slower/lower. Simple and correct for moderate shifts. Extreme shifts (> octave) will sound noticeably "sampler-ish" — that's fine, it's a character.

### Playback Modes

| Mode | Description |
|------|-------------|
| **One-shot** | Play the entire sample regardless of note duration. Good for drums, hits. |
| **Sustain** | Play while note is held, enter release on note-off. Good for pads, loops. |
| **Loop** | Loop between loop_start and loop_end points while note is held. |

### Per-Voice Processing

Each triggered sample voice has:

- **Amp envelope** — ADSR. In one-shot mode, only affects the tail (release phase after sample ends).
- **Filter** — Optional SVF low-pass with cutoff and resonance. Velocity can modulate cutoff.
- **Pan** — Per-zone stereo placement.
- **Volume** — Per-zone gain.
- **Reverse** — Play sample backwards.
- **Start offset** — Skip N samples from the beginning.

### Voice Management

- **Polyphony limit** — Configurable max voices (default 32).
- **Voice stealing** — Oldest voice in release phase, then oldest active.
- **Group choking** — Zones can belong to a choke group. Triggering one zone in a group kills other active voices in the same group. Essential for hi-hats (closed chokes open).

## JSON Input

```json
{
  "bpm": 100,
  "bars": 4,
  "max_voices": 16,
  "zones": [
    {
      "file": "samples/kick.wav",
      "root_note": 36,
      "low_note": 36,
      "high_note": 36,
      "mode": "one_shot",
      "volume": 1.0,
      "pan": 0.0
    },
    {
      "file": "samples/snare.wav",
      "root_note": 38,
      "low_note": 38,
      "high_note": 38,
      "mode": "one_shot",
      "volume": 0.9,
      "pan": 0.0
    },
    {
      "file": "samples/hihat_closed.wav",
      "root_note": 42,
      "low_note": 42,
      "high_note": 42,
      "mode": "one_shot",
      "choke_group": 1,
      "volume": 0.7,
      "pan": 0.1
    },
    {
      "file": "samples/hihat_open.wav",
      "root_note": 46,
      "low_note": 46,
      "high_note": 46,
      "mode": "one_shot",
      "choke_group": 1,
      "volume": 0.7,
      "pan": 0.1
    },
    {
      "file": "samples/pad_c3.wav",
      "root_note": 48,
      "low_note": 36,
      "high_note": 72,
      "mode": "loop",
      "loop_start": 44100,
      "loop_end": 132300,
      "amp_adsr": [0.1, 0.2, 0.8, 0.5],
      "filter_cutoff": 5000,
      "filter_resonance": 0.2,
      "volume": 0.6,
      "pan": -0.2
    }
  ],
  "notes": [
    { "beat": 1.0, "note": 36, "velocity": 1.0, "duration": 0.25 },
    { "beat": 1.0, "note": 42, "velocity": 0.8, "duration": 0.25 },
    { "beat": 1.5, "note": 42, "velocity": 0.6, "duration": 0.25 },
    { "beat": 2.0, "note": 38, "velocity": 1.0, "duration": 0.25 },
    { "beat": 2.0, "note": 42, "velocity": 0.8, "duration": 0.25 },
    { "beat": 2.5, "note": 42, "velocity": 0.6, "duration": 0.25 },
    { "beat": 3.0, "note": 36, "velocity": 0.9, "duration": 0.25 },
    { "beat": 3.0, "note": 46, "velocity": 0.7, "duration": 0.25 },
    { "beat": 3.25, "note": 42, "velocity": 0.9, "duration": 0.25 },
    { "beat": 4.0, "note": 38, "velocity": 1.0, "duration": 0.25 },
    { "beat": 1.0, "note": 48, "velocity": 0.7, "duration": 4.0 }
  ],
  "output": "sampler_beat.wav"
}
```

## Dependencies

- `dsp` crate (envelopes, filters, resampling utilities)
- `pulse` crate (clock sync)
- `hound` (WAV reading + writing)
- `serde` + `serde_json`

## Testing

- Load a mono WAV, play at root note, verify output matches original sample data.
- Pitch shift: play a sample at root+12 (one octave up), verify half the duration and double the frequency.
- One-shot mode: trigger a note with short duration, verify full sample plays out.
- Sustain mode: trigger with long duration, verify note sustains. Trigger with short duration, verify release envelope applies.
- Loop mode: trigger a looping sample, verify seamless loop in output.
- Choke group: trigger open hat then closed hat, verify open hat is cut.
- Voice limit: trigger 20 voices with max_voices=8, verify only 8 are active.
- Reverse: play a sample in reverse, verify output is time-reversed.
- Stereo WAV: load stereo sample, verify both channels preserved.
