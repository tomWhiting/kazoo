# KZ-V01: pYIN judges at the device's rate, every 512 samples, from the newest frame, from 50 Hz to 4 kHz

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core. Time: about 30 minutes. Depends: nothing beyond main.

Why: Tom wants to "sing the melody into the synth ... or whistle it ... hertz for hertz". Track V puts a fast cycle tracker in front (V03) and keeps pYIN behind it as the slow judge of octave and voicing (V04). The judge is wrong today in four ways, and each one is fixed here, before anything is scored on it (V02):

1. **Wrong rate.** `engine::start` builds the detector from `config.pitch` (`kazoo-core/src/engine/mod.rs:183`), and that config's `sample_rate` is the default 44,100 (`analysis/pitch.rs:31`). kazoo-tui passes `EngineConfig::default()` (`kazoo-tui/src/run.rs:85-88`), but the stream runs at the device rate (`engine/mod.rs:176`). On a 48 kHz device every pitch reads ×44.1/48, which is 147 cents flat.
2. **Too slow.** Buffers do not overlap (`pitch.rs:147-150`), so there is one estimate per 2048 samples (43 ms at 48 kHz).
3. **Averaged.** The Hz of every returned frame is averaged (`pitch.rs:186-217`). The frames come back centre-padded, so the first and last are half zeros. An octave flip inside the buffer reads as a note between the two octaves.
4. **Range too narrow.** The range is 60 to 1000 Hz (`pitch.rs:29-30`). A whistle (1 to 3 kHz) is never voiced, and a low hum's bottom notes (G1 is 49 Hz) are cut off.

## Outcome

On any device rate, a 440 Hz sine reads 440 Hz within 5 cents. An estimate comes every 512 samples at 44.1 and 48 kHz (10.7 ms at 48 kHz), and every 1024 or 2048 samples at higher rates, so the window keeps its length in milliseconds. Each estimate is the newest pYIN frame, never an average of frames. Whistles up to 4 kHz and hums down to 50 Hz are voiced. A bench gives the cost of one judge run.

## The design (decided; build exactly this)

1. **The voice range, `kazoo-core/src/analysis/mod.rs`.** Add:
   ```rust
   /// The lowest pitch the voice trackers look for: G1 is 49 Hz, the bottom of a low male hum.
   pub const VOICE_MIN_HZ: f32 = 50.0;
   /// The highest: a whistle tops out near B7 (3951 Hz).
   pub const VOICE_MAX_HZ: f32 = 4000.0;
   ```
   V03's cycle tracker uses the same two constants.
2. **`PitchDetectorConfig` (`pitch.rs:12-36`).**
   - Add `pub hop_length: usize` ("samples between judge runs").
   - Add `pub const fn for_rate(rate: u32) -> Self`. It sets `sample_rate: rate`, `min_frequency: VOICE_MIN_HZ`, `max_frequency: VOICE_MAX_HZ`, `voiced_threshold: 0.3`, `frame_length: 2048 * scale` and `hop_length: 512 * scale`. The scale is a `usize` picked by comparison, with no cast: 1 up to 48,000, 2 up to 96,000, 4 up to 192,000, and 8 above. So the scale is 1 at 44.1 and 48 kHz, 2 at 88.2 and 96 kHz, and 4 at 176.4 and 192 kHz.
     - Why 2048 at 48 kHz: pYIN's longest period is `frame_length - win_length - 1` (pyin 1.2.0, `pyin.rs:138`). With the default `win_length = frame_length / 2`, that is 1023 samples, which is 47 Hz at 48 kHz, just under `VOICE_MIN_HZ`. Doubling the frame at double the rate keeps that floor.
     - Why a hop of a quarter frame: it is pYIN's own default hop (`pyin.rs:131`). 10.7 ms is fast enough for a judge that only rules on octave and voicing.
     - A `for_rate` below 8001 Hz cannot hold `VOICE_MAX_HZ` under Nyquist. `PitchDetector::new` already refuses that (`pitch.rs:92-98`), and that refusal stays the only check.
   - `Default` becomes `Self::for_rate(44_100)`.
   - The `sample_rate` doc becomes "the stream's rate; `engine::start` replaces it with the device's".
