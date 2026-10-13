# KZ-V04: the voice frame: pitch in Hz and volts, gate, onset, loudness, breath and confidence every 64 samples, with the cycle tracker judged by pYIN

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core. Time: about 30 minutes. Depends: KZ-V03 (the cycle tracker and its scoreboard row).

Why: Tom wants his voice to drive "all the engines we have", hertz for hertz or quantised. Every engine needs the same small set of facts about the voice, at a steady rate, already decided: is a note sounding, which pitch, how loud, how breathy, how sure. This lane makes that one record, `VoiceFrame`, and the one thing that makes it from a microphone, `VoiceTracker`: the fast cycle tracker for Hz, the pYIN judge (now on a thread of its own, with the onset detector that is computed and thrown away today, `engine/analysis_thread.rs:125-127`) for octave and voicing, and a gate with hysteresis. V05 quantises it, V06 and V08a run it, V07a puts it on the wire, V09 and V10 follow it.

## Outcome

`VoiceTracker::process(mic, |frame| ..)` emits one `VoiceFrame` every 64 samples, allocation-free, on the audio callback. Its pitch meets the cycle tracker's latency and holds octave errors under 2% on the octave traps (none after the first 100 ms of a note); its gate rises within 2 periods + 3 ms of a note and falls within 2 periods + 15 ms of its end, stays down through noise, and flags a new onset for each re-sung syllable but not for a legato step or a glide.

## The design (decided; build exactly this)

### `kazoo-core/src/voice/mod.rs` (new; `pub mod voice;` in `lib.rs`; add `voice/` to the module graph comment in `lib.rs` if it has one)

```rust
/// Middle C, the pitch 0.0 V stands for (1.0 per octave), as the wall's
/// `catalogue::C4_HZ` (kazoo-wall/src/catalogue.rs:34): the nearest f32 to
/// 440 · 2^(−9/12).
pub const C4_HZ: f32 = 261.625_58;
/// Samples per voice frame: 1.33 ms at 48 kHz, finer than one cycle of any
/// note under 750 Hz; a fixed hop keeps the frame rate steady whatever the
/// device's callback size.
pub const VOICE_HOP: usize = 64;
/// The loudness scale's floor: −60 dBFS is 0.0, 0 dBFS is 1.0.
pub const LOUDNESS_FLOOR_DB: f32 = -60.0;

#[must_use] pub fn volts_from_hz(hz: f32) -> f32;   // log2(hz / C4_HZ); 0.0 for hz ≤ 0 or non-finite
#[must_use] pub fn hz_from_volts(volts: f32) -> f32; // C4_HZ · 2^volts, volts clamped to ±8 first

/// What the voice is doing, once per VOICE_HOP samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceFrame {
    /// Producer's sample count at the end of this frame (frames since the
    /// producer started; the same clock as its audio stream 0 when it sends
    /// audio every block).
    pub at: u64,
    /// Pitch now; held at the last voiced value while unvoiced; 0.0 before
    /// the first note.
    pub hz: f32,
    pub volts: f32,
    /// The quantised pitch. Equal to `hz`/`volts` until a quantiser (V05) writes it.
    pub snapped_hz: f32,
    pub snapped_volts: f32,
    /// A note is sounding (with hysteresis; see the gate).
    pub gate: bool,
    /// The gate rose, or a syllable was re-sung with the gate high, in this frame.
    pub onset: bool,
    /// The tracker has a pitch in this frame (gate may still be high through
    /// its hold).
    pub voiced: bool,
    /// 0..=1: (dBFS − LOUDNESS_FLOOR_DB) / −LOUDNESS_FLOOR_DB, clamped.
    pub loudness: f32,
    /// 0..=1: how much of the sound is breath rather than tone.
    pub breath: f32,
    /// 0..=1: how sure the pitch is.
    pub confidence: f32,
}
```

### The judge thread: `voice/judge.rs`

