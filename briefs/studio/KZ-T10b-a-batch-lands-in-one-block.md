# KZ-T10b: a batch is heard whole or not at all, from one render block on; a refused change never takes the daemon's own pending lines with it

Line numbers are from main at `9d2e569` and may have moved. Find each item by what it does; never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 04:3x, from the Opus review of KZ-T10a ("not confirmed", points 1 and 3). The decisions are Cassio's, each with its reason beside it.

Place: Track T. Time: about 45 minutes. Crates: kazoo-wall (`engine/batch.rs`, the engine's command ring and pump, `daemon/wall/batch.rs`, `daemon/wall.rs` `put_back`, and their tests). Depends on KZ-T10a (landed). Runs beside nothing that touches `engine/` or `daemon/wall.rs`.

## Why

Tom asked for batched changes that come through as one gesture with a note. KZ-T10a makes a batch one change in the log and the tree. The engine, though, gets the batch's commands one at a time through the 512-slot command ring, and the audio thread drains while they are pushed. So a batch can be heard half-made for a block or two. Past about 250 adds and patches, the rest of a batch waits for the next pump. A half-made patch for a block is a click, or a module heard without its cables.

Also, from before T10a and still there: when a change is refused, `put_back` restores the patch as it was. That includes undoing the effects of the daemon's own log lines written meanwhile (a loop take, a desk tempo), which were not part of the refused change.

## Outcome

1. **A batch is applied at one block boundary.** The engine takes a batch's commands as one unit. The daemon writes them all into a staging area the audio thread does not read, then publishes them with one ring entry, a `Batch` marker that holds the staged commands' count. At the start of a block, the audio thread applies every staged command of the marker before rendering, or none if the marker has not arrived. The staging area is allocated once, at engine start, at the ring's size or larger. A batch bigger than the staging area grows it on the control side before publishing, never on the audio thread. Decision: the audio thread never allocates and never sees half a batch. A batch is no longer limited by the ring's size, because the ring carries one marker.
2. **`put_back` restores only what the refused change touched.** A refused change, single or batch, puts back the patch fields it changed and nothing else. Lines the daemon itself logged while the change was being tried stay applied. Decision: the daemon's own lines are separate changes in the log, so undoing them silently breaks the log's promise that it says what the wall is.

## Tests (write them first; each fails before the change)

- A batch of 400 adds and patches: after any number of rendered blocks, the engine has either none of them or all of them. Check it at every block boundary, with a test engine stepped one block at a time while the daemon publishes.
- No allocation on the audio thread while applying a 400-step batch. Use the existing `assert_no_alloc` render test pattern.
- A batch that grows the staging area: published whole, with no allocation on the audio thread.
- A change refused while a loop take's own log line is written meanwhile: the take stays on the wall and in the log, and the refused change's own fields are back as they were.
