# Terminal DAW User Stories

## Studio Launch

### Story: Launch A Studio

As a user, I want to open a mixer, tape machine, and several instruments in separate terminals so that the computer feels like a terminal-based studio.

Acceptance:

- `kazoo mix` starts a studio server.
- Instruments can connect from other terminals.
- Mixer shows each connected instrument on a channel.
- Quitting an instrument does not crash the mixer.

### Story: Unified Launch Later

As a user, I want one `kazoo studio` command to launch my whole session layout so that I do not need to remember seven commands.

Acceptance:

- `kazoo studio my-song` starts mixer and chosen clients.
- Manual terminal startup still works.

## Playing Instruments

### Story: Play Juno Into Mixer

As a user, I want to play `kazoo-juno` in one terminal and hear it through `kazoo-mix` so that the mixer is my studio desk.

Acceptance:

- Juno does not open its own output device in studio mode.
- Pressing a note sends generated audio to assigned mixer channel.
- Holding a key sustains.
- Releasing a key releases envelope.
- Ctrl-Q/C/D and Esc quit reliably.

### Story: Acid And Drums Follow BPM

As a user, I want 303 and 808 patterns to follow the mixer BPM so that everything stays locked.

Acceptance:

- Changing BPM in mixer updates 303 and 808.
- Pattern steps land on expected bar/beat positions.
- Start/stop comes from mixer transport.

## Mixing

### Story: Ride Faders

As a user, I want to adjust faders, trim, pan, EQ, and mute/solo per channel so that I can mix the terminal instruments like a desk.

Acceptance:

- Each channel has fader, pan, mute, solo, arm.
- Levels affect audio immediately without zippering/clicks.
- Meters show pre/post signal.

### Story: See Problems

As a user, I want the mixer to show underruns and disconnected clients so that low-latency problems are visible.

Acceptance:

- Channel shows connected/disconnected.
- Channel shows underrun counter or warning.
- Missing blocks become silence, not a crash.

## Tape And Recording

### Story: Record A Loop

As a user, I want to set a BPM-synced loop region and record takes so that I can build ideas quickly.

Acceptance:

- Loop start/end can be set in bars/beats.
- Recording starts on a frame-aligned boundary.
- Takes are saved as WAV.
- Take metadata includes BPM and loop region.

### Story: Tape Color

As a user, I want to run the master through tape coloration so that the terminal studio sounds glued and vibey.

Acceptance:

- Tape can be bypassed.
- Saturation, head bump, hiss, wow/flutter are adjustable.
- Tape does not add unstable latency or callback blocking.

## Mouth Noises

### Story: Use Voice As Instrument

As a user, I want `kazoo-mouth` to turn humming, beatboxing, and mouth noises into procedural instruments so that the project lives up to the "mouth noises" joke.

Acceptance:

- Mouth instrument connects to mixer like any other client.
- It can generate pitch-tracked, vocoder, granular, or formant sounds.
- It does not own the central mixer/recorder responsibilities.

## Session Restore

### Story: Reopen A Song

As a user, I want to reopen a session and recover mixer routing, channel settings, tape takes, and instrument identities.

Acceptance:

- Session folder contains mixer, transport, routing, and tape metadata.
- Reconnected instruments reclaim prior channels when possible.
- Missing instruments show as disconnected channels.

## Feel And Groove

### Story: Add Swing

As a user, I want swing/groove to be controlled centrally so that drum machines and arps feel together.

Acceptance:

- Mixer transport exposes swing amount/template.
- 303/808/arp apply swing to event timing.
- Audio sample clock remains stable.

### Story: Pino-ish Backbeat Feel

As a user, I want optional laid-back groove templates so that programmed parts can sit behind the beat.

Acceptance:

- Groove template can delay selected subdivisions.
- Groove is deterministic and session-saved.
- Timing offsets are sample-based.
