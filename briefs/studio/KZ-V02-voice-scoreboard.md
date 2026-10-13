# KZ-V02: a scoreboard for pitch trackers: synthetic takes with exact truth, Tom's recorded takes against offline pYIN, and latency, cents, octave and voicing figures for each

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core. Time: about 30 minutes. Depends: KZ-V01, which gives the judge's `for_rate` and `VOICE_MIN_HZ`/`VOICE_MAX_HZ`.

Why: Track V makes promises in milliseconds and cents. Pitch within 2 periods + 1 ms of a note's start, octave errors under half a percent: none of these can be held without a measurement that every tracker lane runs the same way. This lane builds that measurement and scores today's judge (pYIN after V01) as the baseline. V03 (the cycle tracker) and V04 (the fused voice) add their own rows and assert their targets on it. Tom will record takes. A take dropped into the fixtures folder is scored with no code change.

## Outcome

`cargo run --release -p kazoo-core --example voice_score` prints one markdown table: one row per tracker per take, with onset latency, step latency, median and p95 cents error, octave errors, gross errors, missed voicing and false voicing. The takes are the ten synthetic takes at 48 kHz and every WAV in `kazoo-core/tests/fixtures/voice/`. The same table goes to `target/kz-logs/voice-score.md`. In nextest, the harness checks itself against oracle trackers, and the judge holds loose floors on short synthetic takes. Tom's four takes have file names waiting for them.

## The design (decided; build exactly this)

### The module: `kazoo-core/src/analysis/score.rs` with `score/` beside it

Add `pub mod score;` in `analysis/mod.rs`. The files are:

- `score.rs`: the trait, the types and `score`.
- `score/signals.rs`: the synthetic suite.
- `score/metrics.rs`: the arithmetic.
- `score/wav.rs`: reading takes and the offline reference.
- `score/tests.rs`: the tests.

This is library code, because `kazoo mouth --score` (V08a) calls it. It runs offline and may allocate.

```rust
/// Trajectories are compared every SCORE_STEP samples (0.67 ms at 48 kHz):
/// finer than any tracker's update, coarse enough to keep a 30 s take small.
pub const SCORE_STEP: usize = 32;

/// A pitch tracker under test. The scorer pushes SCORE_STEP samples at a time
/// and reads `hz` after each push.
pub trait TrackerUnderTest {
    fn name(&self) -> &'static str;
    fn push(&mut self, block: &[f32]);
    /// The tracker's pitch now, or `None` for "no pitch".
    fn hz(&self) -> Option<f32>;
}

/// One take: its samples and the truth, one entry per SCORE_STEP.
pub struct Take {
    pub name: String,
    pub rate: u32,
    pub samples: Vec<f32>,
    /// The true pitch at the centre sample of each step; `None` where unvoiced.
    pub truth: Vec<Option<f32>>,
    /// Sample index of every articulation: each unvoiced-to-voiced start, and
    /// each re-sung syllable on a held pitch (V04 scores the gate on these).
    pub onsets: Vec<usize>,
    /// True for synthetic truth, false for a recorded take scored against
    /// offline pYIN.
    pub exact: bool,
}

pub struct Score {
    pub onset_ms_median: f32, pub onset_ms_max: f32, pub onsets_missed: usize,
    pub step_ms_median: f32, pub step_ms_max: f32, pub steps: usize,
    pub cents_median: f32, pub cents_p95: f32,
    pub octave_pct: f32, pub gross_pct: f32,
    pub missed_pct: f32, pub false_pct: f32,
    /// Recorded takes only: the lag (ms) at which the tracker best lines up
    /// with the reference.
    pub aligned_lag_ms: Option<f32>,
}

pub fn score(tracker: &mut dyn TrackerUnderTest, take: &Take) -> Score;
pub fn table(rows: &[(&str, &str, Score)]) -> String; // (tracker, take, score) -> markdown
pub struct PyinUnderTest { /* PitchDetector::new(PitchDetectorConfig::for_rate(rate)), holding its latest estimate */ }
```

