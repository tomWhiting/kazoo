# Kazoo Terminal DAW — Best-Case Architecture

## Vision

Kazoo becomes a terminal-native digital audio workstation where every instrument can run in its own terminal, a central console mixes them in real time, and a tape machine/recorder captures the result. The joke is "mouth noises"; the implementation is serious.

The intended feel is not a plugin host with a terminal skin. It is a studio made of terminal programs:

```text
kazoo-mix       — Allen & Heath / Soundcraft-style terminal mixing desk
kazoo-tape      — Ampex / Studer-style tape machine, recorder, loop capture
kazoo-mouth     — voice-driven mouth-noise instrument formerly known as kazoo-tui
kazoo-juno      — procedural Juno-inspired polysynth
kazoo-303       — acid bassline
kazoo-808       — procedural drum machine
kazoo-cs80      — expressive cinematic poly
kazoo-mini      — mono bass/lead
kazoo-prophet   — Prophet-style poly
kazoo-arp       — sequencer/controller
kazoo           — future unified binary containing all of the above subcommands
```

The terminal is the surface. The architecture underneath is a low-latency, process-separated audio graph.

## Non-Negotiables

1. **No borrowed sounds.** Instruments generate sound mathematically. If sampling exists later, it is for the user's own recordings, not bundled sample playback.
2. **Mixer owns the soundcard.** In studio mode, only `kazoo-mix` opens the output audio device.
3. **One transport authority.** BPM, play/stop, bar/beat position, loop region, and recording clock come from the studio transport.
4. **Audio callback never blocks.** No sockets, allocation, locks, serialization, or file I/O in the callback.
5. **Low, stable latency beats theoretical zero latency.** The system targets predictable small buffers and bounded jitter.
6. **Every app remains fun standalone.** Instruments can still run directly when no studio is present.
7. **Terminal-first, not terminal-only internally.** The UI is text; the audio architecture is professional.

## Topology

```text
                              ┌──────────────────────────────┐
                              │          kazoo               │
                              │ future unified launcher      │
                              └──────────────┬───────────────┘
                                             │
              ┌──────────────────────────────┼──────────────────────────────┐
              │                              │                              │
        ┌─────▼─────┐                  ┌─────▼─────┐                  ┌─────▼─────┐
        │kazoo-juno │                  │kazoo-303  │                  │kazoo-mouth│
        │instrument │                  │instrument │                  │voice inst │
        └─────┬─────┘                  └─────┬─────┘                  └─────┬─────┘
              │ low-latency local IPC        │                              │
              └──────────────┬───────────────┴───────────────┬──────────────┘
                             │                               │
                      ┌──────▼───────────────────────────────▼──────┐
                      │                 kazoo-mix                    │
                      │  transport master + audio graph + console    │
                      │                                              │
                      │  channel strips -> groups -> master -> tape   │
                      └────────────────────┬─────────────────────────┘
                                           │ internal bus or IPC
                                  ┌────────▼────────┐
                                  │   kazoo-tape    │
                                  │ tape + recorder │
                                  └────────┬────────┘
                                           │
                                    speaker output
```

## Process Model

### Studio Mode

- `kazoo-mix` starts first and creates a studio session socket.
- Instruments discover/register with the mixer.
- Mixer assigns each client one or more channels.
- Mixer publishes sample rate, block size, BPM, transport state, loop region, swing templates, and session frame.
- Instruments render into mixer-owned timing.
- Mixer mixes, meters, records, and outputs.

### Standalone Mode

If no mixer is present, each instrument may open `cpal` itself and play directly. Standalone mode is for sketching, not final studio routing.

## Best-Case Audio Transport

The best long-term split is **socket control plane + shared-memory audio plane**.

### Control Plane

Unix domain socket:

- registration
- parameter updates
- transport state
- render scheduling
- MIDI/note events
- meters/status
- reconnect/shutdown

### Audio Plane

Shared memory ring buffers:

- one or more producer rings per instrument output
- mixer consumes from lock-free shared rings
- fixed block size negotiated at registration
- frame-indexed packets to detect underruns and drift

### Why Not Only Sockets?

Unix sockets are good for early development, but the best-case version should avoid copying every audio block through serialized messages. Sockets remain the correct tool for control and lifecycle. Shared memory is the correct tool for repeated real-time audio buffers.

## Clocking And Latency

The mixer owns the hardware clock. Instruments do not free-run in studio mode.

Target defaults:

```text
sample rate:       48 kHz
hardware block:    128 frames
render quantum:    128 frames
safety lead:       2-3 blocks
expected latency:  roughly 8-12 ms end-to-end on normal hardware
```

For very low-latency mode:

```text
block:       64 frames
safety lead: 2 blocks
latency:     lower, but more CPU/IPC pressure
```

The design accepts that absolute zero latency is impossible. The goal is stable latency with no random stalls.

## Render Scheduling

The mixer has two real-time-adjacent components:

1. **Audio callback**
   - reads ready blocks from per-channel ring buffers
   - applies gain/EQ/pan/sends/groups/master/tape
   - writes to output
   - records tap points into preallocated buffers
   - emits lightweight meter snapshots

