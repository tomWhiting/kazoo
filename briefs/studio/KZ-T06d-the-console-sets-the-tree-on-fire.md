# KZ-T06d: on Tom's console, `F` on any row of the tree list sets the tree on fire from there (with `[` and `]` for speed), a badge says the tree is burning and how far it has got, and `F` again puts it out

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Items from T04d, T05c and T06b are named as those briefs name them (`Mode::Tree`, `Mode::Go`, `tree_key`, `go_key`, `TreeRow`, `tree_rows`, `confirm_go`, `question`, `cross_beats`, `CROSS_LADDER`, `Request::{Fire, FireOut}`, `FireResult`, `FireOutResult`, `Event::Fire`, `FireNews`, `What::Fired`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Last of PLAN.md's T06 split in four (T06a the tree's side, T06b the wall burns, T06c the seats' `wall_fire`, **T06d** the console's `F`). Runs beside T06c: they share no file.

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T06b has landed; red test 1 names the row by where the cursor stands after `j j j`, not by a count; the by-hand check is Cassio's with Tom after landing, never the builder's. Accepted otherwise as written.

Time: about 40 minutes of a builder's work. One mechanism: the console's fire question, its key and its badge. Crates: kazoo-wall (`tui/` only). Depends on T06b, which must have landed before this lane opens.

