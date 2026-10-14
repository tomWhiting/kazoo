# KZ-T08a: the wall can say the shape of its whole tree in one answer: every branch as a limb grown from its parent, how many modules were alive along it, and where the marks, the lanterns and the wall are on it, counted once and kept, so the little tree can be drawn

Line numbers are from main at `820ccc5` and may have moved; find each item by what it does, never rename. Items from KZ-T02b, KZ-T04a, KZ-T04b and KZ-T07a are named as those briefs name them; if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 13 Oct evening, line by line. Checked: 20,000 commits over 8,192 samples is a stride of 3; 1,002 branches kept to 1,000 limbs drops the two oldest buds. Changes: the lane opens only after T07b, T04b and T04a have landed; `MAX_RINGS`'s doc gives the measured bytes a ring takes with its index entry (print `size_of` of both and the bench tree's total under BENCH) rather than the drafted 40. Accepted otherwise as written.

**Amended by Cassio, 14 Oct 2026, 21:2x, for Tom's tree view (KZ-T08b as rewritten that evening). Where this block and the text below disagree, this block wins.**

1. **A fixed step, not a stride.** The drawing grows each limb step by step, and a step's shape must never change once grown (Tom: the past is fixed, so its picture is too). A stride that grows with the tree would re-sample every limb as the tree grows. So `stride` is the constant `GROWTH_STEP: u32 = 16` commits, always; `MAX_SAMPLES` is gone; a limb's samples are `ceil(length / 16)`, its last sample possibly short. Reason for 16: today's 7,100 commits give about 440 steps up the trunk, enough for a natural curve without a dot per knob turn. A million commits give about 62,500 samples, about 3 MB of JSON, read once and then grown (point 4). Test 3 becomes `the_step_is_fixed`: 20,000 commits on trunk and two buds give `stride` 16 and `ceil(length / 16)` samples per limb, and adding 100 commits leaves every earlier sample of every limb unchanged.
2. **Each sample says when and how it sounded.** `Limb.modules: Vec<u16>` becomes `Limb.samples: Vec<Sample>` with `pub struct Sample { pub modules: u16, pub newest: i64, pub sound: Option<Sound> }`: the most modules alive in the step, its newest commit's committer time (unix seconds; T08b grows steps in time order by it), and the mean of the step's commits' `Kazoo-Sound` trailers (KZ-T08c's `sound_of`; `None` when none of them has one). `Ring` gains the commit's `Sound` (`Option`, read from `%B` added to the log format). Depends on KZ-T08c.
3. **Lanterns do not hold this lane.** It opens after T04a and T04b, not T07b. Until T07b has landed, `lanterns` is always empty and the lantern test (part of test 4) is left out; T07b's lane adds lanterns to the shape and that test (T07b's brief says so).
4. **Only what is new.** `Request::Shape { since: Option<u64> }`: with `since` (a `through` from an earlier answer), the answer holds only the samples and limbs grown after it (a limb's samples from the first one that changed, and its whole entry when it is new), plus every mark and `here`. T08b asks once whole, then with `since`.
5. **Nothing is ever left out (Tom, 14 Oct 2026, about 21:45: "we shouldnt limit the number of branches if we can avoid it, i dont want them being dropped. defeats the purpose entirely if it cant grow and we done want to lose anthing").** `MAX_LIMBS` and `MAX_RINGS` are gone, and so are `limb_count`, Limbs step 4 and the `Full` refusal. Every branch is a limb and every commit is a ring, however many there are. A large tree stays cheap because of point 4 (only what is new is sent) and because the rings grow only by the commits written since. Size is something to measure, not a reason to drop: the bench gains a 2,000-bud tree (by an `Importer`), and its first and following `Shape` times and JSON size go under BENCH. Test 6 becomes `two_thousand_buds_are_all_kept`: 2,000 buds from the trunk and one grown from the oldest of them give 2,002 limbs in birth order, the last growing from the oldest bud, and every commit is on a limb. `Rings`' doc says it holds one ring per commit for the thread's life, with the measured bytes a ring takes.
6. **Branches made by hand keep their place (Cally's reading, 14 Oct evening).** Ordering them by their tip's time lets their order change as they grow, which can hand a commit to a different limb and redraw the past. Instead, after the trunk and the buds have claimed theirs, the hand-made branches are claimed one at a time: each round, every remaining one walks its first-parent chain from its tip to the first claimed commit, and the one whose first unclaimed commit is oldest (by committer time, then by branch name) claims its walk. That commit never changes once written, so the order never changes either. Test 2 gains a branch made by hand that grows after a later one: the order and every `from` are the same before and after.
7. **Steps grow in a causal order, the same as `kazoo tree` (Cassio, 14 Oct 2026, 22:2x, from the Opus review of kazoo-tree).** Point 2's `newest` changes while a step fills, so ordering by it would move a step among the others as it grows. Each `Sample` also carries `first: i64`, its first commit's committer time, and `key: i64`, the step's growth key: `first`, held to be no earlier than the key of the limb's sample before it, and for a limb's first sample no earlier than the key of the sample holding its `from`. A step's key depends only on the past, so it never changes. T08b and T09 grow steps by `(key, the limb's place after its parent, the sample's index)`, as `kazoo-tree/src/growth.rs` (`Growth::order`) does. Test `the_keys_never_step_back`: a limb whose commits' times step back an hour every other step gives keys that never fall along the limb, and a bud's first key is at least its `from` sample's.
8. **Buds claim only their own commits.** A bud's name says the commit it grew from (`bud/<from>/<time>`: a mark's ref part, or the first 12 hex). Step 3's walk for a bud stops at that commit as well as at the first claimed one, so a bud claimed before its parent (two buds named for the same second) never takes the parent's commits. As `kazoo-tree/src/history.rs` (`limbs`, `claim`) does. Test 2 gains two buds named for the same second, the second grown from the first.

PLAN.md's T08 ("the console's tree view: branches, thickness, marks, lanterns, where the wall is") is two lanes, because it touches more than ten files: **T08a** (this: the shape, from the tree thread through the wall's socket) and **T08b** (the console view that draws it). The Bevy tree (T09) will read the same answer.

Time: about 40 minutes of a builder's work. One mechanism: a cached walk of the commit graph with a module count per commit. Crates: kazoo-wall. Depends on T07b, which must have landed (this lane and T07b both edit `daemon/wall/moments.rs`, `daemon/wall.rs` and `protocol/mod.rs`, and the shape places T07a's lanterns), and on T04b and T04a, which must have landed.