2. **Scheduler thread**
   - runs ahead of the hardware callback
   - issues render jobs for future frame ranges
   - receives/observes completed blocks
   - marks missing blocks as underruns
   - updates transport phase and loop state

Instruments render requested frame ranges. They may run their own DSP thread and UI thread, but their audio output is tied to mixer frame positions.

## Transport Authority

`kazoo-mix` and `kazoo-tape` coordinate closely, but only one object owns musical time: the **studio transport**.

Transport state contains:

```text
sample_rate
block_size
session_frame
bpm
time_signature
swing_template
play_state: stopped | playing | recording | count_in
loop_enabled
loop_start_frame
loop_end_frame
bar_beat_tick position
metronome state
```

BPM sync is not optional. 303, 808, arp, delays, LFO sync, and tape loop capture all follow the same transport.

## Swing And Feel

Swing should not be an afterthought. The transport supports future groove templates:

- straight
- MPC-style swing percentages
- backbeat push/pull
- Pino-ish laid-back bass feel templates
- per-lane timing offsets
- humanization with deterministic seeds

Important: groove is control timing, not audio clock drift. The sample clock stays stable.

## Crate Split

### `kazoo-core`

Shared DSP, protocol types, transport math, ring buffers, IPC primitives, sample utilities.

Likely modules:

```text
kazoo-core/src/ipc/
kazoo-core/src/transport/
kazoo-core/src/audio_graph/
kazoo-core/src/mixer/
kazoo-core/src/tape/
kazoo-core/src/protocol/
```

### `kazoo-mix`

The central console and studio host.

Responsibilities:

- audio device ownership
- channel strips
- buses/groups
- sends/returns
- meters
- transport master
- plugin/instrument registration
- master output
- session save/load
- launch/discovery integration

### `kazoo-tape`

Tape model and recorder. It may be a standalone terminal app, a subview in `kazoo-mix`, or both.

Responsibilities:

- master tape coloration
- loop recording
- stem recording
- punch in/out
- take management
- WAV export
- optional tape transport UI

### `kazoo-mouth`

Rename/re-scope current voice-driven `kazoo-tui` concept into a focused mouth-noise instrument.

Responsibilities:

- mic input
- pitch tracking
- beatbox/onset extraction
- formant/vocoder/granular modes
- mouth-noise performance interface
- sends generated audio to mixer like any other instrument

### Existing Instruments

Each existing synth becomes a studio client:

- standalone output mode
- studio client mode
- shared transport receiver
- render-block provider
- UI remains local to the instrument terminal

### Future `kazoo` Unified Binary

One binary with subcommands:

```bash
kazoo mix
kazoo tape
kazoo mouth
kazoo juno
kazoo 303
kazoo 808
kazoo cs80
kazoo mini
kazoo prophet
kazoo arp
kazoo studio    # launch coordinated session layout
```

Internally, these can remain crates. Distribution becomes one installable command.

## Session Model

A Kazoo session is a directory:

```text
my-song.kazoo/
  session.toml
  routing.toml
  mixer.toml
  transport.toml
  tape/
    takes/
    stems/
  patches/
    juno/
    303/
    mouth/
  logs/
```

The mixer can restore:

- connected instrument identities
- channel layout
- faders/EQ/sends
- transport/BPM/loop
- tape takes
- patch references

Instruments remain independent, but the session captures enough to reconstruct a studio.

## Terminal UX Principle

The terminal DAW should feel physical:

- mixer: old console
- tape: reel-to-reel transport
- instruments: hardware front panels
- arp/sequencer: classic pattern box
- mouth: weird lab instrument / vocal processor

Keyboard and mouse support should both exist. Keyboard must always include a reliable escape route.

## Failure Modes

The mixer must survive instrument crashes.

If a client disappears:

- channel goes silent
- meter shows disconnected
- fader/settings remain
- reconnection can reclaim channel
- no mixer panic
- no audio callback block

If mixer stops:

- clients detect disconnect
- optionally fall back to standalone mode
- or show "studio disconnected"

If audio blocks are late:

- mixer outputs silence for that block/channel
- underrun counter increments
- UI shows warning
- no waiting in callback

## Quality Bar

The "best possible" version is not defined by feature count. It is defined by:

- stable transport
- robust process separation
- low predictable latency
- no stuck notes
- no callback blocking
- clean disconnect behavior
- readable terminal UI
- musical default settings
- serious DSP
- fun identity

## Open Design Questions

1. Should `kazoo-tape` be a separate process or a mixer module with optional standalone UI?
2. Should the first IPC implementation go straight to shared memory, or start socket-only with protocol shaped for shared memory?
3. Should `kazoo-mix` be built fresh, or should `kazoo-tui` be renamed and reduced?
4. How many channels should the default console expose: 8, 12, 16, or dynamic?
5. Should recording be mixer-owned or tape-owned?
6. How should terminal layout launching work on macOS: manual terminals, tmux, wezterm, iTerm profiles, or `kazoo studio` spawning panes?
