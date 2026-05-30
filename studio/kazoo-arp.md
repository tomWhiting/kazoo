# kazoo-arp — Arpeggiator

Note scheduler, not a sound source. Drives whichever synth it's routed to (kazoo-mini, kazoo-cs80, or any future instrument). Roland Jupiter-8 inspired feature set. Can run as a companion panel within another instrument's TUI or as its own standalone crate. Connects to hub for transport sync.

## Pattern Modes

**Up:** Ascending pitch order, wraps at top.
```
Notes held: C E G
Pattern:    C E G C E G C E G ...
```

**Down:** Descending pitch order, wraps at bottom.
```
Notes held: C E G
Pattern:    G E C G E C G E C ...
```

**Up/Down (exclusive):** Ascending then descending. Top and bottom notes play once, not doubled. Pattern length = 2*(N-1).
```
Notes held: C E G
Pattern:    C E G E C E G E C ...
```

**Random:** Each tick picks a random note from the held set. Optional no-immediate-repeat rule.

**As-Played:** Notes in the order they were pressed, not sorted by pitch. Maintained as a separate insertion-order list.

## Clock

Syncs to hub transport BPM.

| Division | Notes per beat |
|----------|---------------|
| 1/1      | 0.25          |
| 1/2      | 0.5           |
| 1/4      | 1             |
| 1/8      | 2             |
| 1/16     | 4             |
| 1/32     | 8             |
| 1/8T     | 3 (triplet)   |
| 1/16T    | 6 (triplet)   |

Period in samples at sample rate `Fs` and tempo `bpm` for division `d` (notes per beat):
```
period_samples = Fs * 60.0 / (bpm * d)
```

**Swing:** 0-100%. Applied to even-numbered steps. At 50% = straight. Above 50% = even steps delayed (shuffle feel). Below 50% = even steps pushed forward.

## Features

**Octave Range (1-4):**
- 1 octave: play held notes only
- 2 octaves: play pool, then play again transposed +12 semitones
- 3 octaves: pool, +12, +24
- 4 octaves: pool, +12, +24, +36

Implementation: expand the note pool with transposed copies before cycling.

**Latch:** When active, releasing all keys does not clear the note pool. Arp continues with last held set. Pressing new keys replaces the pool.

**Gate Length:** Percentage of step duration that the note sounds (10-100%). At 100% = legato. At 10% = very staccato.

**Velocity:** Per-step velocity override, or follow input velocity from held keys.

## Core Data Structure

```rust
struct Arpeggiator {
    note_pool: Vec<(u8, u8)>,      // (midi_note, velocity), sorted by pitch
    played_pool: Vec<(u8, u8)>,    // insertion-order for As-Played mode
    index: usize,
    direction: Direction,           // Forward | Backward (for Up/Down)
    octave_offset: u8,              // current octave in multi-octave span
    mode: ArpMode,                  // Up | Down | UpDown | Random | AsPlayed
    division: ClockDivision,
    swing: f32,                     // 0.0 to 1.0 (0.5 = straight)
    gate_pct: f32,                  // 0.1 to 1.0
    octave_range: u8,               // 1 to 4
    latch: bool,
    sample_counter: u64,            // samples since last step
    step_counter: u64,              // which step we're on (for swing)
}
```

At each audio callback: increment `sample_counter`. When `sample_counter >= period_samples` (adjusted for swing on even steps): advance index, emit note-on to target synth. When `sample_counter >= period_samples * gate_pct`: emit note-off.

O(1) per tick. Zero DSP latency. The latency is entirely in the synth it drives.

## TUI Layout

```
+-- ARPEGGIATOR -----------------------------------------------+
|                                                               |
| Mode: [Up] [Down] [Up/Down] [Random] [As-Played]             |
|                                                               |
| Div: 1/16   Swing: 55%   Gate: 75%   Oct: 2   Latch: OFF    |
|                                                               |
| Note Pool: C4 E4 G4 B4                                       |
|                                                               |
| Pattern:                                                      |
| ... G4 B4 C5 E5 G5 B5 [C4] E4 G4 B4 C5 E5 ...              |
|                         ^^^^ current note                     |
|                                                               |
| Target: kazoo-mini                                            |
+---------------------------------------------------------------+
```

- Arrow keys: navigate between mode/division/swing/gate/octave/latch
- +/- to adjust values
- Space to toggle latch
- Number keys to select mode (1-5)
- Visual scrolling pattern display showing past and upcoming notes
- Current note highlighted

## Companion Mode

The arpeggiator doesn't need to be its own terminal. It can embed as a panel within kazoo-mini or kazoo-cs80's TUI. When embedded:

- Takes note input from the host synth's keyboard handler
- Outputs note events back to the host synth's voice engine
- Shares the host's transport/BPM sync
- Rendered as a collapsible panel at the bottom or side of the host TUI

## Crate Structure

```
kazoo-arp/
  Cargo.toml          # minimal deps: no audio, just note scheduling
  src/
    lib.rs            # Arpeggiator engine (usable as library by other crates)
    main.rs           # Standalone TUI mode
    app.rs            # Application state
    engine.rs         # Core arpeggiator: note pool, index, clock, step logic
    clock.rs          # Clock division, swing calculation, hub sync
    ui/
      mod.rs          # Layout
      controls.rs     # Mode/division/swing/gate editors
      pattern.rs      # Scrolling pattern visualization
    ipc.rs            # Hub connection for transport sync + note output
```

Key design decision: `kazoo-arp` exposes its engine as a **library** (`lib.rs`) so other instrument crates can embed the arpeggiator without running it as a separate process. The `main.rs` is for standalone operation.

## Dependencies on kazoo-core

Minimal. The arpeggiator does no DSP — it schedules notes.

Uses from `kazoo-core`:
- Transport/clock types for hub sync
- That's basically it

No new DSP needed. The arpeggiator is pure logic: a state machine consuming a sorted array and emitting note events on a clock.
