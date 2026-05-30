# kazoo-tape — Tape Machine And Recorder Specification

## Identity

`kazoo-tape` is the terminal reel-to-reel: an Ampex/Studer-inspired tape machine, master coloration unit, loop recorder, and take manager. It is fully procedural DSP plus user recording. It does not ship with prerecorded sounds.

The visual metaphor is a serious tape transport:

```text
┌──────────────────────────── KAZOO TAPE ────────────────────────────┐
│  REEL L ◉◉◉◉◉        15 IPS        REEL R ◉◉◉                    │
│                                                                    │
│  [REW] [STOP] [PLAY] [REC] [LOOP] [PUNCH]                         │
│                                                                    │
│  INPUT  ████▌    TAPE  ███▌     OUTPUT ████                       │
│  SAT    o        BIAS  o       HISS   o       WOW    o            │
│  HEAD   o        BUMP  o       FLUT   o       AGE    o            │
│                                                                    │
│  LOOP: 001.1.000 -> 005.1.000     TAKE: mouth_noise_014.wav        │
└────────────────────────────────────────────────────────────────────┘
```

## Roles

`kazoo-tape` has three roles:

1. **Master tape processor**
   - live coloration on the master bus
   - optional on groups/stems

2. **Recorder**
   - writes master, stems, or loops to disk
   - manages takes
   - exports WAV

3. **Tape transport UI**
   - shows reels, counters, levels
   - controls record/play/loop/punch

## Process Placement

Best-case architecture supports two modes:

### Embedded DSP In Mixer

`kazoo-mix` directly runs tape DSP as a master insert. This is best for latency.

```text
master bus -> tape DSP -> limiter -> speakers
```

### Separate Tape UI Process

`kazoo-tape` can connect to the mixer and control/show the tape state. The UI process does not need to carry real-time audio if the DSP is embedded.

```text
kazoo-tape UI -> control IPC -> kazoo-mix tape engine
```

This split gives the best latency and still allows a dedicated tape terminal.

## Tape DSP Model

The tape model is procedural and parameterized.

Signal path:

```text
input trim
  -> pre-emphasis / bias coloration
  -> nonlinear tape saturation
  -> frequency-dependent compression
  -> head bump
  -> high-frequency rolloff
  -> wow/flutter time modulation
  -> hiss/noise injection
  -> crosstalk / stereo glue
  -> output trim
```

### Saturation

Use a smooth nonlinear transfer:

```text
y = tanh(drive * x) / tanh(drive)
```

Best-case enhancement:

- asymmetric saturation
- frequency-dependent drive
- hysteresis approximation
- level-dependent HF loss

### Head Bump

Low-frequency resonance around tape speed/head configuration:

```text
15 ips: bump around 50-70 Hz
30 ips: bump around 80-110 Hz, less obvious
7.5 ips: stronger lower bump, more rolloff
```

Implemented using a low-Q resonant shelf or band emphasis.

### HF Rolloff

Tape speed and age affect high-end:

```text
30 ips: cleaner, extended top
15 ips: classic balanced tone
7.5 ips: darker, noisier
```

### Wow And Flutter

Delay-line modulation:

- wow: slow 0.1-1 Hz movement
- flutter: faster 4-12 Hz subtle movement
- random drift component

Must be bounded and interpolated. No callback allocation.

### Hiss

Procedural noise, never sampled:

- white/pink blend
- shaped by tape speed and noise reduction setting
- optional off
- level low by default

### Crosstalk / Stereo Glue

Small L/R bleed and shared saturation behavior.

## Recorder

Recording is not done in the audio callback. The callback writes audio blocks into a bounded lock-free queue. A disk thread writes WAV files.

### Recording Sources

- master post-tape
- master pre-tape
- selected groups
- selected channels/stems
- tape return
- loop captures

### Take Model

```text
Take {
    id,
    name,
    source,
    start_frame,
    end_frame,
    sample_rate,
    channels,
    file_path,
    bpm_snapshot,
    loop_region_snapshot,
}
```

### Loop Recording

Loop recording belongs to the studio transport but is surfaced heavily in tape.

Features:

- set loop start/end in bars/beats or frames
- count-in
- overdub passes
- keep takes per pass
- auto-name takes
- quick comp later

## Tape Transport And BPM Coordination

Tape is slaved to the studio transport unless explicitly in offline playback mode.

Shared state:

```text
session_frame
bpm
time_signature
loop_start
loop_end
record_state
count_in_state
```

Even though tape is analog-inspired, the recording loop is sample-accurate and BPM-aware.

## Punch In/Out

Best-case punch controls:

- manual punch
- automatic punch region
- pre-roll
- post-roll
- take naming
- non-destructive takes

## File Format

Primary: WAV, 32-bit float.

Optional later:

- FLAC export
- stem folder export
- session archive

No compressed format in the recording path.

## Tape UI Controls

```text
Esc / Ctrl-Q / Ctrl-C / Ctrl-D   quit
Space                            play/stop
r                                record arm / record
l                                loop toggle
[ ]                              move loop boundaries
, .                              rewind/forward
1/2/3                            tape speed: 7.5 / 15 / 30 ips
+ -                              selected parameter
Tab                              focus section
?                                help
```

## Tape Parameters

```text
speed:        7.5 | 15 | 30 ips
input_gain:   dB
output_gain:  dB
saturation:   0..1
bias:         0..1
head_bump:    0..1
hf_loss:      0..1
wow:          0..1
flutter:      0..1
hiss:         0..1
age:          0..1
crosstalk:    0..1
noise_reduction: off | gentle | strong
```

## Internal Architecture

```rust
struct TapeMachine {
    params: TapeParams,
    transport: TapeTransportState,
    left: TapeChannel,
    right: TapeChannel,
    wow_flutter: WowFlutter,
    hiss: NoiseGenerator,
    recorder: RecorderTap,
}

struct Recorder {
    queue: RingBuffer<AudioRecordBlock>,
    disk_thread: JoinHandle<()>,
    active_takes: Vec<TakeWriter>,
}
```

The DSP object must be embeddable in `kazoo-mix` without spawning a process.

## Latency

Tape coloration adds no intentional block latency except wow/flutter delay-line modulation if enabled. Delay line max is small and can be compensated or accepted as part of the master effect.

Recording adds no monitoring latency because it taps already-produced audio.

## Quality Targets

- no callback allocation
- bounded noise generation
- no file writes in callback
- finite output under extreme input
- meter before and after tape
- bypass is sample-stable and click-managed

## Future: Tape As Arrangement Surface

Eventually tape can become the arrangement/timeline view:

- takes on reels
- clip lanes
- splice/cut metaphor
- bounce to tape
- rewind/scrub

But the first best-case design treats tape as live master processor + recorder + loop capture.