`PyinUnderTest::hz` returns the newest voiced estimate's frequency. It holds that until the next estimate, and returns `None` after an unvoiced one.

### The arithmetic (`score/metrics.rs`), exactly

For each step `i` with `truth[i] = Some(t)` and `hz = Some(e)`, the error is `c = 1200 * log2(e / t)` cents.

- **Settle zone.** It runs from each truth *event* for `3 * (rate / t)` samples plus 2 ms, in whole steps. An event is a voiced start (`None` to `Some`), or a *step*: two consecutive voiced truth entries that differ by at least 50 cents. A glide's 3 cents per step and vibrato's 1.3 cents per step are never steps.
- **Scored steps** are voiced truth outside every settle zone. Scored steps where the tracker gives `None` count as *missed*. The rest are classified:
  - *Octave error*: `k = round(c / 1200) != 0` and `|c - 1200k| <= 100`.
  - *Gross error*: not an octave error, and `|c| > 100`.
  - Otherwise `|c|` goes into the cents list, which gives the median and p95 (nearest rank).
- `octave_pct`, `gross_pct` and `missed_pct` are percentages of scored steps.
- **Onset latency.** For each voiced start, the time from the start to the first step where the tracker is within 50 cents and stays within 50 cents for the next `3 * period` samples (at least 3 steps). If that never happens before the note ends, the onset is counted in `onsets_missed` and is not counted in the latency figures.
- **Step latency.** For each step event, the time from the event to the first step within 20 cents that stays within 20 cents for `3 * period` of the new note.
- **False voicing.** Over truth-unvoiced steps, excluding the 30 ms after each voiced region ends (a release allowance: a tracker may hold its last note that long), `false_pct` is the share where `hz` is `Some`.
- Medians and maxima of an empty list are `f32::NAN`, and `table` prints `NaN` as `-`. Recorded takes use the same arithmetic against the reference, plus `aligned_lag_ms` (see `wav.rs`).

### The synthetic suite (`score/signals.rs`), `pub fn suite(rate: u32) -> Vec<Take>`

Every take is deterministic:

- Phase is accumulated in `f64`.
- Noise comes from one private xorshift32 with a fixed seed per take. kazoo-core does not depend on kazoo-fx, so write the three-line generator here.
- Peak 0.5 (−6 dBFS) unless a take says otherwise.
- 200 ms of floor noise before and after each take. The floor is uniform ±0.000316 (−70 dBFS), so a tracker has to reject something rather than read exact zeros.
- The truth is the analytic f0 at each step's centre sample.

| Take | What | Truth onsets |
|---|---|---|
| `sine_ladder` | 110, 220, 440, 880, 1760, 3000 Hz sines, each 400 ms, with 100 ms of floor between | 6 |
| `hum_low` | C2 (65.406 Hz): harmonics 1, 0.5, 0.25 (sine sum, peak normalised to 0.5), 600 ms | 1 |
| `steps` | C3 E3 G3 C4 G3 E3 C3 (equal temperament from A4 = 440), 250 ms each, phase-continuous, no gaps | 1 |
| `vibrato` | 220 Hz centre, 6 Hz, ±50 cents (sinusoidal in cents), 1 s | 1 |
| `glide` | 110 Hz held 200 ms, an exponential glide to 440 Hz over 500 ms, 440 held 200 ms | 1 |
| `octave_bait` | 150 Hz: harmonic 1 at 1.0, 2 at 1.5, 3 at 0.3 (the second harmonic is the loudest), 600 ms | 1 |
| `vowel_ah` | band-limited saw at 130.81 Hz (harmonics up to 0.45 × rate, amplitude 1/k), through three two-pole resonators in parallel: F1 700/BW 80, F2 1220/BW 90, F3 2600/BW 120 Hz (`r = exp(-π BW / rate)`, `a1 = 2r cos(2πF / rate)`, `a2 = r²`, gain `1 - r`), peak normalised to 0.5, 800 ms | 1 |
| `noise_bursts` | 220 Hz 300 ms, then white noise uniform ±0.25 for 200 ms (truth unvoiced), 330 Hz 300 ms, noise 200 ms, 440 Hz 300 ms | 3 |
| `whistle` | 1500 Hz with 5 Hz vibrato of ±20 cents, plus uniform noise ±0.01 (−34 dB under the tone), 800 ms | 1 |
| `rearticulate` | 220 Hz phase-continuous for 1 s in five 200 ms syllables. Between syllables the amplitude ramps down to −24 dB over 10 ms, holds 20 ms, and ramps back up over 10 ms (linear in dB). The truth stays voiced throughout. | 5 (each syllable start) |