## Decided: a new `shape` job, not T04a's `List` extended

T04a's `List` answers `wall_tree` and the console's `B`: a flat list of branches, marks and recent moments, read by people and by seats. A drawing needs something else: every commit's place on a limb and the modules alive at each. Putting that into `List` would put thousands of numbers into every `wall_tree` a seat reads. So the shape is its own job and its own request, and `List` is untouched.

## Decided: modules alive, counted from what each commit adds and removes under `modules/`, once

A commit's tree holds one `modules/<id>.toml` per module (KZ-T01), so the modules alive at a moment is the count of those files. Counting them with `git ls-tree` per commit would be one process per commit: 6,400 processes for today's trunk alone. Instead one `git log` of the commits not yet counted, with `--name-status`, gives each commit's added (`A`) and deleted (`D`) paths against its first parent; a commit's count is its parent's plus its adds under `modules/` minus its deletes there (a root commit's parent count is 0). Whole-wall commits (start, gap, a go's base, KZ-T03's loads) are written with `deleteall` and every file, and `--name-status` against the parent still gives exactly their adds and deletes. The counts are kept in the thread for its life, so the next request walks only the commits written since.

## The design (decided; build exactly this)

### The words (`protocol/tree.rs`)

```rust
/// The whole tree's shape, for drawing (T08b) and for T09.
pub struct TreeShape {
    pub commits: u64,             // commits on the limbs listed, each counted once
    pub stride: u32,              // commits per sample, the same for every limb
    pub limbs: Vec<Limb>,         // trunk first, then buds by the time in their names
    pub limb_count: usize,        // every branch; `limbs` holds at most MAX_LIMBS
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub here: Option<Spot>,       // where the wall is
    pub marks: Vec<SpotMark>,
    pub lanterns: Vec<SpotLantern>,
    pub through: u64,
}
pub struct Limb {
    pub name: String,             // "trunk", "bud/dawn/20261013T090107Z"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Spot>,       // the commit it grew from, on its parent limb
    pub length: u32,              // its own commits
    pub modules: Vec<u16>,        // modules alive, the most in each stride of its commits, oldest first
    pub tip: String,              // 40 hex
    pub tip_at: String,           // UTC
}
/// A commit's place: the limb, and how many of the limb's own commits come before it.
pub struct Spot { pub limb: u32, pub at: u32 }
pub struct SpotMark { pub spot: Spot, pub name: String, pub kind: MarkKind, pub commit: String }
pub struct SpotLantern { pub spot: Spot, pub id: String, pub glow: Glow, pub seat: String, pub commit: String }
pub const MAX_SAMPLES: usize = 8_192;
pub const MAX_LIMBS: usize = 1_000;
pub const MAX_RINGS: usize = 1_000_000;
```

