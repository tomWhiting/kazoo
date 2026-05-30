# kazoo-mix — Terminal Mixing Desk Specification

## Identity

`kazoo-mix` is the studio center: a terminal mixing console inspired by old Allen & Heath, Soundcraft, Midas, and small-format studio desks. It is not just a visual mixer. It owns the audio device, transport, routing, session graph, and real-time safety rules.

## Goals

- Mix many terminal instruments in real time.
- Provide tactile channel-strip controls in a terminal UI.
- Act as transport/BPM/loop authority.
- Route audio to master, groups, auxes, and tape.
- Record stems and master passes without blocking the audio callback.
- Survive client disconnects.
- Keep latency stable and visible.

## Console Layout

Default best-case desk:

```text
16 mono/stereo input channels
4 stereo groups
4 aux sends
2 stereo FX returns
1 master bus
1 tape return
1 monitor bus
```

The UI can page horizontally for more channels.

```text
┌──────────────────────────── KAZOO MIX ─────────────────────────────┐
│ STOP  120.00 BPM  4/4  LOOP 001.1.000-005.1.000  48k/128  CPU 18% │
├────────┬────────┬────────┬────────┬────────┬────────┬─────────────┤
│ CH 01  │ CH 02  │ CH 03  │ CH 04  │ CH 05  │ CH 06  │ MASTER      │
│ JUNO   │ 303    │ 808    │ MOUTH  │ CS80   │ MINI   │             │
│ IN ●   │ IN ●   │ IN ●   │ IN ●   │ IN ○   │ IN ●   │ TAPE ●      │
│ TRIM o │ TRIM o │ TRIM o │ TRIM o │ TRIM o │ TRIM o │ COMP o      │
│ HPF  o │ HPF  o │ HPF  o │ HPF  o │ HPF  o │ HPF  o │ SAT  o      │
│ HI   o │ HI   o │ HI   o │ HI   o │ HI   o │ HI   o │ WIDTH o     │
│ MIDF o │ MIDF o │ MIDF o │ MIDF o │ MIDF o │ MIDF o │             │
│ MIDG o │ MIDG o │ MIDG o │ MIDG o │ MIDG o │ MIDG o │ L █████     │
│ LOW  o │ LOW  o │ LOW  o │ LOW  o │ LOW  o │ LOW  o │ R ████      │
│ A1   o │ A1   o │ A1   o │ A1   o │ A1   o │ A1   o │             │
│ A2   o │ A2   o │ A2   o │ A2   o │ A2   o │ A2   o │             │
│ PAN <>│ PAN <>│ PAN <>│ PAN <>│ PAN <>│ PAN <>│             │
│ M S R │ M S R │ M S R │ M S R │ M S R │ M S R │ REC ●       │
│  █    │  █    │  █    │  █    │  █    │  █    │  █          │
│  █    │  █    │  █    │  █    │  █    │  █    │  █          │
│ fader │ fader │ fader │ fader │ fader │ fader │ master      │
└────────┴────────┴────────┴────────┴────────┴────────┴─────────────┘
```

## Channel Strip

Each input channel has:

### Input

- client connection status
- mono/stereo mode
- input trim, -24 dB to +24 dB
- polarity invert
- input meter pre-fader
- underrun/missing-block indicator

### Filter/EQ

Best-case channel EQ:

- high-pass filter, 20 Hz to 400 Hz, 12/18/24 dB options
- low shelf gain/frequency
- sweepable low-mid bell
- sweepable high-mid bell
- high shelf gain/frequency
- optional EQ bypass

Default should feel like a musical analog desk, not a surgical DAW EQ.

### Dynamics

Per-channel optional dynamics:

- simple compressor
- gate/expander
- sidechain source selector later

Initial best-case design includes the strip points even if all dynamics are not immediately coded.

### Routing

- pan/balance
- mute
- solo/PFL/AFL
- record arm
- group assignment 1-4
- master assignment
- aux sends 1-4
- send mode pre/post fader

### Fader

- long-throw fader visually represented in terminal
- dB scale, not linear amplitude
- automation-ready parameter model

## Buses

### Input Channels

