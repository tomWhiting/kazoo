# KZ-V03: a cycle-by-cycle pitch tracker that answers within two periods of the note, on the audio callback

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core. Time: about 30 minutes. (Edited 13 Oct 23:5x after V02's review: per-event latencies and missed events; `analysis/score.rs` and its metrics file are touched for that.) Depends: KZ-V02 (the scoreboard; this lane adds its row and asserts its targets there), which needs KZ-V01.

Why: Tom wants to drive a synth from his voice "hertz for hertz". pYIN needs a 43 ms window and runs on a thread; even after V01 it answers about 50 ms after a note starts. A hardware pitch-to-voltage converter does better because it measures each cycle as it ends. This lane is that converter, in software, run per sample on the output callback (a thread hop would cost a whole callback, 2.7 ms at 128 frames, which is the entire budget for a whistle). pYIN stays as the judge of octave and voicing; V04 joins the two.

## Outcome

`kazoo_core::analysis::cycle::CycleTracker` takes samples one at a time and, at every cycle of the note, publishes the period, the Hz and a clarity figure. On the scoreboard at 48 kHz: the first pitch within 2 periods + 1 ms of a note's start (2.3 ms for a 1.5 kHz whistle, 10 ms for A3, 31 ms for a C2 hum), a note change within 2 periods of the new note + 1 ms, under 3 cents median on sines, no allocation, and under 1% of a core.

## The design (decided; build exactly this)

### File and API: `kazoo-core/src/analysis/cycle.rs` (`pub mod cycle;` in `analysis/mod.rs`), tests in `analysis/cycle/tests.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleEstimate {
    /// Period in samples, fractional.
    pub period: f32,
    /// `rate / period`.
    pub hz: f32,
    /// Normalised autocorrelation at `period` over one period, 0..=1
    /// (1 = perfectly periodic; McLeod's NSDF, clamped at 0).
    pub clarity: f32,
}

pub struct CycleTracker { /* below */ }

