# KZ-V05: the quantiser: snap the voice to a scale, at a chosen speed, keeping the vibrato if asked, with hysteresis and a blend back to the direct pitch

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core. Time: about 30 minutes. Depends: KZ-V04 (`VoiceFrame`, `voice/mod.rs`). May run beside KZ-V06 (no shared files: this lane adds `voice/quantise.rs` and one `pub mod` line).

Why: Tom: "capture like auto tune kind of things ... When I say auto-tune, obviously, I mean like quantization ... but also to be able to just drive it direct, like, hertz for hertz." The quantiser is its own stage between the voice frame and everything that plays it: the mouth writes `snapped_hz`/`snapped_volts` into each frame before it plays or publishes it, so every follower and the wall get the same tuning, and the direct pitch rides beside it untouched.

## Outcome

`Quantiser::process(&mut frame)` writes the snapped pitch into a `VoiceFrame`. Off (blend 0, the default) it equals the direct pitch exactly: hertz for hertz. On: nearest note of a scale on a root, reached at a snap speed from instant (0 ms) to a slow pull (400 ms), optionally leaving the vibrato on top of the corrected centre, holding a note through 30 cents of wavering past the midpoint, and blendable anywhere between direct and snapped.

## The design (decided; build exactly this)

### `kazoo-core/src/voice/quantise.rs` (`pub mod quantise;` in `voice/mod.rs`), tests in `voice/quantise/tests.rs`

```rust
/// The scales, as the semitones above the root each holds. The same ten, in
/// the same order and with the same names, as the wall's `quant`
/// (kazoo-wall/src/catalogue.rs:850-875); V10 makes the wall use these.
pub const SCALES: [&[u8]; 10] = [ /* copied exactly from catalogue.rs:850-861 */ ];
pub const SCALE_NAMES: [&str; 10] = [ /* "chromatic", "major", "minor", "dorian", "phrygian",
    "lydian", "mixolydian", "pentatonic major", "pentatonic minor", "whole tone" */ ];
pub const NOTE_NAMES: [&str; 12] = [ "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B" ];

/// The wall's rule (kazoo-wall/src/dsp/quant.rs:27-48), moved here as the one
/// copy: `volts` to the nearest degree of `scale` on `root` (semitones above
/// C), the next octave's root a candidate, ties to the lower note, input
/// clamped to ±8 octaves. Returns volts.
#[must_use] pub fn nearest(volts: f32, scale: &[u8], root: f32) -> f32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantiseSettings {
    pub scale: usize,          // index into SCALES, held to 0..=9
    pub root: u8,              // 0..=11, 0 = C
    pub snap_ms: f32,          // 0..=SNAP_MS_MAX
    pub keep_vibrato: bool,
    pub hysteresis_cents: f32, // 0..=HYSTERESIS_MAX
    pub blend: f32,            // 0..=1: 0 = direct, 1 = fully snapped
}
impl Default for QuantiseSettings { /* chromatic, C, 30 ms, keep_vibrato true, 30 cents, blend 0.0 */ }
impl QuantiseSettings { #[must_use] pub fn held(self) -> Self; /* every field clamped; NaN → default */ }

pub struct Quantiser { /* rate, settings, centre filter, target, correction, … */ }
impl Quantiser {
    pub fn new(rate: u32) -> Self;
    pub fn set(&mut self, settings: QuantiseSettings); // `held()` first; callback-safe (Copy)
    pub const fn settings(&self) -> QuantiseSettings;
    /// Write `snapped_hz` / `snapped_volts` from `volts`, `gate`, `onset`, `voiced`.
    pub fn process(&mut self, frame: &mut VoiceFrame);
    pub fn reset(&mut self);
}
```

### Constants (with reasons as doc comments)

| Name | Value | Reason |
|---|---|---|
| `SNAP_MS_MAX` | 400 | the slow end of "snap speed": a 400 ms time constant lets a whole sung slide through and only pulls held notes in, as the slowest retune speeds of commercial pitch correctors do; 0 is instant |
| default `snap_ms` | 30 | corrects inside a syllable's first consonant-to-vowel transition while leaving deliberate slides audible |
| `HYSTERESIS_MAX`, default | 50, 30 cents | a held note must drift 30 cents past the midpoint toward the next note before the target moves: above the 10 to 20 cents a held sung note wavers, well under a semitone |
| `CENTRE_HZ` | 2.5 | keep-vibrato finds the note's centre with two cascaded one-pole low-passes at 2.5 Hz: a 6 Hz vibrato of ±50 cents leaks only ±7 cents into the centre (0.385² ≈ 0.15) |
| `NEW_NOTE_CENTS`, `NEW_NOTE_FRAMES` | 150, 2 | the centre snaps to the input when the input is more than 150 cents from it for 2 frames in a row: a new note, not vibrato (vibrato peaks at about 100 cents) — so the slow centre filter never lags a jump |

All time constants are applied per frame: coefficient `exp(−VOICE_HOP / (rate · t))`; `snap_ms == 0` means coefficient 0 (instant).

