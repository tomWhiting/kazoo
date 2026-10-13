# KZ-T06a: the tree can be walked forward along a branch from any moment, each step read with its wall, its time and its beat, and a step the wall plays again moves the tree's place without writing a thing

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Items from KZ-T01 to T05c are named as those briefs name them (`Birth`, `stamp_births`, `Stamp`, `beats_between`, `Importer`, `TreeItem`, `News`, `TreeHandle`, `Line`, `here::resolve`, `here::name_of`, `Answered`, `HereError`, `TreeItem::Moved`, `moved_to`, `What::Go`, `cross_plan`, `cross`, `CrossTiming`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

PLAN.md's T06 ("the fire: replay of a branch at its beats and any speed; a change mid-fire grows a bud") is four lanes, because it touches more than ten files: **T06a** (this: the tree's side and the change kind a fire's step is), **T06b** (the wall burns: timing, speed, stopping), **T06c** (the seats' `wall_fire`), **T06d** (the console's `F`). They run in that order; T06c and T06d may run side by side.

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T01 to T05c have landed. One consequence for lanterns, decided here: a fire's steps are logged changes, so they raise the wall's revision, but replaying the past is not new work, and Tom ruled that a lantern's time is the work's history. So fire steps must not dim lanterns: see the note in KZ-T06b and KZ-T07b (whichever of the two lands second does it). Accepted otherwise as written.

Time: about 60 minutes of a builder's work. One mechanism: the tree thread reading a line forward, and keeping its place as the wall replays it. Crates: kazoo-wall. Depends on T01 to T05c, which must have landed. Nothing here is reachable from a seat yet; T06b wires it.

Files (ten):
1. `kazoo-wall/src/protocol/mod.rs`: `What::Fired`, `FiredGlide`.
2. `kazoo-wall/src/change.rs`: a step's words.
3. `kazoo-wall/src/patch.rs`: `never_undone` and `Patch::undo` arms.
4. `kazoo-wall/src/fingerprints.rs`: `Dye::touch` arm.
5. `kazoo-wall/src/tree.rs`: `stamp_births` arm.
6. `kazoo-wall/src/daemon/wall.rs`: `mirror`'s arm; the step's tree offer; `fired_files`; a step that puts up a wall of its own, in the places that pick those out.
7. `kazoo-wall/src/tree/thread.rs`: steps taken without a commit; `Moved::through`; `HEAD` written lazily; the `Walk` and `Steps` jobs.
8. `kazoo-wall/src/tree/thread/walk.rs` (new): `walk`, `read_steps`, `Walk`, `Step`, `StepHead`, `BlobCache`.
9. `kazoo-wall/src/tree/thread/walk/tests.rs` (new).
10. `kazoo-wall/src/tree/thread/here/tests.rs`: its `TreeItem::Moved { to }` literal gains `through: None`, if it has one (`rg -n 'Moved \{'`); if it has none, this lane is nine files.

## Outcome

- A new change kind, `What::Fired`, says the wall has become a moment of its tree again: "Tom played moment a1b2c3d4e5f6 again on trunk: Cassio turned vcf3.cutoff from 0.3 to 0.35 over 2 beats". The tree thread writes **no commit** for it (the tree holds that moment already: the past never changes) and moves its place to the moment, as a go does. The first change anyone makes after it grows a bud from that moment (T04a). When the step is the tip of the branch being burnt, the wall is back on that branch, and the next change carries it on.
- The tree thread answers two new jobs: **walk** (from a moment, along a branch: the moments after it in order, up to the tip) and **steps** (for a batch of those moments: each one's wall, its exact files, its beat stamp, its UTC time, who made it and what it said). Reading is one `git cat-file --batch` per batch, with a bounded cache of file contents, so most steps read only the one or two files that changed.

## The design (decided; build exactly this)

### The change kind a step is (`protocol/mod.rs`, `What`, after T04a's `Go`)

```rust
/// A moment of the tree played again by a fire: the wall became the wall
/// of the moment `commit`, which grew from `parent`, the moment the wall was
/// at. The tree holds it already, so nothing is committed for it: the
/// tree's place moves to it. Made by the seat that lit the fire. It cannot
/// be undone on its own: going anywhere (wall_go) or undoing the go that
/// lit the fire brings the wall elsewhere.
Fired {
    /// The moment played (40 hex).
    commit: String,
    /// The moment it grew from (40 hex): where the wall was.
    parent: String,
    /// The branch the fire burns.
    along: String,
    /// The moment's own change number (`Kazoo-Seq`), when it is a change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    change: Option<u64>,
    /// Who made the moment (its author), and what it said (its subject).
    by: String,
    said: String,
    /// The knobs the moment's change turned, with the glides they are
    /// played at, in beats (already divided by the fire's speed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    glides: Vec<FiredGlide>,
    /// The moment put up a wall of its own (its file's `wall` differs from
    /// the moment before: a load, a new wall, a start on another): played
    /// as a cut, and every seat looks again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    replaces: bool,
    /// The moment is the tip of `along` as it was when the fire was lit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    tip: bool,
    /// Modules and cables on the wall now, and the tempo playing.
    modules: u32,
    cables: u32,
    tempo: f64,
},
```
`pub struct FiredGlide { pub module: String, pub knob: String, pub beats: f64 }` (serde, `Debug, Clone, PartialEq`).

**Why a kind of its own, not the moment's own `What` again.** Replaying the original change (`Turn`, `Patch`, `Add`, …) through the patch would remake it from today's code, with today's numbering and dye, and could differ from what the tree holds; and the tree thread could not tell it from a new change. A step instead puts up the moment's wall exactly as its commit holds it (T06b: `Patch::load` of the moment's files, births and ids included), so after every step the wall is that moment, byte for byte in its files, and the change says so by naming the commit. The glides are carried so a turn is heard as it was played (T06b), not as a jump.

**Why it is logged at all.** The log is what the wall did, in order: undo, the start's check of `patch.json` against the log (`resumed`, `daemon/wall.rs:979`), and KZ-T03's evidence all read it. A wall that changed without a line would be a wall the log cannot account for.

The exhaustive matches on `What` gain an arm each, and nothing else in them changes:
- `change.rs` `describe_with` (`:48`): `played moment <first 12 of commit> again on <along>: <said>`, with `said` through `patch::printable` (`patch.rs:1686`), then `, a wall of its own` when `replaces`, then ` (the tip of <along>)` when `tip`. So the summary reads `Tom played moment a1b2c3d4e5f6 again on trunk: Cassio turned vcf3.cutoff from 0.3 to 0.35 over 2 beats`.
- `patch.rs` `never_undone` (`:234`): `What::Fired { .. } => refused("a moment the fire played cannot be undone on its own; wall_go puts the wall at any moment, and undoing the go that lit the fire brings back the wall from before it")`. `Patch::undo` (`:1107`): joins the arms returning `never_undone` (`:1142–1148`).
- `fingerprints.rs` `Dye::touch` (`:180`): joins the arms that leave nothing (`:220–228`): the moment's files bring their own dye.
- `tree.rs` `stamp_births` (KZ-T01): stamps nothing (a step adds no module; every module carries the birth its moment's file gives it, T01: "Replay (T06) reads births from the commits; it never mints them again").
- `daemon/wall.rs` `mirror` (`:1819`): `What::Fired { replaces, .. } => match self.cross_plan.take() { Some(plan) if !replaces => self.cross(*plan), _ => self.swap_engine() }`. T06b makes the plan before the patch changes, as T05b's `replace` does, with `CrossTiming { fades: 0, tempo: 0, knob: <the glides>, .. }`; a step with no plan (none is made in this lane but by its tests) cuts. A step that `replaces` is a cut, because the moment was one (a load cuts).

### A step that puts up a wall of its own (`daemon/wall.rs`)

`What::Fired { replaces: true, .. }` gives the wall a new number (T06b sets `patch.wall` to the step's seq), so every seat's module names are checked afresh (`check_wall`, `:3031`). Each place that picks out the changes that put up a wall picks it out too, beside T04b's `| What::Go { .. }`: `log_opens_on` (`:913`), `walls_of` (`:941`: `walls.up = change.seq`, no copy, because a step is never undone), `resumed`'s `known` (`:997`), `replaced_since` (`:3004`, `:3009`: "(change <n>: <its summary>)" as for a load), `apply`'s memory note (`:2122`). Not `left_out` (`:1008`): a step that never reached `patch.json` is not undone, so there is nothing to leave out. And wherever KZ-T02a breaks the clock line for a load, a replacing step breaks it too. A step that does not replace changes none of these: it is a change to the wall up, as a turn is.

### The step's tree offer (`daemon/wall.rs`, in `apply`, where KZ-T02b offers each change)

- `Wall` gains `pub(super) fired_files: Option<Files>`: the moment's files exactly as its commit holds them, set by T06b just before it applies the step.
- For a `What::Fired`, offer `TreeItem::Change { change, files: Vec::new(), generation }` (no diff: nothing is to be committed). Then, if `fired_files` is `Some(files)`, the mirror becomes `files`: the next change's difference is then taken against the moment's own files, which is exactly what the thread's next commit grows from (its parent is that moment). If it is `None`, call KZ-T02b's resync (`TreeHandle`'s `resync = true`), so the next tick sends the whole wall as a base before anything else is committed.
- If the offer is dropped (KZ-T02b's full queue), set T04a's `moved_to` to `TreePlace { commit, branch: tip.then(|| along.clone()), mark: None, change }` and the new `moved_through = Some(seq)`; T04a's resync then offers `TreeItem::Moved { to, through }` before its whole gap item.

### The thread (`tree/thread.rs`)

- **A `What::Fired` change item** (generation checked as KZ-T02b checks every item):
  - Its `parent` must be where the line is: `Line::At { commit }` with `commit == parent`, or `Line::On { tip, .. }` with `tip` (checkpointed first if it is still a fast-import mark, to have its id) equal to `parent`.
  - When it is: no commit. `line = Line::At { commit, mark: None }`, except when the item's `tip` is set and `git rev-parse -q --verify refs/heads/<along>` is `commit` (one call, for the last step only): then `line = Line::On { branch: along, tip: commit }`, as T04a's go by branch name does. If an importer is running for the line left, it is checkpointed and finished as T04a does when the line changes. `committed_through = seq`. `head_due = true`.
  - When it is not (the tree is somewhere the wall did not leave from: never expected): no commit, `committed_through` unchanged, and `News::NeedWhole`, so the wall sends the whole wall and the gap commit that follows holds it (on a bud, by T04a's rules); the daemon log says "the fire played <short> from <parent short>, but the tree was at <here short>: the next commit holds the whole wall".
- **`TreeItem::Moved`** (T04a) gains `through: Option<u64>`: when `Some(t)`, `committed_through = committed_through.max(t)`, so the whole gap item that follows is a base and not a gap commit (KZ-T02b: a gap at or below `committed_through` is written as the next commit's base).
- **`HEAD` is written lazily for steps.** T04a moves `HEAD` after the checkpoint that makes what it names durable; a step names a commit that is durable already, so the only cost is the `git update-ref` process, which a fire may need hundreds of times a second. So for a line moved by a step, `HEAD` is written when `head_due` and one of: `CHECKPOINT_AFTER` (KZ-T02b, 1 s) has passed since `HEAD` was last written; a job is about to be answered (so every answer sees `HEAD` where the wall is); the line moves for any other reason; `Finish`. A crash loses at most one second of where the fire was: the next start finds `HEAD` behind `patch.json`, and T04a's start rule commits the wall as it starts (a bud from the older place), which says truly what the wall is.
- **The `Walk` and `Steps` jobs.** `TreeItem` gains `Walk { ticket: u64, from: String, along: Option<String> }` and `Steps { ticket: u64, commits: Vec<String> }`; `Answered` gains `Walked(Box<Walk>)` and `Steps(Vec<Step>)`. Each is answered as T04a's jobs are: after a checkpoint if a commit is unacknowledged, `Down` while the tree is down, with `through`. The thread keeps one `BlobCache`, emptied at each `Walk`.

### Walking (`tree/thread/walk.rs`)

```rust
/// A moment's time and stamp.
pub struct StepHead {
    pub commit: String,          // 40 hex
    pub stamp: Stamp,            // KZ-T02b: kind, seq, beat, tempo, clock, gap
    pub at: String,              // its author date, as YYYY-MM-DDTHH:MM:SSZ (UTC)
    pub tempo: f64,              // its wall's tempo (wall.toml)
    pub wall: u64,               // its wall's number (wall.toml `wall`)
}
pub struct Walk {
    pub from: TreePlace, pub along: String, pub here: TreePlace,
    pub start: StepHead, pub start_files: Files, pub start_wall: Box<PatchFile>,
    pub steps: Vec<String>,      // the moments after `from` on `along`, oldest first, to its tip
}
pub struct Step {
    pub head: StepHead, pub parent: String,
    pub by: String, pub said: String,            // author name, subject
    pub change: Option<Box<Change>>,             // Kazoo-Change, for a change commit
    pub files: Files, pub wall: Box<PatchFile>,  // exactly as the commit holds them; tree::from_files of them
}
pub const MAX_FIRE_STEPS: usize = 200_000;
pub const STEPS_READ_BYTES: usize = 16 * 1024 * 1024;
pub const BLOB_CACHE_BYTES: usize = 16 * 1024 * 1024;
```
Reasons, in their doc comments: 200,000 moments is about a year and a half at the rate the log grew (6,373 changes in 17 days), held as 41-byte ids it is 8 MB; past it the fire is lit from a later moment. 16 MiB of file contents a batch keeps one batch's work on the thread that also writes the commits to a few tens of milliseconds and bounds memory at the largest wall file twice over (`MAX_WALL_FILE`, 8 MiB). The cache holds the same bound, because consecutive moments share nearly every file.

`pub fn walk(tree: &Path, line: &Line, from: &str, along: Option<&str>) -> Result<Walk, HereError>`:
1. `from` is found by T04a's `here::resolve` (every form a go takes). It must hold a 40-hex commit; that is `M`.
2. The branch:
   - `along` given: it must be a branch (`trunk` or `bud/…`, `git rev-parse -q --verify refs/heads/<along>`), else `UnknownMoment("no branch \"<along>\": wall_tree lists them")`; and `M` must be an ancestor of its tip (`git merge-base --is-ancestor <M> <tip>`: exit 0 yes, 1 no, anything else `Broken` with git's words), else `UnknownMoment("<from> is not on <along>: the fire burns a branch forward from a moment on it")`.
   - `along` not given and `from` named a branch (T04a's rule 1 matched): that branch, burnt **from its first moment**: for `trunk`, its root (`git rev-list --max-parents=0 refs/heads/trunk`; not exactly one line is `Broken`); for `bud/<from>/<time>`, the moment it grew from, `here::resolve(name_of(<from>))` (T04a names a bud for the mark or the 12 hex of its moment), checked as an ancestor. Reason: "burn trunk" means the whole trunk; a branch's tip has nothing after it.
   - Otherwise: `git for-each-ref --contains <M> --sort=-committerdate --format=%(refname) refs/heads/`: `refs/heads/trunk` if listed, else the first listed (the newest bud holding `M`); none is `UnknownMoment("<from> is on no branch")`. Reason: the trunk is the song; a bud is chosen only when the moment lies on no trunk.
   - The tree never merges (KZ-T02b), so "an ancestor" and "on the first-parent line" are the same.
3. `git rev-list --first-parent --reverse <M>..refs/heads/<along>`, read line by line, each a 40-hex id; past `MAX_FIRE_STEPS` lines the child is killed and waited for and the walk is `Full("<along> holds more than 200,000 moments after <from>: light the fire from a later moment")`. None: `UnknownMoment("nothing grew after <short> on <along>: there is nothing to burn")`.
4. `start` from `read_steps` of `[M]` (its head, files and wall); `here` the line now, as T04a's `Here` answers it.
- `walk_capped(…, cap: usize)` holds the body; `walk` calls it with `MAX_FIRE_STEPS`, so the test can use a small cap.

`pub fn read_steps(tree: &Path, cache: &mut BlobCache, commits: &[String], bound: usize) -> Result<Vec<Step>, HereError>` (the thread passes `STEPS_READ_BYTES`):
- One child, `git -C <tree> cat-file --batch`, stdin and stdout piped, stderr to the daemon's log; every request is a line `<object id>\n`; every answer is `<id> <type> <size>\n`, `size` bytes, `\n` (`<id> missing\n` is `Broken`). A `size` above `MAX_WALL_FILE` is `Broken` before anything is read.
- For each commit: read the commit object (headers `tree`, the first `parent`, `author <name> <<email>> <unix> <tz>`, then the message after the blank line); `at` from `<unix>` as `YYYY-MM-DDTHH:MM:SSZ` with `chrono` (KZ-T02b's way); `said` the first line; `stamp` by `Stamp::parse` (a message it refuses is `Broken`); `change` from the `Kazoo-Change:` trailer by `serde_json::from_str` when the kind is `change` (refused JSON is `Broken`). Then the commit's tree object, its `wall.toml` blob and its `modules` tree, and each `modules/<name>` blob (git's tree format: entries `<mode> <name>\0<20 raw bytes>`; any other path is `Broken`, as `from_files` would refuse it). Blobs come from the cache when there; otherwise they are read and cached. `files` is the map T01's `Files` is; `wall` is `tree::from_files(&files)` (its `TreeError` is `Broken` with its sentence); `tempo` and `wall` in the head come from it.
- After each commit, when the bytes read for this call (not counting cache hits) pass `bound`, stop and answer the steps read (always at least one).
- At the end stdin is closed and the child waited for; a non-zero exit is `Broken` with git's status. Every child is waited for on every path.

`BlobCache`: `HashMap<String, Vec<u8>>` and a `VecDeque<String>` of ids in the order cached, evicting the oldest while the bytes held pass `BLOB_CACHE_BYTES`; `fn reads(&self) -> u64` counts blobs read from git (for the test).

## Not this lane

- Timing, speed, the beat offsets, stopping on a change, the requests, the guard: T06b. The tools and keys: T06c, T06d.
- Lanterns (T07), the little tree (T08).

## Red tests first

In `tree/thread/walk/tests.rs`, each on a tree planted with `tree::git::plant` in a `tempfile::tempdir()`, grown by a `Wall` with the tree attached (as KZ-T02b's and T04a's tests grow one), with `What::Go` and `What::Fired` changes made by hand where the test says, offered as `apply` offers them; `git` 2.36 or later, as before. Each fails today because there is no `What::Fired` and no `walk` module.

1. `a_walk_lists_a_branch_forward_from_a_moment`: ten turns on `trunk`; walking from `#3`: the steps are the commits of changes 4 to 10, in order; `start` is change 3's, with its stamp and its wall.
2. `a_branch_named_alone_burns_from_its_first_moment`: from `trunk`: the root then every commit; from a bud's name: from the moment it grew from (by mark and by hex) to its tip.
3. `the_branch_is_trunk_then_the_newest_bud_holding_the_moment`: a moment on trunk and on two buds gives `trunk`; one only on buds gives the newest; `along` not holding it, a branch that does not exist, and a moment at the tip each give their `UnknownMoment` sentence.
4. `too_long_a_walk_is_refused`: `walk_capped` with a cap of 5 on a line of 6: `Full` with the sentence, and the child was waited for.
5. `each_step_reads_back_its_wall_files_stamp_and_change`: for every step, `files` equals `git show <c>:<path>` for every path, `wall` equals `tree::from_files(&files)`, `stamp` equals `Stamp::parse` of `git log -1 --format=%B`, `change` equals the logged `Change`, `at` equals the change's `at`.
6. `imported_moments_read_without_a_beat`: commits written as KZ-T03 writes them (no `Kazoo-Beat`, `Kazoo-Clock`): `stamp.beat` is `None` and `at` is the author date.
7. `a_batch_stops_at_its_bound_and_the_cache_spares_rereads`: a bound of one file's size reads one step; reading ten consecutive turns of the 85-module evening wall (KZ-T01's fixture) reads 86 blobs for the first and at most 2 for each after it (`reads`).
8. `a_played_step_moves_the_tree_and_writes_nothing`: a go to `#3`, then `Fired` items for changes 4 and 5 (with `fired_files` set to their files): `git rev-list --all --count` unchanged; after `close`, `HEAD` names change 5's commit; then a turn grows `bud/<12 hex of 5's commit>/<time>`, whose parent is change 5's commit and whose files are 5's plus the turn.
9. `the_last_step_puts_the_wall_back_on_the_branch`: a `Fired` with `tip` for trunk's tip: the next turn extends `trunk` (no bud).
10. `a_step_from_elsewhere_asks_for_the_whole_wall`: a `Fired` whose `parent` is not where the line is: no commit for it, then one gap commit holding the wall, on a bud.
11. `a_dropped_step_still_moves_the_tree_before_the_gap`: the thread held by KZ-T02b's paused constructor, the queue filled, a `Fired` offered and dropped, released: no gap commit; the next turn's commit has the step's commit as its parent.
12. `head_is_written_lazily_and_always_by_the_finish`: 200 `Fired` items within a second: `HEAD` is written at most twice during them (count with a `#[cfg(test)]` counter in the thread), and after `close` it names the last.
13. `a_step_without_its_files_resyncs`: a `Fired` with `fired_files` `None`: the next tick offers the whole wall, and the next turn's commit holds exactly the wall's files.
14. `a_played_moment_says_whose_it_was_and_cannot_be_undone`: the summary as written above for a turn, a replacing step and a tip; `Wall::undo` of a step is `not_allowed` with the sentence.
15. `a_replacing_step_is_a_wall_of_its_own`: a log with a `Fired { replaces: true }` at seq `s` and `patch.wall == s`: `resumed` is `None`; a change before `s` cannot be undone after it, and the refusal names change `s`; a seat that looked before `s` is `wall_replaced`.
16. `walk_runs_only_these_git_commands`: `include_str!` of `walk.rs`: every git subcommand is one of `rev-list`, `merge-base`, `for-each-ref`, `cat-file`, `rev-parse`; `--force` and `update-ref` appear nowhere. KZ-T02b's and T04a's command tests still pass (`thread.rs` gains only `rev-parse`, which KZ-T02b allows).

## Bench

`read_steps` over 2,000 consecutive turns of the evening wall (a test helper grows them through an `Importer`), in batches of 32 with the cache: steps per second and milliseconds per batch, release build, `uptime`'s load average. Expected: thousands of steps a second, under 30 ms a batch; under 200 steps a second, say so under FOUND (T06b's top speed would then outrun it).

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tree.rs kazoo-wall/src/tree kazoo-wall/src/protocol kazoo-wall/src/change.rs kazoo-wall/src/patch.rs kazoo-wall/src/fingerprints.rs kazoo-wall/src/daemon/wall.rs
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 16 tests fail first (paste each under RED), then pass; KZ-T02b's, T03's, T04a's and T05b's tests are green.
- [ ] `What::Fired`, `FiredGlide`, `Wall::fired_files`, `moved_through`, `TreeItem::{Walk, Steps}`, `Moved::through`, `Answered::{Walked, Steps}`, `walk`, `walk_capped`, `read_steps`, `Walk`, `Step`, `StepHead`, `BlobCache`, `MAX_FIRE_STEPS`, `STEPS_READ_BYTES`, `BLOB_CACHE_BYTES` exist as named, each constant with its reason.
- [ ] BENCH line written.
- [ ] Under CHANGED: `rg -n 'What::Load \{ \.\. \}' kazoo-wall/src/daemon/wall.rs` after the edits, with each site that picks out walls put up shown to pick out a replacing step too, or why not (`left_out`, `prune`).

## Code quality

As PLAN.md's STACK RULES. `walk.rs` under 1000 lines. `daemon/wall.rs` and `protocol/mod.rs` gain only the variant, the arms, the offer, `fired_files`, `moved_through` and the `| What::Fired { replaces: true, .. }` additions: list both line counts under FOUND. The thread never panics on what git returns: every parse failure is a `HereError::Broken` sentence. Every `git` child is waited for.

## END

```
START KZ-T06a base=<sha> branch=lane/kz-T06a df=<GB> at <time>
END KZ-T06a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: steps per second and ms per batch, with the load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
