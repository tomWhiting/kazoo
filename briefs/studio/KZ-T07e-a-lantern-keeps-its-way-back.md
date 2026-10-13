# KZ-T07e: a lantern keeps its way back: the tree keeps a copy of the lighting seat's conversation up to the lantern, in its own git directory and never pushed, and puts it back when Claude Code has removed it

Items from KZ-T07a to KZ-T07d are named as those briefs name them (`Way`, `Portal`, `Visited`, `resume_point`, `TreeItem::Write`, `tree/thread/lanterns.rs`, `render/lanterns.rs`'s `asked`, `tui/draw/lanterns.rs`, …); none had landed when this was written, so use what landed, and find each item by what it does. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Written by Cassio, 13 Oct evening, after Waffles' reading of T07a against Tom's lantern design. The design names the hazard and says it must be settled before lanterns ship (`ablative/stack/norn/docs/design/norn-memory/MEMORY-AND-LANTERNS.md` §7.2, line 437: "a lantern can open onto nothing"; line 447: "absence that presents as presence"; "either child sessions become retained-by-reference, or a lantern must be able to state that its portal is gone"). T07a does the second. This lane does the first, so that a lantern's way back is kept for as long as the lantern is.

Time: about 45 minutes of a builder's work. One mechanism: keep a lantern's conversation, and put it back. Crates: kazoo-wall, kazoo-mcp (one sentence). Depends on T07c and T07d, which must have landed before this lane opens.

Files (nine):
1. `kazoo-wall/src/tree/lantern/keep.rs` (new): `KeptWay`, `keep`, `restore`, the constants.
2. `kazoo-wall/src/tree/lantern/keep/tests.rs` (new).
3. `kazoo-wall/src/tree/lantern.rs`: `pub mod keep;`; `portal` counts a kept copy.
4. `kazoo-wall/src/tree/thread/lanterns.rs`: the waiting list, the sweep at start, the putting back on a visit.
5. `kazoo-wall/src/tree/thread.rs`: the loop tries the waiting list when it wakes.
6. `kazoo-wall/src/tree/thread/lanterns/tests.rs`: the thread's tests.
7. `kazoo-wall/src/protocol/tree.rs`: `Visited.restore`.
8. `kazoo-mcp/src/render/lanterns.rs`: `asked` says it.
9. `kazoo-wall/src/tui/draw/lanterns.rs`: the console's lantern popup says it.

## Outcome

A lantern lit by a Claude seat today can be asked in a year. When the tree thread sees the answer to the seat's `wall_lantern` call in its conversation, it keeps the conversation up to and including that answer in the tree's git directory. When someone visits the lantern after Claude Code has removed the conversation (`cleanupPeriodDays`, 30 days unless set), the visit puts the kept copy back where Claude Code looks for it, and the two commands work as they did on the day. The visit says plainly that it did so, and that the conversation now ends at the lantern.

## The design (decided; build exactly this)

### What is kept, and where

- The store: `<git dir>/kazoo/ways/` of the tree (`git rev-parse --git-dir`, read once per thread run). Not a ref and not in the work tree, so no clone, fetch or push ever carries it, as T07a keeps `refs/notes/lanterns` out of every copy. Reason: these are the seats' conversations with Tom, and they stay on this Mac.
- One file per session, `<session>.jsonl`, append only: the transcript's bytes from its start through the end of the line that holds the furthest kept lantern's answer. A Claude Code transcript only ever grows, so every lantern of that session is a prefix of one file. Reason: a night's transcript reached 77 MB on 13 Oct; one copy per session rather than per lantern keeps a night with ten lanterns at one night's bytes.
- One record per lantern, `<lantern id>.json`: `{"v":1,"file":"<session>.jsonl","bytes":<n>}`, the kept file and how many of its bytes are this lantern's. `pub struct KeptWay { pub v: u32, pub file: String, pub bytes: u64 }`, serde, `deny_unknown_fields` off (a newer field is kept, as T07a reads lines).
- Tom's spoken words are not kept. Reason: T07a holds them by file and lines only, never the text (Tom's standing line that his speech stays with Dot), and this lane does not change that.

