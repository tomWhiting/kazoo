# KZ-V12: high notes hold: a sung or whistled note from 800 to 2200 Hz stays on its octave

Line numbers are from `main` at `508b869` and may have moved. Find each item by what it does, and never rename anything this brief does not rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place: Track V, after V06 (merged). Drafted 14 Oct 2026, 09:4x, for Cassio to check and commit. Every number below is decided under Tom's rule that internal calls are not parked on him.

Time: about 30 minutes for steps 1 and 2; up to 20 more if step 3 calls for fix B1 or B2. Crates: kazoo-core.

Depends: KZ-V06 (merged). May run beside KZ-V11: both edit `kazoo-core/src/engine/mod.rs`, this lane in `EngineConfig::default`, `judge_config`, one check in `start` and the judge tests, V11 in `start`'s stream and mic code, `hold_streams` and `RingBufferCapacities`, so the merge is line by line. KZ-V05b (in flight) edits `voice/quantise.rs` only; no shared file.

Files (ten, BENCH.md aside; the four marked * are edited only if step 3 calls for B1 or B2): `kazoo-core/src/analysis/pitch.rs`, `kazoo-core/src/engine/mod.rs`, `kazoo-core/src/voice/tracker.rs`, `kazoo-core/src/voice/tests.rs`, `kazoo-core/src/analysis/score.rs` (one `mod` line), `kazoo-core/src/analysis/score/fused.rs`, `kazoo-core/src/analysis/score/signals.rs`, `kazoo-core/src/analysis/score/high_notes.rs` (new), `kazoo-core/src/analysis/cycle.rs`*, `kazoo-core/src/analysis/cycle/tests.rs`*, and `briefs/studio/BENCH.md`. (B2 edits `voice/tracker.rs` and `voice/tests.rs`, which step 1 already edits.)

## Why

