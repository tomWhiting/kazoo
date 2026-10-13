# KZ-T04a: the tree knows where the wall is: commits grow on the line HEAD names, a go moves HEAD and writes nothing, the first change after a go grows a bud, and marks name moments that never move

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Items from KZ-T01, KZ-T02b and KZ-T03 are named as those briefs name them (`tree::from_files`, `tree::git::Importer`, `TreeItem`, `News`, `Importer::tag`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

PLAN.md's T04 ("moments, marks and buds; `wall_mark`, `wall_go`, `wall_tree`; console keys") is four lanes, because it touches more than ten files: **T04a** (this: the tree thread's side, and the change kind a go is), **T04b** (the wall: the requests, the go itself, marks for seats and saves), **T04c** (the seats' tools), **T04d** (the console's keys). They run in that order; T04c and T04d may run side by side.

Read by Cassio, 13 Oct evening. Accepted as written. A go here is a cut (a swap, as a load); KZ-T05 makes it a crossing.

Time: about 45 minutes of a builder's work. One mechanism: the tree thread keeping `HEAD` as the wall's place. Crates: kazoo-wall. Depends: KZ-T01, KZ-T02a, KZ-T02b and KZ-T03 (landed). T03 may edit `tree/thread.rs` (to share the change commit), so this lane starts from it. Nothing here is reachable from a seat yet; T04b wires it.

## Outcome

The tree's `HEAD` is where the wall is: on a line (`HEAD` is the symbolic ref `refs/heads/trunk` or `refs/heads/bud/…`), or at a moment off any line (`HEAD` names a commit, after a go). KZ-T02b's tree thread now commits each change onto the line the wall is on, not always onto the branch it read at attach. A new change kind, `What::Go`, puts the wall at a moment: the thread writes no commit for it and moves only `HEAD`; no branch or tag ever moves for a go. The first commit after a go grows a bud: a new branch `bud/<moment>/<time>` whose first commit's parent is the moment. With no change after the go, nothing is created. Undoing a go (a `What::Go` that `undoes` it) puts `HEAD` back on the line it left. The thread also answers five new jobs from the control side, in queue order after every change before them: mark the moment the wall is at, mark a save, find a moment and read its wall, say where the wall is, and list the tree. Marks are lightweight tags that never move.

## The design (decided; build exactly this)

### Words shared with the wall and its clients (`protocol/tree.rs`, new; `pub mod tree;` in `protocol/mod.rs`)

```rust
/// A place in the tree: a commit, and the line and mark it was reached by.
pub struct TreePlace {
    pub commit: String,                 // 40 hex digits
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,         // "trunk", "bud/…": the wall is on this line
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,           // the mark's own name, when reached by one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<u64>,            // the commit's Kazoo-Seq, when it is a change commit
}
pub struct MomentView { pub commit: String, pub short: String /* 12 hex */, pub change: Option<u64>,
                        pub seat: String, pub at: String, pub summary: String /* the subject */ }
pub struct BranchView { pub name: String, pub tip: MomentView,
                        #[serde(default, skip_serializing_if = "Option::is_none")] pub grew_from: Option<String> }
pub struct MarkView { pub name: String, pub kind: MarkKind /* mark | save */, pub tag: String, pub moment: MomentView }
pub struct MarkResult { pub name: String, pub tag: String, pub at: TreePlace }
pub struct TreeResult {
    pub here: Option<TreePlace>,        // None only in a tree with no commit yet
    pub through: u64,                   // the latest change the tree thread has taken
    #[serde(default)] pub revision: u64,// filled in by the wall (T04b): its latest change
    pub branches: Vec<BranchView>, pub branch_count: usize,
    pub marks: Vec<MarkView>, pub mark_count: usize,
    pub recent: Vec<MomentView>,        // the wall's line, newest first
}
pub const DEFAULT_RECENT: u32 = 20;
pub const MAX_RECENT: u32 = 200;
pub const MAX_LISTED: usize = 200;
pub const MAX_MOMENT: usize = 128;
pub const MAX_MARKS: usize = 1_000;
```