Each connected instrument maps to one or more input channels. A stereo synth gets a stereo channel where possible.

### Groups

Groups are stereo buses for drums, synths, mouth, bass, etc.

### Aux Sends

Aux sends are stereo buses for shared effects:

- delay
- reverb
- weird terminal FX
- external process return later

### Master

Master bus chain:

```text
sum -> bus trim -> optional compressor -> tape send/return -> limiter -> output
```

The limiter is safety only, not a mastering tool.

## Real-Time Audio Rules

The audio callback may:

- read from preallocated ring buffers
- run fixed DSP with no allocation
- update atomics/meters
- write output buffer

The audio callback may not:

- read sockets
- write files
- allocate Vec/String
- lock mutexes
- wait for a client
- parse protocol messages
- draw UI

## Metering

Meters are calculated in the audio callback or a post-callback meter thread with preallocated data:

- peak
- RMS/VU-style slow meter
- clip hold
- underrun count
- latency/safety queue depth per channel

UI should show both signal and health.

## Transport Panel

The mixer transport owns:

- play/stop
- record
- count-in
- BPM
- time signature
- loop start/end
- metronome
- session frame
- bar/beat display
- swing/groove template

Transport shortcuts should be global and never conflict with channel note input because the mixer is not a playable keyboard.

## Recording Integration

The mixer can record:

- master post-tape
- master pre-tape
- groups
- individual channels/stems
- loop takes

Disk I/O is delegated to a recording thread using preallocated block queues.

## Client Lifecycle

### Registration

Client sends:

```text
instrument name
instance id
output count
preferred channel names
capabilities
parameter summary
```

Mixer replies:

```text
session id
channel ids
sample rate
block size
transport state
shared memory descriptors
```

### Disconnect

If client disappears:

- channel remains in desk
- audio goes silent
- channel label shows disconnected
- reconnection can reclaim via stable instance id

## Control Surface Model

Every control is a parameter:

```text
ParameterId
human label
value
range
unit
curve: linear | dB | exponential | enum
automation lane id
```

This makes future automation, MIDI mapping, and remote control possible.

## Keyboard UX

Mixer shortcuts:

```text
Esc / Ctrl-Q / Ctrl-C / Ctrl-D   quit
Space                            play/stop
r                                record
l                                loop on/off
[ ]                              move channel bank
Tab / Shift-Tab                  focus section
← →                              select channel/control
↑ ↓                              select row/control
+ -                              adjust
m/s/a                            mute/solo/arm selected channel
?                                help
```

Quit must be checked before all other mappings.

## Mouse UX

Terminal mouse support should allow:

- click channel
- drag fader
- scroll knob/parameter
- click mute/solo/arm
- click transport

Keyboard remains complete; mouse is convenience.

## Internal Data Structures

```rust
struct MixerEngine {
    sample_rate: f32,
    block_size: usize,
    transport: TransportState,
    channels: Vec<ChannelStrip>,
    groups: Vec<StereoBus>,
    auxes: Vec<StereoBus>,
    master: MasterBus,
    tape: TapeInsert,
}

struct ChannelStrip {
    id: ChannelId,
    input: InputSource,
    trim_db: f32,
    hpf: HighPass,
    eq: ConsoleEq,
    dynamics: ChannelDynamics,
    sends: [AuxSend; 4],
    pan: f32,
    fader_db: f32,
    mute: bool,
    solo: bool,
    arm: bool,
    meter: MeterState,
}
```

In implementation, the audio callback uses fixed arrays or slab-allocated channel storage, not arbitrary Vec mutation inside the callback.

## Best-Case Performance Targets

```text
channels:        16 stereo active
sample rate:     48 kHz
block size:      128 frames
safety lead:     2-3 blocks
xruns:           visible, not silent
CPU:             < 50% on target laptop for normal sessions
```

## Relationship To `kazoo-tape`

Tape can be embedded as the master insert while also exposing a separate terminal UI.

Implementation split:

- tape DSP lives in `kazoo-core` or `kazoo-tape` library
- `kazoo-mix` can instantiate tape DSP directly
- `kazoo-tape` binary can control/show tape machine state

This avoids pushing master audio through another process unless desired.
