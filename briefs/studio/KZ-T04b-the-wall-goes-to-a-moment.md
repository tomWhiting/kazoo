# KZ-T04b: any seat can put the wall at a moment from its past (a cut, undoable), mark the moment it is at, and read the tree; every save made by hand is marked too

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Items from KZ-T01, KZ-T02a, KZ-T02b and T04a are named as those briefs name them; if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Second of PLAN.md's T04 split in four (T04a the tree thread, **T04b** the wall, T04c the seats' tools, T04d the console's keys).

Read by Cassio, 13 Oct evening. Accepted as written: seats may go as they may load (Tom: "we'll do more or less everything for this project together"); marks never move for anyone.

Time: about 45 minutes of a builder's work. One mechanism: the wall asking its tree thread and acting on the answer. Crates: kazoo-wall, and one arm each in kazoo-mcp and the console. Depends: T04a (landed): `What::Go`, `TreePlace`, `TreeResult`, `MarkResult`, `TreeHandle::ask`, the jobs `Mark`, `SaveMark`, `Resolve`, `Here`, `List`, `News::Answer`, `HereError`, `Wall::tree_answered`, and the go's own handling in `Wall::apply`.

## Outcome

Three requests on the wall's socket. `go` puts the whole wall at a moment while it plays: a cut, exactly as a load (the master's short fade, `replace`, `daemon/wall.rs:3420`), with the wall as it was kept first in a save of the wall's own (`before-go-<time>`), logged as one `What::Go` change, told to every seat, undoable exactly. `mark` names the moment the wall is at. `tree` lists where the wall is, the branches, the marks, and the latest moments. Each is answered once the tree thread has done its part, in order behind every change before it, within 5 s or with a sentence saying why not. Every save made by hand is also marked in the tree.

## Who may go (decided, with reasons)

**Any seat but a watcher may go, exactly as any seat may load today** (`Request::Load` doc, `protocol/mod.rs:355–359`: "Any seat but a watcher"), held to the same allowance as a load: a seat that is not a console may put up a wall three times a minute (`LOAD_BURST`, `LOAD_REFILL_PER_SECOND`, `daemon/wall.rs:184`, `:192`), and a go, a load, a new wall and the undoing of any of them all draw on that one allowance (`puts_up_wall`, `:2369`). Tom's console is never held to it. Reasons:
- A go destroys nothing. The past is never changed (no ref moves, T04a), the wall as it was is kept before the go (as for a load, `keeping`, `:3320`), and `wall_undo` of the go brings it back exactly. What a seat may already do with a save, it may do with a moment.
- The rules that make loads safe for others apply unchanged, because a go goes through `replace`: every seat is told (`Event::Loaded`), a seat's changes aimed at the old wall are refused (`wall_replaced`, `check_wall`, `:3031`) until it looks again, the wall's own saves a go needs are kept from pruning (`prune`, `:3351`), and the render guard costs it as it costs a load (today a load is not refused for load, `LOAD_GUARD` doc, `:86–88`; once KZ-D03b has landed, by the modules it wakes: `replace` is where D03b costs it, so nothing here costs it separately).
- Tom's words make the tree a shared journey, and the seats play with him ("live band with Claude"). Only the console may go would make the past Tom's alone, and a seat could still load a save to get the same effect.
- What stays the console's: replacing or deleting a save (`refusal`, `daemon/control.rs:995`). Nothing in this lane deletes or moves a mark, for anyone: marks never move.

