# KZ-T02b: every change the wall makes becomes one git commit in `~/.kazoo/tree`, written by a tree thread the wall never waits for, and what the tree misses is said and closed by one commit that names the gap

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Read by Cassio, 13 Oct 21:1x: git 2.47.1 on this Mac, `chrono` and `tempfile` already kazoo-wall dependencies (so no new crate). Two notes. (1) Test 5: fast-import writes packs into `objects/pack`, so make that directory read-only too, or the failure may not happen; say which you did. (2) KZ-T04 will extend the allowed git command list (making a branch or mark ref for a bud is not moving one backwards); keep the list test as one constant T04 can extend. Opens after T01 and T02a land.

Time: about 30 minutes of a builder's work. One mechanism: a bounded queue from the control thread to a tree thread that drives one `git fast-import`. Crates: kazoo-wall, and kazoo-mcp for one test literal. Depends: KZ-T01 (`tree::to_files`, births) and KZ-T02a (`Change::clock`), both landed. This is the second half of PLAN.md's "T02 a commit for every change"; KZ-T02a is the first.

Files (ten):
1. `kazoo-wall/src/tree.rs`: `pub mod git; pub mod thread;`, plus `Stamp` and `beats_between`.
2. `kazoo-wall/src/tree/git.rs` (new): `plant`, `Planted`, `Importer`.
3. `kazoo-wall/src/tree/thread.rs` (new): `TreeHandle`, `TreeItem`, `News`, the worker.
4. `kazoo-wall/src/tree/thread/tests.rs` (new).
5. `kazoo-wall/src/daemon/wall.rs`: the mirror, the feed, news in `tick`, health in `snapshot`, finish in `close`.
6. `kazoo-wall/src/daemon/mod.rs`: finds the tree and attaches it at start.
7. `kazoo-wall/src/protocol/mod.rs`: `TreeHealth`; `Snapshot::tree`.
8. `kazoo-wall/src/tui/draw/panes.rs`: the tree badge.
9. `kazoo-wall/src/tui/tests/mod.rs`: its `Snapshot` literal (`:88`) gains `tree: None`.
10. `kazoo-mcp/src/render.rs`: its `Snapshot` literal (`:1135`, a test) gains `tree: None`.

## Outcome

With a tree planted at `~/.kazoo/tree`, every change the wall makes becomes one commit on the wall's branch (`trunk`). `git log` and `jj log` (through `jj git clone`) show it like this:

```
commit 6bb5311e…
Author:     Tom <tom@kazoo.invalid>
AuthorDate: 2026-10-13T21:04:11Z
Commit:     kazoo-wall <kazoo-wall@kazoo.invalid>
CommitDate: 2026-10-13T21:04:11Z

    Tom turned vcf3.cutoff from 0.3 to 0.35 over 2 beats

    Kazoo-Kind: change
    Kazoo-Seq: 6400
    Kazoo-Beat: 18734.25
    Kazoo-Tempo: 120
    Kazoo-Clock: 9c41d07a3be2f610.3
    Kazoo-Change: {"seq":6400,"at":"2026-10-13T21:04:11Z","seat":"Tom",…}
```

The commit's tree holds the wall after that change as T01's files: `wall.toml` and `modules/<id>.toml`.
- A turn with a glide is one commit: the change records the target and `glide_beats` (`What::Turn`, `protocol/mod.rs:676`), not the steps.
- Every daemon start is a commit too (`Kazoo-Kind: start`). It is the wall as it starts, on a new clock line.
- If the queue is ever full, or git fails, the wall plays on regardless. The console shows ` TREE BEHIND from 812 ` or ` TREE STOPPED: … `, and the daemon log says which changes were not committed one by one. As soon as it can, the tree takes one `Kazoo-Kind: gap` commit holding the wall as it then is, naming the changes it covers.
- No ref ever moves backwards.

## The design (decided; build exactly this)

### Git: the `git` binary's `fast-import`, not the `gix` crate

Measured on Tom's M1 Max on 13 Oct (git 2.47.1, a bare repo, 85 module files of about 400 bytes and a 3 KB `wall.toml`, each commit changing one module file and `wall.toml`):