3. **The engine (`engine/mod.rs`).**
   - Add `pub fn judge_config(asked: &PitchDetectorConfig, rate: u32) -> PitchDetectorConfig`. It returns `PitchDetectorConfig { min_frequency: asked.min_frequency, max_frequency: asked.max_frequency, voiced_threshold: asked.voiced_threshold, ..PitchDetectorConfig::for_rate(rate) }`. The caller chooses the range and threshold; the device decides the rate, window and hop.
   - `start` calls `PitchDetector::new(judge_config(&config.pitch, sample_rate))` at `:183`.
   - `EngineConfig::pitch`'s doc (`:72-73`) says the rate, frame and hop are taken from the device.
4. **A sliding window (`PitchDetector`, `pitch.rs:55-160`).**
   - Add `pub const JUDGE_FRAMES: usize = 5;` with the doc: "pYIN frames per run; the HMM picks the newest frame's octave with four frames of context (53 ms at 48 kHz) behind it".
   - The buffer becomes a ring `history: Vec<f64>` of `frame_length + (JUDGE_FRAMES - 1) * hop_length` samples (4096 at 48 kHz), zero-filled, with a write index. A second `scratch: Vec<f64>` of the same length holds the window in time order for each run. Both are allocated in `new`.
   - Add a counter `since_run: usize`.
   - `push_samples`: for each sample, sanitise it, write it into the ring and increment `since_run`. When `since_run == hop_length`:
     - copy the ring into `scratch`, oldest first;
     - run `self.executor.pyin(&self.scratch, f64::NAN, Framing::Valid)`. `Valid` gives exactly `JUDGE_FRAMES` frames, with no padding;
     - build the estimate from the **last** element of `f0`, `voiced_flag` and `voiced_prob`;
     - set `since_run = 0`.

     Return the newest estimate made during this call, if any. A call longer than one hop makes several runs and returns only the last; say so in the doc.
   - The ring starts as zeros and counts as full. Zeros read as silence, so the first runs are unvoiced and there is no start-up path to get wrong.
   - The estimate is voiced when `voiced_flag[last]` is true, `voiced_prob[last] >= voiced_threshold`, and `f0[last]` is finite and above 0. `frequency` is `f0[last]` as `f32`, with no averaging. `voiced_probability` is `voiced_prob[last]`, clamped to 0..=1, and 0 when not finite. `midi_note` is as today.
   - The "Average over all returned frames" block (`:186-234`) is deleted.
   - `reset` zeroes the ring, the write index and `since_run`.
   - The executor is built with `win_length: None` and `hop_length: Some(config.hop_length)`. `new` refuses `hop_length == 0` and `hop_length > frame_length` with `Error::Config`.
5. **The bench, `kazoo-core/benches/voice.rs`** (new; add `[[bench]] name = "voice" harness = false` to `kazoo-core/Cargo.toml`). It has one group, `judge`, and one bench, `judge/hop/48k`:
   - the time of one `push_samples` call of exactly 512 samples that triggers one run;
   - on a detector from `for_rate(48_000)`;
   - already primed with 4096 samples of a 130.81 Hz band-limited saw (harmonics 1 to 20 at amplitude 1/k, peak 0.5).

   The budget is 2.1 ms, which is 20% of the 10.7 ms hop. If the bench reads over 2.1 ms on a quiet machine, set `JUDGE_FRAMES = 3`, run the bench again, and record both numbers. The run's cost scales with the frame count, and three frames still give the HMM two frames of context. Write the numbers into `briefs/studio/BENCH.md` under a new section `## KZ-V01`, in the file's table style.

## Not this lane

- The scoreboard (V02), the cycle tracker (V03) and the fusion (V04).
- Moving pYIN off the analysis thread (V06). It stays where it is here, and only its configuration and window change.
- kazoo-tui's pitch display, which shows whatever estimate arrives.

## Red tests first

