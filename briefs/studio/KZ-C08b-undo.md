# KZ-C08b: every change made on the desk can be taken back with `u` and put back with `U`, a fader ride as one step, with the words of what was undone in the footer

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

This is the second half of KZ-C08 (session save, load, undo); KZ-C08a saves and loads, this half undoes. Between the two files every outcome of the plan's C08 row is delivered.

Time: about 30 minutes of a builder's work. One mechanism: a command log with inverses on the UI thread. Crates: kazoo-mix only. Depends: KZ-C08a.

## Outcome

A musician moves a fader, puts a plate on a group, splits a clip, changes the tempo, then presses `u` four times and watches each come back in order, the footer saying `undid: tempo 128 to 120`, `undid: split 808-001`, and so on; `U` redoes. A fader ride of thirty nudges in a second is one undo. Arming, mute, solo, every knob, every insert edit, every clip edit, the loop points and the tempo are in the log; what the transport is doing (play, stop, position) and what the instruments send are not, since those are performance, not editing. The log is kept in memory for the session and survives autosave; it is not in the file.

## The design (decided; build exactly this)

**Edits.** `kazoo-mix/src/undo.rs` (new). Every change the desk makes to its model goes through one place, `Desk::apply(&mut self, edit: Edit) -> Result<(), Refused>`, which performs it and pushes it on the log; the key handlers (`handle_key`, `desk.rs:348`; `handle_mouse`, `desk.rs:405`; the inserts panel, C03; the take list, C06; the timeline keys, C07b) build `Edit`s and call `apply` instead of writing `SharedState` or sending commands themselves. `Edit` is a closed enum with its inverse derivable from the model at the moment it is applied:

```
enum Edit {
    Control  { target: Target, control: AnyControl, from: f32, to: f32 },     // strip knobs, bus knobs, master
    Toggle   { target: Target, which: Toggle, from: bool, to: bool },        // mute, solo, arm, signal
    Bus      { slot: usize, from: Option<u8>, to: Option<u8> },
    Forget   { slot: usize, strip: StripSave },                              // inverse: Reserve + restore (C08a's save shapes)
    Insert   { target: Target, change: InsertChange },                       // Put{slot, save}, Take{slot, save}, Move{from,to}, Bypass{slot,from,to}, Param{slot,index,from,to}
    Clip     { lane: Target, change: ClipChange },                           // Place{save}, Remove{save}, Move{id,from,to}, Trim{id,from,to: (start,end,beat)}, Split{id,at,new_id}, Join{..}, Mute{id,from,to}, Gain{id,from,to}
    Tempo    { from: f32, to: f32 },
    Loop     { from: Option<(f64, f64, bool)>, to: Option<(f64, f64, bool)> },
    Click    { from: bool, to: bool },
}
```