Tom, 14 Oct 2026, 09:1x to 09:2x, singing and whistling into `kazoo mouth` (which runs kazoo-tui's engine, `kazoo-cli/src/main.rs:44`): high notes, above the guitar's high E around its 15th fret, "it really struggles to grab ... flapping really, really quickly between like the note and the octave, maybe the thing below". Then: "800 - 2200 hz is what im seeing but it all moves too quick for me to get anything solid." He trusts his pitch, so this is the tracker's fault. "There is a place for that" (octave flapping as an effect), but never by accident.

### The mechanism, found by reading

**The engine's judge cannot hear above 1000 Hz, and the fusion folds the tracker down to whatever the judge says.**

1. kazoo-tui starts the engine with `EngineConfig::default()` (`kazoo-tui/src/run.rs:85-88`), whose `pitch` is `PitchDetectorConfig::engine_default()` (`engine/mod.rs:104`): 60 to 1000 Hz at 0.1-semitone bins (`analysis/pitch.rs:74-86`). V01 kept that range for "the engine's detector until V06" and said V06 retires it (KZ-V01, design point 3). V06 then wired the judge thread through `judge_config(&config.pitch, sample_rate)` (`engine/mod.rs:121-130`, `:203-207`), which keeps the caller's range on purpose (KZ-V06, step 1). So the judge in `kazoo mouth` today searches only lags of 48 samples and longer at 48 kHz (`48,000 / 1000`), and a hop of a whole 2048-sample window (`with_longest_hop`, `pitch.rs:87-108`).
2. A note at 1500 Hz has a period of 32 samples. Its first YIN dip inside 48 to 800 samples is at 64 samples, so pYIN reports 750 Hz, voiced and sure. At 1100 Hz (period 43.6) it reports 550 Hz. At 800 Hz (period 60) the true period is inside the range and the verdict is right. At 2200 Hz (period 21.8) the first dip in range is at three periods, 733 Hz.
3. The fusion (`voice/tracker.rs:205-230`) takes octave from the judge: the verdict is 1.0 octave under the tracker's 1500 Hz, within `FOLD_WINDOW`, so `fold = −1`, the frame reads 750 Hz, and `cycle.hint(rate / 750)` asks the tracker to try 64 samples (`:225-227`).
4. The tracker takes the hint (its NSDF at 64 samples is high on any periodic note, and a hint at `CLARITY_ON` wins over every candidate, `analysis/cycle.rs:424-470`). That move is 1200 cents, which the fusion counts as a sung leap (`LEAP_CENTS`, `voice/tracker.rs:193-203`), so folding stops until a verdict whose window starts after it: up to two 2048-sample hops, 85 ms. Meanwhile the hint lapses after four cycles, the tracker's own one-cycle interval (32 samples) is again the smallest candidate that passes (`consider`, `cycle.rs:489-503`), and it returns to 1500 Hz: another 1200-cent leap. The next fresh verdict says 750 again.

The frames therefore alternate between the note and the octave below on a cycle of tens of milliseconds, on every note between about 1000 and 1600 Hz, and read wrongly or disagree above (the 2200 Hz case is 1.585 octaves off, outside `FOLD_WINDOW`, so it is not folded but halves confidence). That is "the note and the octave, maybe the thing below", over 800 to 2200 Hz.

Why no test saw it: the scoreboard's fused tracker builds its judge with `PitchDetectorConfig::for_rate(rate)`, 50 to 4000 Hz (`analysis/score/fused.rs:24`), so `voice_meets_its_targets` passes on 1760 and 3000 Hz sines (`score/signals.rs:179-190`) while the engine runs a different judge. And V04's review already recorded the leap half of it (QUEUE.md: "the cycle tracker moving an octave from our own hint ... counts as a sung leap and turns folding off for ~43 ms").

### What reading cannot settle

Whether the tracker alone, and the fusion with a full-range judge, also slip at these pitches with breath noise and with splices. Two facts make it possible:
- The NSDF window at 2 kHz is one 24-sample period (`window`, `cycle.rs:657-670`: half the lag, at least 0.5 ms, but never more than the period), and the tracker lets go after two periods without an accepted mark (`UNLOCK_PERIODS`, `:47-49`), which is 1 ms at 2 kHz against 9 ms at 220 Hz. A breath burst that drops one or two cycles under `CLARITY_ON` unlocks a high note, and a fresh search may then take two periods as easily as one.
- The engine gives the tracker only the mic samples it read (`processing.rs:501-509`, `voice::Input::Mic(num_read)`), and skips a stale backlog (`:473-478`). Tom's first test had the Mac mic in and the AirPods out: two devices, two clocks. A late or doubled mic callback is then a splice of up to one block (128 samples, 2.7 ms) in the tracker's signal, which unlocks a 2 kHz note and leaves a 220 Hz note locked.

So this lane measures first (step 2) and builds one, both or neither of two fixes by a stated rule (step 3).

## Outcome

`kazoo mouth`'s judge is the voice judge (50 to 4000 Hz, the scoreboard's), and the engine refuses a judge narrower than the tracker. The V02 scoreboard has sixteen high-note takes at 800, 1100, 1500 and 2200 Hz (whistles, sung notes with a strong 2nd or 3rd harmonic, and whistles with splices), and on every one of them, through the cycle tracker alone, through the fused voice tracker, and through the engine's own judge, no reading more than 100 ms after the note starts is an octave off, over three seconds of held note.

## The design (decided; build exactly this)

### Step 1, always: the engine judges with the voice judge (fix A)

