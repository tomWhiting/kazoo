# Crate Split, Renames, And Unified Binary Plan

## Desired Split

Kazoo should become a family of focused crates with one future installable command.

```text
kazoo-core       shared DSP, protocol, transport, mixer/tape libraries
kazoo-mix        central terminal mixing desk
kazoo-tape       tape UI + recorder controls + tape library if not in core
kazoo-mouth      voice/mouth-noise instrument, renamed from broad kazoo-tui concept
kazoo-juno       Juno-style synth
kazoo-303        acid bassline
kazoo-808        drum machine
kazoo-cs80       cinematic poly
kazoo-mini       Moog-style mono
kazoo-prophet    Prophet-style poly
kazoo-arp        controller/sequencer
kazoo            unified binary / launcher
```

## Rename `kazoo-tui` To `kazoo-mouth`

The current broad TUI concept should stop being the whole studio and become the mouth-noise instrument.

### Why

The voice-driven concept is strong, but it should be one instrument in the studio, not the mixer, recorder, synth host, and timeline all at once.

### New Identity

`kazoo-mouth`:

- mic input
- humming to synth
- beatbox to drums/control
- mouth noise to granular/formant/vocoder textures
- pitch tracking
- onset tracking
- formant extraction
- sends generated audio to `kazoo-mix`

### Removed From Mouth

These belong elsewhere:

- central mixer -> `kazoo-mix`
- tape/recording -> `kazoo-tape`
- session routing -> `kazoo-mix`
- global transport -> `kazoo-mix`
- DAW timeline -> `kazoo-mix`/`kazoo-tape`

## Library/Binary Layout

Each major crate should expose a library plus binary where useful.

Example:

```text
kazoo-juno/
  src/lib.rs      # synth engine + params
  src/main.rs     # terminal UI + standalone/studio client
```

For new crates:

```text
kazoo-mix/
  src/lib.rs
  src/main.rs
  src/engine.rs
  src/ui.rs
  src/session.rs
  src/client_registry.rs

kazoo-tape/
  src/lib.rs
  src/main.rs
  src/dsp.rs
  src/recorder.rs
  src/ui.rs
```

## `kazoo-core` Responsibilities

Move shared final-architecture pieces here:

```text
protocol types
transport math
BPM/bar/beat conversion
swing/groove math
shared memory abstraction
ring buffer primitives
metering helpers
console EQ DSP
tape DSP if shared by mix and tape
sample utilities
```

Do not put terminal UI code in `kazoo-core`.

## Unified Binary

Eventually add a top-level binary crate/package named `kazoo`.

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
kazoo studio
```

### Why Keep Individual Crates?

- independent development
- smaller compile units during iteration
- clear responsibilities
- easier testing

### Why Add Unified Binary?

- one install command
- easier demo/use
- can launch whole studio
- consistent flags/config

## Shared CLI Flags

All instruments should support:

```text
--standalone             force local audio output
--connect                force studio client mode
--session <id/path>      connect to specific session
--name <display-name>    instance name shown in mixer
--channel <n>            preferred channel
--no-audio               UI/control only where relevant
```

Mixer:

```text
kazoo mix --session my-song --sample-rate 48000 --block-size 128
```

Tape:

```text
kazoo tape --session my-song
```

Studio launcher:

```text
kazoo studio my-song --layout mouth+juno+303+808+tape
```

## Terminal Layout Launcher

Best-case future `kazoo studio` can launch panes/windows.

Backends:

- tmux
- wezterm CLI
- iTerm2 AppleScript on macOS
- plain multiple process launch fallback

This is convenience, not architecture. Manual terminals must always work.

## Mode Matrix

| Crate | Standalone Audio | Studio Client | UI Only | Library DSP |
|---|---:|---:|---:|---:|
| kazoo-mix | yes | server | no | yes |
| kazoo-tape | optional | controller/module | yes | yes |
| kazoo-mouth | yes | yes | no | yes |
| kazoo-juno | yes | yes | no | yes |
| kazoo-303 | yes | yes | no | yes |
| kazoo-808 | yes | yes | no | yes |
| kazoo-arp | no/optional click | controller | yes | yes |

## Session Identity

Every app in a studio session needs:

```text
session_id
client_id
instance_id
human display name
crate name/version
protocol version
```

`instance_id` persists for session restore. `client_id` is assigned per connection.

## Dependency Direction

Good dependency flow:

```text
kazoo-core <- kazoo-juno
kazoo-core <- kazoo-mix
kazoo-core <- kazoo-tape
kazoo-core <- kazoo-mouth

kazoo unified binary depends on all app crates/libraries
```

Avoid instruments depending on `kazoo-mix` directly. They depend on shared client protocol in `kazoo-core`.

## Migration Plan

1. Leave current instruments working standalone.
2. Add common studio client protocol in `kazoo-core`.
3. Add `kazoo-mix` as new central server.
4. Add studio-client mode to `kazoo-juno` first.
5. Add `kazoo-303`, `kazoo-808`, `kazoo-arp` sync next.
6. Rename/split `kazoo-tui` into `kazoo-mouth` once mixer exists.
7. Add `kazoo-tape` as embedded DSP + optional UI.
8. Add unified `kazoo` binary.

## Naming

Recommended final public names:

```text
kazoo mix
kazoo tape
kazoo mouth
kazoo juno
kazoo acid      # alias for 303
kazoo 303
kazoo drums     # alias for 808
kazoo 808
kazoo cs80
kazoo mini
kazoo prophet
kazoo arp
```

For project humor/public posting:

```text
Topic/tagline: Mouth Noises
Serious subtitle: A terminal-native modular DAW for procedural instruments
```
