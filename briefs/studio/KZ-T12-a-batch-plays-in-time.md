# KZ-T12: a batch's changes play in time, each starting when it is told and moving on its own curve

Written by Cassio, 17 Oct 2026, about 18:25. Decided by Cassio; the reasons are given with each decision.

Place: Track T, after KZ-T10c. Crates:

- **kazoo-wall:**
  - `protocol/mod.rs` and `protocol/batch.rs`
  - `patch.rs`
  - `change.rs`
  - `daemon/wall.rs` (`mirror`) and `daemon/wall/batch.rs`
  - `engine/mod.rs`, `engine/room.rs` and `engine/tables.rs`
- **kazoo-mcp:** `tools/batch.rs` and `tools.rs`.
- Tests for all of the above.

## Why

Tom, 17 Oct 2026, about 18:05, on batches: "could we also have timing and sequencing... basically timing and sequencing for and yeah basically fade in dynamics".

Today every step in a batch starts at once, and every knob moves in a straight line. A seat can't make a swell that starts eight beats in, or bring a voice in slowly at first and then quickly, without sending more batches later by hand.

## The rule this keeps

**The patch changes when the change is made.** The wall's state, the log, the tree's moments and undo all hold what each step makes, at the moment the batch is accepted, as now.

**Only the sound is scheduled.** Each step can say when, after the batch, its sound starts, and how it moves.

**Decision:** the tree's past never holds a change that has not happened yet, and an undo undoes what was sent. A knob that is told to rise in eight beats already reads its new value in `wall_look`, which shows it as moving.

## Outcome

### 1. When a step starts: `after_beats`

`after_beats` is a number from 0 to 256 (default 0): how many beats after the batch arrives the step's sound starts. The beats are counted at the tempo when the batch is accepted. A batch's own tempo step moves the tempo, but not that count.

**Which requests take it:**

- **Turn.** The knob rests where it is for `after_beats`, then glides over `glide_beats` as now.
  - In the engine, `Glide` gains `wait` frames. While it waits the knob holds its value, and `gliding` stays set so it is advanced.
  - `Command::Knob` gains `wait_frames`.
- **Patch.** A new cable is silent for `after_beats`, then comes in. A changed amount on an existing cable holds, then moves.
  - `Route` gains `wait: u32` frames. The engine applies it the first time it sees the route's amount, as it does `glide`.
- **Tempo.** It holds for `after_beats`, then moves. `Command::TempoGlide` gains `wait_frames`.

**Who refuses it:** `after_beats` on an add, a remove or an unpatch is refused (`bad_request`), with the reason: "add, remove and unpatch happen at once; fade a level first with after_beats on a turn".

**Decision:** an unplugged cable or a removed module that still sounds would leave the engine playing something the patch no longer holds. That is a second state the patch cannot show. A seat fades the level (a turn with `after_beats`), then sends the removal, and both are visible.

The same fields work on the single tools (`wall_turn`, `wall_patch`, `wall_tempo`), because a batch step is the single tool's request.

### 2. How a knob moves: `curve`

A turn takes `curve`, one of:

| curve | shape over t in 0..1 | use |
|-------|---------------------|-----|
| `linear` (default) | t | today's |
| `ease_in` | t³ | a swell: slow, then fast |
| `ease_out` | 1 − (1 − t)³ | settles in |
| `s_curve` | 3t² − 2t³ | slow at both ends |

**The shape is applied in knob position,** the 0 to 1 travel `KnobSpec::normalise` gives. So a cutoff's curve moves in musical steps, as linear does now.

**In the engine.** `Glide` keeps:

- its start position
- its end position
- its total frames
- the frames gone
- the curve

Each sub-block's `Sweep` is the straight line from the curve's position at the start of the sub-block to its position at the end. Modules read sweeps exactly as now: piecewise linear, at most 32 frames to a piece. The last frame lands exactly on the target.

**Cables fade linearly,** as they do now. A patch takes `fade_beats` (0 to 64, default 0), which is the existing `Route.glide`. A patch carrying a `curve` is refused.

**Decision:** a fade's shape lives on a knob, and a level knob is where a fade belongs. A cable carrying audio at a fixed amount stays simple.

### 3. Records and words

**The record.** `What::Turn` and `What::Patch` record `after_beats` and `curve` / `fade_beats` when they are not the defaults, with `#[serde(default, skip_serializing_if)]`. A log without them reads as before.

**The summary** says them. For example:

> turned out3 level 0 → 0.6 after 8 beats over 16 beats, easing in

**Undo.** An undo glides back as now, at once and linear. An undo of a turn that is still waiting replaces its glide, as any turn does.

### 4. kazoo-mcp

`wall_turn`, `wall_patch`, `wall_tempo` and the matching `wall_batch` steps take:

- `after_beats`
- `curve` (on a turn)
- `fade_beats` (on a patch)

Each schema description says what it does in one line. `wall_look` marks a knob that is waiting or moving with its target, if it does not already show one. Find how it shows a glide now.

### 5. Tests (red first)

**Engine:**

- a knob with `wait` holds exactly that many frames, then glides, and lands exactly;
- each curve's value at a quarter, half and three quarters of its way;
- a new turn mid-wait replaces the wait;
- a cable with `wait` is silent, then fades in over its glide;
- a tempo with `wait` holds, then moves.

**Daemon:**

- beats become frames at the tempo when the change is accepted;
- the refusals above;
- a batch of four turns at `after_beats` 0, 4, 8 and 12 plays them in that order in a rendered engine (render frames and read the knobs).

**Protocol:** a log line without the new fields reads as before.

**MCP:** schema round trip.

## Checks

- nextest, clippy, fmt and ast-grep on `-p kazoo-wall -p kazoo-mcp`
- `cargo test --doc -p kazoo-wall` if doc comments change
- Opus review until READY, then land, publish and install. Tom restarts the wall himself.
