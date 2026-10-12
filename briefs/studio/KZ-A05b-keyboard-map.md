# KZ-A05b: cs80, mini and arp play the shared piano from `kazoo_host::keys`

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Split: PLAN.md's A05 is split into A05a (the module; dx, glass, string), A05b (this: cs80, mini, arp) and A05c (juno, prophet). Depends on A05a.

## Outcome

cs80, mini and the arp play the same piano as every other instrument: chromatic home row from `a`, `z`/`x` for the octave, held notes released on an octave move. Their private layouts go: `kazoo-cs80/src/input.rs:15-61` (two-row, with octave), `kazoo-mini/src/input.rs:27-54` (white home row, no octave), `kazoo-arp/src/app.rs:406-424` (one octave from C4, no octave shift).

## The design (decided; build exactly this)

Each crate depends on `kazoo-host` (`Cargo.toml`), and in each the key handler asks `kazoo_host::keys::piano_key(ch, octave)` before its own commands, as KZ-A05a describes for dx.

### kazoo-cs80 (`input.rs`, `app.rs`, `run.rs`)

- `input.rs`: delete `key_to_note` and its tests (`key_mapping_middle_c`, `key_mapping_upper_octave`, the rest at `input.rs:62-102`); keep `DEFAULT_VELOCITY` (`:64`) and whatever else lives there (aftertouch, mouse zones).
- `app.rs`: `pub octave: i8` (`:116`, default 0 at `:158`, `octave_up`/`octave_down` clamped to +-4 at `:331-338`) becomes `pub octave: kazoo_host::keys::Octave` with `Octave::DEFAULT`; `octave_up`/`octave_down` become `self.octave = self.octave.up()` / `.down()` and, first, `self.release_all_notes()` (the function at `run.rs`/`main.rs:190` is on `App`: find it by what it does) so a key held through the move cannot stick; `key_note_map` (`:121`) stays, since it already maps the release to the note that was pressed.
- `run.rs` (`main.rs` at e0de0c0): the `[`/`]` octave arms (`:231-232`) go; the `KeyCode::Char(ch)` arm (`:235-251`) first matches `piano_key(ch, app.octave)`: `Some(OctaveDown)` / `Some(OctaveUp)` call the app's octave methods and also send `commands.all_notes_off()` (the release went to the app; the engine must hear it too); `Some(Note(note))` is today's note-on path (`key_slot`, `key_note_map`, `note_on`, `commands.note_on`); `None` falls through. `handle_key_release` (`:300`) is unchanged. The cs80 panel help, if it names `[` `]` for the octave, says `z` `x`.
- Tests in `run.rs` (`key_press_and_release_send_note_on_and_off` etc.) use `a` instead of `z` for the note where they press a letter; add `octave_moves_release_held_notes_and_change_the_note` as in KZ-A05a (`a` is 48; after `x`, `a` is 60; the held 48 got a note-off and an all-notes-off command).

### kazoo-mini (`input.rs`, `app.rs`, `run.rs`)

- `input.rs`: delete `key_to_midi_note` (`:14-54`) and its tests; keep `handle_key` (`:60`) and the rest. Mini's `q` quit needs Ctrl (`:64`), so nothing else collides.
- `app.rs`: add `pub octave: kazoo_host::keys::Octave` (default `DEFAULT`) next to `held_notes`.
- `run.rs` (`main.rs:174-206` at e0de0c0, `handle_key_event`): on a `Press` with `KeyCode::Char(ch)`, `piano_key(ch, app.octave)` first: `Note(note)` is today's held/`note_on` path; `OctaveDown`/`OctaveUp` call `app.release_all_notes()`, `commands.all_notes_off()`, then move `app.octave`; `None` falls through to Backspace and `input::handle_key`. On a `Release`, `keys::note(ch, app.octave)` finds the note to release; because every held note is released on an octave move, a release after a move finds nothing held and does nothing. The mini's panel draws the octave (`ui/`) next to the keyboard hint if it has one; otherwise add nothing.
- Tests: `note_key_press_and_release` with `a` (48); add the octave test.

### kazoo-arp (`app.rs`)

- `App::key_to_note` (`:404-424`) goes; add `octave: kazoo_host::keys::Octave` to `App` (`:155` `new`, default `DEFAULT`); in `handle_press` (`:265`) the piano arm (`:291-300`) and in `handle_key`'s release arm (`:257-262`) use `piano_key` / `keys::note(ch, self.octave)`; `OctaveDown`/`OctaveUp` release every held note (`self.held` at `:128`: for each held, `release_note`) and move the octave. The arp's `q`, `c` (Ctrl), digits and Space (`:269-277`) are untouched: none is a piano key. The arp's default octave was C4; it is now C3 like everyone's: the arp's display of its pattern reads MIDI numbers, so nothing else changes.
- Tests in `app.rs` that press `z` for C4 (`:517-559`) press `k` (C4 at the default octave) instead; add the octave test.

## Not this lane

A05a, A05c; the engines; A06.

## Red tests first

Per crate, as named above: cs80 `run.rs` `octave_moves_release_held_notes_and_change_the_note`; mini `run.rs` the same; arp `app.rs` `octave_moves_release_held_notes_and_change_the_note`. They are red because `Octave`/`piano_key` are not wired (the field does not exist). Every existing test keeps running with its note letters moved to the shared layout (cs80 120, mini 64, arp 79 at e0de0c0, minus the deleted layout tests).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-cs80 -p kazoo-mini -p kazoo-arp
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-cs80 -p kazoo-mini -p kazoo-arp --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-cs80 kazoo-mini kazoo-arp
```

## Definition of done

- [ ] `grep -rn 'fn key_to_note\|fn key_to_midi_note' kazoo-cs80 kazoo-mini kazoo-arp` finds nothing.
- [ ] Each of the three sounds C3 on `a` and C4 on `a` after `x`, and a note held through `x` stops (run each for ten seconds; say what you heard, or that no device was present).
- [ ] nextest counts per crate; every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: the piano is asked before any instrument command, in every handler, so the three behave alike.

## END

```
START KZ-A05b base=<sha> branch=lane/kz-a05b df=<GB> at <time>
END KZ-A05b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