### Keeping (`tree/lantern/keep.rs`)

`pub fn keep(store: &Path, id: &str, way: &Way, waited: Duration) -> Result<KeepOutcome, KeepError>` (`waited`: how long since the lantern was lit, from its entry's `at`), `pub enum KeepOutcome { Kept(KeptWay), NotYet, Gone }`:
1. `way.transcript` is not a file → `Gone`.
2. The cut: `resume_point(transcript, call)` (T07a) gives the answer's entry; the cut ends at the end of the line holding it. With no answer yet → `NotYet`, unless `waited` is over `KEEP_GIVE_UP` (`pub const KEEP_GIVE_UP: Duration = Duration::from_secs(3600)`, wall-clock: this is how long a seat may take to receive its answer, not a lantern's age), when the cut ends at the end of the line holding `way.message`. Reason: a seat that stopped before its answer was written still has its conversation up to the call.
3. If `<session>.jsonl` exists with `k` bytes: when `k` is at least the cut, and the transcript's first `cut` bytes equal the file's first `cut` bytes, the record points at it with `bytes: cut` and nothing is copied. When `k` is less than the cut and the transcript's first `k` bytes equal the file's (compared in 64 KiB pieces, never read whole), append the transcript's bytes `k..cut`. Otherwise (Claude Code rewrote the file) write the cut whole to `<session>.<id>.jsonl` and point the record there. Reason: a lantern's copy is never one that differs from what the seat had.
4. Durability (Tom, 3 Oct: one durable operation with one full flush): a new file is written to `<name>.tmp`, `sync_all`, renamed, and the directory synced; an append is `sync_all`ed before the record is written. The record is written last, the same way. A crash leaves a kept file at most longer than its records say, and every record still points at bytes that are there.
5. Errors (`KeepError`, with a sentence each: the transcript unreadable, the store unwritable, the disk full) are said in the daemon log with the lantern's id, and the lantern stays waiting. A keep never fails a write or a visit, never sends `Down`, and never holds a music commit back beyond the time of the copy.

`pub fn kept(store: &Path, id: &str) -> Option<KeptWay>`: the record, when its file holds at least `bytes`.

### When (`tree/thread/lanterns.rs`, `tree/thread.rs`)