Why these: they cover Tom's four recordings (low hum, vibrato phrase, slides, whistle), the two classic octave traps (a loud 2nd harmonic, and formants over a buzz), steps against glides, and the unvoiced material a voice makes between notes. The `rearticulate` truth onsets are for V04's gate. Its pitch is held, so it has no step events here.

### Recorded takes (`score/wav.rs`)

- `pub fn read_take(path: &Path) -> Result<Take>`:
  - Read with `hound` (already a dependency). Mono, or channel 0 of a multi-channel file. 16- or 24-bit integer, or 32-bit float, scaled to ±1. Any rate.
  - An unreadable file, or another format, is `Error::Config` with the path and the reason.
  - The truth is the **offline reference**:
    - `PYINExecutor::new(VOICE_MIN_HZ, VOICE_MAX_HZ, rate, frame, None, Some(SCORE_STEP), None)`, with `frame` as `for_rate(rate)` gives it;
    - run once over the whole take with `Framing::Center(PadMode::Constant(0.0))`, so the HMM sees the whole take and each frame is centred on its step, with no latency;
    - `truth[i] = Some(f0[i])` where `voiced_flag[i]`, else `None`;
    - `onsets` are the voiced starts.
  - `exact` is false.
- `pub fn fixture_takes(dir: &Path) -> Result<Vec<Take>>`: every `*.wav` directly in `dir`, sorted by name. A missing or empty folder gives an empty list.
- **Aligned lag.** Offline centred pYIN has no latency. For a recorded take, `score` searches lags from 0 to 150 ms in `SCORE_STEP` steps. It takes the lag that maximises the count of steps where the tracker, shifted earlier by that lag, is within 50 cents of the reference, and reports it as `aligned_lag_ms`. That lag is the tracker's latency on real singing. The other figures are computed unshifted, so they include the latency the way a listener hears it.

### The fixtures folder

Create `kazoo-core/tests/fixtures/voice/.gitkeep`, so the folder exists in git. Tom's takes go there under the names PLAN.md's Track V gives: `hum-low.wav`, `vibrato-phrase.wav`, `slides.wav` and `whistle.wav`. A WAV under 2 MB (20 s of mono 16-bit at 48 kHz) is committed like any fixture.

### The example: `kazoo-core/examples/voice_score.rs`

