# KZ-T10a: a seat sends several wall changes at once with a note, like a commit message; the wall applies them in order as one change, all or none, and the tree keeps the batch as one moment whose subject is the note and whose steps are listed in it

Line numbers are from main at `e905d15` and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall -p kazoo-mcp`.

Written by Cassio, 15 Oct 2026, 00:2x. Decided by Cassio; reasons beside each decision.

Place: Track T (a new row, T10: batches and radio calls). Time: about 60 minutes. Crates: kazoo-wall (`protocol/mod.rs`, `change.rs`, `daemon/wall.rs`, `daemon/wall/chains.rs` or a new `daemon/wall/batch.rs`, `patch/chains.rs` for the undo, `tree/git.rs`), kazoo-mcp (`tools.rs`, `channel.rs`), and their tests. Runs after KZ-T04d has landed (it touches `wall.rs` and `tree/thread.rs`); never beside T05b or D19 (`wall.rs`). The tree view's side (drawing a batch as a knot with its note) is Cassio's own code in kazoo-tree and is not in this lane.

## Why

Tom, 14 Oct 2026, about 22:50, on first seeing `kazoo tree`: batches, "several changes sent at once with a note (like a commit message)", from the seats or from him; zoomed out a batch is one knot, zoomed in it opens into its changes with the note written beside the limb. And 15 Oct, about 00:25: "I'd love to see the batched changes coming through, and the commit messages and all that kind of stuff." Also Tom, 13 Oct (memory `mcp-improvements`): building a patch took about 150 single calls; a seat should send several changes in one call, applied in the order given.

The wall already applies several changes as one: `ChainAdd` sets `chain_batch` (`daemon/wall.rs:549`), runs each request through `answer` (`chains.rs:174`), restores the patch on any failure (`chains.rs:151`) and logs one `What::Chain` (`chains.rs:154`), which is one seq, one log line, one commit and one undo (`patch/chains.rs:33`). A batch is the same mechanism with the requests given by the sender and a note in place of the chain's name.

## Outcome

1. **`Request::Batch { note: String, changes: Vec<Request> }`.**
   - `note`: 1 to 200 characters after trimming, one line with no control characters (a newline, a tab, an escape or any other control character is refused, never rewritten: the note is the sender's words, and a control character in a commit would drive a terminal or split the tree's log records). Refused with `bad_request` and the reason when empty, too long, or holding a control character.
   - `changes`: 1 or more of `Add`, `Remove`, `Turn`, `Patch`, `Unpatch`, `Tempo`. Any other op (an undo, a go, a load, a chain, a group, an arrange, a nested batch) is refused before anything is applied, naming its index and op. No upper limit on the count (Tom: nothing is capped if it can be avoided); the protocol's frame size is the only bound, and the room for the whole batch is reserved up front as `chain_add` does (`room_to_grow` with the batch's added modules and cables).
   - **A module added in the batch is named by later steps.** An `Add` in a batch may carry `as: "<label>"` (a name with no spaces or dots, unique in the batch). A later step's module or port that starts with `@<label>` (`@osc`, `@osc.out`) is given the id that add made. An unknown label, or one used before its add, is refused before anything is applied. Decision: ids are the wall's to choose (kind plus number), so the sender cannot know them in advance; a label is the smallest way to say "the one I just added".
2. **All or none, in order.** The steps are applied in the order given through `answer`, with `chain_batch` collecting their `What`s. If any step fails, the wall is put back exactly as before the batch (patch, tempo, and the engine: nothing the batch turned or added is heard, not even for one block), and the answer is the error with `step: <index>` and the step's own reason. Decision: a batch is one thought; half of one is a wall nobody meant.
3. **One change in the log: `What::Batch { note, changes: Vec<What> }`**, one seq, one `log.jsonl` line, one `Event::Change`. Its summary (`change.rs`): `{seat}: "{note}" ({n} changes: {a}; {b}; …)` with each step described as `describe` does, the list cut at 240 characters with `…` (the full list stays in the change). The flood guard counts a batch as one change. A glide in a `Turn` step keeps its `glide_beats`.
4. **Undo of a batch undoes the whole batch**, its steps' inverses in reverse order, all or none, as `undo_chain` does (`patch/chains.rs:33`); a batch whose steps include a change that can never be undone (`never_undone`) cannot be undone, with the same message as today.
5. **The tree keeps the batch as one moment with its note.** `Commit::change` (`tree/git.rs:296`) writes a batch as:
   ```
   {seat}: {note}

   Kazoo-Kind: batch
   Kazoo-Seq: {seq}
   Kazoo-Beat / Kazoo-Tempo / Kazoo-Clock (as today)
   Kazoo-Note: {note}
   Kazoo-Step: {describe(step 1)}
   Kazoo-Step: {describe(step 2)}
   …
   Kazoo-Change: {json}
   ```
   One `Kazoo-Step` trailer per step, in order, every step (no cut). Every other change's commit is unchanged. Decision: trailers, because `git log --format=%(trailers:key=…)` reads them without the whole message (kazoo-tree reads its moments this way, `kazoo-tree/src/history.rs:147`), and repeated keys keep their order.
6. **Seats: `wall_batch`** in kazoo-mcp, arguments `note` and `changes` (an array of objects, each `{"op": "turn" | "add" | "remove" | "patch" | "unpatch" | "tempo", …}` with the same fields as the single tools, plus `as` on an add). Its description says when to use it: whenever a seat makes more than one change toward one idea, so the tree shows the idea, with its note. Its answer lists each step's result (an add's new id beside its label). Other seats' notifications (`channel.rs`) carry the batch's one summary line, as for any change.
7. **The console's side is KZ-T10b** (`:batch <note>`, held changes, Enter sends), split off by the ten-file rule. Not in this lane.

## Tests (write them first; each fails before the change)

- protocol: a batch round-trips through JSON; `as` and `@label` parse; the refused ops, an empty note, a 201-character note, a note with a newline and a note with `\u{1e}` are each refused before anything is applied.
- wall: a batch of add (as `osc`), patch `@osc.out` to an existing input, turn `@osc.freq` with a glide, and tempo applies in order and logs one change with one seq; its summary has the note and four steps.
- wall: a batch whose third step names an unknown module leaves the patch, the tempo and the engine's commands as before (no command reaches the engine), and the answer says `step: 2` and why.
- wall: undoing a batch puts the wall back as before it in one change; a batch holding a step that can never be undone cannot be undone.
- wall: a batch counts once against the flood guard (31 single changes trip it; 31 batches of 10 steps each trip it at the same point).
- tree: a batch's commit has the subject `{seat}: {note}`, `Kazoo-Kind: batch`, a `Kazoo-Note` and one `Kazoo-Step` per step in order; a single change's commit is byte for byte as before.
- kazoo-mcp: `wall_batch` sends one request and lists each step's result with the add's id beside its label; another seat gets one notification line for it.