- `analysis/pitch.rs`: remove `PitchDetectorConfig::engine_default` (`:74-85`); its only user is `EngineConfig::default`. Add `pub fn with_hop_pyin_holds(self) -> Self`: this configuration if pyin builds it (`validate`, `pitch.rs:273-316`), else with its hop halved until it does, down to 64 samples; if none builds, unchanged, so `PitchDetector::new` reports why. Doc: the judge keeps its 512 × scale hop wherever pyin can hold it, and halves only where it cannot.
- `engine/mod.rs`:
  - `EngineConfig::default().pitch` is `PitchDetectorConfig::for_rate(44_100)` (`start` replaces the rate): 50 to 4000 Hz, `JUDGE_RESOLUTION`, threshold 0.3. Its field doc says so.
  - `judge_config(asked, rate)` returns `PitchDetectorConfig { min_frequency: asked.min_frequency, max_frequency: asked.max_frequency, voiced_threshold: asked.voiced_threshold, ..PitchDetectorConfig::for_rate(rate) }.with_hop_pyin_holds()`: the voice judge's window, hop and resolution at the device's rate, exactly what the scoreboard runs. The doc's "once per window at 0.1-semitone resolution" paragraph (`:114-120`) is replaced with that. Reason for 0.34-semitone bins and a 512 hop over 0.1 and a whole-window hop: since V06 the engine's Hz comes from the cycle tracker, and the judge only rules on octave and voicing (`JUDGE_RESOLUTION`'s own doc, `pitch.rs:15-20`); a 512 hop brings a fresh verdict every 10.7 ms instead of 43.
  - `pub fn check_judge_range(asked: &PitchDetectorConfig) -> crate::Result<()>`: `Error::Config` unless `asked.min_frequency <= VOICE_MIN_HZ` and `asked.max_frequency >= VOICE_MAX_HZ`, with the text "the judge's range {min} to {max} Hz must cover the tracker's 50 to 4000 Hz: a narrower judge folds every note above its top down an octave". `start` calls it before `judge::spawn`. Reason: the fold trusts the judge's octave, so the judge must be able to hear every pitch the tracker can report.
  - Tests in the same file, changed deliberately (say so under CHANGED): `the_engine_judges_at_the_device_rate` asserts `hop_length == 512`, the 440 Hz verdicts within 20 cents (half of a 34-cent bin is 17, as V01 set for `for_rate`), and one verdict per completed 512-sample hop; `the_engine_builds_its_judge_at_every_device_rate` keeps every rate it lists and asserts `hop_length == PitchDetectorConfig::for_rate(rate).hop_length` at every rate from 22,050 up (below that, `with_hop_pyin_holds` may halve it; the test prints what it chose); `engine_config_default_values` asserts 50, 4000 and `JUDGE_RESOLUTION`. If a listed rate (8,000 is the likely one: 4000 Hz is its Nyquist) is refused even at a 64-sample hop, `judge_config` holds `max_frequency` to `0.45 × rate` for that rate and the test asserts that; say so under FOUND.
- `voice/tracker.rs`, the hint is not a leap (fix A2, the V04 review's finding): store `hinted: Option<(u64, f32)>` (sample, Hz) when `frame` calls `self.cycle.hint(..)` (`:225-227`). In the leap check (`:193-203`), a new cycle Hz that is within 50 cents of the hinted Hz and arrives within `HINT_LEAP_MS` of the hint does not move `last_leap`, and clears `hinted`. `pub const HINT_LEAP_MS: f32 = 10.0;` with its reason as its doc: a hint is tried for four cycles (`HINT_MARKS`, `cycle.rs`), at most 80 ms at 50 Hz but under 10 ms from 400 Hz up, where a slip costs most; a hint that lands later is treated as a sung change, as now. `reset` clears it. Make `last_leap` `pub(super)` for the test.
- The other users of `judge_config(&EngineConfig::default().pitch, ..)` (`analysis/pitch/tests.rs:455`, `benches/voice.rs:83-99`) keep compiling and now measure the voice judge; the `engine` bench rows then duplicate the `judge` rows, which is correct and is not edited here.

### Step 2, always: the high-note rows, and the measurement

- `analysis/score/signals.rs`: `pub fn high_suite(rate: u32) -> Vec<Take>`, sixteen takes built by the existing `make` (`:71-175`, 200 ms of floor each end), each with a 3,000 ms body at `f · 2^(15 · sin(2π · 5 · t) / 1200)` (a ±15-cent, 5 Hz wobble: a held note is never dead still) for `f` in 800, 1100, 1500 and 2200 Hz, named `whistle_800`, ..., `splice_2200`. Four new `Sound` arms, each added to `make`'s peak normalisation list (`:137-152`):
  - `HighWhistle`: `0.5 sin φ + 0.0158 sin 2φ + 0.05 · noise` (second harmonic 30 dB down; breath about 22 dB under the tone).
  - `SungSecond`: `sin φ + 1.2 sin 2φ + 0.3 sin 3φ + 0.02 · noise` (a head-voice "ee" with the second harmonic on a formant).
  - `SungThird`: `sin φ + 0.4 sin 2φ + 1.1 sin 3φ + 0.02 · noise`.
  - `Spliced`: `HighWhistle`, with `φ` advanced by `2π · hz · 128 / rate` every `rate / 20` samples of the body (a 128-sample splice every 50 ms, as a late or doubled 128-frame mic callback leaves it; the truth is unchanged, because the pitch is unchanged).
  The ten V02 takes and `suite` are untouched.
- `analysis/score/fused.rs`: `pub fn with_judge(rate: u32, config: PitchDetectorConfig) -> crate::Result<Self>`; `new(rate)` becomes `with_judge(rate, PitchDetectorConfig::for_rate(rate))`. The onset detector's scale stays `for_rate(rate)`'s, as now.
- `analysis/score/high_notes.rs` (`#[cfg(test)] mod high_notes;` in `score.rs`): the tests below, using the `Recorded` wrapper's idea (`score/tests.rs:675-694`: copy it here, it is private to that file). Each test writes `score::table(..)` of its rows to `target/kz-logs/kz-v12/<test name>.md` (the folder the scoreboard's tests already use, `score/tests.rs:276-279`) before it asserts, so the numbers exist whether it passes or fails.

**Targets** (every row, at 48 kHz unless named):
- **No octave flip:** no scoring step later than 100 ms after the note's onset reads within 100 cents of a whole, nonzero number of octaves from the truth (`is_octave_error`, `score/tests.rs:696-702`). That is three seconds of held note with no flip. Why 3 s: it is 2,400 to 6,600 cycles and about 280 judge verdicts at a 512 hop, so a flip that happened once a second would show three times; and it is longer than any note Tom described as "too quick to get anything solid".
- `octave_pct == 0`, `gross_pct <= 1`.
- `missed_pct <= 2` (whistle, sung) and `<= 5` (splice).
- `cents_median <= 5`, `cents_p95 <= 25` (the ±15-cent wobble read half a cycle late is under 5 cents).
- Onset within 2 periods + 1 ms (whistle, splice) or 3 periods + 1 ms (sung, as `vowel_ah` and `octave_bait` have), per event, as `voice_meets_its_targets` checks it (`score/tests.rs:730-746`).

**Step 2's measurement:** after step 1 is green, run `high_whistles_hold`, `high_sung_seconds_hold`, `high_sung_thirds_hold`, `high_splices_hold` and `high_notes_hold_at_96k`, each scoring every row through `cycle` (`CycleUnderTest`) and `voice` (`FusedUnderTest::new`), and paste their tables under BENCH.

### Step 3, by rule: B1, B2, both or neither

- Every row meets its targets on both trackers: build neither; the rows stay as guards. Say so under DONE.
- Any `cycle` row fails: build **B1**, then run everything again.
- Then, any `voice` row fails whose `cycle` row passes: build **B2**, then run everything again.
- Rows still failing after both: stop; paste the tables under BLOCKED ON. Never loosen a target, mark a row ignored, or drop a take to pass.

**B1, the tracker holds a high note** (`analysis/cycle.rs`, tests in `analysis/cycle/tests.rs`):
- `UNLOCK_MIN_MS: f32 = 3.0`: unlock after `max(UNLOCK_PERIODS × period, UNLOCK_MIN_MS)` with no accepted mark (the check in `push`, `:306-310`). Reason: it bridges one lost or doubled engine block (2.7 ms at 128 frames and 48 kHz) and a breath dip at any pitch, and changes nothing below 667 Hz, where two periods are already longer. A whistle lets go 3 ms after it stops instead of 1.
- `MIN_WINDOW_MS` 0.5 → 1.0, and `window`'s own size becomes `max(lag / 2, min(min_window, 2 × lag))` (`:663-670`): at least 1 ms, but never more than two periods. Reason: at 2 kHz the old window was one 24-sample period, so a −22 dB breath moves its NSDF by about ±0.1 across `CLARITY_ON`; 48 samples halve that variance. The latency budget still holds: a first reading needs window plus lag, 72 samples (1.5 ms) at 2 kHz against 2 periods + 1 ms = 2 ms, 48 samples (1.0 ms) at 3 kHz against 1.67 ms, 36 (0.75 ms) at 4 kHz against 1.5 ms; at 800 Hz, 108 samples (2.25 ms) against 3.5 ms. Update `MIN_WINDOW_MS`'s doc to say this.
- `HOLD_CLARITY: f32 = 0.70` and `HOLD_CENTS: f32 = 50.0`: while locked, a candidate whose refined period is within 50 cents of the held period is accepted at NSDF ≥ 0.70; every other candidate still needs `CLARITY_ON` (in `consider`, `:489-503`). Reason: the held note is the likeliest answer at every cycle, so it keeps its lock through a noisy cycle, and a multiple of it (the octave below) can only replace it when the held period itself has failed, not when both score near 0.8 and noise picks one.
- Red tests: `a_breathy_high_note_holds_its_period` (1500 Hz at 0.5 plus seeded uniform noise ±0.08, 1 s at 48 kHz: after 50 ms no estimate within 100 cents of 750 or 3000 Hz, and `Some` at ≥ 98% of 32-sample steps); `a_splice_at_a_high_note_is_bridged` (2000 Hz with 128 samples cut out at 0.5 s: never `None` across the splice, and within 50 cents of 2000 Hz again within 3 ms of it); `a_high_note_lets_go_after_three_ms` (2000 Hz then the −70 dB floor: `Some` 2 ms after the end, `None` by 3 ms + one period). `it_lets_go_after_two_periods_of_silence` (220 Hz) and every V03 target are unchanged and must pass. If a B1 test is already green before B1, paste that and keep it as a guard.

**B2, one verdict never folds** (`voice/tracker.rs`, tests in `voice/tests.rs`):
- `FOLD_AGREE: u8 = 2`: the applied fold moves to a new nonzero octave only after two consecutive fresh verdicts give that same octave; it goes to 0 at once whenever the frame's own fold would be 0 (a verdict that agrees with the tracker, a stale or unsure verdict, or a leap), as now. Count verdicts by `judge_at` changing, not by frames (a verdict is read by every frame until the next one). Reason: a fold rewrites the note, so one stray verdict must not, while a real octave slip is caught one hop (10.7 ms) later than now, inside `vowel_ah`'s and `octave_bait`'s "no octave error after 100 ms".
- Red test: `one_stray_verdict_does_not_fold_a_high_note` (a 1500 Hz sine for 1 s, verdicts every 512 samples at 1500 Hz and 0.9, except a single one at 750 Hz at 0.5 s: no frame after 50 ms reads within 100 cents of 750 Hz). `the_judge_folds_an_octave_slip` and `a_sung_octave_leap_is_not_folded_back` (`voice/tests.rs:60-131`) must pass unchanged.

## Not this lane

- An octave-flapping effect. Tom: "There is a place for that", but never by accident. If it is wanted it is a quantiser or synth mode on the mouth (V08b's controls), built on purpose, after this lane makes the plain pitch hold.
- The mic path's own faults: counting the engine's mic underruns, and P01's elastic ring for the mouth. This lane makes the tracker survive a splice; it does not stop the splices (see KZ-V11's Not this lane).
- The mouth's held readout: V08b's 4-second trail is how a value is read on screen (QUEUE.md, 14 Oct 09:2x).
- Tom's own takes: when he records `whistle.wav` and a high sung take into `kazoo-core/tests/fixtures/voice/` (PLAN.md, Track V, "For Tom by hand"), the existing WAV path scores them; no code here.
- Notes above 2200 Hz and below 800 Hz: the V02 and V03 targets cover them already.

## Red tests first

Step 1 and 2 tests, in `analysis/score/high_notes.rs` unless named otherwise:

1. `the_engine_judge_is_the_voice_judge`: for 22,050, 44,100, 48,000, 96,000 and 192,000 Hz, `judge_config(&EngineConfig::default().pitch, rate)` has the same `min_frequency`, `max_frequency`, `resolution`, `frame_length`, `hop_length` and `voiced_threshold` as `PitchDetectorConfig::for_rate(rate)`. Red: 60, 1000, 0.1 and a 2048 hop.
2. `a_narrow_judge_is_refused`: `check_judge_range` refuses 60 to 1000 Hz with the "must cover" text and accepts `for_rate`'s range. Red: no such check.
3. `high_notes_hold_through_the_engine_judge`: the `whistle_*` and `sung2_*` rows through `FusedUnderTest::with_judge(48_000, judge_config(&EngineConfig::default().pitch, 48_000))`, to the targets. Red on `main`: octave flips at 1100 and 1500 Hz at least; paste which rows fail and their flip counts.
4. `a_hint_landing_is_not_a_leap` (`voice/tests.rs`): a 150 Hz tone whose second harmonic dominates (as the second half of `the_judge_folds_an_octave_slip` builds it), verdicts at 150 Hz every 512 samples: when the tracker's estimate lands on the hinted period, `last_leap` does not move to that sample. Red: it does.
5. Step 2's `high_whistles_hold`, `high_sung_seconds_hold`, `high_sung_thirds_hold`, `high_splices_hold` (each: four frequencies, `cycle` and `voice`, 48 kHz) and `high_notes_hold_at_96k` (`whistle_2200` and `sung2_2200` at 96 kHz, `cycle` and `voice`: Tom's interface runs at 96 kHz). These are the measurement: whether they are red on `main` is not predicted; paste their first result either way, and step 3 follows from it.
6. B1's and B2's tests above, only if step 3 builds them.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `sysctl -n vm.loadavg` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-core/src/voice kazoo-core/src/engine
```

Doc tests, since doc comments change: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`. While iterating, filter nextest to `-E 'test(high) | test(judge) | test(cycle) | test(voice)'`; run the whole of kazoo-core once at the end.

