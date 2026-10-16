# KZ-T10c: a batch's changes are each a moment on the tree, and any change can carry its sender's comment

Written by Cassio, 16 Oct 2026, about 09:45, and brought up to date 17 Oct 2026, about 03:30, after the first Opus review. Decided by Cassio; the reasons are given with each decision. Cassio wrote this lane directly, with no builder (memory `do-it-directly`, `tree-code-is-cassios`).

Place: Track T, after KZ-T10a and T10b. Crates:
- kazoo-wall: `protocol`, `daemon/control.rs`, `daemon/server.rs`, `daemon/wall.rs`, `daemon/wall/batch.rs`, `daemon/wall/moments.rs`, `tree/git.rs`, `tree/thread.rs`, `tree/thread/here.rs`, `tree/rings.rs`, `tree/rings/parts.rs`
- kazoo-mcp: `link.rs`, `tools.rs`, `tools/batch.rs`, `channel.rs`, `render.rs`
- kazoo-tree: `history.rs`, `app.rs`
- tests for all of the above

## Why

On 16 Oct 2026, about 09:40, Tom watched Cally send batches of 55 changes and said: "it only added one... the tree is basically not growing at all."

Then: "We're supposed to group them, so they were one change together, but each one forms individual moments from the individual changes inside."

And: "I thought we were supposed to have a comment with batch changes... comments and changes as well."

## Outcome

### 1. A batch is still one change in the log

It keeps one seq, one log line, one undo, one notice to the seats and one block of sound.

**Decision:** an idea sent at once stays one thing to undo and one thing to hear.

### 2. The tree grows a moment for each step

**Parts.** Each step before the last becomes a *part*. A part's commit has these trailers:
- `Kazoo-Kind: part`
- `Kazoo-Batch: <seq>`
- `Kazoo-Note`
- `Kazoo-Part: k/n`
- `Kazoo-Step`

A part carries no `Kazoo-Seq`, so a tree that holds a batch's parts but not its own commit is still missing that change, and it is healed like any other. The last step is the batch's own commit (`Kazoo-Part: n/n`).

**How the parts are made.** The tree's thread makes them from the wall just before the batch (`TreeItem::Batch`), using the model that `kazoo wall tree import` replays the log with. So a live tree and an imported tree hold the same walls, and the wall's own thread does none of that work.

**Decision:** a batch of hundreds of steps costs the wall's control loop nothing.

**When parts can't be made.** The batch is one moment, and the daemon's log says why.

**Reading where the tree stands.** The tree at a part holds through the change before its batch: reading it walks back past parts (`git::through_at`).

**kazoo tree.** It ties one knot on the batch's own moment.

### 3. Any change can carry a comment

**Which requests take one.** A request line may carry `"comment"` if it makes a change of its own (`Request::takes_comment`):
- turn, patch, unpatch, add, remove
- chain add, undo, tempo, loop, record, load, go, new

**The rules.** A comment is 1 to 200 characters after trimming, on one line. It may not contain control characters, line or paragraph separators, or direction marks, embeddings, overrides or isolates (`batch::breaks_line`).

**What is refused** (`bad_request`):
- a comment on any other request, a hello included;
- a comment on a batch, which says why with its note;
- a comment written inside a batch step.

**Where it is kept.** The comment is kept trimmed on the change (`Change.comment`), in the log, and as a `Kazoo-Comment` trailer on the change's moment.

**Where it is said.** After the summary, in:
- the console's log
- the seats' news
- `wall_log`

**Comments that arrive later.** A go and an undo of a go wait for the tree, and keep the comment until their change is made. A recording stopped while the log owes keeps its comment in the owed queue. A request that ends up changing nothing (starting a recording while one runs) keeps no comment, and its answer says that nothing changed.

**Decision:** a comment is the sender's words about one change. It goes where that change goes and nowhere else, and it is never dropped without a word.

### 4. Each step of a batch can carry its own comment

**The shape.** `"comments"` holds one entry per step, with `null` for a step without one. A list of all `null` counts as no comments.

**Where they go.**
- Each part carries its step's comment as `Kazoo-Comment`.
- The batch's own commit carries every step's comment as `Kazoo-Step-Comment: k: …`, so none is lost when the parts can't be made, and the last step's comment never reads as the reason for the whole batch.

**Undo.** An undo of a batch carries no step comments.

### 5. A wall older than comments

`HelloResult.comments` says whether the wall keeps comments. kazoo-mcp refuses to send a comment to a wall that would drop it, and says how to restart that wall.

### 6. Seats and kazoo tree

**kazoo-mcp.** Every tool that makes a change takes `comment`, and each `wall_batch` step takes `comment`. A chain save or list refuses one.

**kazoo tree.** It reads `Kazoo-Comment` as the moment's "why", and `Kazoo-Step-Comment` beside each listed step.

## Checks

- nextest `-p kazoo-wall -p kazoo-mcp -p kazoo-tree`
- clippy pedantic with `-D warnings`
- fmt
- ast-grep
- Opus review until READY, then land and install. Tom restarts the wall himself.
