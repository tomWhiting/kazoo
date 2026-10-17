# KZ-T13: a batch carries a description and attached notes, kept with it on the tree

Written by Cassio, 17 Oct 2026, about 18:35. Decided by Cassio; the reasons are given with each decision.

Place: Track T, after KZ-T10c. It runs beside KZ-T12, and they touch some of the same files (`protocol/mod.rs`, `tools/batch.rs`). Whichever lands second merges.

Crates:

- **kazoo-wall:**
  - `protocol/mod.rs`
  - `protocol/batch.rs`
  - `store.rs` (or a new module beside it)
  - `daemon/wall/batch.rs`
  - `daemon/server.rs` (for the new request)
  - `tree/git.rs`
  - `tree/thread.rs`
  - `change.rs`
- **kazoo-mcp:**
  - `tools/batch.rs`
  - `tools.rs`
  - `render.rs`
  - `channel.rs`
- **kazoo-tree:** not in this lane. Cassio adds its notes view on top (memory `tree-code-is-cassios`).
- Tests for all of the above.

## Why

Tom, 17 Oct 2026, about 18:05: "can you also please look at being able to add notes with it so add attachments with the batch change so you can describe your batch change all the things go together".

On what Cally is playing: "it's gonna come with a like a mathematical proof which I just think will be funny".

Today a batch carries a note of 200 characters on one line. That is room for a title, not a proof.

## Outcome

### 1. What a batch can carry

`Request::Batch` gains two optional fields. Both default to empty.

**`description`:** markdown text up to 8 KiB in UTF-8, on as many lines as it likes. It says what the batch is and why.

**`attachments`:** up to 8 of `{ "name": ..., "text": ... }`.

- **The name:**
  - 1 to 64 characters of `A-Z a-z 0-9 . _ -`.
  - It does not start with a `.`.
  - It is unique in the batch, ignoring case.
  - It is not `description.md`, which the description is kept as.
- **The text:** UTF-8, up to 64 KiB.

**Limits:**

- Together, the description and the attachments are at most 128 KiB.
- Text may hold line feeds and tabs, and no other control characters (`\r` is refused, with a reason that names it).
- An empty description or an empty attachment text is refused. So is either one on an undo.

**Refusals** are `bad_request` with the reason, before anything is applied, as the note's are now (`protocol/batch.rs::validate`).

**The line cap.** `MAX_LINE` rises from 64 KiB to 512 KiB, so a batch with its notes and its steps fits, as does the answer that reads them back (§3), whose JSON escapes can double the text. A batch may still have at most the number of steps it may today: `MOST_PARTS` in `tree/git.rs` stays 4096, a constant of its own and no longer derived from `MAX_LINE`.

**Decision:** a proof, a score or a long explanation fits, and a mistaken paste of a whole file does not.

### 2. Where the notes are kept

**The store.** The texts are kept in a notes store, never in the log line. The store is `<state dir>/notes/<sha256 hex>`, one file per text, named by its content, so the same text twice is kept once.

**The order of writes.** It writes them all before the change is logged:

1. each text to a temporary file in that directory;
2. one `sync_all` per file;
3. a rename;
4. one sync of the directory at the end (`store.rs` has the helpers).

**The log line.** The `Change` carries `notes: Vec<NoteRef>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`). Each `NoteRef` holds `{ name, bytes, sha256 }`, and the description comes first, as `description.md`.

**Decision:**

- The log, the seats' news and `wall_log` carry the names and sizes only. Every startup that reads the log and every notice to every seat stays as small as today.
- The texts are durable before the change that names them exists.
- Content addressing means nothing is ever rewritten.

**If the store cannot be written,** the batch is refused (`internal`, with the reason), and nothing is applied.

**A log line naming a note the store lacks** (a store deleted by hand) still replays. The notes answer (§3) says which note is missing.

### 3. Reading them back

**The request.** A new request, `Notes { seq }`, answers with `{ seq, note, description, attachments: [{name, text}] }`, read from the store. It is refused (`not_found`) when change `seq` has no notes, and the reason says whether that change exists.

**The MCP tool.** kazoo-mcp gains `wall_notes`, with `seq`. It prints the description and then each attachment under its name.

### 4. The tree

**The batch's own commit:**

- The commit message carries the description after the subject line, as the body. Then the trailers follow, as now.
- The description is cut to its first 2000 characters, ending in `…` when cut.
- The message gets one `Kazoo-Attachment: <name> (<bytes>)` trailer per note, the description included.
- Every note goes into the commit's tree as `notes/<seq>/<name>`, through the `files` argument `Commit::change` already takes.

**What stays the same:**

- The parts carry none of the notes.
- The `Kazoo-Change` JSON carries the `NoteRef`s, not the texts.
- Later commits keep `notes/` as they keep any other file. Nothing deletes it.

**Decision:** a tree copied anywhere carries its notes. `git show <moment>:notes/<seq>/proof.md` reads one without the wall. kazoo tree reads them with the git it already uses.

**Healing and `kazoo wall tree import`** read the texts from the store, exactly as the live tree does. If one is missing from the store, the commit has no file for it, and the daemon's log says which.

### 5. Words

**Summaries.** A batch with notes adds, after its count of changes:

> , with notes description.md and proof.md (12 KB)

This shows in the console's log, in the seats' news (`channel.rs`) and in `wall_log`.

**Undo.** An undo of a batch carries no notes.

### 6. kazoo-mcp

`wall_batch` takes `description` (a string) and `attachments` (a list of `{name, text}`).

Each schema description says what it is for. For `attachments`, say:

> a longer text that goes with the change, such as a proof, a score or the reasoning; read back with wall_notes

`HelloResult` gains `notes: bool`, as `comments` was added. A seat refuses to send notes to a wall without it, and says to restart the wall.

### 7. Tests (red first)

**Validation:** each limit, at the limit and one past it, every refusal named above included.

**The store:**

- written before the log line;
- the same text kept once;
- a failed write refuses the batch, and the patch is unchanged (use a state dir whose notes directory is a file).

**Reading back:**

- `Notes` returns the texts;
- `not_found` for a change without notes;
- a missing store file is named.

**The tree commit:**

- the body;
- the trailers;
- the files at `notes/<seq>/`;
- the parts without notes;
- import gives the same commit as live.

**Words:**

- the summary words;
- an undo carries no notes;
- MCP schema round trip, and a hello-gated refusal.

## Checks

- nextest, clippy, fmt and ast-grep on `-p kazoo-wall -p kazoo-mcp`
- `cargo test --doc -p kazoo-wall` if doc comments change
- Opus review until READY, then land, publish and install. Tom restarts the wall himself.