**Marks**: any seat but a watcher, three a minute for a seat that is not a console (the save allowance's numbers, `SAVE_BURST` and `SAVE_REFILL_PER_SECOND`, `:167`, `:172`, its own bucket), and not past `MAX_MARKS` (1,000, T04a) for a seat that is not a console. Reason: a mark is permanent and named, like a save; it costs a ref, not a file of a wall.

**The tree listing**: anyone, watchers too (it is a read), at most 10 in a burst refilled at one a second per seat (`SAVES_BURST`, `SAVES_REFILL_PER_SECOND`, `:155`, `:164`, its own bucket). Reason: a listing walks refs and up to 200 commits on the thread that also writes the commits; a seat asking in a loop must not hold up the commits.

## The design (decided; build exactly this)

### Requests (`protocol/mod.rs`, `Request`, `:168`)

```rust
/// Name the moment the wall is at (see `MarkResult`). Not a change to the
/// sound; nothing is logged; every subscriber is told (`Event::Marked`).
/// Any seat but a watcher; marks never move.
Mark { name: String },
/// Put the wall at a moment (a branch, a mark, a save's tag, `#<change>`,
/// or a commit id) while it plays: a `LoadResult`. As a load, the wall as
/// it was is kept first and the go is one change (`What::Go`). Any seat
/// but a watcher.
Go { moment: String },
/// The tree: a `TreeResult`. A read: anyone may ask.
Tree {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recent: Option<u32>,
},
```

`op()` (`:382`): `"mark"`, `"go"`, `"tree"`. `is_change()` (`:439`): `Mark` and `Go` (watchers are refused them by `refusal`, `control.rs:997`, and they count against the flood guard). `acts_on_modules()` (`:420`): none of them (a go names no module).

`Event` (`:908`) gains, after `SaveDeleted`:

```rust
/// A seat marked a moment: sent to every subscriber, the marker included.
/// Marking is not a change: nothing is logged.
Marked { seat: String, name: String, tag: String, at: TreePlace },
```

Its two exhaustive consumers get an arm: `kazoo-mcp/src/channel.rs:194`'s `match` (another seat's mark is news, as a save is at `:169`: `"<who> marked the moment as \"<name>\""` plus `" (after change <n>)"` when `at.change` is set; the seat's own mark is not news) and the console's `on_event` (`tui/app/mod.rs:974`): another seat's mark is a log note and a status line in the same words; the console's own is not.

### Error codes (`protocol/mod.rs`, `ErrorCode`, `:550`)

`UnknownMoment` (`unknown_moment`) and `MarkExists` (`mark_exists`), appended; `ALL` (`:596`) becomes 19 long. `kazoo-mcp/src/link.rs:93` names every code in an exhaustive `match`: add the two names there. T04a's `HereError` maps: `UnknownMoment` → `unknown_moment`, `MarkExists` → `mark_exists`, `Full` → `full`, `Broken` → `broken_save`, `Down` → `internal`; the sentence is the error's message.

### The wall's side (`daemon/wall/moments.rs`, new, `impl Wall` as `daemon/wall/chains.rs` is; `mod moments;` in `daemon/wall.rs`)

- Jobs go to the tree through KZ-T02b's `TreeHandle`, which the `Wall` owns (`Wall::attach_tree`), with T04a's `TreeHandle::ask`; answers are taken from T04a's `Wall::tree_answered`. `Wall` gains `tree_waiting: BTreeMap<u64, Waiting>`, `next_tree_ticket: u64` starting at `TREE_TICKETS = 1 << 63`, `mark_buckets` and `tree_buckets` (`HashMap<String, Bucket>`, `Bucket::of`, `:214`).
  - Reason for the ticket base: answers later than the request already exist (`Outcome::pending`, `:427`, for `speak`), and `control.rs` keeps one map of them (`awaiting`, `control.rs:253`). The speech renderer counts its tickets up from 1 (`daemon/speech.rs:283`) and never reaches 2^63, so one map holds both without a clash. `Outcome::pending`'s doc gains "or (mark, go, tree, undo of a go) from `Wall::tree_answers`".
- `Waiting { seat: String, console: bool, asked: Instant, what: Asked }`, with `Asked::Mark`, `Asked::List`, `Asked::Go { made: u64, tries: u8, moment: String }`, `Asked::Back { go: u64, backup: String, fingerprint: String, back_to: TreePlace, made: u64, tries: u8 }`. `made` is the wall's latest change (`self.made`) when the job was sent.
- Constants (in `moments.rs`, each with its reason in its doc comment):
  - `TREE_ANSWER_WITHIN: Duration = Duration::from_secs(5)`: a resolution reads one commit's files (milliseconds) and the thread's queue drains far faster; 5 s means the tree is stuck, and it is under the seats' own 10 s wait (`ANSWER_WITHIN`, `kazoo-mcp/src/link.rs:37`), so a seat hears why instead of timing out.
  - `GO_TRIES: u8 = 3`: see staleness below.
- No tree (no `TreeHandle`: KZ-T02b attaches none until KZ-T03 has planted the tree, and tests make walls without one): `mark`, `go` and `tree` are refused `internal` "this wall keeps no tree yet (kazoo wall tree import plants it); nothing was changed". The tree down (`TreeHealth::error` set): `internal` "the tree is stopped (<its error>); nothing was changed".
- `ask` fails because KZ-T02b's queue is full: `slow_down` "the tree is behind (its queue is full); ask again in a moment". Nothing is waiting then.

**`mark(seat, console, name, now)`**: `valid_save_name(name)` (`protocol/mod.rs:102`) or `bad_name` "a mark's name is 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a letter or digit". A seat that is not a console takes from its mark bucket or `slow_down` "<seat> has marked three moments in a minute; wait a moment". Send `Mark { ticket, name, console }`; answer pending. When `Marked` comes back: `Outcome { result: MarkResult, change: None, event: Some(Event::Marked { seat, name, tag, at }) }`. A `HereError` comes back as its code (below) with its sentence, and a refusal other than `internal` gives the seat its mark back (`Bucket::refund`, `:225`).

**`go(seat, console, moment, now)`** (the load allowance was taken by `request_as`, `:2240–2255`, because `puts_up_wall` now says yes to `Request::Go`):
1. `moment` is 1 to `MAX_MOMENT` bytes with no control character, or `bad_request` "a moment is a branch (trunk, bud/…), a mark, #<change> or a commit id". Send `Resolve { ticket, moment }` with `made = self.made`; pending.
2. `Resolved { to, here, wall }` comes back. If `self.made` has moved since the job was sent, other changes were logged between, so `here` may not be where the wall now is: send `Resolve` again with the new `made` (`tries + 1`); after `GO_TRIES` tries, answer `slow_down` "the wall kept changing while the tree found the moment; ask again". Reason: the go records where the wall was (`from`) so its undo can put the wall back on its line, and that must be the place after the last change before the go, never an older one.
3. `to.commit == here.commit`: `not_allowed` "the wall is at that moment already (<place>); nothing changed" (the allowance comes back).
4. Else `let (incoming, notes) = Patch::load(&wall)` (`patch.rs:1319`, the forgiving load every save gets; an `Err` is `broken_save` with its sentence) and `self.replace((seat, console), incoming, &Source::Moment { to, from: here }, notes)`. Its answer is the load's (`LoadResult`, `protocol/mod.rs:1830`), its change a `What::Go`, its event `Event::Loaded { save: None, .. }`, so every seat and console forgets the old wall and looks again, as after a load.
- Every refusal from the tree other than `internal` gives the seat its load allowance back (`load_buckets`, as `request_as` does for a load refused at once, `:2273–2280`).

**Undoing a go.** `Wall::undo` (`:2830`), beside its load branch (`:2870`): for `What::Go { backup, fingerprint, from, .. }`, send `Here { ticket }` with `Asked::Back { go: seq, backup, fingerprint, back_to: from, made, tries: 0 }`; pending. When `here` comes back (staleness and retries as for a go): read the backup exactly as `undo_load` does (`:3268`; move its reading and checking into one function `read_backup(seq, backup, fingerprint) -> Result<ReadSave, WallError>` that both call, with `undo_load`'s sentences unchanged) and `replace` with `Source::Back { go: seq, kept, to: back_to, from: here }`. The change is a `What::Go` that `undoes` the go: T04a's thread puts `HEAD` back on the line the go left.

**`tree(seat, recent, now)`**: take from the seat's tree bucket or `slow_down` "<seat> has asked for the tree more than 10 times in a few seconds; wait a moment"; send `List { ticket, recent }`; pending. When it comes back, set `revision = self.revision()` and answer the `TreeResult`.

**`pub fn tree_answers(&mut self, now: Instant) -> Vec<(u64, Result<Outcome, WallError>)>`**: take everything in `tree_answered` (T04a fills it as `tick` drains the tree's news), act on each as above; an answer whose ticket is not waiting (answered already by the deadline) is dropped silently; then every `Waiting` older than `TREE_ANSWER_WITHIN` is answered `internal` "the tree did not answer within 5 s (<mark | go | the tree | undo of a go>): nothing was changed, and the wall plays on" (a go's allowance comes back).

**`daemon/control.rs`**: the body of `answer_speech` (`:664`) becomes `fn answer_later(&mut self, answers: Vec<(u64, Result<Outcome, WallError>)>)`, called by `answer_speech` with `self.wall.spoken()` and by a new `answer_tree` with `self.wall.tree_answers(Instant::now())`; `housekeeping` (`:799`) calls `answer_tree` right after `answer_speech` (`:816`).

### `Source`, `replace`, and every place the wall knows a load (`daemon/wall.rs`)

- `Source` (`:786`) gains `Moment { to: TreePlace, from: TreePlace }` and `Back { go: u64, kept: Kept, to: TreePlace, from: TreePlace }`. `how()`: `"go"` and `"undo"` (so the automatic saves are `before-go-<time>` and `before-undo-<time>`). `save()`: `None` for both. `undoes()`: `Back` → `Some(go)`. `kept()`: `Back` → `Some(kept)`; `Moment` → `None` (no save holds the wall from the tree).
- `replace` (`:3420`) builds `What::Go { to, from, backup, fingerprint, modules, cables, tempo: playing, by_console: console }` for those two sources and `What::Load { .. }` for the rest, exactly as now; nothing else in it changes.
- Each of these, which pick out the changes that put up a wall, picks out `What::Go` too, with the same meaning (`| What::Go { .. }` beside `What::Load { .. }`): `log_opens_on` (`:913`), `walls_of` (`:941`, including its undo branch: undoing go G puts up a copy of the wall G replaced), `resumed`'s `known` (`:997`) and `left_out` (`:1008`), `apply`'s memory note (`:2122`), `puts_up_wall` (`:2375`: undoing a go puts up a wall), `replaced_since` (`:3004`, `:3009`), and `prune` (`:3354`: a go's backup is needed as a load's is, by its `by_console`), and wherever KZ-T02a starts a new clock line for a load (a go starts one too: the wall put up is not the music that was playing). Search the file for `What::Load` once more after these edits and say under CHANGED that no other site picks out loads.
- `answer` (`:2381`): `Request::Mark { name } => self.mark((seat, console), name, now)`, `Request::Go { moment } => self.go((seat, console), moment, now)`, `Request::Tree { recent } => self.tree(seat, *recent, now)`.
- `request_as` (`:2195`): `Request::Tree` takes its own bucket there, beside the saves listing's (`:2216–2231`).

### Saves are marked (`save_as`, `:3101`)

After a save by hand is written (seat or console, new or saved over; `save_as` never writes the wall's own `before-…` saves, `keeping` does), send `SaveMark { name: stored, at: <the save's UTC time, as save_info gives it> }`. No answer is awaited; the save's answer is as today. If the job cannot be sent (no tree, or its queue is full), the save stands and its answer's `notes` gain "it is not marked in the tree (<why>)". Reason: a save stays what it is (a named file, loaded by `wall_load`), and its moment is also named in the tree, so the tree view (T08) and `wall_go` can reach it; the tag is a new one each time, so a save written over never moves an old moment's name.

## Not this lane

- The tree thread's side (HEAD, buds, tags, resolution): T04a. The seats' tools (`wall_mark`, `wall_go`, `wall_tree`): T04c. The console's keys and the tree list: T04d.
- A go that glides: T05. Notes on moments: T07.
- Moving or deleting marks: never.

## Red tests first

In `daemon/wall/moments/tests.rs` (declared from `moments.rs`), with a real `Wall` on a `Store::peek` state directory and a tree planted with `tree::git::plant` in a temporary directory and attached as KZ-T02b's tests attach one (they need `git` 2.36 or later), and a wall with no tree for the refusals. Answers that come later are collected by calling `tree_answers` until they come (at most 2 s in a test, polling every 5 ms). Each fails today because there is no `mark`, `go` or `tree` request.

1. `a_go_puts_the_wall_at_the_moment_exactly`: ten changes, go to `#4`: the patch equals `tree::from_files` of change 4's commit's files, births included; the change is a `What::Go` whose `to.change` is 4; `Event::Loaded` is sent; the backup is `before-go-…`.
2. `undoing_a_go_brings_back_the_wall_and_its_place`: go, then undo: the patch is the wall before the go (`fingerprint` equal to before), and `HEAD` is the symbolic `trunk` again; the next change extends `trunk`.
3. `the_first_change_after_a_go_grows_a_bud_and_none_grows_without_one`: go to a mark, turn a knob: `bud/<mark>/<time>` exists with that commit; go, undo, nothing new.
4. `a_seats_goes_share_the_load_allowance_and_unknown_moments_give_it_back`: a seat's three goes in a minute, the fourth `slow_down`; a go to "nowhere" is `unknown_moment` and the allowance comes back; the console is never held.
5. `a_go_answered_after_other_changes_asks_the_tree_again`: a change logged between the `Resolve` and its answer: `Resolve` is sent again and the logged go's `from` is the place after that change; three such in a row: `slow_down`.
6. `a_go_to_where_the_wall_is_changes_nothing`: `not_allowed`, nothing logged.
7. `a_seat_must_look_again_after_another_seats_go`: a turn carrying the old wall's number is `wall_replaced`.
8. `marks_are_answered_once_written_and_told_to_everyone`: `MarkResult` with `at.change`, `Event::Marked` in the outcome; a second mark "DAWN" after "dawn" is `mark_exists`.
9. `a_seat_marks_three_times_a_minute_and_the_console_any_number`.
10. `every_save_by_hand_is_marked_and_the_walls_own_are_not`: a save, a save over it by the console, a load (its `before-load-…` save): two `save/<name>/<time>` tags, none for the automatic save.
11. `the_tree_answers_within_five_seconds_or_the_seat_is_told`: a tree thread held by KZ-T02b's paused test constructor (`TreeHandle::attach_paused`): `internal` with that sentence after the deadline; a late answer is dropped.
12. `a_wall_with_no_tree_says_so`: the three requests are `internal` "this wall keeps no tree…".
13. `watchers_read_the_tree_and_may_not_mark_or_go`: through `refusal` in `control.rs`.
14. `a_go_survives_a_restart_as_a_load_does`: go, close, start a new `Wall` on the state directory and its log: no `What::Resumed` is noted, the wall is the moment's, and the go can still be undone.
15. `a_change_from_before_a_go_cannot_be_undone_after_it`: `not_allowed` with "replaced since (change <go>: <its summary>)".
16. `the_backups_goes_need_are_not_pruned`: 25 goes by a console: their `before-go-…` saves needed by the log stay (`AUTO_SAVES_KEPT` is 20, `store.rs:152`).

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/daemon kazoo-wall/src/protocol kazoo-wall/src/tui/app/mod.rs kazoo-mcp/src/channel.rs
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 16 tests fail first (paste each under RED), then pass.
- [ ] `Request::{Mark, Go, Tree}`, `Event::Marked`, `Source::{Moment, Back}`, `Wall::tree_answers`, `read_backup`, `TREE_ANSWER_WITHIN`, `GO_TRIES`, `TREE_TICKETS`, `ErrorCode::UnknownMoment` and `ErrorCode::MarkExists` exist as named.
- [ ] Files: `protocol/mod.rs`, `daemon/wall.rs`, `daemon/wall/moments.rs`, `daemon/wall/moments/tests.rs`, `daemon/control.rs`, `tui/app/mod.rs` (one arm), `kazoo-mcp/src/channel.rs` (one arm), `kazoo-mcp/src/link.rs` (two names): eight.
- [ ] By hand after the landing reinstall (with T04c or T04d landed), on Tom's wall: mark "first mark", turn a knob, go to "first mark", turn a knob, `git -C ~/.kazoo/tree branch` shows the bud, undo twice. Say what you saw.

## Code quality

As PLAN.md's STACK RULES. `moments.rs` under 1000 lines. `daemon/wall.rs` (4,691 lines) gains only the routing arms, the fields, `Source`'s two variants and arms, the `| What::Go { .. }` additions and the save mark: list its line count under FOUND. Nothing on the control thread waits for the tree thread.

## END

```
START KZ-T04b base=<sha> branch=lane/kz-T04b df=<GB> at <time>
END KZ-T04b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