| run | time | per commit |
|---|---|---|
| 1,000 commits, one checkpoint at the end | 0.35 s | 0.35 ms |
| checkpoint every 5 (`core.fsync=committed`, `fastimport.unpackLimit=0`) | 3.41 s for 200 checkpoints | about 17 ms per durable checkpoint |
| checkpoint every commit, default unpack limit (loose objects) | 128 s | 128 ms (avoid) |
| `git gc --auto` over the 200 small packs | 0.68 s | down to 1 pack |

Reasons for choosing `fast-import`:
1. **It is git itself.** Every commit is canonical by construction, readable by `git log`, `jj` and any clone.
2. **It refuses to move a branch backwards.** Without `--force`, `fast-import` will not update a branch whose new tip does not contain the old one. That was checked: it printed `warning: Not updating refs/heads/trunk (new tip … does not contain …)`, exited 1, and left the ref unchanged.
3. **Durability.** With `-c core.fsync=committed` it flushes packs and refs at each checkpoint. `gix` does no fsync at all: `gix-odb` 0.80 and `gix-ref` 0.63 have no `fsync` or `sync_all` (checked in the registry sources). So a power cut could leave a ref naming an object that never reached the disk.
4. **Cost.** No new crate. `gix` 0.89.0 (current on crates.io, checked with `cargo search` on 13 Oct) brings about fifty `gix-*` crates even without default features. Tom's builds are rationed, his disk filled twice in October, and a cold build would grow by a minute or more. `gix` 0.89 also needs Rust 1.88, while kazoo-wall declares 1.85.
5. **Speed.** 0.35 ms a commit, and about 17 ms for a durable checkpoint.

The costs:
- **A runtime need for `git` 2.36 or later**, for `core.fsync`. `--initial-branch` needs 2.28 and `repack --geometric` needs 2.33. Check it at attach with `git version`. A missing or older git is a tree error shown on the console; the wall plays on.
- **A child process to look after.** That is the tree thread's whole job.

`unsafe_code = "forbid"` is untouched: we spawn a process through `std::process::Command`.

### Where the tree is (`daemon/mod.rs`)

- The tree directory is `$KAZOO_TREE_DIR` when it is set and not empty, else `tree` beside the state directory (`config.state_dir.parent()?.join("tree")`). So `~/.kazoo/wall` gives `~/.kazoo/tree`, and a test daemon's temporary state finds no tree.
- **The daemon never plants a tree.** It attaches one only when that directory is a bare git repository (`git -C <dir> rev-parse --is-bare-repository` prints `true`). Otherwise `Snapshot::tree` says `planted: false` and nothing is written.
- **Why.** PLAN.md imports the log since 26 Sep "as the trunk" (KZ-T03), and nothing in the tree is ever rewritten. A tree planted by the daemon before T03 runs would start `trunk` at today's change, and the history could never be put beneath it. So T03 plants the tree with its import. Tests, and T03, plant with `tree::git::plant`.
- There is no `DaemonConfig` field. It would break five literals in three crates (`kazoo-wall/tests/daemon.rs:29`, `tui/tests/live.rs:35`, `kazoo-cli/src/cli.rs:310`, `kazoo-mcp/tests/seat.rs:97`, `kazoo-mcp/src/mcp.rs:19`).

### `tree/git.rs`

- `pub const MIN_GIT: (u32, u32) = (2, 36);`
- `pub fn plant(dir: &Path) -> Result<(), TreeError>`:
  - Refuse a directory that exists and is not empty.
  - Create it with mode 0700 (`DirBuilderExt::mode`, as `store.rs:23`).
  - Run `git init --bare --quiet --initial-branch=trunk <dir>`.
  - Then run `git -C <dir> config gc.auto 0`. Nothing but this thread packs the tree (below), so `git` run by hand never repacks behind its back.
