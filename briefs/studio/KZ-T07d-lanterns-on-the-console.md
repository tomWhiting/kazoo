# KZ-T07d: on Tom's console, `n` lights a lantern where the wall is, the tree list (`B`) shows the lanterns and notes near the wall and how bright each still is, `n` there adds a note to any moment or lantern, and Enter on a lantern shows its way back

Line numbers are from main at `820ccc5` and may have moved; find each item by what it does, never rename. Items from KZ-T04d, KZ-T07a and KZ-T07b are named as those briefs name them (`TreeList`, `tree_rows`, `TreeRow`, `RowKind`, `tree_key`, `Mode::{Tree, Go}`, `NoteView`, `Visited`, `commands`, `text_problem`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T07b and T04d have landed; the by-hand check is Cassio's and Tom's after landing. If the terminal cannot tell Alt-Enter from Enter (crossterm reports the modifier on Ghostty and Terminal.app; say under FOUND if a test shows otherwise), `Ctrl-J` also puts a line break in the words. Accepted otherwise as written.

Last of PLAN.md's T07 split in four (T07a the tree thread, T07b the wall, T07c the seats' tools, **T07d** the console). The little tree drawn as a tree is T08b, which puts the same lanterns on its branches. Runs beside T07c: they share no file.

Time: about 35 minutes of a builder's work. One mechanism: the console's lantern key and the lantern rows of T04d's tree list. Crates: kazoo-wall (`tui/` only). Depends on T07b and T04d, which must have landed.

## Outcome

In the wall console:
- `n` asks "light a lantern here:" in the prompt line, and Enter lights it on the moment the wall is at. The status line says "lit a lantern at #812 · your spoken words around it are its way back · B lists it". Tom has no Claude conversation to go back to, so the console sends no way; his own spoken words five minutes either side are the lantern's way (T07b sets the window).
- `B` (T04d's tree list) gains a last section, `lanterns & notes near here`: each lantern with its glow (`✦` lit, `✧` dim, `·` faint), who lit it, when, its words cut to the row, its visits; each note with who and when. A moment row (branch, mark, recent) that holds a lantern ends with that lantern's glyph.
- In the list, `n` on any row adds a note: to that row's moment, or, on a lantern's row, to the lantern (its epilogue).
- Enter on a lantern's row visits it and shows it whole: the words, the notes added to it, its visits, how far the wall has moved on since, where Tom's spoken words around it are kept (file and lines), and its way back as the two commands to run in a terminal (`claude -p --resume … --fork-session --resume-session-at …` and `claude --resume … --fork-session`). Enter on a note's row asks to go to its moment, as T04d's Enter does on any moment.
- Another seat's lantern or note is said in the log and on the status line (T07b's `Event::Lit` arm), and the list, when open, is fetched again.

## The design (decided; build exactly this)

### Keys (decided, with reasons)

`n` in normal mode: free in every mode of today's console (`tui/app/keys.rs:90–141`), and claimed by none of the briefs written so far (KZ-L12 `T`, and `G`, `e`, `space`, `h`, `l`, `1`–`8` inside the tape view; KZ-L13 `W`, and `+`, `-` inside the loom; KZ-T04d `b`, `B`; KZ-T08b `Y`). `n` for note: a lantern is a note with a way back. Inside T04d's tree list `n` is free too (it uses `j`, `k`, `r`, Enter, Esc, `q`, `B`); inside T04d's go question `n` already means "stay", and that is untouched.

### Modes (`tui/app/mod.rs`, `Mode`)

```rust
/// Words for a lantern on the moment the wall is at.
Light { text: String },
/// Words for a note on a moment or a lantern, from the tree list's row `index`.
Note { target: NoteTarget, label: String, text: String, index: usize },
/// A visited lantern, shown whole, from the tree list's row `index`.
Lantern { visited: Box<Visited>, index: usize, scroll: u16 },
pub enum NoteTarget { Moment(String), Lantern(String) }
```

T04d's `TreeList` gains `lanterns: Option<NotesResult>`, `fetching_lanterns: bool` and `visiting: Option<usize>` (the row a visit was asked from).

### The rows (`tui/app/tree.rs`, T04d's file)

`tree_rows(result: &TreeResult, lanterns: Option<&NotesResult>) -> Vec<TreeRow>` (both callers updated). After T04d's sections, when `lanterns` is `Some`:
- A header row `lanterns & notes near here (<count>)`, not selectable.
- Each `NoteView` in the listing's order: a lantern: `<glyph> <short id>  #<n>  <seat>  <when>  “<text>”  · <visits> visits`, kind `RowKind::Lantern { id }`, moment the lantern's commit; a note: `✎ #<n>  <seat>  <when>  “<text>”`, kind `RowKind::Note`, moment the note's commit. `<glyph>` by `glow`: `✦` lit, `✧` dim, `·` faint. `<short id>` is the first 6 hex. Words have line breaks made spaces; the row is cut by the drawing, not here. Hand notes: `✎ #<n>  by hand  “<text>”`, kind `RowKind::Note`.
- `and <n> more` when `count` is past the listed, not selectable; an empty listing is one row `no lanterns or notes near here: n lights one`, not selectable.
- Each T04d moment row whose moment (commit) holds a lantern in the listing gains ` <glyph>` of its brightest lantern.
- `RowKind` gains `Lantern { id: String }` and `Note`; `<when>` is as T04d writes it (local `HH:MM`, or the date when not today).