```rust
#[derive(Debug, Clone, Copy)]
pub enum JudgeResult { Pitch(PitchEstimate), Onset { strength: f32 } }

pub struct JudgeFeed   { /* HeapProd<f32>, the judge Thread, Arc<JudgeStats> */ }
pub struct JudgeResults { /* HeapCons<JudgeResult> */ }
pub struct Judge       { /* JoinHandle<()>, Arc<JudgeStats> */ }

/// The onset detector's settings: FFT size at 48 kHz (scaled with the rate
/// as `PitchDetectorConfig::for_rate` scales its window) and sensitivity.
/// `Default` is 1024 and 0.3, the engine's existing onset defaults
/// (`engine/mod.rs:105-106`), which V06 passes through from `EngineConfig`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnsetConfig { pub onset_fft_size: usize, pub onset_threshold: f32 }

/// Start the judge. Its rate is `pitch.sample_rate`; callers pass
/// `PitchDetectorConfig::for_rate(rate)` or, in the engine, V01's
/// `judge_config(&config.pitch, rate)`. An invalid `pitch` is
/// `Error::Config`, raised here before the thread starts. The thread is
/// named `kazoo-judge`.
pub fn spawn(pitch: PitchDetectorConfig, block: usize, onsets: OnsetConfig) -> crate::Result<(JudgeFeed, JudgeResults, Judge)>;

impl JudgeFeed {
    /// Callback side: push what fits (the rest counted as dropped), then
    /// `Thread::unpark` once. Constant time, no lock, no allocation.
    pub fn push(&mut self, samples: &[f32]);
}
impl JudgeResults { pub fn pop(&mut self) -> Option<JudgeResult>; }
impl Judge {
    pub fn stats(&self) -> JudgeStatsSnapshot; // samples_dropped, results_dropped
    pub fn join(self) -> std::thread::Result<()>;
}
```

- Feed ring: `rate / 10` samples (rate = `pitch.sample_rate`) (100 ms: three judge hops of slack). Results ring: 64 (a pitch every 10.7 ms and onsets: over half a second of backlog).
- The thread: `PitchDetector::new(pitch)` (built in `spawn`, so a bad config fails the call) and `OnsetDetector::new(onsets.onset_fft_size * scale, onsets.onset_fft_size * scale / 4, rate, onsets.onset_threshold)` (the hop is a quarter of the FFT, as `analysis_thread.rs:81` does; `scale` is `for_rate`'s rate scale). Loop: `pop_slice` into a buffer of `block.max(256)`; nothing read and the producer gone (`!write_is_held()`) → exit; nothing read → `std::thread::park_timeout(Duration::from_millis(20))` (the callback unparks it; the timeout only notices shutdown — no sleep-polling, B01's rule); else push to both detectors and push each `Pitch` and each `OnsetEvent` (as `Onset { strength }`) as a result, counting any that do not fit.
- The judge thread may allocate (pyin and the onset detector do); nothing it hands back owns memory (`JudgeResult` is `Copy`), so the callback never frees anything.

### The fusion and the gate: `voice/tracker.rs` and `voice/gate.rs`

```rust
pub struct VoiceTracker { /* CycleTracker, judge state, Gate, envelopes, counters */ }
impl VoiceTracker {
    pub fn new(rate: u32) -> Self;                      // allocates once (the cycle history)
    pub fn judge(&mut self, result: JudgeResult);       // callback side
    pub fn process(&mut self, mic: &[f32], emit: impl FnMut(VoiceFrame)); // per sample; a frame each VOICE_HOP
    pub fn reset(&mut self);
}
```

Per sample: `cycle.push(x)`; update the three level envelopes; count samples; every `VOICE_HOP` samples build and emit a frame. Constants (each with its reason as a doc comment):

