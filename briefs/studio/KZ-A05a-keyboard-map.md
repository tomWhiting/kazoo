# KZ-A05a: one typing-keyboard piano in `kazoo_host::keys` (layout, octave, the macOS repeat probe), used by dx, glass and string

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Split: PLAN.md's A05 ("one keyboard map") is split into A05a (the shared module plus the three crates that already use this layout), A05b (cs80, mini, arp) and A05c (juno, prophet). b and c depend on a; they share no files with each other. Depends on A04 (the `kazoo-host` crate exists; if it is not there when this lane starts, create `kazoo-host` as KZ-A04a describes: a library crate with `crossterm = "0.29.0"`, `ratatui = "0.30.0"`, `color-eyre = "0.6.5"`, `[lints] workspace = true`, added to the workspace members).

## Outcome

Every kazoo instrument plays the same typing-keyboard piano: the home row is the white keys and the row above is the black keys, `a` is C, `;` is the E an octave and a third up, `z` and `x` move the octave down and up, and every held note is released when the octave moves so nothing sticks. Today there are five incompatible layouts (`kazoo-dx/src/main.rs:627`, `kazoo-juno/src/input.rs:7`, `kazoo-cs80/src/input.rs:15`, `kazoo-arp/src/app.rs:406`, `kazoo-mini/src/input.rs:27`) and three copies of the macOS key-repeat probe (`kazoo-dx/src/main.rs:598`, `kazoo-glass/src/main.rs:581`, `kazoo-string/src/main.rs:567`). After this lane there is one of each, in `kazoo_host::keys`, and dx, glass and string use it.

## The design (decided; build exactly this)

### Why this layout