Derives as T04a's words. Each constant's doc comment gives its reason:
- `MAX_SAMPLES`: a 200-row terminal is 800 braille dots tall, and a Bevy tree is a few thousand vertices a limb; 8,192 samples over the whole tree is past both, and keeps an answer under 50 KB of JSON. `stride = max(1, ceil(commits / MAX_SAMPLES))`, the same for every limb, so one vertical scale holds for the whole drawing; a limb's `modules` has `ceil(length / stride)` entries, each the most modules alive in its stride of commits (the most, so a short burst of many modules still shows).
- `MAX_LIMBS`: buds grow one per go-then-change; a thousand is years of going back. Past it, the buds whose names' times are oldest are left out (the trunk never is), and a kept bud whose parent was left out is drawn from its nearest kept ancestor limb (below). `limb_count` says how many there are.
- `MAX_RINGS`: the thread keeps one small record per commit (below), about 40 bytes; a million commits is about 40 MB and over seven years at 370 changes a day. Past it the shape is refused (`Full("the tree has more than a million moments; the little tree draws up to a million")`), and nothing else is affected.

### Requests (`protocol/mod.rs`)

`Request::Shape {}` (a struct variant, so it can grow fields): a `TreeShape`. A read: anyone may ask. `op()`: `"shape"`. Not a change; acts on no module.

### The rings (`tree/thread/shape.rs`, new, declared from `tree/thread.rs`)

```rust
struct Ring { id: [u8; 20], parent: Option<u32>, modules: u16, time: i64 }
pub(super) struct Rings { rings: Vec<Ring>, index: HashMap<[u8; 20], u32>, tips: Vec<String> }
```

- `Rings::grow(&mut self, tree: &Path, tips: &[String]) -> Result<usize, HereError>`: runs `git -C <tree> log --reverse --topo-order --root --no-renames --name-status --format=%x1e%H%x00%P%x00%ct --stdin` with stdin lines: every current branch tip, then `--not`, then every tip in `self.tips`. Each record: the commit, its first parent (the tree has no merges; a second parent is `Broken("<short> has two parents; the tree never merges")`), its committer time, and the count: parent's count (0 for a root) plus `A` lines under `modules/` minus `D` lines there, saturating within `u16`. A parent neither known nor earlier in this walk is `Broken`. Then `self.tips = tips`. Answers how many commits it added. Reason for the tips as the boundary: branches only grow (KZ-T02b, T04a), so everything behind a tip seen before is counted already.
- Kept in the tree thread for its life, empty at attach; filled on the first `Shape` job. Dropped and rebuilt from nothing if a `grow` fails, so a half-walked state is never used.

### Limbs (`tree/thread/shape.rs`, `fn limbs(…) -> TreeShape`)

