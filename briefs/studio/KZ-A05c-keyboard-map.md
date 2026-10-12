# KZ-A05c: juno and prophet play the shared piano from `kazoo_host::keys`; their byte-identical `input.rs` files go

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Split: PLAN.md's A05 is split into A05a, A05b and A05c (see KZ-A05a). This is c = kazoo-juno and kazoo-prophet. Depends on A05a.

## Outcome

Juno and prophet play the same piano as every other instrument (chromatic home row from `a`, `z`/`x` octave, held notes released on a move). `kazoo-juno/src/input.rs` and `kazoo-prophet/src/input.rs` (36 lines, byte-identical, `cmp` says so) are deleted. The `h j k l` parameter aliases in juno (`kazoo-juno/src/main.rs:325-328`) go: `h`, `j`, `k`, `l` are piano keys and were already shadowed by the two-row layout's `h j l` notes; the arrows keep doing the work.

## The design (decided; build exactly this)

Both crates depend on `kazoo-host`. In both, `lib.rs` drops `mod input;` and `input.rs` is deleted.

### kazoo-juno (`lib.rs`, `app.rs`, `run.rs`, delete `input.rs`)

- `app.rs`: add `pub octave: kazoo_host::keys::Octave` (default `DEFAULT`) to `App` (fields at `:96-126`); `key_note_map: [Option<u8>; 128]` (`:96`) stays: it maps the pressed key to the note it sounded, so a release after an octave move still finds the right note (that is exactly what cs80 does, and why juno's release path at `main.rs:346-352` is already correct).
- `run.rs` (`main.rs` at e0de0c0): in `handle_key_press` (`:292-335`), after `is_quit_key`, a `KeyCode::Char(ch)` is classified with `kazoo_host::keys::piano_key(ch, app.octave)`: `Note(note)` is today's path from `:303-320` (the `key_index` repeat guard, velocity 1.0 with Shift else 0.82, `send_command(NoteOn)`, `key_note_map`, `add_held_note`); `OctaveDown`/`OctaveUp` send `AudioCommand::AllNotesOff` and, when accepted, clear `key_note_map` and `held_notes` (the Space arm at `:329-332` does exactly this: share the code) and then move `app.octave`; `None` falls to the `match code` with the `h j k l` aliases removed (`:325-328` keep only the arrow arms). The release path (`:340-355`) is unchanged.
- The panel (`ui.rs`) shows the octave number next to the keyboard hint it already draws (find the hint; if there is none, draw `oct 3` in the footer).
- Tests (`:382-479`): `press` helpers that use `z` for C3 use `a`; add `octave_moves_release_held_notes_and_change_the_note` (press `a`: NoteOn 48; press `x`: AllNotesOff, maps cleared, `app.octave == Octave::new(4)`; press `a`: NoteOn 60).

### kazoo-prophet (`lib.rs`, `app.rs`, `run.rs`, delete `input.rs`)

The same, from `kazoo-prophet/src/main.rs` (its `handle_key_press` checks quit first, `:298`, then returns without the alias arms). Tests at `:364-473`.

## Not this lane

A05a, A05b; the engines; A06.

## Red tests first

`kazoo-juno/src/run.rs` and `kazoo-prophet/src/run.rs`: `octave_moves_release_held_notes_and_change_the_note` as above; `the_piano_is_the_shared_one`: pressing `;` sends NoteOn 64 and pressing `q` sends nothing (under the old layout `q` was C4 and `;` was D#4). Red because the handlers still call `input::key_to_note`.

Every existing test keeps running (juno 22, prophet 24 at e0de0c0).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-juno -p kazoo-prophet
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-juno -p kazoo-prophet --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-juno kazoo-prophet
```

## Definition of done

- [ ] `kazoo-juno/src/input.rs` and `kazoo-prophet/src/input.rs` do not exist; `grep -rn 'key_to_note' kazoo-juno kazoo-prophet` finds nothing.
- [ ] Both sound C3 on `a`, C4 on `a` after `x`, and a note held through `x` stops (ten seconds each; say what you heard, or that no device was present).
- [ ] nextest counts; every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: juno and prophet stay the same shape as each other (diff their `run.rs` files; only names and juno's `key_note_map` lines should differ, as today).

## END

```
START KZ-A05c base=<sha> branch=lane/kz-a05c df=<GB> at <time>
END KZ-A05c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