| Name | Value | Reason |
|---|---|---|
| `JUDGE_FRESH_MS` | 150 | a pitch verdict older than this (about 14 judge hops) no longer describes the note |
| `JUDGE_MIN_PROB` | 0.5 | the judge rules on octave only when pYIN is more sure than not |
| `FOLD_WINDOW` | 0.25 octave | the tracker is folded to the judge's octave only when it sits within 300 cents of an exact octave of it; anything further is a different note, and the tracker (faster) wins |
| `VETO_RESULTS`, `VETO_PROB` | 3, 0.1 | three judge runs in a row (32 ms) saying "unvoiced" with probability under 0.1 unlock a tracker locked onto something periodic that is not a voice |
| `DISAGREE_CENTS` | 100 | after folding, a judge more than a semitone away halves the confidence |
| `GATE_ON_DB` / `GATE_OFF_DB` | −50 / −56 | the gate opens on a voiced pitch above −50 dBFS and closes below −56: 6 dB of hysteresis, more than the ripple of the fast level on a 50 Hz note |
| `GATE_OFF_HOLD_MS` | 12 | the gate closes only after 12 ms of "off", in whole frames (9 at 48 kHz): bridges a glottal break without holding a released note audibly |
| `ONSET_RISE_DB` | 9 | a re-sung syllable dips 10 to 30 dB and comes back; tremolo and vibrato move the level 3 to 6 dB |
| `ONSET_FLUX_RISE_DB` | 4 | a spectral-flux onset from the judge counts with a smaller rise (a consonant with only a shallow dip), but never with none: a legato step changes the spectrum without a dip and must not retrigger |
| `ONSET_FLUX_WINDOW_MS` | 30 | a flux onset and a ≥ 4 dB rise must fall within 30 ms of each other (the flux arrives one judge hop late) |
| `ONSET_REFRACTORY_MS` | 60 | at most one onset per 60 ms: a fast "da-da-da" is about 8 syllables a second |
| `FAST_ATTACK_MS` | 1 | the fast level (gate and onset) rises within a millisecond |
| fast release | `max(10 ms, 1.5 × tracked period)` | short enough to see a 20 ms syllable dip on an A3, long enough that a 50 Hz note's rectified ripple stays under 3 dB |
| `VALLEY_RISE_MS` | 80 | the valley (the lowest recent fast level) follows the fast level down at once and up with this time constant; an onset is a rise of the fast level over the valley, and the valley is reset to the fast level at each onset |
| loudness envelope | `EnvelopeFollower::new(5.0, 50.0, rate)` | the existing input-level follower's times (`processing.rs:247`), so loudness reads as the tui's meter does |
| `BREATH_SMOOTH_MS` | 20 | breath moves at syllable speed, not cycle speed |

The frame, at each `VOICE_HOP` boundary:

1. **Judge.** Drain is the caller's job (`judge()` per result). Keep the newest `Pitch` result and its age in samples (reset to 0 on arrival). `VETO`: count consecutive `Pitch` results with `frequency == None && voiced_probability < VETO_PROB`; at `VETO_RESULTS` while the cycle tracker is locked, `cycle.unlock()`; any voiced result zeroes the count.
2. **Pitch.** `fc = cycle.estimate()`. Keep `last_leap`: the sample count at which the cycle tracker's Hz last moved by more than `LEAP_CENTS = 300` between consecutive frames, or went from `None` to `Some` (reason: a singer's own octave leap must not be folded back by a judge that heard the old note; 300 cents is past vibrato and slides frame to frame). A judge `Pitch` result counts as **after the leap** only when `arrival − frame_length ≥ last_leap`, where `arrival` is the sample count when `judge()` received it and `frame_length` is the judge's (V01: the newest pYIN frame reads the last `frame_length` samples, 2048 at 48 kHz, so the verdict describes only sound after the leap). If `fc` is `Some` and the judge is fresh (`age ≤ JUDGE_FRESH_MS`), after the leap, voiced, with probability ≥ `JUDGE_MIN_PROB`: `r = log2(f_judge / fc.hz)`, `k = round(r)`; if `k ≠ 0` and `|r − k| ≤ FOLD_WINDOW`, the frame's Hz is `fc.hz · 2^k`, and, the first frame `k` takes a new value, `cycle.hint(rate / that Hz)`. Else the frame's Hz is `fc.hz`. `voiced = fc.is_some()`. Unvoiced frames hold the last Hz. `volts = volts_from_hz(hz)`; `snapped_* = hz/volts`.
3. **Levels.** fast level `|x|` peak follower (attack `FAST_ATTACK_MS`, release as the table) → `fast_db` (20·log10, floored at −120); valley as the table; loudness from the 5/50 ms follower → `loudness`.
4. **Gate.** Opens in this frame when `voiced && fast_db ≥ GATE_ON_DB`. While open, a frame where `!voiced || fast_db < GATE_OFF_DB` counts toward the hold; any other frame resets the count; at the hold count (`ceil(GATE_OFF_HOLD_MS · rate / 1000 / VOICE_HOP)`) it closes.
5. **Onset.** `true` in the frame the gate opens. While open, and at least `ONSET_REFRACTORY_MS` since the last onset: `true` when `fast_db − valley ≥ ONSET_RISE_DB`, or when a judge `Onset` and a rise of ≥ `ONSET_FLUX_RISE_DB` fall within `ONSET_FLUX_WINDOW_MS` of each other (keep the time of the last flux onset and the time of the last ≥ 4 dB rise; whichever comes second completes the pair). On every onset, valley = fast level.
6. **Breath.** Raw: `fast_db < GATE_ON_DB` → 0; else `voiced` → `clamp(2 · (1 − cycle.last_clarity()), 0, 1)` (clarity under 0.5 is as good as noise); else 1. Smoothed one-pole per frame with `BREATH_SMOOTH_MS`.
7. **Confidence.** `voiced` → the estimate's clarity, halved when the judge is fresh, voiced and more than `DISAGREE_CENTS` from the frame's Hz; else 0.
8. `at` = samples processed so far. Every float in the frame is checked finite before `emit`; a non-finite value becomes 0.0 (and `gate`/`voiced` false for a non-finite Hz) — NaN never leaves the tracker.

### On the scoreboard