## Definition of done

- [ ] Tests 1 to 4 fail first on `main` (paste), then pass; test 5's first results are pasted; every kazoo-core test passes, including every V02, V03 and V04 target.
- [ ] `git grep -n engine_default kazoo-core` finds nothing.
- [ ] BENCH.md has `## KZ-V12 high notes` with the step 2 tables (before and, if B1 or B2 was built, after), the load average, the date and the sha, and one line saying which of B1 and B2 step 3 built and why.
- [ ] Every new constant has its reason as its doc comment.
- [ ] Not the builder's, listed under NOT CONFIRMED: after landing, Cassio installs and asks Tom to whistle and sing held notes between 800 and 2200 Hz into `kazoo mouth`, Mac mic in and AirPods out as on 14 Oct, and to say whether the reading holds. The builder never opens Tom's mic or speakers.

## Code quality

As PLAN.md's STACK RULES. `CycleTracker::push` and `VoiceTracker::process` run on the output callback: no allocation, lock, I/O, format or panic; `processing_never_allocates` in both modules still passes. The new takes are built off the audio path and may allocate. `high_notes.rs` and `signals.rs` stay under 1000 lines. No `as` casts clippy pedantic flags beyond the patterns `signals.rs` already uses.

## END

```
START KZ-V12 base=<sha> branch=lane/kz-V12 df=<GB> load=<1m> at <time>
END KZ-V12 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the step 2 tables before and after; which of B1 and B2 were built, and why
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