- `pub struct Planted { pub branch: String, pub tip: Option<String>, pub through: Option<u64> }`, read by `pub fn read(dir: &Path) -> Result<Planted, TreeError>`:
  - `branch` from `git -C <dir> symbolic-ref -q HEAD` (`refs/heads/trunk` once planted). This is the wall's current branch. T04 will move `HEAD`, and this lane only reads it.
  - `tip` from `git -C <dir> rev-parse -q --verify <branch>^{commit}`; empty means a branch with no commits.
  - `through` from `git -C <dir> log -1 --format=%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C) <branch>`, parsed as a `u64`. When the tip has no `Kazoo-Seq` but has `Kazoo-After` (KZ-T03's start, gap and settle commits carry that instead), read that the same way. A tip with neither (made by hand) is `None`.
- `pub struct Importer`. It owns the `Child` of `git -C <dir> -c core.fsync=committed -c core.fsyncMethod=fsync -c fastimport.unpackLimit=0 fast-import --quiet --done`, with `stdin` and `stdout` piped. `stderr` is inherited, so git's own words land in the daemon's log. The stream opens with `feature get-mark\n`. Methods:
  - `commit(&mut self, c: &Commit) -> Result<u64, TreeError>` writes one commit command and returns its mark. Marks count from 1 within one importer; they are not seqs, because a start commit shares its seq with the commit before it.
  - `checkpoint(&mut self, mark: u64) -> Result<String, TreeError>` writes `checkpoint\n\nprogress kazoo <mark>\n\nget-mark :<mark>\n`. It reads stdout lines until it has the line `progress kazoo <mark>` and then a 40-hex sha, and returns the sha. **Then it runs `git -C <dir> rev-parse --verify <branch>` and requires that same sha.** A refused update (rule 2 above) or anyone else moving the ref shows here as `TreeError::Git("the branch is at … but the tree wrote …")`.
  - `finish(self) -> Result<(), TreeError>` writes `done\n`, closes stdin, waits, and requires exit status 0.
  - A write that fails (`EPIPE`: Rust ignores `SIGPIPE`, so the write returns an error), or a read that hits end of file, is `TreeError::Git` with what git's exit status says.
- **The commit command** (raw dates, `--date-format=raw`, the default):

  ```
  commit <branch>
  mark :<k>
  author <seat> <<email>> <unix> +0000
  committer kazoo-wall <kazoo-wall@kazoo.invalid> <unix> +0000
  data <len>
  <message bytes>
  [from <tip>]            only on this importer's first commit, when the branch has a tip
  [deleteall]             for a whole item
  M 100644 inline <path>
  data <len>
  <bytes>
  D <path>
  <blank line>
  ```

  Data goes as counted bytes (`data <len>`), so no content is ever escaped. Paths are `wall.toml` and `modules/<valid id>.toml`: ASCII, no spaces, no quotes (T01's `valid_id`).
- **Author.** The name is the seat, as it is: seat names are 1 to 24 of `A-Z a-z 0-9 space _ . -` (`protocol/mod.rs:74`), and git refuses only `<`, `>` and line breaks.
  - The email is `<slug>@kazoo.invalid`, where `slug` is the seat lower-cased with each space made `-`. So `Tom` is `tom@kazoo.invalid` and `Tom W.` is `tom-w.@kazoo.invalid`; git accepts both (checked).
  - `.invalid` is reserved by RFC 6761, so no mail can ever go to it, and it says plainly that this is not an address.
  - The wall's own changes (seat `kazoo-wall`, `daemon/wall.rs:49`), and start and gap commits, are authored `kazoo-wall <kazoo-wall@kazoo.invalid>`.
- **Dates.** Author and committer dates are both the change's `at` (`protocol/mod.rs:654`, `YYYY-MM-DDTHH:MM:SSZ`), parsed with `chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%SZ")` to Unix seconds, `+0000`. Start and gap commits are dated when they are written. The same change on the same parent therefore makes the same commit id every time, which T03's import can rely on.
- **The message.**
  - The first line is the change's summary, with any line break made a space.
  - Then a blank line and the trailers, one per line, in this order:
    - `Kazoo-Kind: change|start|gap`
    - `Kazoo-Seq: <seq>`
    - `Kazoo-Beat: <beat>` and `Kazoo-Tempo: <tempo>`, each as Rust's shortest `{}` for `f64`
    - `Kazoo-Clock: <line>`
    - `Kazoo-Gap: <first>-<last>` (gap and start commits that cover missed changes)
    - `Kazoo-Change: <the Change as compact JSON>`, change commits only: `serde_json::to_string(&change)`, the log line itself, one line because compact JSON escapes every line break.
  - Beat, Tempo and Clock are left out when the change has no `clock` (KZ-T02a: changes made before the engine).
  - The JSON never holds a speaker's words (`What::Speak` holds counts, `protocol/mod.rs:731`), so the tree is as private as the log.

### What `Kazoo-Beat` and `Kazoo-Clock` mean, and reading them (`tree.rs`)

They are `Change::clock` from KZ-T02a, copied: `beat` is where the change could first sound, `tempo` is what was playing, and `line` is `<start>.<n>`, which changes on every restart, every load and every switch of the desk's clock. KZ-T02a states it in full.
- A start commit stamps the clock at the moment of attach, on the run's line `<start>.0`. So the first commit of every daemon run is on a fresh line.
- `pub struct Stamp { pub kind: String, pub seq: u64, pub beat: Option<f64>, pub tempo: Option<f64>, pub clock: Option<String>, pub gap: Option<(u64, u64)> }`.
- `pub fn Stamp::parse(message: &str) -> Result<Stamp, TreeError>` reads the trailers of a commit message.
- `pub fn beats_between(a: &Stamp, b: &Stamp) -> Option<f64>` is `Some(b.beat − a.beat)` only when both have a beat and the same `clock`. Otherwise it is `None`: two beats on different lines are never compared. T06's rule for crossing lines (KZ-T02a: the UTC difference at the earlier tempo, at most 16 beats) works from the dates, not from this.

### What each commit holds, and the mirror (`daemon/wall.rs`)

- Every commit's tree is T01's `to_files(&self.patch.to_file())` after the change has been made. It is computed on the control thread in `Wall::apply` (`daemon/wall.rs:2092`), the one place every made change passes: requests, undos, loads, and the wall's own changes (`pay_owed_all`, `:2051`).
- The wall keeps `tree_mirror: Files`, the files as last sent. Each item carries only what differs from it: `(path, Some(bytes))` for each new or changed file and `(path, None)` for each gone. Then the mirror becomes the new files.
- **Why the whole wall is turned into files and compared, rather than working out which files a change touched.** It is right by construction: rack moves and groups (`arrange`, `group`, `daemon/wall.rs:2798`, `:2814`) change `wall.toml` without being changes, and a hand-kept list of touched files would miss them. A turn changes one module file (knob and dye) and `wall.toml` (`last_seq`). A load changes nearly all.
- **Cost.** It is paid on the control thread, which is not real-time and already walks the whole wall for the dye after every change (`fingerprints.rs`, `Dye::flow`). Measure it in a test for the 85-module evening wall (KZ-T01's fixture) and report it under BENCH. Expected: under 0.5 ms in a release build.
- A change that is not made (the log refused it, `daemon/wall.rs:1954`) never reaches `apply`, so it is never committed. Its number is a gap in `Kazoo-Seq`, and its `Abandoned` note is a commit of its own.

### The queue and the thread (`tree/thread.rs`)

```rust
pub enum TreeItem {
    /// A change: what differs from the last item.
    Change { change: Box<Change>, files: Vec<(String, Option<Vec<u8>>)>, generation: u64 },
    /// The whole wall, after change `seq`.
    Whole { kind: WholeKind, seq: u64, clock: Option<ClockStamp>, files: Files, generation: u64 },
    /// Finish: checkpoint, `done`, and say so.
    Finish,
}
pub enum WholeKind { Start, Gap }
pub enum News {
    Committed { through: u64 },
    Gap { first: u64, last: u64 },
    Down { error: String, through: Option<u64> },
    NeedWhole,
    Finished(Result<(), String>),
}
```

**The constants, each with its reason in its doc comment:**
- `pub const TREE_QUEUE: usize = 1_024;` The log's busiest second held 30 changes, its busiest 10 s 57, and its busiest minute 165 (all 6,373 changes, 26 Sep to 13 Oct). The flood guard holds each seat to 30 at once and 3 a second after (`daemon/wall.rs:55`, `:58`), so even eight seats emptying their allowance together make 240. 1,024 is four times that, and over six minutes of the busiest minute ever logged.
- `pub const TREE_QUEUE_BYTES: usize = 64 * 1024 * 1024;` An item is about 4 KB (a module file, `wall.toml`, the change). A whole item is the wall's files, bounded by `store::MAX_WALL_FILE` (8 MiB, `store.rs:67`). So 64 MiB holds eight of the largest walls or about sixteen thousand ordinary changes. Memory stays bounded whatever happens.
- `pub const CHECKPOINT_AFTER: Duration = Duration::from_secs(1);` and `pub const CHECKPOINT_COMMITS: u64 = 256;` A checkpoint costs about 17 ms (measured), so at most one a second costs under 2% of the thread. A crash loses at most a second, or 256 commits, of commits. They are never lost from the log, and the next start's commit covers them.
- `pub const REPACK_EVERY: u64 = 64;` After every 64 checkpoints, run `git -C <dir> -c core.fsync=committed repack -d -q --geometric=2` between items. Each checkpoint writes one pack; geometric repacking keeps their number logarithmic without rewriting everything. It never touches a ref, and the queue absorbs the second or so it takes.
- `pub const RETRY_AFTER: Duration = Duration::from_secs(5);`

**`TreeHandle`** is owned by the `Wall`, and only the control thread touches it. It holds a `SyncSender<TreeItem>` from `std::sync::mpsc::sync_channel(TREE_QUEUE)`, a `Receiver<News>`, an `Arc<AtomicUsize>` of queued bytes, the `JoinHandle`, and the control side's state: `generation: u64`, `resync: bool`, `behind_from: Option<u64>`, `through: Option<u64>` and `error: Option<String>`.
- `pub fn attach(dir: &Path) -> Result<TreeHandle, TreeError>` checks git's version, reads `Planted`, and spawns the thread `kazoo-wall-tree` (as `daemon/record.rs:250` spawns its writer).
- `pub fn offer(&mut self, item: TreeItem) -> bool` **never blocks**. It adds the item's bytes to the counter; if that passes `TREE_QUEUE_BYTES`, or `try_send` returns `Full`, or the thread has gone (`Disconnected`), the item is dropped, the bytes taken back, `resync = true`, and `behind_from` set to the item's seq if it is not set already. On the first drop of an episode it says so once in the daemon log:

  > the tree's queue is full (1,024 changes or 64 MiB waiting): change 812 and those after it are not committed one by one; the tree will hold them in one commit as soon as it has room

  The thread takes the bytes back off the counter as it takes each item.

**The control thread's part (`daemon/wall.rs`).**
- In `apply`: if `resync` is set, send nothing; the coming whole item covers this change. Otherwise offer `Change { generation }` with the diff against the mirror.
- In `tick` (`:4383`, every 20 ms at most):
  - Drain `News`. `Committed` and `Gap` move `through`; `Gap` clears `behind_from` and says in the daemon log "the tree holds changes 812 to 840 as one commit". `Down` sets `error`, says it in the daemon log with the changes from `through + 1` on that are not committed, and sets `resync`. `NeedWhole` sets `resync`.
  - Then, if `resync` is set: `generation += 1`, the mirror becomes `to_files` of the wall, and offer `Whole { kind: Gap, seq: self.revision(), clock: <stamp now>, files, generation }`. If that offer succeeds, `resync = false`; if not, try again next tick.
- In `Wall::attach_tree(&mut self, handle: TreeHandle)`, called by `daemon/mod.rs` in the `build` closure (`daemon/mod.rs:199`): the mirror is the wall's files, and it offers `Whole { kind: Start, … }` at generation 1.
- In `close` (`:3865`): offer `Finish`, then wait on the news for `Finished` up to 5 s (`recv_timeout`), and say in the daemon log what it got or that it gave up. A daemon stopping is the only time the control thread waits on the tree, and then it waits at most 5 s.
- `snapshot()` (`:3912`) sets `tree: Some(self.tree_health())`, or `planted: false` when there is no handle.

**The thread's loop.**
- At start, `Importer` begins on `Planted.branch` with `from <tip>` when there is a tip. `committed_through = Planted.through`, and `accepted_generation = 0`.
- It takes items with `recv_timeout` up to `CHECKPOINT_AFTER`.
- A **Change** item is committed only when its generation equals `accepted_generation`. Any other is dropped: it is a diff against a mirror the tree may not hold, and a whole item is coming.
- A **Whole** item is written with `deleteall` and every file, and sets `accepted_generation`.
  - **Start**: always committed, the daemon's start on a fresh clock line. It names any gap: when `committed_through` is below `seq`, the message says "the wall started after change <seq>; the tree did not hold changes <t+1> to <seq> one by one", with `Kazoo-Gap`. When the tree is ahead of the wall (the wall started from a `patch.json` older than its log, `What::Resumed`), it says "the tree held changes up to <t>; the wall started from change <seq>", and the next commits put the tree where the wall is.
  - **Gap**: committed with `Kazoo-Gap: <committed_through + 1>-<seq>` when `seq` is above `committed_through`. Otherwise it is written as the base of the next change commit, through `deleteall`, with no commit of its own, so the tree never gains a commit that is not a change, a start or a gap.
- After each item, checkpoint when the queue is empty and `CHECKPOINT_AFTER` has passed since the last one, or when `CHECKPOINT_COMMITS` commits are unacknowledged. A checkpoint's sha check passing moves `committed_through` to the last seq written and sends `Committed`.
- On any `TreeError`, send `Down { error, through: committed_through }`, drop the importer (kill the child if it lives, and wait for it), and go down. While down, drain and drop every item, so the control thread's queue never fills because of a dead git. Every `RETRY_AFTER`, start a new importer from `read(dir)`; on success send `NeedWhole`.
- On `Finish`: checkpoint, `finish`, and send `Finished`.

**How the code guarantees no ref ever moves backwards.**
1. The importer is the only writer of any ref, and it never gets `--force`. Git itself refuses any update whose new tip does not contain the old (measured above).
2. Each importer's first commit names the branch's current tip as `from`, and every later one continues from the last.
3. Every checkpoint compares the branch with the sha git says it wrote. A mismatch stops the tree (`Down`) before another commit is written.
4. No code runs `update-ref`, `reset`, `branch -f`, `push`, `tag -f` or `gc --prune`. The only git commands the crate runs are `version`, `init` (plant), `config gc.auto 0` (plant), `rev-parse`, `symbolic-ref -q HEAD` (read), `log -1` (read), `fast-import` and `repack -d -q --geometric=2`. A test (below) holds the source to that list.

### Showing it (`protocol/mod.rs`, `tui/draw/panes.rs`)

- `pub struct TreeHealth { pub planted: bool, pub through: Option<u64>, pub waiting: u32, pub behind_from: Option<u64>, pub error: Option<String> }`, serde with defaults. `is_ok()` is `planted && error.is_none() && behind_from.is_none()`.
- `Snapshot` (`protocol/mod.rs:1120` onward) gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub tree: Option<TreeHealth>`. A wall older than this sends none and nothing is shown.
- `panes.rs`, beside the log badge (`:97`):
  - ` TREE STOPPED: <error, first 40 characters> ` in `bold(INK, RED)` when `error` is set;
  - else ` TREE BEHIND from <n> · <waiting> waiting ` in `bold(INK, RED)` when `behind_from` is set;
  - else ` NO TREE ` in `plain(TEXT, PANEL)` when not planted, so Tom can see that KZ-T03 has not planted it yet.
  - Nothing at all when the tree is well.

## Not this lane

- Planting `~/.kazoo/tree` with the imported log (KZ-T03). Marks, buds, moving `HEAD`, `wall_tree` (T04). Notes (T07).
- Showing the tree in the MCP's `look` text. kazoo-mcp gets only its test literal.
- Committing anything for a rack move or a group change on its own: they ride in the next change's `wall.toml`.

## Red tests first

In `kazoo-wall/src/tree/thread/tests.rs`. Each plants a tree in a `tempfile::tempdir()` with `git::plant` and drives a `Wall` built as `daemon/wall/tests.rs:16` builds one. The tests need `git` 2.36 or later on `PATH`; without it they fail, never skip. Each fails today because `tree::git` and `tree::thread` do not exist.

1. `every_change_is_one_commit_with_its_trailers`. Attach, make a turn (with a 4-beat glide), a plug and an add, then `close`. `git log` shows a start commit and three change commits, in order. Each change commit:
   - is authored `Tom <tom@kazoo.invalid>`, with author and committer dates equal to the change's `at`;
   - has the summary as its subject;
   - has `Kazoo-Seq`, `Kazoo-Beat`, `Kazoo-Tempo`, `Kazoo-Clock` and a `Kazoo-Change` that parses back to the very `Change`.

   The turn is one commit, its `Kazoo-Change` holding `glide_beats: 4`.
2. `each_commit_holds_the_wall_after_its_change`. For each commit, `git archive` or `git show <c>:<path>` for every path gives files that `tree::from_files` turns into the wall's `to_file()` as it stood after that change. Keep a copy at each step in the test to compare against.
3. `two_commits_either_side_of_a_restart_are_on_different_clocks`. On one planted tree: wall A attaches, renders 1 s, turns a knob, and closes. Wall B, a new `Wall` (a simulated restart), attaches, renders 1 s, and turns a knob.
   - The two change commits' `Stamp`s have different `clock`, and `beats_between` gives `None`.
   - B's start commit is on B's line `.0`.
   - Between A's start and A's turn, `beats_between` gives `Some(d)` with `d` within one sub-block of the beats rendered.
4. `a_full_queue_never_stops_the_wall_and_ends_in_one_gap_commit`. Attach to a thread made to sleep before each item (`TreeHandle::attach_paused`, a `#[cfg(test)]` constructor whose thread waits on a test latch). Make 1,100 turns, passing each request an `Instant` 400 ms after the last so the flood guard (3 a second) never refuses one (`Wall::request` takes `now`, `daemon/wall.rs:2178`). Every request answers `Ok`. `snapshot().tree.behind_from` is the first change dropped. Release the latch:
   - the tree ends with one `gap` commit whose `Kazoo-Gap` begins at that change and ends at the last;
   - its files equal the wall's;
   - `behind_from` is cleared.
5. `a_git_failure_is_said_and_the_tree_recovers`. Attach. Make the repository's `objects` directory read-only (mode 0500) and turn a knob: `tree.error` is set within 2 s of ticks, and the daemon carries on answering requests. Restore the mode: after `RETRY_AFTER` the next ticks bring a gap commit, and `error` clears.
6. `a_ref_moved_by_anyone_else_stops_the_tree_and_is_never_moved_back`. Attach and commit one change. From the test, make a commit on `trunk` with `git commit-tree` and `git update-ref`, as a stranger would. Turn a knob:
   - the checkpoint's sha check fails, and `tree.error` names the branch;
   - `trunk` still points at the stranger's commit;
   - the stranger's commit is an ancestor of every later tip.
7. `no_tree_no_writes`. A daemon whose state directory has no planted sibling `tree` sends `planted: false`, writes nothing, and a turn still answers.
8. `the_tree_runs_only_these_git_commands`. Read `tree/git.rs` and `tree/thread.rs` with `include_str!` and find every string passed to `.arg(` or `.args(`: each git subcommand is one of the allowed list above, and `--force` appears nowhere.
9. `stamps_read_back_exactly`. `Stamp::parse` of a message this code wrote gives back every field. A message without trailers is `Err`.

## Checks

As PLAN.md's STACK RULES, run on Tom's Mac under the shared lock:
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4`
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings`
- `cargo fmt --all --check`
- `ast-grep scan kazoo-wall/src/tree.rs kazoo-wall/src/tree kazoo-wall/src/daemon kazoo-wall/src/protocol kazoo-wall/src/tui/draw/panes.rs kazoo-mcp/src/render.rs`
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`

## Definition of done

- [ ] The 9 tests fail first (paste each under RED), then pass.
- [ ] `plant`, `Planted`, `Importer`, `TreeHandle`, `TreeItem`, `News`, `TreeHealth`, `Snapshot::tree`, `Stamp`, `beats_between` and the six constants exist as named, each constant with its reason.
- [ ] No new crate: `Cargo.lock` unchanged.
- [ ] BENCH: the time `apply` spends making files and the diff for the evening wall (mean of 1,000 turns, release build), and the commits per second the thread sustains for the same turns with one checkpoint a second. Say whether the machine was quiet.
- [ ] Under DONE, paste `git log -3 --format=fuller` from test 1's tree.

## Code quality

As PLAN.md's STACK RULES:
- Nothing here runs on the audio thread. The control thread never blocks on the tree, except `close`'s wait of at most 5 s.
- Every child process is waited for; none is left a zombie.
- Every git failure carries git's exit status and what was asked of it. None is `.ok()`-discarded.
- New files are under 1000 lines. `daemon/wall.rs` gains only the calls named here, about 80 lines.

## END

```
START KZ-T02b base=<sha> branch=lane/kz-T02b df=<GB> at <time>
END KZ-T02b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