- `analysis/score.rs`: `pub struct FusedUnderTest` (name `"voice"`): a `VoiceTracker` plus an inline `PitchDetector` and `OnsetDetector` run synchronously on the same samples, their results handed to `judge()` **one judge hop late** (512 samples at 48 kHz: the thread's real lag), its `hz` = the latest frame's Hz when `voiced`, else `None`.
- `analysis/score/gate.rs` (new): `pub trait GateUnderTest: TrackerUnderTest { fn gate(&self) -> bool; fn onsets(&self) -> u64; }`, `pub struct GateScore { rise_ms_max, fall_ms_max, noise_high_pct, onsets_counted, onsets_expected }`, `pub fn score_gate(..)`: rise = per voiced start, time to gate high; fall = per voiced end, time to gate low; noise_high = gate high over truth-unvoiced steps outside the 30 ms release allowance; onsets as counted against `Take::onsets.len()`.
- The `voice_score` example adds the `voice` row and a second table, the gate table, for `voice`.

## Not this lane

The quantiser (V05; `snapped_*` stays equal to the direct pitch here). Any engine or program running the tracker (V06, V08a). The wire (V07a).

## Red tests first

`voice/tests.rs` (unit) and `analysis/score/tests.rs` (`voice_meets_its_targets`, `voice_gate_meets_its_targets`). Red for all: the module does not exist.

- `volts_are_one_per_octave_from_c4`: `volts_from_hz(C4_HZ) == 0`, `(523.2511) ≈ 1.0` within 1e-5, `(130.81278) ≈ −1.0`; `hz_from_volts(volts_from_hz(440)) ≈ 440` within 0.01 cent; 0, −1, NaN, ∞ give 0.0 volts; volts ±20 clamp to ±8 octaves.
- `a_frame_comes_every_64_samples`: 48,000 samples in pushes of 1, 37, 128 and 4096 give exactly 750 frames each, `at` = 64, 128, … 48,000.
- `the_judge_folds_an_octave_slip`: `octave_bait` (150 Hz with the loud 2nd harmonic) with a stub judge feeding `Pitch(150 Hz, p = 0.9)` every 512 samples: every voiced frame after 100 ms reads 150 Hz within 10 cents, and the tracker was hinted (its own estimate reaches 150 within 4 marks of the first fold).
- `a_sung_octave_leap_is_not_folded_back`: 220 Hz for 400 ms, then 440 Hz phase-continuous for 400 ms, with a stub judge that reports the true pitch of the 2048 samples before each 512-sample hop (so for 43 ms after the leap it still says 220, p = 0.9): every voiced frame from `2/440 s + 1 ms` after the leap reads 440 Hz within 20 cents; none reads within 100 cents of 220. Red: the module does not exist; and a fold without `last_leap` pulls the frames back to 220 for about 50 ms.
- `three_unvoiced_verdicts_unlock_a_false_pitch`: a 1 kHz square at −10 dBFS (periodic, locks the tracker) with a stub judge saying unvoiced p = 0.02 every 512 samples: `voiced` is false within 4 judge hops of the first verdict.
- `the_gate_has_hysteresis`: a 220 Hz sine whose level steps −45 → −53 → −58 dBFS (each 200 ms): gate opens at −45, stays open at −53, closes within 12 ms + 2 periods after the step to −58.
- `a_re_sung_syllable_is_an_onset_and_a_legato_step_is_not`: `rearticulate` gives exactly 5 onset flags; `steps`, `glide`, `vibrato` give exactly 1 each.
- `nonsense_never_leaves_the_tracker`: NaN, ±Inf, 1e30 bursts: every frame field finite, gate false through them.
- `the_judge_thread_comes_and_goes`: `spawn(PitchDetectorConfig::for_rate(48_000), 128, OnsetConfig::default())`, push 1 s of a 220 Hz sine in 128-sample pushes, pop results: at least 80 `Pitch` results, the voiced ones at 220 within 10 cents; drop the feed, `join` returns within 100 ms.
- `processing_never_allocates`: 10 s of `vowel_ah` through `process` with judge results interleaved, inside `assert_no_alloc`; `JudgeFeed::push` likewise.
- `voice_meets_its_targets` (scoreboard, 48 kHz): every target `cycle_meets_its_targets` (V03) holds for `voice`, and `octave_bait` and `vowel_ah` have octave ≤ 2% overall and no octave error later than 100 ms after an onset; `noise_bursts` false ≤ 1%.
- `voice_gate_meets_its_targets`: `sine_ladder`, `steps`, `hum_low`, `whistle`: rise ≤ 2 periods + 3 ms, fall ≤ 2 periods + 15 ms per note; `noise_bursts`: noise_high ≤ 1%, 3 onsets; `rearticulate`: 5 onsets; `sine_ladder`: 6.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/voice kazoo-core/src/analysis kazoo-core/examples
nice -n 10 env CARGO_BUILD_JOBS=4 cargo run --release -p kazoo-core --example voice_score
```

Doc tests: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`.

## Definition of done

- [ ] All red tests fail first (paste under RED), then pass; every existing test passes.
- [ ] BENCH.md `## KZ-V04 voice trackers`: the pitch table (pyin, cycle, voice) and the gate table, load average, date, sha.
- [ ] `JudgeResult` is `Copy`; nothing the judge hands the callback owns memory (say how you checked).
- [ ] Every constant in the table exists with its reason as its doc comment.

## Code quality

As PLAN.md's STACK RULES. `VoiceTracker::process`, `judge`, and `JudgeFeed::push` run on audio callbacks: no allocation, lock, I/O, format or panic; `Thread::unpark` is the one system call allowed (B01's reason). The judge thread is the only place pyin and the onset detector run. Files under 1000 lines (`voice/tracker.rs` and `voice/gate.rs` are separate for that). No `as` casts that clippy pedantic flags.

## END

```
START KZ-V04 base=<sha> branch=lane/kz-V04 df=<GB> at <time>
END KZ-V04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the voice_score pitch and gate tables
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