1. The branches: `git for-each-ref --format=%(refname)%00%(objectname)%00%(committerdate:unix) refs/heads`. The trunk is `refs/heads/trunk`; with none, the branch holding the oldest root commit is first.
2. Order: the trunk; then every `bud/<from>/<time>` by its `<time>` (with its `-2`, `-3` suffix, as T04a writes it), then by name; then any other branch (made by hand) by its tip's time. Reason: a bud's name holds the time it grew, and a bud can only grow from a commit that already existed, so a parent bud always comes before its children in this order, and a commit is always claimed by the limb it first grew on.
3. Each limb in order walks its first-parent chain from its tip, claiming every commit not yet claimed, and stops at the first claimed one: that commit is `from` (its limb and its index there). A limb that reaches a root without meeting a claimed commit has no `from`. A limb with no commit of its own (a branch made by hand at an existing commit) is a limb of `length` 0 with its `from`.
4. Past `MAX_LIMBS`, the left-out buds' commits are claimed by the limbs that walk through them, as if they had never been branches, so every kept limb still grows from a kept one.
5. `here`: T04a's line's commit, as a spot. Marks: `refs/tags/mark/*` and `refs/tags/save/*` (T04a's `for-each-ref` reading), each as a spot with its name read back (`name_of`) and its kind. Lanterns: every lantern T07a's reading of every note finds (`Scope::Every`), with its glow computed against the job's `revision`. A mark, a lantern or `here` on a commit no kept limb holds (impossible while every commit is on a branch) is left out and said in the daemon log once.

### The thread's job (`tree/thread.rs`)

`TreeItem::Shape { ticket: u64, revision: u64 }`; `Answered::Shape(Box<TreeShape>)`. As every T04a job: checkpoint first if commits are unacknowledged; answered `Down` while the tree is down; carries `through`.

### The wall's side (`daemon/wall/moments.rs`, `daemon/wall.rs`)

- T04b's `Asked` gains `Shape`. `fn shape(&mut self, seat: &str, now: Instant) -> …` takes T04b's tree-listing bucket (10 in a burst, one a second), sends `Shape { ticket, revision: self.revision() }`, and answers pending; `tree_answers` gains the arm that answers the `TreeShape`. No tree and a stopped tree are refused with T04b's sentences.
- `answer` (`:2381`): `Request::Shape {} => self.shape(seat, now)`. `request_as` (`:2195`): `Request::Shape` takes the tree bucket beside `Request::Tree`.

## Not this lane

Drawing (T08b). The Bevy tree (T09). Seats' tools: no tool asks for the shape (a seat has `wall_tree`); the request is on the socket for the console and for T09.

## Red tests first

In `tree/thread/shape/tests.rs` (declared from `shape.rs`), on trees planted with `tree::git::plant` in a temporary directory, driven through a `Wall` with the tree attached as T04b's tests do, or through an `Importer` in a helper for the large tree. They need `git` 2.36 or later. Each fails today because there is no `Shape` job.

1. `modules_alive_follow_the_files`: adds, removes, a turn, a load and a go: for every commit, the count equals the number of `modules/` paths `git ls-tree -r --name-only <c> modules/` lists (the test's cross-check, not the code's way).
2. `limbs_are_the_trunk_then_buds_by_their_birth`: a go to change 3 and a turn (bud A), a go to change 7 and a turn (bud B), a go into A and a turn (bud C from A): limbs `trunk`, A, B, C in that order; `from` of A is trunk at 3's index, of C is A at its index; lengths exact.
3. `the_stride_keeps_the_answer_bounded`: a tree of 20,000 commits on trunk and two buds: `stride` 3, every limb's `modules` has `ceil(length / 3)` entries, each the most in its stride; the JSON is under 64 KB.
4. `marks_lanterns_and_the_wall_are_placed`: a mark, a save's mark, a lit and a faint lantern, and the wall after a go: each at its exact spot, with its kind and glow.
5. `the_rings_walk_only_new_commits`: a second `Shape` after one more change: `grow` reports 1.
6. `a_thousand_and_one_buds`: 1,000 buds from the trunk and one more grown from the oldest of them, by an `Importer`: 1,000 limbs (the trunk and 999 buds), `limb_count` 1,002, the two oldest buds left out, the bud grown from it starting from the trunk with the left-out bud's commits as its own, and `commits` counting only commits on the listed limbs.
7. `a_wall_with_no_tree_or_a_stopped_tree_says_so` (T04b's sentences), and watchers may ask.
8. `shape_runs_only_these_git_commands`: read `shape.rs` with `include_str!`; every git subcommand is `log`, `for-each-ref` or `rev-parse`; `--force` appears nowhere.

## Bench

The first `Shape` and a following one after one change, on KZ-T03's measured scratch tree if it is still on this machine (about 6,400 commits), else on test 3's 20,000-commit tree. Print both times under BENCH. Expected under 1 s and under 50 ms; over twice that, say so under FOUND.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tree kazoo-wall/src/protocol kazoo-wall/src/daemon
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 8 tests fail first (paste each under RED), then pass.
- [ ] `TreeShape`, `Limb`, `Spot`, `SpotMark`, `SpotLantern`, the three constants, `Request::Shape`, `TreeItem::Shape`, `Answered::Shape`, `Asked::Shape` and `Rings` exist as named.
- [ ] Files: `protocol/tree.rs`, `protocol/mod.rs`, `tree/thread.rs`, `tree/thread/shape.rs`, `tree/thread/shape/tests.rs`, `daemon/wall/moments.rs`, `daemon/wall.rs`: seven.
- [ ] BENCH lines written.

## Code quality

As PLAN.md's STACK RULES. `shape.rs` under 1000 lines. The rings are bounded by `MAX_RINGS`; nothing else grows with the tree. Every `git` child is waited for. `daemon/wall.rs` gains only the routing arm and the bucket line.

## END

```
START KZ-T08a base=<sha> branch=lane/kz-T08a df=<GB> at <time>
END KZ-T08a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the first and the following shape, and the tree they ran on
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