The chromatic home row (`a w s e d f t g y h u j k o l p ;`) is Ableton Live's computer keyboard, it is what the three newest instruments already use, and it leaves `z x c v b n m q r i` and the digits free for commands. The two-row layout (`z s x d c … / q 2 w 3 e …`) used by cs80, juno, prophet and arp takes nearly every letter, and dx, glass and string already bind `z x c v n` to octave, velocity and the phrase. Mini's layout (white keys only on the home row) cannot play a chromatic line. So: the chromatic home row, one octave shift on `z`/`x` (dx's own keys, `main.rs:740-746`), default octave 3 (`a` is C3, MIDI 48: three of the five layouts start there, and the bass instruments want it; two octaves up is two presses).

### `kazoo-host/src/keys.rs` (new; `pub mod keys;` in `kazoo-host/src/lib.rs`)

```rust
/// The typing keyboard as a piano. The home row is the white keys and the
/// row above the black keys: `a` is C, then a semitone per key up to `;`
/// (seventeen keys, an octave and a third). Case does not matter.
#[must_use]
pub fn semitone(key: char) -> Option<u8>;          // the table at kazoo-dx/src/main.rs:627-648, on key.to_ascii_lowercase()

/// The key that moves the piano an octave down, and the one that moves it up.
pub const OCTAVE_DOWN: char = 'z';
pub const OCTAVE_UP: char = 'x';

/// Which octave the `a` key plays: the MIDI octave number, 0 (C0 = 12) to 8
/// (C8 = 108; the `;` key then plays 124, still a MIDI note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Octave(i8);
impl Octave {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(8);
    /// `a` is C3 (MIDI 48).
    pub const DEFAULT: Self = Self(3);
    /// An octave held to 0..=8.
    #[must_use] pub const fn new(number: i8) -> Self;
    #[must_use] pub const fn number(self) -> i8;
    /// One octave down, stopping at MIN.
    #[must_use] pub const fn down(self) -> Self;
    /// One octave up, stopping at MAX.
    #[must_use] pub const fn up(self) -> Self;
}
impl Default for Octave { DEFAULT }

/// The MIDI note `key` plays at `octave`: `None` for a key that is not a
/// piano key. (Octave 8 keeps every key under 128, so no key is ever out of range.)
#[must_use]
pub fn note(key: char, octave: Octave) -> Option<u8>;   // (octave + 1) * 12 + semitone, as kazoo-dx/src/main.rs:429

/// What a key does for the piano: a note, an octave move, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PianoKey { Note(u8), OctaveDown, OctaveUp }
/// Classify a key press for the piano at `octave`. Instruments ask this
/// first, before their own commands, so the piano wins every key it uses.
#[must_use]
pub fn piano_key(key: char, octave: Octave) -> Option<PianoKey>;

/// The OS key-repeat delay (macOS `defaults read -g InitialKeyRepeat`, in
/// units of 15 ms, held to 100 ms..=2 s). The error says why it could not be
/// read, for a status line. (kazoo-dx/src/main.rs:598-623, moved.)
pub fn system_repeat_delay() -> Result<Duration, String>;
pub fn parse_initial_key_repeat(text: &str) -> Result<Duration, String>;
/// What to assume when the setting cannot be read.
pub const DEFAULT_REPEAT_DELAY: Duration = Duration::from_millis(250);
```

`semitone`, `note`, `Octave` and `piano_key` are pure; the probe runs a process and is never called on an audio thread.

### Callers in this lane: kazoo-dx, kazoo-glass, kazoo-string (their `run.rs` after A01; at e0de0c0 their `main.rs`)

In each:

- Delete `piano_offset`, `system_repeat_delay`, `parse_initial_key_repeat`, `DEFAULT_REPEAT_DELAY` (dx `main.rs:39, 598-648`; glass `:45, 581-631`; string `:~44, 567-617`) and the tests that tested them (`piano_maps_an_octave_and_a_third`, `key_repeat_setting_parses`, `out_of_range_notes_play_nothing`: they move to `kazoo-host` as the red tests below).
- `App.octave: i8` (dx `:267`, default 4 at `:327`) becomes `octave: kazoo_host::keys::Octave` with `Octave::DEFAULT`; the note calculation at `:429` becomes `keys::note(key, self.octave)`; the `z`/`x` arms at `:740-747` become `self.octave = self.octave.down()` / `.up()` after `release_all` (keep the release); `handle_key` at `:691-700` uses `keys::piano_key(c, app.octave)`: `Note` presses/releases/repeats as today, `OctaveDown`/`OctaveUp` on a press call the octave arms (so the two explicit arms go).
- The repeat probe: where `system_repeat_delay()` was called (`App::new`, find it by the call), call `kazoo_host::keys::system_repeat_delay()` and `DEFAULT_REPEAT_DELAY` from the host.
- Add `kazoo-host = { path = "../kazoo-host" }` to each crate's `Cargo.toml` (dx and string have no `lib.rs` dependency on it yet; glass neither).

The default octave moves from C4 to C3 for these three: say so in each crate's `USAGE` last line is not needed; the panel shows the octave already (dx `ui.rs` draws `app.octave`: it now draws `octave.number()`).

## Not this lane

A05b (cs80, mini, arp) and A05c (juno, prophet); the held-key auto-release logic (`HeldKey`, `REPEAT_HOLD`) stays per crate; velocity keys stay per crate; A04's terminal guard.

## Red tests first

`kazoo-host/src/keys.rs` tests:

- `the_home_row_is_a_chromatic_octave_and_a_third`: `semitone('a') == Some(0)`, `'w'` 1, `'s'` 2, `'e'` 3, `'d'` 4, `'f'` 5, `'t'` 6, `'g'` 7, `'y'` 8, `'h'` 9, `'u'` 10, `'j'` 11, `'k'` 12, `'o'` 13, `'l'` 14, `'p'` 15, `';'` 16; `'A'` 0; `'q'`, `'z'`, `' '`, `'1'` None.
- `the_default_octave_puts_a_on_c3`: `note('a', Octave::DEFAULT) == Some(48)`, `note(';', Octave::DEFAULT) == Some(64)`.
- `octaves_stop_at_the_ends_and_every_key_stays_a_midi_note`: `Octave::MIN.down() == Octave::MIN`, `Octave::MAX.up() == Octave::MAX`, `Octave::new(9) == Octave::MAX`, `Octave::new(-1) == Octave::MIN`; for every octave 0..=8 and every piano key, `note` is `Some(n)` with `n <= 127`; `note('a', Octave::MAX) == Some(108)`.
- `z_and_x_move_the_octave_and_are_never_notes`: `piano_key('z', Octave::DEFAULT) == Some(PianoKey::OctaveDown)`, `'x'` `OctaveUp`, `'a'` `Note(48)`, `'q'` None.
- `key_repeat_setting_parses` (from dx `main.rs:981`): `"25"` is 375 ms, `"1"` is held to 100 ms, `"1000"` to 2 s, `"abc"` is `Err` naming the text.

Per instrument (dx, glass, string `run.rs` tests): `octave_moves_release_held_notes_and_change_the_note`: press `a` (note 48 on), press `x`: a note-off for 48 is sent and `app.octave == Octave::new(4)`; press `a` again: note 60 on.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-host -p kazoo-dx -p kazoo-glass -p kazoo-string
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-host -p kazoo-dx -p kazoo-glass -p kazoo-string --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-host kazoo-dx kazoo-glass kazoo-string
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-host
```

## Definition of done

- [ ] `kazoo_host::keys::{semitone, note, piano_key, PianoKey, Octave, OCTAVE_DOWN, OCTAVE_UP, system_repeat_delay, parse_initial_key_repeat, DEFAULT_REPEAT_DELAY}` exist as above, every item documented.
- [ ] `grep -rn 'InitialKeyRepeat\|fn piano_offset' kazoo-dx kazoo-glass kazoo-string` finds nothing; `grep -rln 'InitialKeyRepeat' kazoo-*/src` lists only `kazoo-host/src/keys.rs`.
- [ ] Run `cargo run -p kazoo-dx --release` for ten seconds: `a` sounds C3, `x` then `a` sounds C4, a note held through `x` stops (say what you heard, or that no device was present).
- [ ] nextest counts per crate (dx 53 - 3 + 1 at least, glass 54 - 3 + 1, string 81 - 3 + 1, host at least 5 new); every check green with its exit code.

## Code quality

The tree's rules: `unsafe_code` is forbidden; no `#[allow]` or `#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; nothing allocates, locks, formats or does I/O on an audio callback; every audio path survives NaN and Inf; pre-allocate; red test first, then green, one behaviour per commit; files under 1000 lines; no TODO, no deferred work, no stubs; clippy all + pedantic + nursery clean under `-D warnings`; `cargo fmt`; a doc comment on every public item; no emoji. Specific: `const fn` wherever clippy asks; `Octave`'s inner number is private so an out-of-range octave cannot be built.

## END

```
START KZ-A05a base=<sha> branch=lane/kz-a05a df=<GB> at <time>
END KZ-A05a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