All derive `Debug, Clone, PartialEq, Serialize, Deserialize`; `MarkKind` is `#[serde(rename_all = "snake_case")]`. Each constant's doc comment gives its reason: 20 recent moments fill a screen; 200 is the most one listing walks on the thread that also writes the commits (a `git log` of 200 is a few milliseconds); 200 branches or marks listed, the counts saying the rest; a moment is at most 128 bytes (a bud's name is under 90); `MAX_MARKS` is more than a year of daily marking by every seat, and past it a list of marks is no longer read. Tom's console is never held to it (T04b).

### The change kind a go is (`protocol/mod.rs`, `What`, `:670`)

Added after `Load` (`:806`):

```rust
/// The wall put at a moment in the tree while it played (a go), or, when
/// the change `undoes` a go, put back where that go left. As a load, the
/// wall as it was is kept in the save `backup` first, and undoing this
/// brings it back exactly from that save.
Go {
    to: TreePlace,          // where the wall is now
    from: TreePlace,        // where it was: HEAD as the tree thread had it just before
    backup: String,
    fingerprint: String,    // hash_bytes of backup's file as written
    modules: u32,
    cables: u32,
    tempo: f64,             // the tempo playing now
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    by_console: bool,
},
```

Reason it is a kind of its own and not a field on `Load`: `What::Load` is built field by field in eight places in seven test files across two crates (`by_console:` at `store.rs:2741`, `change.rs:537`, `protocol/tests.rs:807`, `daemon/wall/tests.rs:3236`, `:3339`, `:3688`, `tui/tests/saves.rs:88`, `kazoo-mcp/src/render.rs:1896`), and a go is not a save loaded: its words and its place in the tree are its own. An older build reads a `go` line as `What::Unknown` (`#[serde(other)]`, `:871`), as it reads any change from a newer wall.

The exhaustive matches on `What` gain an arm, each with a load's meaning, and nothing else in them changes in this lane:
- `change.rs` `describe_with` (`:48`): `What::Go` → a new `went(to, undo, backup, (modules, cables, tempo))` beside `loaded` (`:216`). Without `undo`: `went to <place>: <n> modules, <n> cables, <bpm>; the wall as it was is saved as "<backup>"`. With `undo` (`summarise_with` already says "undid change N: "): `went back to <place>: …` with the same tail. `<place>` is `mark "<name>"` when `to.mark` is set, else `<branch>` when `to.branch` is set, else `the moment after change <n>` when `to.change` is set, else `moment <first 12 hex>`; followed by `, after change <n>` when a mark or a branch is shown and `to.change` is set. Counts and tempo through `count` and `format::bpm`, as `loaded` writes them. Example: `Tom went to mark "dawn", after change 812: 41 modules, 51 cables, 120 BPM; the wall as it was is saved as "before-go-20261013-201500"`.
- `patch.rs` `never_undone` (`:234`): `What::Go { .. }` joins the arms returning `None` (`:262–269`).
- `patch.rs` `Patch::undo` (`:1107`): `What::Go { .. } => Err(error(ErrorCode::Internal, "goes are undone by the daemon"))`, beside the load's arm (`:1141`).
- `fingerprints.rs` `Dye::touch` (`:180`): `What::Go { .. }` joins the arms that leave nothing (`:220–228`): the wall put up brings its own dye.
- `tree.rs` `stamp_births` and `has_add` (KZ-T01, both written with no `_` arm, so both meet a new `What`): `What::Go` stamps nothing and has no add (a go adds no module; its modules keep the births their files carry).
- `daemon/wall.rs` `mirror` (`:1819`): `What::Go { .. } => self.swap_engine()`, as the load's arm (`:1899`).

### The line (`tree/thread.rs`, and the new `tree/thread/here.rs` declared from it)