- Usage: `voice_score [--rate <hz>] [<wav>...]`. The rate defaults to 48,000 and is only for the synthetic suite.
- It scores every tracker it knows. Here that is only `PyinUnderTest`; V03 and V04 each add one line.
- It scores them over `suite(rate)`, then over `fixture_takes("kazoo-core/tests/fixtures/voice")` (resolved from `CARGO_MANIFEST_DIR`), then over each WAV argument.
- It prints `table(..)` and writes it to `target/kz-logs/voice-score.md`, creating the folder (G02's rule: a program makes the folder it uses).
- It parses arguments by hand, like the other examples. A bad argument prints usage to stderr and exits 2.
- Why an example in release, and not a test: offline pYIN over a 30 s take is minutes of work in a debug build, and nextest runs debug.

### The baseline

Run the example in release on this Mac and paste its table into `briefs/studio/BENCH.md` under `## KZ-V02 voice trackers (baseline: the judge alone)`, with the load average, the date and the head sha. V03 and V04 append their own sections, never edit this one.

## Not this lane

- The cycle tracker (V03).
- The fusion and the gate figures (V04): gate rise and fall times and onset counts use `Take::onsets`, which this lane fills but does not score.
- Any CLI for scoring (V08a's `kazoo mouth --score`).

## Red tests first

All are in `analysis/score/tests.rs`. Each test's takes are generated at 48 kHz and cut to at most 1.5 s, so the debug build runs them in seconds. Red, for each: the module does not exist yet.

- `an_oracle_scores_perfectly`: a tracker that returns the truth, 480 samples late, on `steps`, `vibrato` and `glide`:
  - onset latency is 10 ms ± one step;
  - step latency is 10 ms ± one step;
  - cents median and p95 are 0 (within 0.01);
  - octave, gross, missed and false are 0.
- `an_octave_high_oracle_is_all_octave_errors`: truth × 2 on `sine_ladder` gives `octave_pct == 100` and `gross_pct == 0`.
- `a_silent_tracker_misses_everything`: always `None` gives `missed_pct == 100`, `false_pct == 0`, and every onset in `onsets_missed`.
- `a_tracker_that_never_stops_is_false_voiced`: the truth while voiced, and 220 Hz while unvoiced, gives `false_pct` above 90 on `noise_bursts`, and 0 inside each 30 ms release allowance.
- `the_suite_is_what_the_brief_says`: ten takes with the names above. Each truth is voiced exactly where the table says. `steps` has six step events, `glide` none and `vibrato` none. `rearticulate` has five onsets. Every sample is finite and within ±0.5. Two calls give bit-identical samples.
- `a_recorded_take_is_read_and_referenced`:
  - Write a 1 s, 220 Hz, 16-bit mono WAV at 44,100 Hz with `hound` into `target/kz-logs/voice-score-test/` (create it).
  - `read_take` gives `rate == 44_100` and `exact == false`, and at least 90% of the steps from 100 ms to 900 ms have reference voiced at 220 Hz within 5 cents.
  - Scoring an oracle that returns the reference 960 samples late gives `aligned_lag_ms` of 21.8 ms ± one step.
- `the_fixtures_folder_may_be_empty`: `fixture_takes` on a fresh empty folder, and on a missing path, both give `Ok(vec![])`. On a folder holding the WAV above and a `notes.txt`, it gives one take.
- `the_judge_holds_its_floors` (the baseline as a floor, not a target). `PyinUnderTest` at 48 kHz:
  - `sine_ladder` cut to its first three notes: cents median ≤ 10, octave 0, missed ≤ 10%;
  - `noise_bursts`: false ≤ 10%.

  The same first three notes at 44,100 Hz give a cents median within 5 cents of the 48 kHz figure, which guards V01's rate fix.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-core/examples
nice -n 10 env CARGO_BUILD_JOBS=4 cargo run --release -p kazoo-core --example voice_score
```

Doc tests: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`.

## Definition of done

- [ ] The eight red tests fail first (paste each failure under RED), then pass. Every existing test passes.
- [ ] The example prints the table for the ten synthetic takes (plus any WAV present) and writes `target/kz-logs/voice-score.md`. Paste the table under BENCH.
- [ ] BENCH.md has the `## KZ-V02` section with the table, the load average, the date and the sha.
- [ ] `kazoo-core/tests/fixtures/voice/.gitkeep` is committed.

## Code quality

As PLAN.md's STACK RULES. Each file stays under 1000 lines (signals, metrics and wav are separate for this reason). No `as` casts that clippy pedantic flags: sample indices to `f64` go through `u32::try_from` plus `f64::from`, or through the file's existing helper. Errors come back as `crate::Error`, never as a panic. The example returns `Result` from `main`.

## END

```
START KZ-V02 base=<sha> branch=lane/kz-V02 df=<GB> at <time>
END KZ-V02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the voice_score table, with load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