`Edit::inverse(&self) -> Edit` swaps `from` and `to` (a `Put` inverts to a `Take` carrying the same `InsertSave`; a `Split` inverts to a `Join { left, right }` which the engine applies as remove-right-and-extend-left, allocation-free since both halves share the audio; a `Forget` inverts to a `Reserve` plus the strip's controls and inserts restored). `Edit::describe(&self, undo: bool) -> String` gives the footer line in the wall's style (`kazoo-wall/src/change.rs:42`, `describe`): `fader 808 -3.0 dB to -6.0 dB`, `put Plate reverb on G2 slot 1`, `split 808-001 at 9.1`.

**Applying.** `apply` performs the edit through the same paths the keys use today (controls to `SharedState`, inserts through the `InsertBay`, clips through the `TakeBay`, the hub through `HubCommand`), then pushes it. An edit the engine refuses (a bay `Refused` arriving in `tick`) is popped from the log and its model change reverted, with the reason in the footer: the log never holds a change that did not happen. For bay-carried edits the push waits for the `Done`: `pending: VecDeque<Edit>` holds them until their event arrives, in order, and `u` during a pending edit is a footer note `wait for the desk`.

**The log.** `UndoLog { done: Vec<Edit>, undone: Vec<Edit>, cap: 1000 }`: `push` clears `undone` and drops the oldest when over the cap; `undo` pops `done`, applies the inverse through `apply_silent` (performs without logging), pushes the original onto `undone`; `redo` the reverse. Coalescing: a `Control` or `Param` edit on the same target and control within `COALESCE_MS = 300` of the last pushed one replaces it, keeping the first `from` and the new `to`, so a ride is one step; a `Toggle` never coalesces. Mouse drags coalesce by the same rule (the drag's many `apply_fader` calls, `desk.rs:698`). `u` undoes, `U` redoes, on the desk and on the timeline; the inserts panel's and take list's `u` also reach the log (the panel keys fall through for them). The footer shows `undid: ...` or `redid: ...` for 4 s; `nothing to undo` when empty. The header shows the log's depth as `u12` in `TEXT_DIM` when non-zero.

**What is not logged.** Play, stop, record, locate, zoom, scroll, bank, focus, help, clip lights, instruments plugging in and out, and takes placed automatically at the end of a recording (C06): that placement is a consequence of recording, and undoing it would hide a take; `d` on the clip is logged as usual.

**Autosave and the log.** Undo and redo mark the session dirty (C08a) like any change. The log is not saved; a reopened session starts with an empty log, stated in the module doc.

## Not this lane

- KZ-C08a owns the save shapes (`StripSave`, `InsertSave`, `TakeSave`) the log reuses; KZ-C10b adds automation edits to the enum in its own lane.
- No change to the engine except `TakeCommand::Join` (the inverse of a split), added here with its test.

## Red tests first

Log tests (`undo.rs`, pure, no engine): `undo_and_redo_walk_the_log_in_order`, `a_new_edit_after_undo_clears_the_redo_stack`, `rides_on_one_control_coalesce_within_the_window_and_not_across_it` (fake clock), `toggles_never_coalesce`, `the_log_drops_the_oldest_past_its_cap`, `every_edit_inverts_to_one_that_inverts_back` (for a sample of each variant: `inverse(inverse(e)) == e`), `every_edit_describes_itself_in_words` (one assertion per variant on `describe`).

Desk tests (`desk.rs` tests, with the bays' callback ends serviced by hand on a `MixerEngine`): `u_takes_back_a_fader_move_and_U_puts_it_back` (`SharedState` values), `a_mouse_drag_is_one_undo`, `undoing_a_put_takes_the_effect_out_and_redo_puts_the_same_settings_back` (the `Put` on redo carries the params the effect had), `undoing_a_split_joins_the_halves_and_the_audio_is_unchanged` (engine output before the split equals after undo, sample for sample), `undoing_a_forget_brings_the_strip_back_parked_with_its_controls` (the hub end sees `Reserve`), `a_refused_edit_is_not_in_the_log` (inject a `Refused` through the bay; `u` says nothing to undo), `transport_and_focus_are_not_in_the_log`, `undo_marks_the_session_dirty`.

Engine test (`engine.rs` tests): `join_undoes_a_split_exactly`.

UI tests: `the_footer_says_what_was_undone` and `the_header_shows_the_log_depth`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green; every key handler that changes the model goes through `apply` (grep for `store_channel_controls`, `store_master_controls`, `InsertModel::`, `TakeModel::` outside `apply`: none in `desk.rs`, `ui/`, except reads).
- [ ] The sequence in the outcome, by hand: fader, insert, split, tempo; four `u`, four `U`; a thirty-nudge ride is one step.
- [ ] Footer words for every variant; header depth; refused edits never logged.
- [ ] `undo.rs` module doc lists what is and is not logged and the cap.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: the log lives on the UI thread; undo reaches the engine only through the bays and `SharedState`, as the keys do; `Join` on the callback is field writes and one `swap_remove` on the reserved `Vec`, with the removed half's `Arc`s handed back in the event; nothing is ever freed on the callback. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C08b base=<sha> branch=lane/kz-c08b df=<GB> at <time>
END KZ-C08b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
