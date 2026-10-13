# KZ-T07b: any seat or Tom's console can light a lantern where the wall is, add a note to any moment or lantern, visit a lantern, and list them; every seat is told of a new lantern or note, and the music never changes for one

Line numbers are from main at `820ccc5` and may have moved; find each item by what it does, never rename. Items from KZ-T04a, KZ-T04b and KZ-T07a are named as those briefs name them; if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T07a and T04b have landed; `ErrorCode::ALL` grows by one from whatever the base holds (19 at T04b's count; another lane may have added one since). The window is computed from the seconds `change::utc` is given (`now − WORDS_BEFORE`, `now + WORDS_AFTER`), never by parsing `at` back. Accepted otherwise as written.

Second of PLAN.md's T07 split in four (T07a the tree thread, **T07b** the wall, T07c the seats' tools, T07d the console).

Time: about 35 minutes of a builder's work. One mechanism: four requests the wall hands to its tree thread, as T04b's are. Crates: kazoo-wall, and one arm each in kazoo-mcp and the console. Depends on T07a, which must have landed: `NoteEntry`, `Way`, `Words`, `NoteView`, `NotesResult`, `Visited`, `TreeItem::{Write, Notes}`, `Target`, `Scope`, `Answered::{Wrote, Visited, Notes}`, `HereError::NoLantern`, `NOTE_TEXT_MAX`, `WORDS_BEFORE`, `WORDS_AFTER`; and T04b, which must have landed: `Waiting`, `Asked`, `tree_answers`, `TREE_TICKETS`, the tree bucket, `answer_later`.

## Outcome

Four requests on the wall's socket:
- `lantern` lights a lantern on the moment the wall is at: the seat's words, the way back to its conversation (sent by the seat, T07c; none from the console), and Tom's spoken words five minutes either side (`words`, set here).
- `note` adds a note to a moment, to a lantern (its epilogue), or, with neither, to the moment the wall is at.
- `visit` counts a visit to a lantern and answers it whole, with where its way back starts and where Tom's words around it are.
- `lanterns` lists the notes and lanterns near a moment (by default where the wall is), on a branch, or everywhere.

A lantern or a note is told to every seat (`Event::Lit`); a visit is not. None of the four is a change: nothing is logged, nothing is undone, and the sound and the patch are exactly as before.

## Who may write (decided, with reasons)

- **Lanterns, notes and visits: any seat but a watcher, and the console.** They write to the tree, so watchers are refused them as they are refused a mark (`refusal`, `daemon/control.rs:997`). Reason: Norn's design makes lanterns communal ("they belong to the landscape, not the walker", `DESIGN.md`), and Tom's wish is that anyone can go back and ask a moment how it felt.
- **Three a minute per seat, for the three together**, in a bucket of their own with the mark allowance's numbers (`SAVE_BURST`, `SAVE_REFILL_PER_SECOND`, `daemon/wall.rs:167`, `:172`, as T04b's marks use). The console is never held. Reason: a lantern is a deliberate act (Tom, 8 Aug: "declared at moments of completion or success or … learnings"); three a minute is more than any deliberate act needs and stops a seat in a loop from filling a moment's note (1 MiB, T07a). A visit writes a line too, so it shares the bucket.
- **The listing: anyone, watchers too**, through T04b's tree-listing bucket (10 in a burst, one a second). Reason: it is a read, and it walks notes on the thread that writes the commits.

## The design (decided; build exactly this)

### Requests (`protocol/mod.rs`, `Request`, `:168`)

```rust
/// Light a lantern on the moment the wall is at (a `NoteView`). Not a
/// change: nothing is logged. Any seat but a watcher.
Lantern {
    words: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    way: Option<Way>,
},
/// Add a note (a `NoteView`): to `moment`, to `lantern` (its epilogue), or,
/// with neither, to the moment the wall is at. Any seat but a watcher.
Note {
    words: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    moment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lantern: Option<String>,
},
/// Visit a lantern (a `Visited`): the visit is counted. Any seat but a watcher.
Visit { lantern: String },
/// The notes and lanterns (a `NotesResult`): near `near` (default where the
/// wall is), on `branch`, or `every` one. A read: anyone may ask.
Lanterns {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    near: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    every: bool,
},
```

`op()` (`:382`): `"lantern"`, `"note"`, `"visit"`, `"lanterns"`. `is_change()` (`:439`): `Lantern`, `Note` and `Visit` (watchers are refused them, and they count against the flood guard), as T04b made `Mark` one. `acts_on_modules()` (`:420`): none.

`Event` gains, after T04b's `Marked`:

```rust
/// A seat lit a lantern or added a note: sent to every subscriber, the
/// writer included. Not a change: nothing is logged.
Lit { seat: String, kind: EntryKind, id: String, moment: MomentView,
      #[serde(default, skip_serializing_if = "Option::is_none")] on: Option<String>,
      text: String },
```

Its two exhaustive consumers get an arm, worded as T04b's `Marked` arms are:
- `kazoo-mcp/src/channel.rs` (beside T04b's `Marked` arm): another seat's is news, `"<who> lit a lantern at <place>: “<text>”"`, `"<who> noted <place>: “<text>”"`, or `"<who> added to lantern <id>: “<text>”"`, the text through `channel::clean` (`channel.rs:301`, which also cuts it to `MAX_SUMMARY`); `<place>` is `#<n>` when `moment.change` is set, else `moment.short`. The seat's own is not news.
- the console's `on_event` (`tui/app/mod.rs:974`): another seat's is a log note and a status line in the same words, with ` · B lists it`; the console's own is not.

### Error codes (`protocol/mod.rs`, `ErrorCode`, `:550`)

`NoLantern` (`no_lantern`), appended; `ALL` (`:596`) one longer than the base holds (19 → 20 if nothing else has landed one). `kazoo-mcp/src/link.rs:93`'s exhaustive `match` gains the name. T07a's `HereError::NoLantern` maps to it; every other `HereError` maps as T04b maps it (`Full` → `full`, `Broken` → `broken_save`, `UnknownMoment` → `unknown_moment`, `Down` → `internal`).

### The wall's side (`daemon/wall/lanterns.rs`, new, `impl Wall` as T04b's `moments.rs` is; `mod lanterns;` in `daemon/wall.rs`)

`Wall` gains `note_buckets: HashMap<String, Bucket>` (`Bucket::of`, `:214`) and `dot_dir: Option<PathBuf>`, set in `attach_tree` from T07a's `tree::lantern::read::dot_dir()` and passed in every `Write` job; `#[cfg(test)] pub(crate) fn set_dot_dir(&mut self, dir: PathBuf)` lets a test point it at a temporary directory (the 2024 edition makes `std::env::set_var` unsafe, and the tree forbids unsafe code). T04b's `Asked` gains `Write { kind: EntryKind }` and `Notes`. Jobs go through T04a's `TreeHandle::ask`; the answers come back through T04b's `tree_answers`, which gains two arms that call `lanterns.rs`. No tree and the tree down are refused with T04b's sentences; a full queue is T04b's `slow_down`.

**Checking the words** (all three writes): T07a's `tree::lantern::text_problem`; a problem is `bad_request` with its sentence. The words are kept exactly as sent, never trimmed.

**Making an entry** (`fn entry(&self, seat, console, kind, text, on, way, now) -> NoteEntry`): `id` is `format!("{:016x}", chance::fresh_wall_seed())` (`chance.rs:27`); `at` is `change::utc_now()` (`change.rs:354`); `change` is `self.revision()`; for a lantern, `words` is `at − WORDS_BEFORE` to `at + WORDS_AFTER` in the same UTC form. Then `entry.check()`: a refusal is `bad_request` with its sentence (it catches a bad `way` a seat sent: "a lantern's way back names a session that is not a session id", and so on, one sentence per rule).

**`lantern((seat, console), words, way, now)`**: the words, the note bucket (a seat that is not a console: `slow_down` "<seat> has lit or written three lanterns, notes or visits in a minute; wait a moment"), the entry, then `Write { ticket, entry, to: Target::Here, revision }`; pending. A `way` from the console is refused (`bad_request` "the console has no conversation to go back to"): the console sends none (T07d).

**`note((seat, console), words, moment, lantern, now)`**: both `moment` and `lantern` → `bad_request` "a note goes on a moment or on a lantern, not both". `lantern: Some(id)` → `Target::Lantern(id)` with `on: Some(id)`; `moment: Some(m)` (1 to `MAX_MOMENT` bytes, no control character, as T04b checks a go's moment) → `Target::Moment(m)`; neither → `Target::Here`.

**`visit((seat, console), lantern, now)`**: `lantern` is 16 lower-case hex, else `bad_request` "a lantern is named by its 16-hex id, as wall_lanterns lists it". An entry of kind `visit` with `on: Some(lantern)`, `Target::Lantern(lantern)`.

**`lanterns(seat, near, branch, every, now)`**: more than one of the three → `bad_request` "list near a moment, on a branch, or every lantern: one of the three". T04b's tree bucket. `Scope::Near(near)`, `Scope::Branch(b)` or `Scope::Every`, with `revision`.

**Answers** (`tree_answers`' two new arms):
- `Wrote(view)` for a lantern or a note: `Outcome { result: view, change: None, event: Some(Event::Lit { seat, kind, id, moment, on, text }) }`. For a note added to a lantern, the view is the lantern's and the event carries the note's own `id`, `text` and `on`.
- `Visited(v)`: `Outcome { result: v, change: None, event: None }`. Reason: a visit is one seat's quiet conversation with a moment; telling every seat would make visiting loud.
- `Notes(r)`: the `NotesResult`.
- A `HereError` other than `Down` gives the seat's note allowance back (`Bucket::refund`, `:225`), as T04b gives a mark's back.

**Routing.** `answer` (`:2381`): the four arms. `request_as` (`:2195`): `Request::Lanterns` takes T04b's tree bucket there, beside `Request::Tree`'s.

### Nothing in the music changes

The four requests touch no module, cable, tempo, log line, undo list or save. A test holds it (below). `Snapshot` is unchanged: lanterns are listed on request, never pushed into every `look`.

## Not this lane

The tree thread (T07a). The tools and finding a seat's way (T07c). The console's keys and list (T07d). Tom's spoken words are never read here: the wall sets the window and T07a's visit says where they are.

## Red tests first

In `daemon/wall/lanterns/tests.rs` (declared from `lanterns.rs`), with a real `Wall` on a `Store::peek` state directory and a tree planted with `tree::git::plant` in a temporary directory and attached as T04b's tests attach one (they need `git` 2.36 or later), and a wall with no tree for the refusals. Later answers are collected with `tree_answers` as T04b's tests do. Each fails today because there is no `lantern`, `note`, `visit` or `lanterns` request.

1. `a_lantern_is_lit_where_the_wall_is_with_its_way_and_window`: ten changes, then a lantern with a `way`: the `NoteView`'s moment is change 10's commit; the entry's `way` is the one sent; `words` is exactly 300 s either side of `at`; `Event::Lit` is in the outcome.
2. `notes_go_on_a_moment_a_lantern_or_where_the_wall_is`: the three targets; both `moment` and `lantern`: `bad_request`.
3. `a_visit_is_counted_and_tells_no_one`: `visits` 1 then 2; no event; `resume_at` and `heard` present (with `set_dot_dir` pointing at an empty temporary directory, `heard` is empty).
4. `the_three_writes_share_three_a_minute_and_the_console_is_never_held`.
5. `words_and_ways_are_checked_plainly`: empty, 2,001 characters, a tab, only spaces; a `way` whose session is not a uuid, whose transcript is relative; a `way` from the console: each its sentence, nothing sent to the tree.
6. `an_unknown_lantern_is_no_lantern_and_gives_the_allowance_back`.
7. `the_listing_takes_one_scope`: near, branch, every; two at once: `bad_request`.
8. `watchers_list_and_may_not_write`: through `refusal` in `control.rs`.
9. `a_wall_with_no_tree_or_a_stopped_tree_says_so`.
10. `lanterns_change_nothing_in_the_music`: before and after a lantern, a note, a visit and a listing: the patch's fingerprint, `revision()`, the undo list's length and `log.jsonl`'s bytes are equal.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/daemon kazoo-wall/src/protocol kazoo-wall/src/tui/app/mod.rs kazoo-mcp/src/channel.rs kazoo-mcp/src/link.rs
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 10 tests fail first (paste each under RED), then pass.
- [ ] `Request::{Lantern, Note, Visit, Lanterns}`, `Event::Lit`, `ErrorCode::NoLantern`, `Wall::note_buckets`, `Wall::dot_dir` and `Asked::{Write, Notes}` exist as named.
- [ ] Files: `protocol/mod.rs`, `daemon/wall.rs`, `daemon/wall/moments.rs` (the two `Asked` variants and the two `tree_answers` arms), `daemon/wall/lanterns.rs`, `daemon/wall/lanterns/tests.rs`, `tui/app/mod.rs` (one arm), `kazoo-mcp/src/channel.rs` (one arm), `kazoo-mcp/src/link.rs` (one name): eight.

## Code quality

As PLAN.md's STACK RULES. `lanterns.rs` under 1000 lines. `daemon/wall.rs` gains only `mod lanterns;`, the field, the routing arms and the bucket line: list its line count under FOUND. Nothing on the control thread waits for the tree thread, and nothing on it reads any file outside the state directory.

## END

```
START KZ-T07b base=<sha> branch=lane/kz-T07b df=<GB> at <time>
END KZ-T07b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
