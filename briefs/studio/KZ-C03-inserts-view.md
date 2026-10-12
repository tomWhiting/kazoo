# KZ-C03: from any strip or group, open its inserts, add an effect by its catalogue name, remove, reorder, bypass and turn its knobs, with the keyboard and the mouse

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. One mechanism: an inserts panel on the desk driving KZ-C02's `InsertBay`. Crates: kazoo-mix only. Depends: KZ-C02.

## Outcome

A musician focuses a strip, presses `i`, and the master section gives way to that strip's rack: eight slots, each naming its effect as the wall names it (`Tape delay`, `Plate reverb`, `Compressor`) or `empty`. `a` opens a picker of every kazoo-fx effect by family (drive, time, space, lofi) with its one-sentence description; `Enter` puts it in the focused slot; `d` takes it out; `J` and `K` move it down and up; `b` bypasses it (drawn dim, still there); `j`/`k` walk its knobs, `+`/`-`, `}`/`{` and `0` turn and reset them with the value in its unit, exactly as the strip's own knobs behave; the mouse clicks a slot, turns a knob with the wheel, toggles bypass. `Esc` or `i` closes the panel and the master section comes back. Nothing a musician does here can leave the desk in doubt: a refused command shows its reason in the footer in the engine's words.

## The design (decided; build exactly this)

**Panel placement.** The inserts panel replaces the master section's rectangle (`MASTER_WIDTH = 34` columns at the right, `kazoo-mix/src/ui.rs:35`, `draw_master` at `:658`) while open, so the strips keep their place and the bank arithmetic in `draw_desk` (`ui.rs:135`) is unchanged. Reason: a side panel is how kazoo-tui exposes effects (`kazoo-tui/src/ui/effects.rs`, reached by Tab), and the desk is already laid out as strips plus one right-hand section; swapping that section keeps every hit region of the strips valid mid-edit. The panel is 34 columns; at `MIN_WIDTH` (`ui.rs:38`) it fits by construction.

**Desk state** (`kazoo-mix/src/desk.rs:257` `Desk`). New `inserts: Option<InsertsPanel>` where `InsertsPanel { target: Target, slot: usize, knob: usize, picker: Option<Picker> }` and `Picker { family: usize, row: usize }`. `Target` is C02's (`inserts.rs`). The desk also owns the `InsertModel` (C02) and the desk end of the `InsertBay`, both handed to `Desk::new` by `main.rs` (`run_desk`, `main.rs:108`); the desk drains `InsertEvent`s in `tick` (`desk.rs:312`), applying `Done` to the model and turning a `Refused` into `footer_note: Option<(String, Instant)>` shown for 6 s in `AMBER`, dropping any returned Box there (off the audio thread).

**Keys** (`handle_key`, `desk.rs:348`). Resolution order as kazoo-tui's: panel keys first when the panel is open, then the desk's. Opening: `i` with focus on a strip or group opens the panel for that target, slot 0, knob 0; `i` on the master does nothing until KZ-C09. Panel keys:

- `Esc`, `i`: close (picker open: `Esc` closes the picker only).
- `j`/`k`, `↓`/`↑`: next/previous knob within the focused slot; past the last knob moves to the next slot's first knob; `Tab`/`Shift-Tab`: next/previous slot.
- `1`..`8`: focus that slot (digits are strip jumps on the desk; inside the panel they are slot jumps, stated in help).
- `a`: open the picker; in the picker `h`/`l`/`←`/`→` change family, `j`/`k` move, `Enter` puts (`InsertModel::put` after `(kind.build)()` and `prepare`), `Esc` cancels. A put into an occupied slot first `Take`s it (two commands, model applies both on their `Done`s).
- `d`: take the focused slot's effect.
- `J`/`K`: move the focused slot down/up (`Move`), focus follows it.
- `b`: toggle bypass.
- `+`/`-`: fine step; `}`/`{`, `PgUp`/`PgDn`: coarse; `0`: the knob's default (`ParamSpec::default`, `kazoo-fx/src/lib.rs:65`).
- Every other key falls through to the desk (so space, `c`, `t`, `[`, `]`, `m`, `s`, `?` and quit keep working with the panel open).

**Knob steps** (`inserts::knob_step`, pure, tested). From `ParamSpec` (`lib.rs:56`): `Linear`: fine = range/100, coarse = range/10; `Log`: fine multiplies by 2^(1/12) (a semitone, so a frequency knob walks musically), coarse by 2^(1/2); `Stepped`: fine and coarse both move one step. The value is clamped by `ParamSpec::clamp` (`lib.rs:76`) and shown by `inserts::format_value(spec, value)`: stepped shows its label (`Curve::Stepped { labels }`, `lib.rs:47`); `Hz` shows `1.2k` above 1000; `s` shows `ms` below 1; `dB` shows one decimal with sign; `%` whole; empty unit two decimals.

**Mouse.** New `HitTarget`s (`desk.rs:41`): `InsertSlot { slot }` (click focuses; double-click is not a thing in crossterm, so a click on a focused empty slot opens the picker), `InsertKnob { slot, index }` (click focuses, wheel turns fine, as `desk.rs:465` does for strip knobs), `InsertBypass { slot }`, `InsertClose`, `PickerRow { row }`, `PickerFamily { family }`.

