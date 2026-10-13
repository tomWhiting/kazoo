# KZ-F01: three faults the reviews found outside their lanes: the pitch judge refuses a table it cannot afford, an empty `say` render is tried once more, and the spring's reset sounds like a fresh spring

Written by Cassio, 13 Oct 2026, from the Opus reviews of KZ-V01 (head dc5777a) and KZ-W01b (head 26526da). Each fault was on main before those lanes; none was caused by them. Three crates, five files at most.

## Outcome

1. `PitchDetector::new` never builds a pYIN table larger than 64 MiB. A config whose table would be larger is refused with `Error::Config` naming the size, before any allocation.
2. When macOS `say` exits successfully but writes a WAV with no frames, the render is tried once more before the request fails.
3. A spring reverb that is reset renders exactly what a freshly built spring with the same knobs, rate and seed renders, at every knob setting, including every knob at maximum.

## The design (decided; build exactly this)

### 1. The judge's table has a ceiling (`kazoo-core/src/analysis/pitch.rs`)

pYIN 1.2 builds a dense transition matrix of `(2 × pitch_bins)²` `f64`s inside `new`, where `pitch_bins` is the figure `check_pyin_limits` already computes (`:341-342`). At resolution 0.01 over 50–4000 Hz that is 15,174² × 8 bytes, about 1.8 GB. `validate` accepts resolution from 0.01 (`:300`), so a caller can ask for it today.

- Add `const PYIN_TABLE_CEILING: f64 = 64.0 * 1024.0 * 1024.0;` (bytes) beside the other pitch constants, with a doc comment saying where the figure comes from: the largest table any caller uses is the engine's (0.1 over 60–1000 Hz) and the wall listener's (0.1 over the listener's range), both under 19 MB; 64 MiB leaves room for a caller at 0.1 over the full 50–4000 Hz voice range (about 18.4 MB) and refuses 0.05 over that range (about 73.7 MB).
- In `check_pyin_limits`, after `pitch_bins` is known and before the transition-width check, compute `let table = (2.0 * pitch_bins).powi(2) * 8.0;` and refuse when `table > PYIN_TABLE_CEILING` with `Error::Config(format!("resolution {} over [{fmin}, {fmax}] Hz needs a {:.1} MiB pyin table, more than the 64 MiB ceiling", config.resolution, table / 1_048_576.0))`.
- The accepted resolution range stays 0.01..1.0 (exclusive of 1.0 as now): a narrow frequency range at a fine resolution is still allowed when its table fits.
- `with_longest_hop` (KZ-V01) goes through `validate`, so it inherits the ceiling with no change.

Reason for a ceiling rather than a higher minimum resolution: the cost is the product of range and resolution, so a floor on either alone would refuse cheap configs or allow dear ones.

### 2. An empty `say` render is tried once more (`kazoo-speech/src/tts/mod.rs`)

`read_render` (the function ending at `:757`) returns `SpeechError::EmptyRender` when the WAV has no frames. The Opus review of W01b saw this once under heavy load and could not reproduce it in 17 direct renders.

- In the caller that runs `say` and reads its file (`render`, `:425`, and the `Command` it builds at `:559`), when reading answers `EmptyRender`, run the same `say` command once more into a fresh path and read that. Any other error is returned at once, not retried.
- If the second render is also empty, return a new variant `SpeechError::EmptyRenderTwice`, displayed as "say rendered no audio twice". Keep `EmptyRender` for the single read (the function at `:757` is unchanged).
- Both temporary files are removed whatever the outcome, as the existing path removes its one file. No sleep between the attempts.
- To test this without the OS, the code that runs `say` goes behind the smallest seam that lets a test supply a renderer which writes an empty WAV first and a real one second. Use the seam the file already has if there is one (check `render_arguments`, `:287`, and the test module); add none if an existing one serves.

Reason for one retry and no more: the fault is a rare empty file from a command that exited 0; a second empty file means `say` is truly not producing audio, and the player must hear that rather than wait.

### 3. The spring's reset matches a fresh spring (`kazoo-fx/src/space/spring.rs`)

The W01b review found that with every knob at maximum, a spring that is reset does not render what a fresh spring renders, seeded or unseeded, and that main behaves the same. `reset` is at `:864`; the coil structs' resets are at `:253`, `:396` and `:498`; `prepare` is at `:794`.

- Find every piece of state that `new` and `prepare` set and `reset` leaves behind (filter memories, delay lines and their write positions, smoothed knob values, chirp or dispersion allpass states, the drip and shake generators, anything else). Make `reset` restore each to what `new` followed by `prepare` at the current rate and knobs would hold, keeping the module's seed (KZ-W01b's `seeded` streams return to their seed as now).
- Smoothed knob values: after `reset` they sit at their targets, as a fresh spring's do once its knobs are set.
- No allocation in `reset`; it stays callable from the audio thread.

## Not this lane

Any other effect's reset (the W01b review checked the other ten random effects and found them sound). Any change to the spring's sound while it runs. Any change to `say`'s arguments or voice handling. The engine's or the judge's chosen resolutions.

## Red tests first

1. `kazoo-core`, `a_pyin_table_past_the_ceiling_is_refused`: resolution 0.01 over 50–4000 Hz at 48 kHz is refused with `Error::Config` whose message contains "MiB"; resolution 0.05 over the same range is refused; resolution 0.1 over the same range is accepted; the engine's `judge_config` at 48 kHz and `PitchDetectorConfig::for_rate(48_000)` are accepted. The refusal must happen without building the detector: assert it under the workspace's allocation guard if the test module has one, else assert the error comes from `validate` directly.
2. `kazoo-speech`, `an_empty_render_is_tried_once_more`: the test renderer writes an empty WAV, then a one-second tone; the render succeeds with the tone's samples, and the renderer was called exactly twice.
3. `kazoo-speech`, `two_empty_renders_fail_with_their_own_error`: both writes empty; the error is `EmptyRenderTwice`; the renderer was called exactly twice; no temporary file is left behind.
4. `kazoo-fx`, `a_reset_spring_renders_like_a_fresh_one`: for knobs at default, every knob at minimum, every knob at maximum and three seeded random settings (seeds 1, 2, 3 through the test module's existing generator), at 44.1 and 48 kHz, seeded with 5 and unseeded: build, prepare, render one second of the test file's existing impulse-plus-noise input, reset, render again; the second render equals a fresh spring's render bit for bit. Today this fails at maximum knobs.

## Checks

```
nice -n 10 lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core -p kazoo-speech -p kazoo-fx -p kazoo-wall -j 4
nice -n 10 lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core -p kazoo-speech -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-speech/src/tts kazoo-fx/src/space
```

Doc tests: `nice -n 10 lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -p kazoo-speech -p kazoo-fx`.

## Definition of done

- [ ] The four red tests fail first (paste each failure under RED), then pass. Every existing test passes.
- [ ] The spring's unseeded output while running is unchanged: an unseeded spring's first render at default and maximum knobs hashes the same before and after this lane (paste both hashes).
- [ ] `say` is still run with exactly the arguments it was run with before.

## Code quality

As PLAN.md's STACK RULES. No lint bypasses, no `let _ =`, no `.ok()` discards, no `as` casts clippy pedantic flags. Errors come back as the crate's error type, never as a panic.

## END

```
START KZ-F01 base=<sha> branch=lane/kz-F01 df=<GB> at <time>
END KZ-F01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