### Handlers (`tui/app/lanterns.rs`, new, `impl App` as `saves.rs` is)

- `open_light(now)`: `Mode::Light { text: String::new() }`.
- `light_key(key, text, now) -> Mode`: typing, Backspace and Esc as T04d's `mark_key` does; `Alt-Enter` (or `Ctrl-J`) puts a line break in the words (a lantern may hold several lines; Enter alone sends). Enter: `tree::lantern::text_problem(&text)` said with the prompt kept, else `Job::Call(Request::Lantern { words: text, way: None })`, status "lighting a lantern…", `Mode::Normal`.
- In T04d's `tree_key`: `n` on a selectable row → `Mode::Note { target, label, text: "", index }`, the target `NoteTarget::Lantern(id)` on a lantern's row, else `NoteTarget::Moment(<the row's moment>)`; `label` the row's label cut at 40 characters. Enter on a `RowKind::Lantern` row → `visiting = Some(index)`, `Job::Call(Request::Visit { lantern: id })`, status "visiting lantern <short id>…", the mode stays `Tree`. Enter on any other row is T04d's go question, unchanged.
- `note_key(key, target, label, text, index, now) -> Mode`: typing as `light_key`; Esc → `Mode::Tree { index }`; Enter: `text_problem`, else `Job::Call(Request::Note { words, moment, lantern })` from the target, status "adding a note to <label>…", `Mode::Tree { index }`.
- `lantern_key(key, visited, index, scroll, now) -> Mode`: `j`/↓ and `k`/↑ scroll; `n` → `Mode::Note { target: NoteTarget::Lantern(id), … }`; Esc, `q` or Enter → `Mode::Tree { index }`.
- Fetching: T04d's `fetch_tree` also sends `Request::Lanterns { near: None, branch: None, every: false }` and sets `fetching_lanterns`; so `B`, `r`, and every refetch T04d makes bring both.
- Answers (`on_result`, `tui/app/mod.rs:1145`, four arms that call into `lanterns.rs`):
  - `Request::Lantern` → `NoteView`: `Tone::Done` "lit a lantern at #<n> · your spoken words around it are its way back · B lists it" (`at <short>` with no change number); the list is fetched again if open.
  - `Request::Note` → `NoteView`: "noted #<n>" or "added to lantern <short id>"; the list is fetched again if open.
  - `Request::Visit` → `Visited`: when the mode is `Tree` and `visiting` is set, `Mode::Lantern { visited, index, scroll: 0 }`; otherwise the status line says "lantern <short id> visited · B, then its row, shows it". `visiting = None` either way.
  - `Request::Lanterns` → `NotesResult`: kept; `fetching_lanterns = false`; the cursor held inside the new rows.
  - A refusal of any of the three writes is said as every refusal is (`on_failure`, `:1306`); a refused `Note` or `Lantern` also reopens its prompt with the typed words (as T04d reopens a taken mark's name), so nothing typed is lost.
- Events: T07b's `Event::Lit` arm also fetches the list again when it is open.

### Drawing

- **The prompts** (`tui/draw/panes.rs`, the prompt line, as T04d's mark prompt at `:700`): `light a lantern here: <text>█` with the help `   ⏎ lights it · Alt-⏎ a new line · your spoken words around it are its way back · Esc cancels`; `note on <label>: <text>█` with `   ⏎ adds it (notes never change and never dim) · Alt-⏎ a new line · Esc cancels`. Line breaks show as `⏎` in the prompt line.
- **The list** (`tui/draw/tree.rs`, T04d's file): the new section and glyphs in the rows' own colours: `✦` in `AMBER`, `✧` in `STEEL`, `·` and `✎` in `TEXT_DIM`, from `tui/theme.rs`. The keys line becomes `j k move · ⏎ go there (on a lantern: visit it) · n note · r again · Esc closes`.
- **The lantern** (`tui/draw/lanterns.rs`, new; dispatched from `tui/draw/mod.rs`'s overlay `match`): a popup the size of T04d's tree popup, titled `lantern`, lines wrapped to its width and scrolled by `scroll`:
  ```
  <glyph> lantern <id> (<glow>) on #<n> <short> · lit by <seat><" at the console"> <date> <time>
  “<words>”                                            (every line of them)
  added: <seat> <when>: “<words>”                       (each added note, oldest first)
  visited <n> times · since it was lit the wall has made <since_lit> changes; since anyone was last here, <since_last>
  Tom's spoken words around it (<from>–<to>), kept by Dot:
    <lines> lines in <path>, seq <first>–<last>          (each `Heard`; or "none are kept now")
  the way back (kazoo never starts it; run it in a terminal):
    ask the self at that moment:
      <ask>
    the whole conversation (it also knows what came after):
      <open>
  ```
  `<ask>` and `<open>` from T07a's `commands(way, resume_at, None)`, so `<ask>` ends in `'<your question>'` for Tom to replace. The way-back block is instead `the way back is gone: Claude Code no longer keeps <transcript>` (`Portal::Gone`) or `lit at the console: there is no conversation to go back to; your words are its way` (`Portal::Words`). The keys line: `j k scroll · n add a note · Esc back`. Commands are drawn whole, wrapped at the popup's edge with no hyphen, in `CREAM` on `PANEL`, so they can be selected and copied with the terminal's mouse.
- `draw/mod.rs`'s overlay `match` (`:92`) gains `Mode::Lantern`; `Mode::Light` and `Mode::Note` join the modes with no overlay (they draw in the prompt line), except that `Mode::Note` keeps T04d's tree popup drawn beneath it, at its `index`.
- Help (`overlay.rs`, after T04d's `B` line): `("n", "light a lantern on the moment the wall is at, with your words; your spoken words around it are its way back (in B: n adds a note to a moment or a lantern, ⏎ on a lantern shows its way back)")`.

### Keys wiring (`tui/app/keys.rs`)

The mode dispatch (`:68–70`) gains `Mode::Light { text } => self.light_key(key, text, now)`, `Mode::Note { target, label, text, index } => self.note_key(key, target, label, text, index, now)` and `Mode::Lantern { visited, index, scroll } => self.lantern_key(key, visited, index, scroll, now)`. Normal mode (`:128–129`) gains `KeyCode::Char('n') => self.open_light(now)`.

## Not this lane

- The tree drawn as a tree, with lanterns on its branches: T08b.
- `:lantern` and `:note` commands: not now (the ten-file rule; `n` reaches both).
- Running a Claude conversation from the console: never; the console shows the command.
- Showing the words Tom spoke: never; the console says where Dot keeps them.

## Red tests first

In `tui/tests/tree.rs` (T04d's file), with the console's test helpers (`press`, `calls`, the `TestBackend` renders). Each fails today because `n` does nothing and the list has no lanterns.

1. `n_asks_for_words_and_enter_lights_a_lantern_here`: `n`, type "first light", Alt-Enter, "again", Enter: exactly `[Request::Lantern { words: "first light\nagain", way: None }]`; empty words are said and nothing is sent.
2. `a_refused_lantern_reopens_the_prompt_with_its_words`.
3. `the_tree_list_shows_lanterns_and_notes_near_here`: `B` sends `Request::Tree { recent: None }` and `Request::Lanterns { near: None, branch: None, every: false }`; with a `NotesResult` of a lit, a dim and a faint lantern, a note and a hand note: the rendered buffer at 120 × 40 holds the header, `✦`, `✧`, `·`, `✎`, `by hand`, `2 visits`, and the moment row of the lit lantern's commit ends with `✦`.
4. `n_in_the_list_notes_a_moment_or_a_lantern`: on a recent row: `Request::Note { moment: Some("#812"), lantern: None, … }`; on a lantern row: `lantern: Some(<id>)`; Esc returns to the list at the same row.
5. `enter_on_a_lantern_visits_it_and_shows_its_way_back`: Enter sends `Request::Visit`; a `Visited` answer opens `Mode::Lantern`; the render holds the words, `added:`, `since it was lit`, the two commands exactly as T07a's `commands` writes them, and the `Heard` path and seqs; `Portal::Gone` and `Portal::Words` draw their sentences instead.
6. `another_seats_lantern_refreshes_the_open_list`: an `Event::Lit` while `Mode::Tree`: `Request::Lanterns` is sent again.
7. `help_lists_n`.

T04d's `capital_b_lists_the_tree_and_enter_then_y_goes` expects `B` to send `Request::Tree` alone; update it to expect `Request::Tree` then `Request::Lanterns`, and say so under CHANGED. No other existing test changes.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tui
```

## Definition of done

- [ ] The 7 tests fail first (paste each under RED), then pass.
- [ ] Files: `tui/app/lanterns.rs`, `tui/app/mod.rs`, `tui/app/keys.rs`, `tui/app/tree.rs`, `tui/draw/lanterns.rs`, `tui/draw/tree.rs`, `tui/draw/mod.rs`, `tui/draw/overlay.rs`, `tui/draw/panes.rs`, `tui/tests/tree.rs`: ten.
- [ ] Not the builder's: after landing and reinstalling, Cassio asks Tom to press `n` in his console ("first lantern from the console"), then `B` (the lantern listed, lit), `n` on it ("added by hand"), Enter on it (its way back: his words), and Esc. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. Colours from `tui/theme.rs` only. `tui/app/keys.rs` and `tui/app/mod.rs` gain only the mode arms, the key, the result arms and the list's fields; everything else lives in `tui/app/lanterns.rs` and `tui/draw/lanterns.rs`. List both line counts under FOUND.

## END

```
START KZ-T07d base=<sha> branch=lane/kz-T07d df=<GB> at <time>
END KZ-T07d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