- The thread keeps `waiting: Vec<(String, Way, DateTime<Utc>)>`: the id, the way, and the entry's `at`.
- A lantern written with a `way` joins `waiting` (it cannot be kept at once: its answer is written after the wall answers).
- At start, once, after the first checkpoint: every lantern with a `way` and no `kept` record joins `waiting`. Reason: the lanterns lit before this lane, or while the thread was down.
- The loop (T02b's, which wakes at least every `CHECKPOINT_AFTER`) tries `waiting` after an item or a timeout when the queue is empty and `KEEP_EVERY` (`pub const KEEP_EVERY: Duration = Duration::from_secs(30)`) has passed since the last try. Each `Kept` or `Gone` leaves `waiting` (`Gone` with a line in the daemon log: its way is lost before it could be kept); `NotYet` and errors stay.

### Putting it back (`tree/thread/lanterns.rs`, in a visit)

Before the visit's answer is made: when the lantern has a `way`, `way.transcript` is not a file, and `kept` gives a record, `restore(store, &record, &way.transcript)`:
- Create the transcript's directory if missing. Copy the record's first `bytes` of the kept file to `<transcript>.kazoo-tmp`, `sync_all`, then `std::fs::hard_link` it to the transcript's path and remove the temporary. Reason: a hard link fails if the file exists, so a conversation Claude Code wrote back in the meantime is never overwritten.
- The answer's new field: `pub restore: Option<Restore>` on `Visited` (`#[serde(default, skip_serializing_if = "Option::is_none")]`), `pub enum Restore { Restored, Failed(String) }`. `Restored` makes the lantern's `portal` `Open`; `Failed` leaves it `Gone`, with the reason.
- `portal` (T07a's rule) gains: a lantern whose transcript is gone but which has a `kept` record reads `Open` in listings. Reason: its way back is there; the visit will put it in place.
- With the kept copy as the transcript, `resume_point` finds the answer's entry as on the day, and the commands are T07a's, unchanged.

### What the visit says

- `render/lanterns.rs`'s `asked`, after the way-back block, for `Restored`: `Claude Code had removed this conversation; kazoo put back the copy it kept when the lantern was lit, so the conversation now ends at the lantern (open knows nothing that came after).`; for `Failed(why)`: `Claude Code has removed this conversation, and putting back the copy kazoo kept failed: <why>.`
- `tui/draw/lanterns.rs`, the same line in the console's words: `the conversation was put back from the copy kept at lighting; it ends at the lantern` / `the kept copy could not be put back: <why>`, in `TEXT_DIM` and `RED` from `tui/theme.rs`.

## Not this lane

A size limit on the store: what it holds is bounded by what Claude Code kept for those sessions (341 MB for this project on 13 Oct), and a limit that dropped a lantern's way would be the hazard this lane closes. The daemon log says the store's size at each start, so it is watched. Narrowing dimming to the modules a seat's hands were on (Waffles, 13 Oct: the change log names them): T07a's whole-wall count stands.

## Red tests first

Each fails today because nothing keeps a way.

1. `keep.rs`: `a_lantern_is_kept_through_its_answer`: a fixture transcript with the call's entry and its answer, then more lines: the kept file ends exactly at the end of the answer's line; the record's `bytes` is that length.
2. `keep.rs`: `two_lanterns_of_one_session_share_one_file`: the second lantern later in the same transcript appends only the new bytes; both records point at one file with their own `bytes`.
3. `keep.rs`: `a_rewritten_transcript_is_kept_apart`: the transcript's first bytes changed after the first keep: the second lantern's cut is `<session>.<id>.jsonl`, and the first lantern's record and bytes are unchanged.
4. `keep.rs`: `no_answer_waits_then_keeps_through_the_call`: `NotYet` before `KEEP_GIVE_UP` (`waited` given to the function, never a slept hour), then a cut through the call's line; a missing transcript is `Gone`.
5. `keep.rs`: `putting_back_never_overwrites`: `restore` into a missing path gives the kept bytes exactly; into a path that exists, it fails with its sentence and the file there is unchanged.
6. `thread/lanterns`: `a_visit_puts_a_removed_conversation_back`: a lantern lit, the thread wakes and keeps it, the transcript is deleted, the listing says `Open`, a visit answers `Restored` with `resume_at` the answer's entry, and the transcript is back.
7. `thread/lanterns`: `keeping_never_holds_the_music`: a store made read-only: the music's commits all land, the lantern stays waiting, the daemon log names it, and nothing sends `Down`.
8. `render/lanterns.rs` and the console's render: the two sentences each, exact.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src kazoo-mcp/src
```

## Definition of done

- [ ] The 8 tests fail first (paste each under RED), then pass.
- [ ] Files: the nine above. T07a's test `lanterns_run_only_these_git_commands` still passes (the only new git subcommand is `rev-parse --git-dir`; add it to that test's list).
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, with Tom: light a lantern from a Claude seat, wait a minute, see `<git dir>/kazoo/ways/` hold the session's file and the lantern's record; move the transcript aside, visit, run the `ask` command; move the transcript back.

## Code quality

As PLAN.md's STACK RULES. No file read whole that can be larger than 4 MiB except through `resume_point`'s own limits; copies stream in 64 KiB pieces. Every `std::fs` result is handled with its own sentence.

## END

```
START KZ-T07e base=<sha> branch=lane/kz-T07e df=<GB> at <time>
END KZ-T07e head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