- The thread keeps `line: Line`, `enum Line { On { branch: String, tip: Tip }, At { commit: String, mark: Option<String> } }`, where `Tip` is the last commit on that branch, as a fast-import mark (`:<k>`) or a 40-digit id.
- **At attach, and at every restart of the importer after a failure**, the thread reads the line with `here::read_line(tree) -> Result<(Line, Option<u64>), HereError>` instead of KZ-T02b's `git::read` (which stays as it is, for its own tests): `git symbolic-ref -q HEAD` names the branch (then its tip and `through` exactly as `git::read` reads them, `Kazoo-Seq` or else `Kazoo-After`), or exits 1, and then `HEAD` is detached at `git rev-parse -q --verify HEAD`. `line` starts as `On { branch, tip }` or `At { commit, mark: None }`.
- **One importer per line.** KZ-T02b's `Importer` writes to one branch and its checkpoint checks that branch. So the thread runs an importer only for the line it is on: built from a `Planted { branch, tip, through }` it makes itself (`Planted`'s fields are public), so its first commit has `from <tip>`. When the line changes (a go, a bud, an undone go), the running importer is checkpointed and finished first. Spawning one costs a few milliseconds, and lines change only when someone goes somewhere. No change to `tree/git.rs`.
- **A change commit** goes through the importer of the line the wall is on:
  - `On { branch, tip }`: through the line's importer; `tip` becomes the new commit.
  - `At { commit: M, mark }`: this is the bud. Its name is `bud/<from>/<time>`: `<from>` is `ref_part(mark)` (below) when the go named a mark, else the first 12 hex digits of `M`; `<time>` is the change's `at` as `YYYYMMDDTHHMMSSZ` (`2026-10-13T09:01:07Z` → `20261013T090107Z`). If `git rev-parse -q --verify refs/heads/<name>` finds it, or this importer wrote it already, add `-2`, `-3`, … to `<time>` until free; past `-99`, the change is `Down` through KZ-T02b's failure path and `line` is left as it was. Then start the bud's importer from `Planted { branch: "refs/heads/<name>", tip: Some(M), through }` (so its first commit is `from M` and creates the branch), commit, and `line = On { branch: <name>, tip: <the commit> }`. After a restart with `HEAD` detached the mark is not known, and the hex is used.
  - Examples: a go to mark "dawn" (after change 812), then Tom turns `vcf1.cutoff` at `2026-10-13T09:01:07Z`: `refs/heads/bud/dawn/20261013T090107Z`. A go to `#812`: `refs/heads/bud/a1b2c3d4e5f6/20261013T090107Z`.
  - Reason for the form: Tom's words, a bud "named for its moment and time"; a mark is the name people gave the moment, the hex is its name otherwise; UTC times sort; the `/` keeps every bud from one moment together in `git branch` and `jj`.