Files (ten):
1. `kazoo-wall/src/tui/app/fire.rs` (new): `FireLook`, the handlers, the answers' and the news' words.
2. `kazoo-wall/src/tui/app/mod.rs`: `Mode::{Fire, FireOut}`; `App::{fire, fire_speed}`; the result arms; `on_event`'s `Event::Fire` arm (T06b's) calls into `fire.rs`; `mod fire;`.
3. `kazoo-wall/src/tui/app/keys.rs`: `F` in normal mode; the two modes' dispatch.
4. `kazoo-wall/src/tui/app/tree.rs`: `F` on a row of the tree list.
5. `kazoo-wall/src/tui/app/command.rs`: `:fire`, `:douse`, `:firespeed`.
6. `kazoo-wall/src/tui/draw/mod.rs`: the overlay arms for the two modes.
7. `kazoo-wall/src/tui/draw/tree.rs`: `confirm_fire`, `confirm_douse`; the tree list's keys line.
8. `kazoo-wall/src/tui/draw/overlay.rs`: the help line.
9. `kazoo-wall/src/tui/draw/panes.rs`: the `FIRE` badge.
10. `kazoo-wall/src/tui/tests/tree.rs`: the tests (T04d's file; no new test module, so `tui/tests/mod.rs` is untouched).

## Outcome

In the wall console:
- In the tree list (`B`, T04d), `F` on a row asks: `set the tree on fire from “dawn”? along its line at 1× · [ ] slower or faster · the wall goes there first (z undoes it) · y lights it · n stays`. On a branch's row it reads `burn trunk from its first moment?`. `y` lights it.
- While it burns, the header shows ` FIRE 2× 37/214 ` and the status line says what each moment played was, as the log pane lists each one. `F` asks `put the fire out? the wall stays where it reached · y puts it out · n lets it burn`.
- When anyone lights it, puts it out, or it reaches the tip, the status line says so, and the badge goes.
- `:fire <moment>` lights it at the console's speed, `:douse` puts it out, `:firespeed 4` sets the speed.

## The design (decided; build exactly this)

### Keys (decided, with reasons)

`F`: free in every mode of today's console (`tui/app/keys.rs:90–134`; `f` is the log, `:101`) and unclaimed by the briefs written so far (L12 takes `T`, `G`, `e` in the tape view; L13 `W`; T04d `b`, `B`). F for fire. Outside the tree list it needs a moment, which the tree list gives, so `F` in normal mode with no fire opens the tree list (as `B`) with the status "choose a moment and press F to set the tree on fire there"; with a fire, it asks to put it out. `[` and `]` for speed, as they are glide and crossing length elsewhere (`keys.rs:123–124`, T05c). `:douse` and not `:fire out`, because "out" is a valid mark name.

### State and modes (`tui/app/mod.rs`, `tui/app/fire.rs`)

- `App` gains `fire: Option<FireLook>` and `fire_speed: f64` (starting at `DEFAULT_FIRE_SPEED`).
- `pub(super) struct FireLook { pub from: String, pub along: String, pub speed: f64, pub played: u32, pub moments: u32 }`, set by `Event::Fire` `Lit` (and by the console's own `FireResult`), `played` raised by each `Event::Change` whose `what` is `What::Fired`, and cleared by `Out` and `Done`.
- `Mode::Fire { moment: String, label: String, index: usize, speed: f64 }` and `Mode::FireOut`.
- `FIRE_LADDER: [f64; 9] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0]`: doubling, as tempo is felt.

### Handlers (`tui/app/fire.rs`)

- `open_fire(now)` (normal `F`): with `self.fire` set → `Mode::FireOut`; else T04d's `open_tree(now)` and the status above.
- From `tree_key` (T04d, `tui/app/tree.rs`): `F` on a selectable row → `Mode::Fire { moment: row.moment, label, index, speed: self.fire_speed }`, `label` being `“<mark>”`, `#<n> <short>`, or, for a branch row, `<branch> from its first moment`.
- `fire_key(key, …) -> Mode`: `]` and `[` step `speed` along `FIRE_LADDER` (a value between rungs, from `:firespeed`, goes to the nearest rung that way) and `fire_speed` follows; `y` → `Job::Call(Request::Fire { from: moment, along: None, speed: Some(speed) })`, status "lighting the fire at <label>…", `Mode::Normal`; `n` or Esc → `Mode::Tree { index }`; any other key leaves the question up.
- `douse_key(key, now) -> Mode`: `y` → `Job::Call(Request::FireOut)`, status "putting the fire out…"; `n` or Esc → `Mode::Normal`.
- Answers (`on_result`, `tui/app/mod.rs:1145`, two arms calling into `fire.rs`):
  - `Request::Fire` → `FireResult`: `fire = Some(FireLook { … })`; when `go` is `Some`, it is handled exactly as T04d (with T05c's words) handles a go's answer (the wall forgotten and looked at afresh, the log noted); the status "the tree is on fire from <place> along <along>: <moments> moments at <speed>× · F puts it out".
  - `Request::FireOut` → `FireOutResult`: `fire = None`, status "the fire is out at <place> after <played> moments · the next change grows a bud from there".
  - A refusal is said as every refusal is (`on_failure`, `:1306`).
- News (`on_event`'s `Event::Fire` arm, which T06b wrote as a log note, now calls `fire::news`): `Lit` by another seat sets `fire` and says "<seat> set <along> on fire from <place> at <speed>×"; `Out` clears it and says "the fire went out at <place> after <played> moments: <why>"; `Done` clears it and says "the fire reached the tip of <along> after <played> moments · the wall is on <along>". Each is a log note and the status line (`Tone`, `tui/app/mod.rs:145`): `Info` for `Lit` and `Out`, `Done` for `Done`, and `Trouble` for an `Out` whose `cause` (T06b's `FireOut`) is `Tree`, `Log`, `Guard` or `Broken`. The console's own lighting is said by its answer, not again by its news.
- `Event::Change` with `What::Fired`: `played += 1` and the status "playing <short> again: <said>" (the change's own words, through the console's existing cleaning). A step that `replaces` is handled as a load (T06b's arm, unchanged).

### Commands (`tui/app/command.rs`, the table at `:49–53`)

`("fire", "<moment>", "set the tree on fire from a moment, at the fire speed (now <n>×)")`, `("douse", "", "put the fire out where it has reached")`, `("firespeed", "<speed>", "how fast a fire you light burns: 0.25 to 64 (1 is as it was played)")`. In `run_command` (`:468`): `fire <rest>` sends `Request::Fire { from: rest, along: None, speed: Some(fire_speed) }` without asking (typing it is the asking, as `:go`); `douse` sends `Request::FireOut`; `firespeed` with a number sets `fire_speed` held to 0.25..=64 and says "a fire now burns at <n>×", else says "a fire's speed is a number from 0.25 to 64" and changes nothing. `fire` completes marks' and branches' names from the last `TreeResult`, as `go` does (T04d).

### Drawing

- `tui/draw/tree.rs`: `pub fn confirm_fire(buf, area, label, speed, branch_row: bool)` with T04d's `question` (`overlay.rs:901`, `pub(super)` since T04d): title `fire`; lines `set the tree on fire from <label>?` (or `burn <label>?` for a branch row), `along its line at <speed>× · [ ] slower or faster`, `the wall goes there first (z undoes that); any change puts the fire out`, keys `y lights it · n stays`. `pub fn confirm_douse(buf, area, look: &FireLook)`: title `fire`; `put the fire out at <played> of <moments>?`, `the wall stays where it reached; the next change grows a bud there`, keys `y puts it out · n lets it burn`. The tree list's keys line (T04d) becomes `j k move · ⏎ go there · F burn from here · r again · Esc closes`.
- `tui/draw/mod.rs`'s overlay `match` (T04d's arms, `:92`) gains `Mode::Fire { .. }` and `Mode::FireOut`.
- `tui/draw/panes.rs`, beside KZ-T02b's tree badge (`:97`): ` FIRE <speed>× <played>/<moments> ` in `bold(INK, AMBER)` (`theme.rs:24`) while `app.fire` is set; nothing otherwise. Amber, not red: red is the tree's and the log's trouble (` TREE STOPPED `); a fire is wanted.
- Help (`overlay.rs`, after T04d's `B` line): `("F", "in the tree list, set the tree on fire from the chosen moment ([ ] speed); while it burns, put it out")`.

## Not this lane

The wall's side (T06b). `wall_fire` (T06c). The tree drawn as a tree, with the fire on it (T08, T09).

## Red tests first

In `tui/tests/tree.rs` (T04d's), with the console's test helpers (`press`, `calls`, the `TestBackend` renders, as `tui/tests/saves.rs`). Each fails today because `F` does nothing.

1. `f_on_a_row_asks_and_y_lights_the_fire`: `B`, a `TreeResult` (2 branches, 3 marks, 4 recent), `j j j`, `F`: `Mode::Fire` for the selectable row the cursor stands on after `j j j` (as T04d's list moves it), at 1×; `]` `]` gives 4×; `y` sends exactly `[Request::Fire { from: <that row's moment>, along: None, speed: Some(4.0) }]`; `n` from the question returns to the list at the same row.
2. `f_on_a_branch_row_burns_it_from_its_first_moment`: the question for a branch row reads "burn trunk from its first moment?" and `y` sends `from: "trunk"`.
3. `the_badge_counts_the_moments_played`: a `FireResult` of 214 moments at 2×, then three `Event::Change` with `What::Fired`: the rendered header at 120 × 40 holds ` FIRE 2× 3/214 `; after `Event::Fire` `Done`, no badge, and the status says the tip.
4. `f_while_it_burns_asks_and_y_puts_it_out`: with a fire, `F` shows the question; `y` sends `[Request::FireOut]`; the `FireOutResult` clears the badge and says where the wall is.
5. `another_seats_fire_is_shown`: `Event::Fire` `Lit` by "Cassio": badge and status; `Out` with a `why`: the status holds it.
6. `the_commands_fire_douse_and_firespeed`: `:firespeed 8`, `:fire dawn` sends `speed: Some(8.0)`; `:douse` sends `FireOut`; `:firespeed soon` changes nothing and says so; `:fire` completes a mark's name from the last tree.
7. `help_lists_f`.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tui
```

## Definition of done

- [ ] The 7 tests fail first (paste each under RED), then pass.
- [ ] `FireLook`, `FIRE_LADDER`, `App::{fire, fire_speed}`, `Mode::{Fire, FireOut}`, `confirm_fire`, `confirm_douse` and the three commands exist as named.
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, in Tom's console with Tom there: `B`, a mark of the evening, `F`, `]`, `y`; watch the badge count for half a minute; turn a knob (the fire goes out, the status says why); `B` shows the new bud; `z` undoes the turn. Say what was seen and heard.

## Code quality

As PLAN.md's STACK RULES. Colours from `tui/theme.rs` only. `tui/app/keys.rs` and `tui/app/mod.rs` gain only the key, the mode arms, the fields and the result arms; everything else lives in `tui/app/fire.rs` and `tui/draw/tree.rs`. List both line counts under FOUND.

## END

```
START KZ-T06d base=<sha> branch=lane/kz-T06d df=<GB> at <time>
END KZ-T06d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
