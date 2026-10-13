# KZ-T04d: on Tom's console, `b` marks the moment, `B` opens the tree (where the wall is, the branches, the marks, the latest moments), and Enter then `y` puts the wall at the chosen moment

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Last of PLAN.md's T04 split in four (T04a the tree thread, T04b the wall, T04c the seats' tools, **T04d** the console's keys). The little tree drawn as a tree is T08; this is the list a go is chosen from.

Read by Cassio, 13 Oct 22:3x. Accepted as written.

Time: about 35 minutes of a builder's work. One mechanism: the console's tree list and its two keys. Crates: kazoo-wall (`tui/` only). Depends: T04b (landed). Runs beside T04c: they share no file.

## Outcome

In the wall console:
- `b` asks "mark this moment as:" (like `w` asks for a save's name), and Enter marks it. The status line says "marked “dawn” (after change 812) · B shows the tree". A name taken is said plainly, and the prompt stays open with the name so it can be changed.
- `B` opens the tree list: where the wall is, then the branches, the marks (marks and saves' marks), and the latest moments on the wall's line, each one row. `j`/`k` (or ↑/↓) move, Enter on a row asks "go to <row>? the wall as it is is kept first; z undoes it · y goes · n stays", and `y` sends the go. `r` asks for the tree again. `Esc` or `B` closes it.
- `:mark <name>`, `:go <moment>` and `:tree` do the same from the command line.
- A go's answer is shown as a load's is: the wall is forgotten and looked at afresh, and the status line says where the wall went and that `z` undoes it.

## The design (decided; build exactly this)

### Keys (decided, with reasons)

`b` and `B`: free in every mode of today's console (`tui/app/keys.rs:90–134`), and not claimed by the briefs written so far (KZ-L12 takes `T`, `G` and `e` inside the tape view; KZ-L13 takes `W`). `b` for bookmark, which is what Tom called a mark ("come back to a bookmark"); `B` for branches. The console's existing pairs work the same way (`w` saves and `o` lists the saves; here `b` marks and `B` lists the tree).

### Modes (`tui/app/mod.rs`, `Mode`, `:255–293`)

```rust
/// Naming a mark for the moment the wall is at.
Mark { text: String },
/// The tree list.
Tree { index: usize },
/// Asking before the wall goes to a moment.
Go { moment: String, label: String, index: usize },
```

`App` gains `tree: TreeList { result: Option<TreeResult>, fetching: bool }` beside `saves` (`:426`).

### The rows of the list (`tui/app/tree.rs`, new)

`pub(super) fn tree_rows(result: &TreeResult) -> Vec<TreeRow>`, with `TreeRow { label: String, moment: String, kind: RowKind }` in this order:
1. Where the wall is (not selectable): `here: on <branch> at #<n> <short>` or `here: at #<n> <short>, off any line — the next change grows a bud`, then `the tree is <n> changes behind the wall` when `through < revision`.
2. Each branch: label `<name>  #<n>  <at as local HH:MM, or the date when not today>  <seat>`, moment `<name>`.
3. Each mark: label `“<name>”  #<n>  …` (a save's mark: `save “<name>”`), moment the mark's name (a save's mark: its `tag`).
4. Each recent moment on the wall's line: label `#<n> <short>  <seat>: <summary>`, moment `#<n>` (or the 40-digit commit when it has no change number).
Lists cut at `MAX_LISTED` end with an `and <n> more` row (not selectable). `index` counts selectable rows only.

### Handlers (`tui/app/tree.rs`)

- `open_mark(now)`: `Mode::Mark { text: String::new() }`.
- `mark_key(key, text, now) -> Mode`: typing, Backspace, Esc as `save_key` does (`tui/app/saves.rs:132`); Enter with `mark_name_problem(&text)` (`valid_save_name`, `protocol/mod.rs:102`: "a mark's name is 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a letter or digit"; nothing is reserved, unlike saves' `before-`) said and the prompt kept, else `Job::Call(Request::Mark { name })`, status "marking “<name>”…", `Mode::Normal`.
- `open_tree(now)`: `Mode::Tree { index: 0 }` and `fetch_tree` (`Job::Call(Request::Tree { recent: None })`, `fetching = true`, as `fetch_saves`, `saves.rs:93`).
- `tree_key(key, index, now) -> Mode`: `j`/↓, `k`/↑ move within the selectable rows; `r` fetches again; Enter → `Mode::Go { moment, label, index }`; Esc, `q` or `B` → `Mode::Normal`.
- `go_key(key, moment, label, index, now) -> Mode`: `y` → `Job::Call(Request::Go { moment })`, status "going to <label> (the wall as it is is kept first)…", `Mode::Normal`; `n` or Esc → `Mode::Tree { index }`; any other key leaves the question up (as `confirm_load`, `saves.rs:260`).
- Answers (`on_result`, `tui/app/mod.rs:1145`, three arms that call into `tree.rs`):
  - `Request::Mark` → `MarkResult`: `Tone::Done` "marked “<name>” (after change <n>) · B shows the tree" (`(at <short>)` when `at.change` is `None`); the list is fetched again if open.
  - `Request::Go` → `LoadResult`: as `loaded` does for a load (`saves.rs:357`: log the change, forget the wall, notes to the log), with the status "went to <place> · the wall as it was is saved as “<backup>” · z undoes it", `<place>` as T04a's `went` words it (read from the change's `What::Go`'s `to`).
  - `Request::Tree` → `TreeResult`: kept; `fetching = false`; the cursor held inside the new rows.
  - A refusal of `Request::Mark` with `mark_exists` reopens `Mode::Mark` with the typed name and says the wall's sentence; any other refusal is said as every refusal is (`on_failure`, `:1306`).
- Events: `Event::Marked` (T04b gave it an arm) and `Event::Loaded` fetch the tree again when the list is open. `z` needs nothing: undoing a go is an undo like any other (its answer parses as a `ChangeResult`, `:1166–1176`).

### Commands (`tui/app/command.rs`, the table at `:49–53`)

`("mark", "<name>", "name the moment the wall is at (marks never move)")`, `("go", "<moment>", "put the wall at a moment: a mark, a branch, #<change> or a commit (z undoes it)")`, `("tree", "", "the tree: branches, marks, the latest moments")`. In `run_command` (`:468`): `mark` checks the name as `mark_key` does and sends `Request::Mark`; `go` sends `Request::Go { moment: rest }` without asking (typing the command is the asking, as `:load` does, `:531`); `tree` opens the list. Completions for `go` offer the branches' and marks' names from the last `TreeResult` (as `save | load` offer saves, `:274`).

### Drawing (`tui/draw/tree.rs`, new; dispatched from `tui/draw/mod.rs`)

- `pub fn tree(buf, area, app, index)` as `overlay::saves` (`tui/draw/overlay.rs:609`) draws the saves: a popup the same size, titled `tree`, the here row first, section headers `branches`, `marks`, `recent`, the selected row in the selection colour from `theme.rs`, the keys line `j k move · ⏎ go there · r again · Esc closes`. "asking the wall…" while `fetching` with no result yet; "the tree has no moments yet: the first change makes one" for an empty tree.
- `pub fn confirm_go(buf, area, label)` uses `overlay`'s `question` (`overlay.rs:901`, make it `pub(super)`): title `go`, lines `go to <label>?`, `the wall as it is is kept first; z undoes it`, keys `y goes · n stays`.
- `draw/mod.rs`'s overlay `match` (`:92`) gains `Mode::Tree` and `Mode::Go`; `Mode::Mark` joins the modes with no overlay (`:104`).
- The mark prompt is drawn in the prompt line as the save's is (`tui/draw/panes.rs:700`): `mark this moment as: <text>█`, help `   1 to 64 of A-Z a-z 0-9 space _ . - · ⏎ marks it (marks never move) · Esc cancels`. The other new modes join that `match`'s no-prompt arm.
- Help (`overlay.rs`, after the `o` line, `:119–122`): `("b", "mark the moment the wall is at under a name (marks never move)")` and `("B", "the tree: where the wall is, branches, marks, the latest moments; ⏎ then y puts the wall at one (z undoes it)")`.

### Keys wiring (`tui/app/keys.rs`)

The mode dispatch (`:68–70`) gains `Mode::Mark { text } => self.mark_key(key, text, now)`, `Mode::Tree { index } => self.tree_key(key, index, now)`, `Mode::Go { moment, label, index } => self.go_key(key, moment, label, index, now)`. Normal mode (`:128–129`) gains `KeyCode::Char('b') => self.open_mark(now)` and `KeyCode::Char('B') => self.open_tree(now)`.

## Not this lane

The tree drawn as a tree, thickness, lanterns: T08. A go that glides: T05. The wall's side: T04b.

## Red tests first

In `tui/tests/tree.rs` (`mod tree;` in `tui/tests/mod.rs`), with the console's test helpers (`press`, `calls`, the `TestBackend` renders, as `tui/tests/saves.rs`). Each fails today because `b` and `B` do nothing.

1. `b_asks_for_a_name_and_enter_marks_the_moment`: `b`, type "dawn", Enter: exactly `[Request::Mark { name: "dawn" }]`; a bad name ("bad/name") is said and nothing is sent.
2. `a_mark_name_taken_reopens_the_prompt_with_it`: a `mark_exists` failure: `Mode::Mark { text: "dawn" }` and the wall's sentence on the status line.
3. `capital_b_lists_the_tree_and_enter_then_y_goes`: `B` sends `Request::Tree { recent: None }`; with a `TreeResult` of 2 branches, 3 marks and 4 recent, `j j j`, Enter shows the question for the third selectable row, `y` sends `Request::Go { moment }` with that row's moment; `n` returns to the list at the same row.
4. `the_tree_list_draws_here_branches_marks_and_recent`: the rendered buffer at 120 × 40 holds the here line, the three headers, the rows' labels, and `and 5 more` for a cut list; the empty-tree line for an empty tree.
5. `a_go_answer_forgets_the_wall_and_says_where_it_went`: a `LoadResult` whose change is a `What::Go` to mark "dawn": the snapshot is dropped (as after a load), the status line holds `went to mark "dawn"` and `z undoes it`.
6. `the_commands_mark_go_and_tree`: `:mark dawn`, `:go #812`, `:tree` give the requests above; `:go` completes a mark's name from the last tree.
7. `help_lists_b_and_capital_b`.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tui
```

## Definition of done

- [ ] The 7 tests fail first (paste each under RED), then pass.
- [ ] Files: `tui/app/tree.rs`, `tui/app/mod.rs`, `tui/app/keys.rs`, `tui/app/command.rs`, `tui/draw/tree.rs`, `tui/draw/mod.rs`, `tui/draw/overlay.rs`, `tui/draw/panes.rs`, `tui/tests/tree.rs`, `tui/tests/mod.rs`: ten.
- [ ] By hand after the landing reinstall, in Tom's console: `b` "first light", turn a knob, `B`, choose "first light", `y`, turn a knob, `B` again (a bud is listed with "grew from first light"), `z` twice. Say what you saw.

## Code quality

As PLAN.md's STACK RULES. Colours from `tui/theme.rs` only. `tui/app/keys.rs` (1,217 lines) and `tui/app/mod.rs` (1,644) gain only the mode arms, the two keys, the result arms and the `tree` field; everything else lives in `tui/app/tree.rs` and `tui/draw/tree.rs`. List both line counts under FOUND.

## END

```
START KZ-T04d base=<sha> branch=lane/kz-T04d df=<GB> at <time>
END KZ-T04d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