### `process`, per frame

Work in semitones from C4: `s = 12 · frame.volts`.

1. **Unvoiced** (`!frame.voiced`): leave the state as it is; `snapped_*` = the last snapped values (0 before any).
2. **Centre.** If `keep_vibrato`: update the two-pole centre filter toward `s`; if `|s − centre| > NEW_NOTE_CENTS/100` for `NEW_NOTE_FRAMES` frames, set both filter stages to `s`. If not `keep_vibrato`: `centre = s`.
3. **Target.** `n = 12 · nearest(centre / 12, SCALES[scale], root)`. With no target yet, `target = n`. Else, if `n ≠ target` and `|centre − target| − |centre − n| > hysteresis_cents / 100`, `target = n`.
4. **Correction.** `goal = target − centre`. If `frame.onset` (a new note): `correction = goal` at once and the centre filter set to `s` (a new note starts in tune; the snap glide is for wandering within a note, never for arriving at it). Else `correction` moves toward `goal` with the `snap_ms` one-pole.
5. **Output.** `snapped = s + blend · correction` semitones; `snapped_volts = snapped / 12`; `snapped_hz = hz_from_volts(snapped_volts)`. Because the correction is added to `s` (not to `centre`), the vibrato (`s − centre`) rides on top when keep-vibrato is on; with it off, `s == centre` and the output sits on the note.
6. Every value checked finite; anything not finite resets the quantiser and writes the direct pitch.

`blend == 0` writes exactly `frame.volts` (bit for bit: `s + 0·c` is `s`; assert it in a test) — the direct, hertz-for-hertz path.

## Not this lane

Where settings are changed from (the mouth's screen, V08b). The wall's use of the shared tables (V10).

## Red tests first

`voice/quantise/tests.rs`, at 48 kHz, frames built by a helper from a pitch curve (one frame per 64 samples). Red for all: no module.

- `off_is_hertz_for_hertz`: default settings, 1,000 frames of a random walk in pitch: `snapped_volts.to_bits() == volts.to_bits()` every frame.
- `chromatic_snaps_a_flat_note`: steady A4 − 30 cents, snap 0, blend 1, keep_vibrato off: every frame after the first is A4 (440 Hz) within 0.01 cents.
- `snap_speed_is_a_time_constant`: steady A4 − 40 cents arriving without an onset after the target is set, snap 100 ms: the correction reaches 63% ± 3% of 40 cents at 100 ms (75 frames) and 95% ± 2% at 300 ms.
- `keep_vibrato_keeps_the_wobble`: 6 Hz vibrato ±50 cents around A4 + 20 cents, snap 0, blend 1, keep on: over the second second, the output's mean is A4 within 4 cents and its peak-to-peak is 100 ± 20 cents; keep off with ±30 cents vibrato around A4 + 20: the output stays on A4 within 1 cent (no vibrato, and no switch to A#: 50 − 50 = 0 is not past the hysteresis).
- `hysteresis_holds_then_lets_go`: coming from A4, a centre at A4 + 55 cents stays on A4; at A4 + 66 it moves to A#4 (66 − 34 = 32 > 30); coming back down, A4 + 45 stays on A#4.
- `scale_and_root_follow_the_wall_rule`: `nearest` against a table of 24 pinned cases computed from the wall's rule (`kazoo-wall/src/dsp/quant.rs:27-48`), including D major (`root 2`) turning C#4 + 10 cents into C#4, F4 (a tie between E and F#) into E4, a whole-tone tie, and ±9 octaves clamped to ±8.
- `blend_mixes_direct_and_snapped`: A4 − 30 cents, snap 0, blend 0.5: A4 − 15 cents within 0.1.
- `a_new_note_starts_in_tune`: snap 400 ms; a note at E4 − 30 cents with `onset` on its first frame: that first frame is E4 within 0.1 cents.
- `unvoiced_frames_hold`: voiced A4, then 50 unvoiced frames with a garbage `volts`: snapped stays A4.
- `settings_are_held_to_their_ranges`: scale 12 → 9, root 14 → 11, snap −5 → 0 and 900 → 400, blend 2 → 1, NaN → default.
- `processing_never_allocates`: 10,000 frames with settings changes between them inside `assert_no_alloc`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/voice
```

Doc tests: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`.

## Definition of done

- [ ] All red tests fail first (paste under RED), then pass; every existing test passes.
- [ ] `SCALES` and `SCALE_NAMES` are byte-for-byte the wall's (paste `diff` of the two arrays' text).
- [ ] `blend == 0` is bit-identical to the direct pitch (the first test).

## Code quality

As PLAN.md's STACK RULES. `process` and `set` run on the audio callback: no allocation, lock, I/O, format or panic. Settings are `Copy` and reach the callback by value (V08a sends them over its command channel). No `as` casts clippy pedantic flags.

## END

```
START KZ-V05 base=<sha> branch=lane/kz-V05 df=<GB> at <time>
END KZ-V05 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