Each one fails today for the reason given. Put the tests in `kazoo-core/src/analysis/pitch/tests.rs` (`#[cfg(test)] mod tests;` in `pitch.rs`, the existing tests moved there unchanged), plus one test in the engine's tests.

- `pitch::tests::config_for_rate_scales_the_window`: `for_rate` gives 2048/512 at 44,100 and 48,000, 4096/1024 at 88,200 and 96,000, and 8192/2048 at 192,000. Every rate has min 50 and max 4000. Red: there is no `for_rate` or `hop_length`.
- `engine::tests::the_engine_judges_at_the_device_rate`:
  - `judge_config(&EngineConfig::default().pitch, 48_000)` has `sample_rate == 48_000`, `frame_length == 2048` and `hop_length == 512`.
  - A detector built from it, fed 1 s of a 440 Hz sine at 48 kHz in 128-sample pushes, gives voiced estimates after the first 4096 samples that are all 440 Hz within 5 cents.

  Red: there is no `judge_config`. Today the same sine through `EngineConfig::default().pitch` reads about 404 Hz. Put that figure in the test's doc as the reason the test exists.
- `pitch::tests::estimates_come_every_hop`: 1 s at 48 kHz in 128-sample pushes gives 93 or 94 estimates (48,000 / 512 = 93.75). Red: today it gives 23.
- `pitch::tests::a_note_change_reads_the_new_note_not_an_average`: at 48 kHz, 220 Hz for 0.5 s, then 330 Hz (phase-continuous) for 0.5 s. Every voiced estimate from a run that ended at least 4096 samples after the change reads 330 Hz within 10 cents. No voiced estimate anywhere in the take lies between 240 and 310 Hz. Red: averaging over centre-padded frames gives in-between values and estimates that come late.
- `pitch::tests::a_whistle_is_voiced`: 2500 Hz, peak 0.5, plus uniform noise of ±0.01 from a fixed xorshift seed, for 1 s at 48 kHz. At least 90% of the estimates after the first 4096 samples are voiced and within 10 cents. Red: the maximum is 1000 Hz.
- `pitch::tests::a_low_hum_is_voiced`: 55 Hz with a second harmonic at half amplitude, for 1 s at 48 kHz. At least 90% of the estimates after the first 4096 samples are voiced and within 10 cents. Red: the minimum is 60 Hz.
- `pitch::tests::a_bad_hop_is_refused`: hops of 0 and of `frame_length + 1` give `Err(Error::Config(_))`. Red: there is no field.

Every existing pitch test passes, moved unchanged. `detects_440hz_sine` uses its own explicit config, so it gains `hop_length: 512` and nothing else.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-core/src/engine/mod.rs kazoo-core/benches
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p kazoo-core --bench voice -- 'judge'
```

Doc comments change, so also run `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core`. Because `EngineConfig` and `PitchDetectorConfig` gain a field, kazoo-tui must still compile: run `nice -n 10 env CARGO_BUILD_JOBS=4 cargo check -p kazoo-tui` once, as the only touch outside this lane's crate.

## Definition of done

- [ ] The seven red tests fail first (paste each failure under RED), then pass. Every existing test passes.
- [ ] `rg 'sum_freq|voiced_count' kazoo-core/src/analysis` finds nothing.
- [ ] `engine::start` builds the detector through `judge_config` at the device's rate.
- [ ] `judge/hop/48k` is in BENCH.md with the machine's load average. If it was over 2.1 ms, `JUDGE_FRAMES` is 3 and both numbers are written down.

## Code quality

As PLAN.md's STACK RULES. The judge runs on the analysis thread, where pyin's own allocations are allowed (`analysis_thread.rs` docs), and this lane adds no allocation outside `new`. No `as` casts that clippy pedantic flags: `f64::from` widens, and the narrowing to `f32` for `frequency` goes through the file's existing pattern. `pitch.rs` stays under 1000 lines, because its tests move out.

## END

```
START KZ-V01 base=<sha> branch=lane/kz-V01 df=<GB> at <time>
END KZ-V01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: judge/hop/48k (and the 3-frame figure if taken), with load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