- **The checkpoint** is KZ-T02b's, unchanged: one importer, one branch, checked against the sha git wrote, so a branch moved by anyone else still stops the tree.
- **`HEAD` follows the line**, written only after the checkpoint that made what it names durable (until then the thread's `line` rules): `On` → `git symbolic-ref -m "kazoo: <the change's summary>" HEAD refs/heads/<branch>` when `HEAD` does not already name it; `At` → `git update-ref --no-deref -m "kazoo: <summary>" HEAD <commit>`. `HEAD` is not a branch: moving it moves where the wall is, never anything in the past. These two are the only commands that touch `HEAD`, and `update-ref` is run for nothing but `HEAD` with `--no-deref`.
- **A `What::Go` change item.** No commit. `committed_through` becomes its seq (it is taken). If `to.branch` is `Some(b)` and `b`'s tip (`rev-parse`, after a checkpoint if commits are pending) is `to.commit`, `line = On { branch: b, tip: to.commit }`; else `line = At { commit: to.commit, mark: to.mark }`. Then `HEAD` as above. The same for a go that undoes a go: its `to` is where the undone go left.
- **The wall after a go is the base of the next commit.** In `Wall::apply` (`daemon/wall.rs:2092`), where KZ-T02b offers each change: for a `What::Go`, after offering its change item, raise the generation, make the mirror `tree::to_files` of the wall now up, and offer it as KZ-T02b's `Whole { kind: Gap, seq: change.seq, … }`. Its seq equals `committed_through`, so the thread writes it as the next commit's base (`deleteall` and every file) with no commit of its own, exactly as KZ-T02b handles a gap that covers nothing. Reason: the next commit's parent is the moment, and its files must be the whole wall then, never a difference from the wall before the go.
- **A go the queue could not take.** If offering the go's change item fails, the wall keeps `moved_to: Option<TreePlace>` (its `to`); KZ-T02b's resync then offers a new `TreeItem::Moved { to }` before its `Whole` gap item, taking `moved_to` only when that offer succeeds. The thread treats `Moved` as a go's change item without the seq. Reason: the thread must never go on committing on the line the wall left.
- **A start at a moment.** At attach with `line = At { commit: M, .. }`, KZ-T02b's `Start` whole item is compared with `M`'s files: equal, it is written as the base with no commit and the wall stays at `M`; different (changes the tree did not take before a stop), it is committed, and it is the bud's first commit.
- **Nothing else moves a ref.** The importer is never given `--force`; a new branch or tag is written only after `rev-parse` found it absent.

### Names of marks and saves in refs (`here.rs`)

Mark names are save names (`valid_save_name`, `protocol/mod.rs:102`: 1 to 64 of `A-Z a-z 0-9 space _ . -`, starting with a letter or digit, not ending in space or `.`). Two names are one mark when equal ignoring ASCII case.

- `pub fn ref_part(name: &str) -> String`: each space becomes `+`; a `.` that follows a `.` in the name becomes `=`; if the result ends in `.lock`, that `.` becomes `=`. `pub fn name_of(part: &str) -> String`: `+` → space, `=` → `.`. Reason: git refuses spaces, `..` and a trailing `.lock` in a ref name (`git check-ref-format`), and save names never contain `+` or `=`, so the map is one to one and reads back. "gorgeous toms evening 2026-10-13 1913" is `gorgeous+toms+evening+2026-10-13+1913`.
- A mark is `refs/tags/mark/<ref_part(name)>`, keeping the case it was given.
- A save's mark is `refs/tags/save/<ref_part(lower-cased name)>/<YYYYMMDDTHHMMSSZ>`, the save's UTC time, with `-2`, … if taken. Lower-cased, because a save of the same name in another case is the same save. A save written over gets a new tag beside the old: the old moment keeps its name.
- Tags are written through `Importer::tag(name, from)` (KZ-T03) with the commit id: on the running importer, followed by a checkpoint with its last mark; with none running (the wall at a moment), on a short-lived importer that only tags and then `finish`es, which writes and flushes the refs as it exits.

### Jobs and answers (`tree/thread.rs`)

- `TreeItem` gains `Mark { ticket: u64, name: String, console: bool }`, `SaveMark { name: String, at: String }`, `Resolve { ticket: u64, moment: String }`, `Here { ticket: u64 }`, `List { ticket: u64, recent: Option<u32> }` and `Moved { to: TreePlace }`.
- `TreeHandle::ask(&mut self, item: TreeItem) -> bool` sends a job with `try_send`, counting its bytes as `offer` does, and never touches `resync` or `behind_from`: a job that cannot be sent is the asker's to refuse (T04b), not a change the tree missed.
- `News` gains `Answer { ticket: u64, through: u64, answer: Result<Answered, HereError> }`, `Answered` being `Marked(MarkResult)`, `Resolved { to: TreePlace, here: TreePlace, wall: Box<PatchFile> }`, `Here(TreePlace)`, `Listed(Box<TreeResult>)`. `HereError` is `UnknownMoment(String)`, `MarkExists(String)`, `Full(String)`, `Broken(String)`, `Down(String)`, each a whole sentence (T04b maps them to error codes).
- KZ-T02b's `tick` drain (`daemon/wall.rs`) keeps each `News::Answer` in a new `Wall` field `tree_answered: Vec<(u64, u64, Result<Answered, HereError>)>` for T04b to take; nothing else is done with it here.
- Before answering `Mark`, `Resolve`, `Here` or `List`, the thread checkpoints if any commit is not yet acknowledged, so git's own commands see every commit before the job. While the tree is down (KZ-T02b drops items then), a job is answered `Down` with KZ-T02b's error, never dropped.
- `Mark`: no commit on the wall's line yet (`On` with no tip, or an empty tree) → `UnknownMoment("the tree has no moment yet: nothing has been committed")`. A mark that reads back (`name_of`) equal to `name` ignoring case → `MarkExists("a mark \"<existing>\" names <short> already; marks never move: choose another name")`. Not `console` and `MAX_MARKS` marks in the tree → `Full("the tree holds 1,000 marks; only Tom's console marks more")`. Else tag the line's commit, checkpoint, and answer `MarkResult { name, tag, at: here }`.
- `SaveMark`: tag the line's commit as the save's mark; no answer; a failure goes through KZ-T02b's failure path, said as "the save \"<name>\" could not be marked in the tree: <why>".
- `Resolve`: find the moment (below), read its files (`git ls-tree -r -z --full-tree <commit>`, then the blobs through one `git cat-file --batch`), turn them into a save with `tree::from_files` (`Broken("the moment <short>'s files cannot be read: <why>")` if it refuses), and answer with `here`, the line now.
- `Here`: answer the line as a `TreePlace` (with the mark the go named, when the wall is at a moment).
- `List`: `here`; `through`; every `refs/heads/*` newest tip first (by the tip's commit time, then name), at most `MAX_LISTED`, with `grew_from = name_of(<from>)` for `bud/<from>/<time>`; every `refs/tags/mark/*` and `refs/tags/save/*` newest first, at most `MAX_LISTED`; the first-parent walk from the wall's commit, `min(recent.unwrap_or(DEFAULT_RECENT), MAX_RECENT)` long. Refs through one `git for-each-ref`, commits through `git log` (`--no-walk=unsorted` for the tips, `--first-parent -n` for the walk), every field read with a `%x00`-separated format.
- Every answer carries `through`.

### Finding a moment (`here.rs`, `pub fn resolve(tree: &Path, line: &Line, moment: &str) -> Result<TreePlace, HereError>`)

In this order, the first that matches decides:
1. `trunk`, or a name starting `bud/`, that is a branch: its tip, `branch: Some(name)`.
2. Exactly 40 lower-case hex digits naming a commit: that commit.
3. A mark's name, ignoring case: its commit, `mark: Some(<the mark's own name>)`.
4. A save's tag as listed, `save/<part>/<time>`: its commit.
5. `#<digits>`: the change commit (`Kazoo-Kind: change`) whose `Kazoo-Seq` is that number. The thread builds a map from seq to commit on the first such request, by one `git log --all --format=%H%x00%(trailers:key=Kazoo-Kind,valueonly)%x00%(trailers:key=Kazoo-Seq,valueonly)`, keeping only change commits (start, gap and settle commits share their seq with the change before them), and adds each change commit it writes after that. None: `UnknownMoment("change <n> has no moment of its own: a go, a change that did not happen, or one the tree has not taken yet")`.
6. 7 to 39 lower-case hex digits: `git rev-parse -q --verify <x>^{commit}`; git saying it is ambiguous → `UnknownMoment("<x> names more than one moment; give more of it")`.
7. Else `UnknownMoment("no moment \"<x>\": give a branch (trunk, bud/…), a mark, a change as #<number>, or a commit (7 to 40 hex digits)")`.

`change` in the answer is the commit's `Kazoo-Seq` when it is a change commit. A whole commit id always means that commit; a mark wins over a shorter prefix because a person typed the mark's name (`wall_tree` shows whole ids for this reason).

## Not this lane

- The `mark`, `go` and `tree` requests, `replace` for a go, allowances, the undo of a go on the wall, error codes, telling other seats: T04b. The seats' tools: T04c. The console: T04d.
- The crossing (a go that glides): T05. The fire: T06. Notes on moments: T07.

## Red tests first

In `tree/thread/here/tests.rs`, each on a tree planted with `tree::git::plant` in a `tempfile::tempdir()`, driving a `Wall` with the tree attached as KZ-T02b's tests do, and `What::Go` changes made by hand (`Change` values offered as `apply` offers them; T04b makes real ones). They need `git` 2.36 or later, as KZ-T02b's do. Each fails today because there is no `What::Go`, no `here` module, and commits go onto the branch read at attach whatever `HEAD` says.

1. `changes_grow_the_line_head_names`: three changes; `trunk` moves each time, `HEAD` stays the symbolic `refs/heads/trunk`.
2. `a_go_moves_head_and_writes_nothing`: a go to change 2's commit: after a checkpoint `HEAD` names that commit, `trunk` is unchanged, `git rev-list --all --count` is unchanged.
3. `the_first_change_after_a_go_grows_a_bud_named_for_its_moment_and_time`: by commit, `bud/<12 hex>/<time>`; by mark "dawn", `bud/dawn/<time>`; the bud's first commit's parent is the moment, its files the whole wall; `HEAD` is the symbolic ref to the bud.
4. `with_no_change_after_a_go_nothing_grows`: a go, then the go undone: no new ref; `HEAD` the symbolic `trunk`, which never moved.
5. `undoing_a_go_puts_head_back_on_the_line_it_left`: then the next change extends `trunk`.
6. `undoing_an_old_go_whose_line_grew_since_leaves_the_wall_at_its_moment`: `HEAD` names `from.commit`; the next change buds.
7. `a_go_by_branch_name_carries_on_that_line`.
8. `two_buds_from_one_moment_in_one_second_have_their_own_names`: `…Z` and `…Z-2`.
9. `a_go_the_queue_dropped_is_still_taken_before_the_gap`: a thread held by KZ-T02b's paused test constructor, the queue filled, a go offered and dropped, released: the gap commit is on the bud, not on `trunk`.
10. `a_start_at_a_moment_with_the_same_wall_writes_nothing`: close and attach again while at a moment: no start commit; with a different wall, the start commit buds.
11. `marks_name_where_the_wall_is_and_never_move`: "Dawn" then "dawn": `MarkExists`, the tag unchanged; a seat at `MAX_MARKS` gets `Full`, the console does not.
12. `mark_and_save_names_are_refs_and_read_back`: for "a b", "a..b", "x.lock", "Gorgeous Toms", and 1,000 random valid save names: `git check-ref-format refs/tags/mark/<ref_part(n)>` exits 0 and `name_of(ref_part(n)) == n`.
13. `moments_resolve_by_branch_id_mark_save_change_and_prefix`: each rule, their order, an ambiguous prefix (make two commits until their first 7 digits match, or use a 7-digit prefix of a tree whose objects share it), and `#n` on a commit KZ-T03 imported.
14. `a_resolved_moment_is_its_wall`: `Resolved.wall` equals `tree::from_files` of the commit's files, births included.
15. `the_tree_lists_within_its_bounds`: 205 marks list 200 with `mark_count` 205; `recent` 500 walks 200; `grew_from` reads back.
16. `no_branch_or_tag_ever_moves_backwards`: after tests 1 to 11's moves, every branch's reflog has only fast-forwards and no tag's reflog has more than one entry.
17. `here_runs_only_these_git_commands`: read `here.rs` with `include_str!`; every git subcommand passed is one of `symbolic-ref`, `update-ref`, `rev-parse`, `for-each-ref`, `log`, `ls-tree`, `cat-file`, `check-ref-format`; `update-ref` appears only as `update-ref --no-deref -m … HEAD`; `--force` appears nowhere. (KZ-T02b's own command test reads `git.rs` and `thread.rs`; `thread.rs` gains no git subcommand here, every new one is in `here.rs`.)
18. In `change.rs`'s tests: `goes_say_where_the_wall_went_and_where_it_was_kept` (the four `<place>` forms and the undo form, exact strings as above).

## Bench

Build the seq map on KZ-T03's measured scratch tree if it is still on this machine (6,4k commits), else on a tree of 20,000 commits made through an `Importer` in a test helper; print the time under BENCH. Expected under a second; over two, say so under FOUND.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tree.rs kazoo-wall/src/tree kazoo-wall/src/protocol kazoo-wall/src/change.rs kazoo-wall/src/patch.rs kazoo-wall/src/fingerprints.rs kazoo-wall/src/daemon/wall.rs
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 18 tests fail first (paste each under RED), then pass.
- [ ] `What::Go`, `TreePlace`, `TreeResult`, `MarkResult`, `MomentView`, `BranchView`, `MarkView`, `MarkKind`, the five constants, `Line`, `TreeHandle::ask`, the six new `TreeItem`s, `News::Answer`, `Answered`, `HereError`, `Wall::tree_answered`, and `here::{ref_part, name_of, resolve}` exist as named.
- [ ] Files: `protocol/mod.rs`, `protocol/tree.rs`, `change.rs`, `patch.rs`, `fingerprints.rs`, `tree.rs` (the `stamp_births` arm: KZ-T01 wrote it with no `_` arm so a new `What` is met there), `daemon/wall.rs`, `tree/thread.rs`, `tree/thread/here.rs`, `tree/thread/here/tests.rs`: ten. `tree/git.rs` is not touched.
- [ ] BENCH line written.

## Code quality

As PLAN.md's STACK RULES. `here.rs` under 1000 lines. `protocol/mod.rs` (1,861 lines) and `daemon/wall.rs` (4,691) gain only the variant, the arms, the base offer, `moved_to` and `tree_answered`: list both under FOUND with their line counts. The thread never panics on a git failure: each is a sentence through KZ-T02b's failure path, and the wall plays on. Every `git` child is waited for.

## END

```
START KZ-T04a base=<sha> branch=lane/kz-T04a df=<GB> at <time>
END KZ-T04a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the seq map's build time, and the tree it was built on
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