**Drawing** (`ui.rs`, new `draw_inserts(frame, rect, desk, shared)` called from `draw_desk` in place of `draw_master` when `desk.inserts.is_some()`). Block with `BorderType::Double` in `BRASS`, title ` INSERTS · 03 808 ` or ` INSERTS · G2 `. Rows: for each of the 8 slots one header line `1 Tape delay` (or `1 empty` in `TEXT_DIM`; bypassed in `TEXT_DIM` with `byp` at the right; focused in `BRASS` bold), and for the focused slot its knobs below it, one per line as `draw_knob_row` draws a strip knob (`ui.rs:292`: a 3-letter label, the pointer glyph from `knob_glyph`, the value text), label being the param name cut to 8 characters. When the knobs do not fit the height, the knob list scrolls so the focused knob is visible. Footer line inside the panel: `a add  d take  J K move  b bypass  i close`. The picker is drawn over the panel's inner rect: a family tab line `drive  time  space  lofi` with the current family in `BRASS`, then one row per `EffectKind` in that family (`kazoo_fx::drive::KINDS`, `time::KINDS`, `space::KINDS`, `lofi::KINDS`: `kazoo-fx/src/drive/mod.rs:32`, `time/mod.rs:34`, `space/mod.rs:31`, `lofi/mod.rs:26`) as `Plate reverb`, with the focused kind's `description` (`lib.rs:97`) word-wrapped under the list in `TEXT_DIM`. Names and descriptions come straight from `EffectKind`; the wall's `wall_catalogue` reads the same table, so the names match by construction.

**Strip head badge.** A strip or group with any insert shows `fx` after its status (`draw_strip_head`, `ui.rs:446`), `fx·byp` when every insert is bypassed.

**Help** (`ui.rs:1417`): add `i` (`inserts of the focused strip or group`) and one line for the panel's keys.

**Footer** (`footer_detail`, `ui.rs:1323`): with the panel open the detail is the focused knob: `808 · 2 Tape delay · time 375 ms (3/8)`; a refusal note takes the detail's place while it lasts.

## Not this lane

- KZ-C02 owns the engine, the bay and the model; if a command the view needs is missing, it is a FOUND, not an addition here.
- KZ-C09's master chain opens the same panel on the master; the `Target::Master` branch is left unbound here.
- KZ-C08 saves and KZ-C10 automates; nothing here persists.
- kazoo-fx is not edited.

## Red tests first

Desk tests (`desk.rs` tests at `:734`, `key()` at `:740`, `mouse()` at `:749`), with a test bay whose callback end is serviced by hand on a `MixerEngine` in the test (as `patchbay.rs:254` does):

- `i_opens_the_inserts_panel_for_the_focused_strip_and_closes_it`: focus strip 2, `i`: `inserts == Some(Strip(2))`; `i` again: `None`; `Esc` with the panel open closes it and does not quit.
- `a_enter_puts_the_picked_effect_and_the_model_shows_it_after_done`: `a`, `l` (time), `Enter`: a `Put` arrives at the callback end with `kind.id == "tape"`; before servicing, the model slot is still empty; after servicing and `tick`, the model shows `tape` with its defaults.
- `d_takes_the_effect_and_the_box_is_dropped_on_the_desk_thread`: after a put, `d`: `Take`; serviced; `tick` drains `Taken` and the model slot is empty.
- `J_and_K_move_the_focused_insert_and_focus_follows`.
- `b_bypasses_and_un_bypasses`.
- `plus_minus_and_zero_turn_the_focused_knob_within_its_range_and_in_its_curve`: on `tape`'s `time` (log): 12 fine steps double it; `0` restores the default; a stepped knob moves one label per step and never between labels.
- `a_refusal_reaches_the_footer_in_the_engines_words`: inject `Refused { why: TooMuchLatency { frames: 9000 } }` through the bay; `desk.footer_note` contains "latency" and "9000".
- `desk_keys_still_work_with_the_panel_open`: space toggles play; `m` mutes the focused strip.
- `mouse_clicks_a_slot_turns_a_knob_and_closes`: with hits pushed by hand (as `desk.rs:922` does).

UI tests (`ui.rs` tests at `:1556`, `render_with` at `:1581`, `text` at `:1622`):

- `the_inserts_panel_replaces_the_master_section_and_names_the_effects`: open on a strip with `plate` in slot 1 and slot 2 bypassed: text contains `INSERTS`, `Plate reverb`, `empty`, `byp`, and not `MASTER`.
- `the_picker_lists_every_family_and_every_kind_by_its_catalogue_name`: for each family, every `EffectKind.name` in its `KINDS` appears when that family is shown.
- `a_strip_with_inserts_shows_fx_in_its_head`.
- `the_panel_renders_at_every_size_without_panicking`: the loop of `renders_at_many_sizes_without_panicking` (`ui.rs` tests) run with the panel and the picker open.
- `knob_values_read_in_their_units`: `format_value` table: `1200 Hz` is `1.2k`, `0.375 s` is `375 ms`, `-6 dB` is `-6.0 dB`, a stepped `model` at 1 is `fet` (`kazoo-fx/src/drive/comp.rs:118`).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green.
- [ ] From a strip and from a group: open, add from every family, take, move, bypass, turn every knob kind (linear, log, stepped), close; by keyboard and by mouse.
- [ ] Every refusal shows in the footer; no command is ever sent twice for one key.
- [ ] Help and footer updated; `fx` badge on strips and groups.
- [ ] `ui.rs` stays under 1000 lines: the panel goes in `kazoo-mix/src/ui/inserts.rs` (make `ui.rs` a `ui/mod.rs` if that is the clean way; keep every public item where it is).

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: the panel touches the callback only through the bay; `(kind.build)()` and `prepare` run on the UI thread; every Box the callback hands back is dropped in `Desk::tick`; the UI thread never blocks on the bay (a full ring is a footer note "desk busy, try again", never a spin). Strings are built on the UI thread only. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C03 base=<sha> branch=lane/kz-c03 df=<GB> at <time>
END KZ-C03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
