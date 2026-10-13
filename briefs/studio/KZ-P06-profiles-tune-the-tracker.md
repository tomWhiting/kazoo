# KZ-P06: the cycle tracker and the voice tracker take a profile, voice, guitar, bass or line, that sets their range and times; voice stays Track V's numbers exactly, and a swept note on guitar or bass is a new onset

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 6, the first of the tracking lanes. It reuses V03's `CycleTracker` and V04's `VoiceTracker`, judge and `VoiceFrame` as they are, and makes their constants a settings value. Nothing is copied.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: the cents per frame for a slide, a bend and a vibrato; the DC blockers' loss at B0 (−0.5 dB at 10 Hz, −2.9 dB at 30 Hz); the 4096-sample history for bass; the 60 ms refractory time against a 75 ms sweep; P01's four ranges. Change: test 7's reason now gives this take's own slide rate (2.3 cents a frame), not the 12-semitone figure.

Time: about 40 minutes. Crates: kazoo-core.

Depends:
- KZ-V04 (the voice frame, the judge and the fusion), and so V03, V02 and V01, must have landed before this lane opens.
- KZ-P01 (`InputProfile`, `range_hz`) must have landed before this lane opens.
- If KZ-V05 or KZ-V06 is in flight (they read V04's types), they must have landed first, so this lane's change of constants into settings is the last edit to those files.

Files (8): `kazoo-core/src/analysis/cycle.rs`, `voice/tracker.rs`, `voice/gate.rs`, `voice/judge.rs` (the judge's frame length per profile), `voice/profile.rs` (new), `voice/profile/tests.rs` (new), `analysis/score/instrument.rs` (new), and the `voice_score` example (one row group).

Why: Tom wants voice, guitar and bass each to drive the synths, with "some specialized tuning for guitar and bass". Track V tuned everything for a voice: 50 Hz to 4 kHz (V01), a 30 Hz DC blocker (V03 `DC_HZ`), a 30 ms peak release (`PEAK_RELEASE_MS`), and a judge window of 2048 at 48 kHz. A 5-string bass's B0 is 30.87 Hz: under the range, cut by the DC blocker, and longer than the peak follower's release (a 32.4 ms period). His sweep picking is several notes a second on different strings that ring into each other, and V04 rightly does not retrigger on a legato step, so a sweep would play as one long glide. Tom also said nothing may stick to notes: slides, bends and vibrato must pass.

## Outcome

`CycleTracker::with_settings(rate, CycleSettings)` and `VoiceTracker::with_settings(rate, TrackSettings)` exist. `new(rate)` is `with_settings(rate, …::VOICE)`, and every V03 and V04 test and scoreboard row passes unchanged. That is the proof voice did not move. `InputProfile::tracking()` gives each profile's settings. On the new plucked-string takes:
- the bass profile locks a B0 within 2 periods + 1 ms (66 ms);
- the guitar profile gives one onset for each note of a six-note sweep;
- neither gives an onset on a slide, a bend or a vibrato;
- every frame's Hz is continuous, with no quantising.

## The design (decided; build exactly this)

### Settings (`analysis/cycle.rs`, `voice/tracker.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleSettings {
    pub min_hz: f32, pub max_hz: f32,      // were VOICE_MIN_HZ / VOICE_MAX_HZ
    pub dc_hz: f32,                         // was DC_HZ
    pub peak_release_ms: f32,               // was PEAK_RELEASE_MS
    pub early: bool,                        // P07's half-cycle estimate; false here everywhere
}
impl CycleSettings { pub const VOICE: Self = /* V03's constants exactly */; }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSettings {
    pub cycle: CycleSettings,
    pub judge_frame_48k: usize,             // the judge's pYIN frame at 48 kHz, scaled with rate as V01 scales it
    pub gate_on_db: f32, pub gate_off_db: f32,
    pub onset_rise_db: f32,
    /// Some(c): a frame-to-frame move of at least c cents within two frames, while voiced, is an onset.
    pub jump_onset_cents: Option<f32>,
}
impl TrackSettings { pub const VOICE: Self = /* V04's constants exactly, jump None */; }
```

The other V03 and V04 constants (`LP_FACTOR`, `LP_MIN_HZ`, `SCHMITT`, `SILENCE_PEAK`, `CLARITY_ON`, `HALF_MARGIN`, `UNLOCK_PERIODS`, `INTERVALS`, `SEARCH_MARKS`, the judge's fold and veto, the gate hold and the refractory time) stay constants. They describe how the method works, not the instrument, and keeping them fixed keeps this lane small. The history length follows `max_period = rate / min_hz` by V03's formula, so bass at 48 kHz gets `(2·1656 + 8).next_power_of_two() = 4096`.

### The profiles (`voice/profile.rs`: `impl InputProfile { pub const fn tracking(self) -> TrackSettings }`)

| | voice | guitar | bass | line |
|---|---|---|---|---|
| `min_hz`, `max_hz` | 50, 4000 | 60, 1400 | 29, 500 | 30, 4000 |
| `dc_hz` | 30 | 20 | 10 | 10 |
| `peak_release_ms` | 30 | 30 | 80 | 80 |
| `judge_frame_48k` | 2048 | 2048 | 4096 | 4096 |
| `gate_on_db`, `gate_off_db` | −50, −56 | −48, −54 | −48, −54 | −50, −56 |
| `onset_rise_db` | 9 | 6 | 6 | 9 |
| `jump_onset_cents` | none | 80 | 80 | none |

The reasons, each as a doc comment:
- **Range**: P01's `range_hz`, so a profile has one range everywhere.
- **`dc_hz`**: V03's reasoning was 0.7 octave under the lowest note at under 1 dB loss there. The same 0.7 to 1.5 octaves under `min_hz` gives 20 for guitar (60 Hz) and 10 for bass (29 Hz). 30 Hz would take 3 dB off B0.
- **`peak_release_ms`**: V03's rule is "long against the longest period": 34.5 ms at 29 Hz and 33 ms at 30 Hz need 80. Guitar's longest is 16.7 ms, so 30 holds.
- **`judge_frame_48k`**: pYIN needs about two periods of the lowest note in its frame. 2048 at 48 kHz is 42.7 ms, which is two periods of 47 Hz, so the bass and line profiles take 4096 (85 ms). The judge only folds octaves and vetoes voicing (V04), so a slower judge delays no pitch. Its cost about doubles: the bench row below must stay under 4.2 ms per run (40% of the 10.7 ms hop, on its own thread).
- **Gate**: an instrument through a preamp sits higher above its floor than a voice in a room. −48 dBFS opens on a soft note and stays shut on hum at −55 to −70.
- **`onset_rise_db`**: a new pick on a ringing string lifts the level less than a re-sung syllable, because the old note is still sounding. 6 dB catches it, and V04's vibrato and tremolo argument (3 to 6 dB) is still met by the refractory time and the valley follower.
- **`jump_onset_cents`**: on a plucked instrument, a new pitch arriving within two frames (2.7 ms) is a new note: a sweep across strings, a hammer-on or a pull-off. A slide or a bend moves continuously. A 12-semitone slide in 100 ms is 16 cents a frame, a 2-semitone bend in 100 ms is 2.7 cents a frame, and vibrato of ±30 cents at 6 Hz peaks at 1.5 cents a frame. 80 cents in two frames is five times past the fastest of those and less than a semitone, so every fretted note change counts. Voice keeps none, because a sung legato step is one note (V04's rule).

### The onset rule (`voice/gate.rs`)

Beside V04's level onset and flux pairing: when `jump_onset_cents` is `Some(c)`, the gate is open, the frame is voiced, the frame two back was voiced, and `|1200·log2(hz_now / hz_two_back)| ≥ c`, then the frame is an onset, subject to V04's refractory time (60 ms). Why the refractory time still holds: a fast sweep is 16th notes at 200 BPM, 75 ms apart, so it stays clear of 60 ms. The valley resets as at any onset.

### Nothing sticks to notes

Frames carry the tracker's continuous Hz. `snapped_hz == hz` until a quantiser writes it (V04's rule), and no profile turns quantising on. A doc comment on `InputProfile::tracking` says so, and P08 sends frames unquantised.

### Plucked-string takes on the scoreboard (`analysis/score/instrument.rs`)

`pub fn instrument_suite(rate) -> Vec<Take>`, in V02's `Take` form with exact truth. The tones are made with Karplus-Strong (a delay of `rate/f` with a two-point average and a loss of 0.996, excited by seeded noise). Why: it is a plucked string, it decays and it carries the harmonics that make octave errors, and it is deterministic.
- `bass_open`: B0, E1, A1, D2, G2, each plucked and ringing for 1.5 s.
- `guitar_open`: E2 to E4, six strings, 1 s each.
- `sweep`: six notes of an A minor arpeggio, 70 ms apart, each ringing on under the next (so the level never dips).
- `slide`: one plucked A2 slid up a perfect fifth over 400 ms, then back (the delay length glides).
- `bend`: G3 bent up 200 cents over 150 ms, held, released.
- `vibrato`: D3 with ±30 cents at 6 Hz.
- `glide`: the perfect fourth over 2 s (sine, as P-track's common glide).
- `noise_hum`: 50 Hz hum at −55 dBFS plus pick noise.

The `voice_score` example adds an instrument table for `voice:guitar` and `voice:bass`. Truth Hz for a slid or bent take is the delay line's instantaneous frequency.

## Not this lane

P07's early estimate (`early` stays false). Running a tracker on an input (P08). The desk. The quantiser (V05), which inputs do not use.

## Red tests first

`voice/profile/tests.rs` and `analysis/score/tests.rs`. Red for all: none of these names exist.

1. `voice_is_track_v_exactly`: `CycleSettings::VOICE` equals V03's constants field by field, and `TrackSettings::VOICE` equals V04's. Every V03 and V04 test and scoreboard assertion passes unchanged (name them in END).
2. `the_bass_profile_hears_low_b`: `bass_open` with `InputProfile::Bass.tracking()`: each note's first frame within 50 cents arrives within 2 periods + 1 ms (B0: 65.8 ms); median ≤ 5 cents after 200 ms; octave errors ≤ 2% overall and none after 100 ms of a note.
3. `the_voice_profile_does_not_hear_low_b`: the same take with voice settings: B0 is never voiced within 50 cents (proof that the range matters).
4. `a_sweep_is_one_onset_per_note`: `sweep` with guitar settings: exactly 6 onsets, each within 2 periods + 3 ms of its note's pluck.
5. `slides_bends_and_vibrato_are_not_onsets`: `slide`, `bend`, `vibrato` and `glide` with the guitar and bass settings: exactly 1 onset each (the first pluck), and none after.
6. `the_frames_follow_a_glide_and_a_vibrato_without_steps`: `glide` and `vibrato` with guitar settings: every voiced frame within 5 cents of the truth at a lag of at most one period, and no frame differs from the one before by more than the truth's change in that frame plus 2 cents.
7. `slides_are_continuous`: `slide` with guitar settings: every voiced frame within 10 cents of the truth at a lag of at most one period (the take slides a fifth in 400 ms, about 2.3 cents a frame; the 10 cents covers the tracker's one-period lag on a moving pitch), and `snapped_hz == hz` in every frame.
8. `hum_is_not_a_note`: `noise_hum` with every profile: gate high in at most 1% of frames.
9. `nonsense_never_leaves_the_tracker`: V04's nonsense test, run with each profile.
10. `processing_never_allocates_with_any_profile`: 10 s of `bass_open` and of `sweep` through `process` inside `assert_no_alloc`.
11. Bench `judge/hop/48k/bass` beside V01's `judge/hop/48k`: at most 4.2 ms per run; the result in BENCH.md.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/voice kazoo-core/src/analysis kazoo-core/examples
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-core --bench voice -j 4 -- 'judge'
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo run --release -p kazoo-core --example voice_score -j 4
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`.

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-core test passes, V03's and V04's unchanged.
- [ ] BENCH.md `## KZ-P06 profiles`: the instrument table for guitar and bass, `judge/hop/48k/bass`, the load average, date and sha.
- [ ] Every profile number has its reason as a doc comment.
- [ ] If any of Tom's takes are in `kazoo-core/tests/fixtures/instrument/`, the example scores them; say which were present.

## Code quality

As PLAN.md's STACK RULES. `process`, `push` and `judge` stay allocation-free, lock-free and panic-free for every profile. The history is allocated once, sized by the profile's `min_hz`. No `as` casts clippy pedantic flags. Files under 1000 lines: if `analysis/cycle.rs` passes 1000 with the settings, move the settings into `analysis/cycle/settings.rs`.

## END

```
START KZ-P06 base=<sha> branch=lane/kz-P06 df=<GB> at <time>
END KZ-P06 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: judge/hop/48k/bass; the instrument table
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
