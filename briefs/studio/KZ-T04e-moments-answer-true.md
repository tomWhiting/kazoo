# KZ-T04e: the wall's moments answer true: a mark, a go and a tree listing never take another request's answer, a mark that is written is always said, and nothing is marked against a tree that is behind

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall -p kazoo-mcp`.

Place: Track T. Written by Cassio, 14 Oct 2026, 23:1x, from the Opus review of KZ-T04b (merged at 8f397db). Decided by Cassio; every choice below has its reason beside it.

Time: about 45 minutes. Crates: kazoo-wall (`daemon/wall/moments.rs`, `daemon/wall/loops.rs`, `daemon/wall.rs`, `daemon/control.rs`, `protocol/mod.rs`, `tui/app/saves.rs`, the tests named below).

## Why

Tom's rule for the wall's history: nothing is lost, and the past is never wrong. A mark can never be moved or deleted, so a mark on the wrong commit, or a mark written while its seat was told nothing changed, is a permanent wrong. The review found these, most severe first.

1. **Tickets collide.** Tree tickets count up from `1 << 63` (`TREE_TICKETS`, `moments.rs:16-17`); loop-save tickets are `(1 << 63) | next_save` (`loops.rs:332-333`). Both reach `Control::awaiting` (`control.rs:645`) as one map. A mark waiting as `2^63 + 1` and a loop save made then as `2^63 + 1` overwrite each other: the console gets a `MarkResult` for its save, the seat times out, and the save's answer is dropped.
2. **A late mark is written but said not to be.** `tree_answers` expires every request older than 5 s, even one whose answer arrived in the same batch (`moments.rs:414-456`). A `Mark` job still writes its tag, so the seat hears "nothing was changed", `Event::Marked` is never sent, its allowance is not refunded, and a retry gets `mark_exists`.
3. **Marks can land on an old commit while the tree catches up.** With the tree's queue full, changes are dropped from it and `resync` is set (`tree/thread.rs:312-357`); a whole "gap" item follows only at a later `tick_tree`. A `Mark`, `SaveMark`, `Resolve` or `Here` sent in between reaches the thread first and sees the line before the missed changes. Answers' `through` is ignored (`moments.rs:416`).
4. **A go cannot be undone while the tree is stopped,** though its backup save is on disk: `undo_go` asks `tree_ready()?` before `read_backup` (`moments.rs:269-296`).
5. **A save's tree note says "nothing was changed"** about a save that was written, shown as a fault (`moments.rs:470-477`, `tui/app/saves.rs:344`).
6. **A go to a branch whose tip is the wall's own commit is refused** (`moments.rs:365`), though it would change the line the next change grows.
7. **`undo_go` takes `Instant::now()`** where every other request takes the caller's `now` (`moments.rs:285`).
8. **Comments and docs are stale or missing** (below), and five tests do not prove what T04b's brief asked.

## The design (decided; build exactly this)

### 1. One ticket space, split by kind (`daemon/wall/moments.rs`, `daemon/wall/loops.rs`)

- Speech keeps its tickets below `1 << 63` as now. Loop saves take `(1 << 63) | n` with `n < 1 << 62`; tree requests take `(1 << 63) | (1 << 62) | n` with `n < 1 << 62`. Two constants beside each other in `moments.rs`, `LOOP_SAVE_TICKETS = 1 << 63` and `TREE_TICKETS = (1 << 63) | (1 << 62)`, each with a doc comment naming the three ranges. `loops.rs` uses `LOOP_SAVE_TICKETS`.
- Each counter wraps within its own range (`n = (n + 1) & ((1 << 62) - 1)`), never into another's. Reason: a wall runs for months, and 2^62 requests will not wrap in its life, but the arithmetic must still be closed.
- Test `a_loop_save_and_a_mark_waiting_together_each_get_their_own_answer` in `kazoo-wall/tests/daemon.rs`, over a real socket: a loop module recording, a seat's `mark` held by a held tree thread (as T04b's test 11 holds it), and the console's save: each asker gets its own answer type.

### 2. An answer that has arrived is acted on, whatever its age (`moments.rs`, `tree_answers`)

- First take every answer that has arrived, and act on each (send the result to its asker, broadcast `Event::Marked` for a mark, refund as T04b says). Only then expire the requests still unanswered and older than `TREE_ANSWER_WITHIN`.
- An unanswered `mark` expires with `slow_down`: "the tree has not answered in 5 s; the mark may still be written: wall_tree shows it once it is". An unanswered `go`, `tree` or undo of a go keeps T04b's sentence (they only read).
- Expired mark tickets are kept in `expired_marks: VecDeque<(u64, String, String)>` (ticket, seat, name), at most 64, oldest dropped first with a line in the daemon log saying which. When a `Marked` answer arrives for one, `Event::Marked` is broadcast as for any mark, and the daemon log says the mark was written after its seat was answered. Reason: the mark exists; everyone watching must hear of it.
- T04b's test 11 is strengthened: after release, wait on the tree's news (2 s limit, 5 ms polls), then: the tag `refs/tags/mark/late` exists, `Event::Marked` was broadcast, and `tree_answers` holds nothing for the expired ticket.

### 3. Nothing is marked or gone to while the tree is behind (`moments.rs`)

- `mark`, `go` and undo of a go answer `slow_down` with "the tree is catching up with the wall's last changes; ask again in a moment" while `tree.resync || tree.behind_from.is_some()`. `tree` (the listing) is still answered: it only reads, and its answer carries `through`.
- A save made while the tree is behind is not marked; its note says "not marked in the tree: the tree is catching up" (the note tone, point 5).
- Every request records the wall's `made` when sent (T04b already keeps it for go). When an answer's `through` is below that `made`, a `mark` or `go` is sent again once (T04b's retry, counted in `GO_TRIES`) and after three tries answers `slow_down`.
- Test `a_mark_waits_while_the_tree_catches_up`: fill the tree's queue (T04b's tests show how), `mark` answers `slow_down`; after the gap item is written, `mark` succeeds and the tag is on the wall's latest commit.

### 4. A go is undone from its backup, with or without the tree (`moments.rs`, `daemon/wall.rs`)

- `undo_go` reads the backup first (`read_backup`); a gone or changed backup is refused at once with its sentence.
- With the tree ready, as now: `Here`, then put up the backup.
- With no tree, or the tree stopped: put up the backup with `from = back_to` and no tree round trip; the change is logged as T04b logs an undo. When the tree comes back, its resync moves `HEAD` to the wall's line as for any change (KZ-T02b). Reason: Tom must always be able to come back by undo; the tree follows the wall, never the other way round.
- Test `a_go_is_undone_with_the_tree_stopped`.

### 5. A save's tree note says what happened (`moments.rs`, `tui/app/saves.rs`)

- The note is built from the reason alone: "not marked in the tree: this wall keeps no tree yet", "… the tree is stopped (<its error>)", "… the tree's queue is full", "… the tree is catching up". Never the refusal sentence with "nothing was changed".
- The console shows a save's tree note in the note tone, not the fault tone.

### 6. A go to a branch at the wall's own commit is allowed (`moments.rs`)

Refused `not_allowed` only when `to.commit == here.commit` and (`to.branch` is `None` or equals `here.branch`). Otherwise it goes: the wall is put up from the same commit and the line is that branch, so the next change grows it. Test `a_go_to_a_branch_at_the_walls_own_commit_moves_the_line`.

### 7. One clock (`moments.rs`, `daemon/wall.rs`)

`Wall::undo` takes the caller's `now` and passes it to `undo_go`; `Instant::now()` is gone from `moments.rs`.

### 8. Comments, docs and tests

- `wall.rs`: the "A load keeps the wall it replaced…" comment sits above the load branch; the go branch gets its own. `puts_up_wall`, `prune` and `walls_of` say "a load or a go".
- `protocol/mod.rs`: `Request::Go`'s doc lists the moment forms (a branch, a mark, a save's tag, `#<change>`, a commit id); every field of `Request::Mark`, `Request::Go` and `Event::Marked`, and the `pub(super)` functions in `moments.rs` (`mark`, `go`, `tree`, `undo_go`, `read_backup`), get doc comments.
- `moments/tests.rs`: every wait has a 2 s limit with 5 ms polls (no bare `recv()`), failing with a sentence. Test 2 checks `here.branch == Some("trunk")` and trunk's tip moved. Test 14 checks the wall after restart is the moment's wall (its modules and knobs), not only the undo. Test 6 checks a seat's allowance comes back. Test 10's first save is a seat's.

## Red tests first

Each test above fails on main first; paste each under RED, then GREEN.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-mcp/src
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The new and strengthened tests fail first (RED), then pass.
- [ ] Tickets are in three ranges, each counter closed in its own.
- [ ] Arrived answers are acted on before anything expires; a late mark is broadcast.
- [ ] `mark`, `go` and undo refuse `slow_down` while the tree is behind; stale answers are retried.
- [ ] A go is undone with no tree, from its backup.
- [ ] Save notes say what happened, in the note tone.
- [ ] Every check above clean.

## END