impl CycleTracker {
    /// Allocates the history once; nothing after this allocates.
    pub fn new(rate: u32) -> Self;
    /// One sample (sanitised first). Never allocates, locks or panics.
    pub fn push(&mut self, sample: f32);
    /// `push` over a block.
    pub fn process(&mut self, block: &[f32]);
    /// The current pitch, held between cycles; `None` when unlocked.
    pub const fn estimate(&self) -> Option<CycleEstimate>;
    /// Clarity of the newest cycle mark's best candidate, accepted or not
    /// (V04 derives breathiness from it); 0 before any mark.
    pub const fn last_clarity(&self) -> f32;
    /// A period (samples) to try as an extra candidate at the next 4 marks:
    /// how the judge's octave correction reaches the tracker (V04).
    pub fn hint(&mut self, period: f32);
    /// Drop the lock now (V04's voicing veto).
    pub fn unlock(&mut self);
    /// Cycle marks seen since `new` or `reset` (for tests and the scoreboard).
    pub const fn marks(&self) -> u64;
    pub fn reset(&mut self);
}
```

### Constants (each with its reason, as a doc comment on the constant)

| Name | Value | Reason |
|---|---|---|
| `DC_HZ` | 30.0 | a one-pole high-pass at 30 Hz removes mic DC and handling rumble, 0.7 octave under `VOICE_MIN_HZ` (V01), costing under 1 dB at 50 Hz |
| `LP_FACTOR` | 3.0 | when locked, the two-pole tracking low-pass sits at 3 × f: a two-octave upward jump is still at half amplitude (so the Schmitt trigger still fires), while the 3rd harmonic is halved. The NSDF check, not the filter, decides the period |
| `LP_MIN_HZ` | 100.0 | the filter never closes below 100 Hz, so a 50 Hz hum's fundamental passes at full strength |
| unlocked cutoff | `VOICE_MAX_HZ` (4000) | unlocked, nothing is filtered away that could be the note; the first lock costs no filter delay |
| `SCHMITT` | 0.2 | a cycle mark needs the filtered signal to go below −0.2 × peak and then above +0.2 × peak: well above harmonic ripple on a hummed note, well below the fundamental's swing |
| `PEAK_RELEASE_MS` | 30.0 | the peak follower (instant attack) falls with this time constant: long against a 50 Hz period (20 ms), short against a phrase |
| `SILENCE_PEAK` | 0.001 | below −60 dBFS peak no marks fire: the loudness floor of the whole track, so room noise never starts a search |
| `CLARITY_ON` | 0.80 | a candidate is accepted at NSDF ≥ 0.8: McLeod and Wyvill pick key maxima between 0.8 and 1.0; 0.8 still accepts a breathy hum |
| `HALF_MARGIN` | 0.05 | a candidate's half period wins only if its NSDF is ≥ `CLARITY_ON + 0.05`, so a loud 2nd harmonic does not read an octave up |
| `UNLOCK_PERIODS` | 2.0 | with no accepted mark for 2 periods of the last pitch, the tracker unlocks: one lost cycle is bridged, two are not |
| `INTERVALS` | 4 | candidates are the last 1, 2, 3 and 4 mark-to-mark intervals: up to four marks per true period (a loud 2nd, 3rd or 4th harmonic) still leave one candidate equal to the period |

`min_period = rate / VOICE_MAX_HZ`, `max_period = rate / VOICE_MIN_HZ`. History length `H = (2 * ceil(max_period) + 8).next_power_of_two()` (2048 at 48 kHz, 4096 at 96 kHz, 8192 at 192 kHz): one window of the longest period plus the lag of the longest period, plus slack for the ±1 lag and interpolation.

### Per sample (`push`)

1. `x = sanitize_sample(sample)`; `d` = one-pole DC blocker at `DC_HZ` (`y = a·(y + x − x_prev)`, `a = exp(−2π·DC_HZ/rate)`); write `d` into the history ring (this ring, unfiltered, is what the NSDF reads: the filter is only for marking cycles).
2. `f` = two cascaded one-pole low-passes of `d` at the current cutoff (`fc = clamp(LP_FACTOR · estimate.hz, LP_MIN_HZ, VOICE_MAX_HZ)` when locked, `VOICE_MAX_HZ` unlocked; the coefficient `exp(−2π·fc/rate)` is recomputed only when a new estimate is accepted, not per sample).
3. Peak follower on `|f|`: `peak = max(|f|, peak · r)`, `r = exp(−1 / (PEAK_RELEASE_MS · rate / 1000))`.
4. Schmitt trigger with `θ = SCHMITT · peak`, only while `peak ≥ SILENCE_PEAK`: when `f < −θ`, `armed = true`. On every upward zero crossing of `f` (`prev < 0 ≤ f`), `last_zero = n − 1 + prev / (prev − f)` (sample time as `f64`, interpolated). When `armed` and `f ≥ θ`: a **mark** at `last_zero`, `armed = false`.
5. Unlock check: when locked and `n − last_accepted_mark > UNLOCK_PERIODS · estimate.period`, unlock (`estimate = None`, cutoff back to `VOICE_MAX_HZ`, hint cleared).

### At each mark

1. Push the mark time into a ring of the last `SEARCH_MARKS + 1` marks (`SEARCH_MARKS = 16`; reason: a buzz through a strong formant, Tom's "ah" at 130 Hz under F1 700 Hz, can mark five or six times per period while the filter is open at 4 kHz, and 16 covers up to the 16th harmonic). With fewer than two marks, stop here.
2. Candidates:
   - **Locked:** for `k` in 1..=`INTERVALS` where `k` earlier marks exist, `τk = mark − mark[−k]`; plus the hint, if one is pending.
   - **Unlocked, or locked and no locked candidate reached `CLARITY_ON` at this mark (the search):** `τk` for every earlier mark in the ring with `τk ≤ max_period`, `k` up to `SEARCH_MARKS`; plus the hint. Why: with the filter wide open an `INTERVALS`-only search can never lock on a vowel whose formant makes more than four marks per period, and the filter only closes once locked; the search closes that loop. Its cost is bounded by `SEARCH_MARKS` and is paid only while searching.
   - Keep those in `[min_period, max_period]`.
3. For each candidate `τ`, evaluate the NSDF at integer lags `L ∈ {round(τ)−1, round(τ), round(τ)+1}` over the window of `W = round(τ)` samples ending at the newest sample: `n(L) = 2·Σ x[t]·x[t−L] / Σ (x[t]² + x[t−L]²)` (0 when the denominator is below `1e-12`). Refine by parabolic interpolation through the three points to `τ*`, `n*` (if the middle point is not the largest, take the largest of the three without interpolation).
4. Choose: the smallest `τ*` with `n* ≥ CLARITY_ON`. Then the half-period check: if `τ*/2 ≥ min_period` and `n(τ*/2)` (refined the same way) `≥ CLARITY_ON + HALF_MARGIN`, take `τ*/2`. `last_clarity` = the chosen candidate's `n*`, or the best `n*` among all candidates if none reached `CLARITY_ON`.
5. If a candidate was chosen: `estimate = Some(CycleEstimate { period: τ*, hz: rate/τ*, clarity: n* })`, `last_accepted_mark = mark`, recompute the low-pass coefficient; a hint is consumed after 4 marks or as soon as the chosen period is within 1% of it.

Cost bound, locked: at most 5 candidates (+ 1 half check) × 3 lags × 3 multiply-adds per sample × `W ≤ max_period` per mark: about 52k multiply-adds at 50 Hz, 50 times a second (2.6M a second); tiny at whistle pitch (W = 12 at 4 kHz). Searching: at most 17 candidates (+ 1 half check) per mark, each candidate's window only as long as itself; the bench below runs `vowel_ah`, which searches at every onset, so the 1% budget covers it. Bounded by construction; no loop is open-ended.

Why marks from zero crossings and the period from NSDF: the crossings are when a cycle ends (the latency), the NSDF over exactly one period is what the cycle was (the accuracy); counting crossings alone reads harmonics as extra cycles, NSDF alone has no cue for when to look.

### On the scoreboard

`analysis/score.rs` gains `pub struct CycleUnderTest(CycleTracker)` (`name` = `"cycle"`, `hz` = `estimate().map(|e| e.hz)`), and the `voice_score` example scores it beside `pyin`. Add the bench `cycle/second/48k` to `kazoo-core/benches/voice.rs`: one second of `vowel_ah` (from `score::suite(48_000)`) through `process`; budget 10 ms (1% of real time).

### Test speed (decided by Cassio, 13 Oct 23:5x, from V02's review)

V02's review found the scoreboard tests are kazoo-core's slowest in debug (the judge floor 28 s, each WAV test about 19 s, V01's pitch tests 90 s under load), all from pYIN's per-frame search run unoptimised. The workspace `Cargo.toml` gains:

```toml
[profile.dev.package.pyin]
opt-level = 3
```

Why only pyin: it optimises the one dependency whose search dominates, and leaves every kazoo crate's own code built as today (debug assertions, overflow checks and all). Paste the four tests' times before and after under BENCH. If pyin's own heavy dependencies (find them with `cargo tree -p pyin --depth 1`) carry the cost instead, add each by name in the same form and say which.

## Not this lane

The judge's octave folding and voicing veto, gate, loudness, breath, frames (V04). Wiring the tracker into any engine (V06, V08a).

## Red tests first

Unit tests in `analysis/cycle/tests.rs`; scoreboard targets in `analysis/score/tests.rs` (`cycle_meets_its_targets`). Every scoreboard figure is taken at 48 kHz on `score::suite(48_000)` takes, cut to at most 1.5 s each where the take is longer. Red for all: the module does not exist.

- `a_sine_is_tracked_within_three_cents_at_every_rate`: 220 Hz at 44,100, 48,000, 96,000 and 192,000 Hz: after 50 ms, every estimate within 3 cents, clarity ≥ 0.98.
- `the_first_pitch_comes_within_two_periods`: a 220 Hz sine starting from the −70 dBFS floor at a phase of 0, then again at phases π/2, π, 3π/2: `estimate()` is `Some` and within 50 cents no later than `2 × period + 1 ms` after the note starts (10.1 ms).
- `a_note_change_lands_within_two_periods_of_the_new_note`: 220 → 330 Hz phase-continuous: within 20 cents of 330 by `2 × (1/330 s) + 1 ms` after the change; and 330 → 220: by `2/220 s + 1 ms`.
- `harmonics_do_not_double_the_pitch`: `octave_bait` (150 Hz, 2nd harmonic at 1.5): no estimate after the first lock reads within 100 cents of 300 Hz in more than 5% of estimates.
- `a_formant_rich_vowel_locks`: `vowel_ah` from `score::suite(48_000)`: `estimate()` is `Some` and within 50 cents of 130.81 Hz no later than 3 periods + 1 ms after the start. Red: the module does not exist; and with `INTERVALS`-only candidates and the filter open at 4 kHz, a buzz under a 700 Hz formant can mark more than four times a period and never lock.
- `noise_never_locks`: 1 s of uniform noise ±0.25: `estimate()` is `None` at ≥ 98% of 32-sample steps; and 1 s of the −70 dBFS floor alone: never a mark (`marks() == 0`).
- `it_lets_go_after_two_periods_of_silence`: 220 Hz for 200 ms then the floor: `None` within `2 × period + 1 ms` of the end.
- `a_hint_brings_an_octave_slip_back`: a signal tracked at 300 Hz because of its partials, `hint(rate/150)`: the next estimate chooses the hinted 150 Hz period when its NSDF ≥ `CLARITY_ON` (construct with fundamental 1.0, 2nd harmonic 1.5 and check the hinted period is taken).
- `nonsense_input_is_survived`: NaN, ±Inf, ±1e30 and a DC step of 1.0: no panic, every estimate finite, then a clean 220 Hz sine locks again within 3 periods.
- `processing_never_allocates`: `process` over 10 s of `vowel_ah` inside `assert_no_alloc::assert_no_alloc` (dev-dependency already present).
- `cycle_meets_its_targets` (scoreboard, 48 kHz):
  - `sine_ladder`, `steps`, `hum_low`, `whistle`: onset latency ≤ 2 periods of that note + 1 ms for every note, `onsets_missed == 0`, cents median ≤ 3, p95 ≤ 10, octave 0, missed ≤ 2%.
  - `steps`: step latency ≤ 2 periods of the new note + 1 ms for every step, and `steps_missed == 0` (V02's review: a target checked on the steps that landed passes on one).
  - V02's `Score` gives only the median and maximum across notes, so a per-note bound cannot be read from it. This lane adds `pub events: Vec<EventLatency { at_sample: usize, kind: Onset | Step, truth_hz: f32, latency_ms: Option<f32> }>` to `Score` (`None` for a missed event), filled where V02's metrics already compute each event's latency, and asserts each event against its own note's period. The existing figures are unchanged (V02's tests prove it).
  - `vibrato`: median ≤ 8, p95 ≤ 20 (a per-cycle estimate lags half a cycle of a 1885 cents/s slope: about 4 cents).
  - `glide`: median ≤ 10, p95 ≤ 25.
  - `vowel_ah`, `octave_bait`: octave ≤ 5% (the judge brings these under 0.5% in V04), onset ≤ 3 periods + 1 ms.
  - `noise_bursts`: false ≤ 2%.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-core/benches kazoo-core/examples
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-core --bench voice -- 'cycle'
nice -n 10 env CARGO_BUILD_JOBS=4 cargo run --release -p kazoo-core --example voice_score
```

Doc tests: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`.

## Definition of done

- [ ] All red tests fail first (paste under RED), then pass; every existing test passes.
- [ ] `cycle/second/48k` ≤ 10 ms in release, in BENCH.md.
- [ ] BENCH.md has `## KZ-V03 voice trackers` with the full `voice_score` table (pyin and cycle rows, plus any of Tom's takes present), the load average, date and sha.
- [ ] Every constant in the table above exists with its reason as its doc comment.

## Code quality

As PLAN.md's STACK RULES. `push` is called on the audio callback by V06 and V08a: no allocation, lock, I/O, format or panic in `push`, `process`, `estimate`, `hint`, `unlock`; every index into the history ring is masked by `H − 1` (H a power of two), never bounds-checked into a panic path by construction; all arithmetic on finite, sanitised samples, with any non-finite result (a denominator of 0) mapped to "no candidate". No `as` casts clippy pedantic flags: lags round through `f32::round` then a checked conversion helper written once in this file. Files under 1000 lines.

## END

```
START KZ-V03 base=<sha> branch=lane/kz-V03 df=<GB> at <time>
END KZ-V03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: cycle/second/48k; the voice_score table
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
