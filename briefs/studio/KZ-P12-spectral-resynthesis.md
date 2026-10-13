# KZ-P12: spectral resynthesis: the input's strongest spectral peaks, measured to a fraction of a bin, each drive an oscillator (sine, saw or square) that glides with them, with shift, an optional chromatic snap, and freeze

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 12, the third chord lane, in the `shape` family. "Garbled by nature, which is the point", and chords come for free because every peak gets its own oscillator.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: rustfft 6.4.1 is kazoo-core's (Cargo.lock), kazoo-fx has no dependencies today, `N/2 + H` is 2304, the 64-cent slide per hop and the 8-bin phase range hold, and a linearly interpolated 4096-point sine table is near −130 dB. Changes: tests 5 and 6 now give numbers in place of "the glide's own rate allows" and "exactly".

Time: about 40 minutes. Crates: kazoo-fx, which gains its first dependency, rustfft (kazoo-wall in the checks).

Depends: KZ-P10 (the `shape` family, `shape/blep.rs`, `shape/testkit.rs`) must have landed before this lane opens.

Files (6): `kazoo-fx/Cargo.toml` (`rustfft = "6.4.1"`, kazoo-core's version, so `Cargo.lock` gains only the edge), `kazoo-fx/src/shape/resynth.rs` (new), `shape/peaks.rs` (new and public: analysis, which P13 reuses), `shape/resynth/bank.rs` (new: oscillators), `shape/resynth/tests.rs` (new), `shape/mod.rs` (one entry).

Why rustfft in kazoo-fx, not kazoo-core's phase vocoder: kazoo-core's `PhaseVocoder` (`synthesis/phase_vocoder.rs:16`, a fixed 2048-point frame with 4x overlap) processes and resynthesises bins. It is not a peak analyser, and it sits behind kazoo-core's device and IPC dependencies, which kazoo-fx (no dependencies today) must not pull in. rustfft is already built for the workspace.

## What is known about how it is done (research, with sources)

- **The sinusoidal model.** R. J. McAulay and T. F. Quatieri, and J. O. Smith and X. Serra's PARSHL (CCRMA). Short-time FFTs at regular frames; amplitude peaks picked in each; each peak's frequency refined by a parabola through the log magnitudes of the peak bin and its neighbours; peaks matched frame to frame into tracks (an unmatched peak is a birth, an unmatched track a death); oscillators driven by the tracks, with their envelopes interpolated from frame rate to sample rate.
- **The phase vocoder's frequency.** J. Laroche and M. Dolson, "Improved phase vocoder time-scale modification of audio", IEEE TSAP 1999, and the classic phase vocoder: a bin's phase advance between two frames a hop apart gives the frequency inside the bin, exactly for a steady sinusoid.
- **Where they disagree.** The parabola is biased with a Hann window, by up to about a tenth of a bin: 1.2 Hz at 4096 points and 48 kHz, which is 10 cents at 200 Hz. The phase advance is exact for a steady partial but wrong across a transient. Decided: take the phase-advance frequency, and fall back to the parabola for any peak where the two differ by more than one bin. Why: Tom's slides and vibrato need the exact frequency, and a transient lasts one frame.

## Outcome

`kazoo_fx::find("resynth")` hears up to 32 spectral peaks of whatever is played (a chord, a slide, a scream) and plays each as an oscillator:
- sines (a clean ghost of the input), saws or squares (a buzzy, garbled one);
- shifted by any interval, continuously;
- snapped to semitones only when asked;
- frozen on a key.

The oscillators follow slides, bends and vibrato smoothly, because each peak's frequency is measured to a fraction of a bin and interpolated across the hop. Nothing snaps to bins.

## The design (decided; build exactly this)

Mono inside, as P10.

### Parameters (in this order)

`low` (30 to 1000 Hz log, 60), `high` (1000 to 16,000 Hz log, 8000), `peaks` (1 to 32, whole numbers, 16), `depth` (20 to 80 dB, 50: peaks within this many dB of the frame's strongest), `shape` (stepped `sine`, `saw`, `square`; `sine`), `shift` (−24 to +24 semitones, continuous, 0), `snap` (stepped `off`, `chromatic`; `off`), `freeze` (stepped `off`, `on`; `off`), `glide` (0 to 200 ms, 0: extra smoothing on every oscillator's frequency), `mix` (0 to 100%, 100), `level` (−24 to 12 dB, 0). P05 sets `low` and `high` from an input's profile.

Why only chromatic snapping: scales live in kazoo-core (V05's `voice::quantise`, shared with the wall since V10), which kazoo-fx does not depend on, and a second table would be the duplication the track forbids. A scaled line is made on the wall with `quant`.

### Analysis (`shape/peaks.rs`, public, every hop)

The peak picker is public as `kazoo_fx::shape::peaks::{PeakPicker, Peak}`: `PeakPicker::new(rate, n, hop)` allocates, and `analyse(&mut self, newest_n_samples: &[f32], low, high, depth, max) -> &[Peak]` (each `Peak { hz, amp }`) allocates nothing. KZ-P13's chord follower uses this same picker, so there is one in the workspace.

- **Frame and hop.** `N = 4096`, `H = 256` at 48 kHz, scaled with the rate as `N = next_power_of_two(0.085 × rate)` and `H = N/16`. Why: an 85 ms Hann window has a 47 Hz main lobe, the most a playable latency allows; a 5.3 ms hop is fast enough for a picked note. The phase advance is unambiguous for deviations up to `N/(2H) = 8` bins.
- **FFT.** Window, then rustfft's forward complex FFT of `N` (plan and scratch made in `prepare`, run with `process_with_scratch`). Keep this frame's phases and the last frame's.
- **Peaks.** For bins between `low` and `high`: a local maximum over both neighbours, at or above `max(−80 dBFS, frame max − depth)`. Keep the `peaks` strongest in a fixed array, by insertion.
- **Each peak's frequency.** `Δ = princarg(φ_k − φ_k,prev − 2π k H / N)`, `f = (k + Δ N / (2π H)) · rate / N`. If that differs from the parabolic estimate by more than one bin, use the parabolic one.
- **Amplitude.** The parabola's peak in log magnitude, normalised so a full-scale sine reads 1.0 (test 1 pins the factor).

### Tracks and oscillators (`resynth/bank.rs`)

- 32 oscillators with `freq`, `amp`, their targets and their phases.
- **Matching** (McAulay and Quatieri's rule): peaks in order of strength each take the nearest unmatched sounding oscillator within 100 cents. A peak with no match takes a silent oscillator, which is born at that frequency with amplitude 0. An oscillator nothing matched aims for amplitude 0. Why 100 cents: past the fastest slide in one 5.3 ms hop (a 12-semitone slide in 100 ms moves 64 cents a hop), and short of the next semitone's partial in a chord.
- **Per sample.** The frequency moves geometrically from its last target to its new one across `H` samples (linear in log frequency), then through the `glide` one-pole. The amplitude moves linearly across `H`. The played frequency is `freq × 2^(shift/12)`, then snapped to the nearest equal-tempered semitone of A440 when `snap` is `chromatic`. Snapping is the only stepped path, and it is off by default.
- **Waves.** `sine` from a 4096-point table with linear interpolation (better than −90 dB, which the test checks); `saw` and `square` as polyBLEP oscillators (P10's `blep.rs`).
- **`freeze`**: matching stops, the targets hold, and the oscillators keep sounding (shift still applies).
- **Output**: `level × (mix × Σ amp·wave + (1 − mix) × input)`.

### Latency and cost

- `latency()` is `N/2 + H` (2304 samples, 48 ms at 48 kHz): the oscillators sound the frame's centre, half a frame behind its newest sample, interpolated over one more hop. This is declared so a host can align a dry path. On an input, P05's exemption keeps the input itself undelayed, and the resynthesis comes 48 ms after the note, which is in the nature of a spectral effect.
- Per 512-frame block at 48 kHz: two 4096-point FFTs (about 20 µs each), two peak passes (about 5 µs each), and 32 oscillators × 512 samples (about 30 µs as sines, about 60 µs as polyBLEP saws). That is about 130 µs, 1.2% of the block. The bench must show at most 300 µs.

## Not this lane

Scales beyond chromatic (the wall's `quant`). Garbling: `crush` and `ringmod` after it. Time-stretching (kazoo-core's phase vocoder).

## Red tests first

`shape/resynth/tests.rs`, using `shape/testkit.rs`. Red for all: the effect does not exist.

1. `a_sine_is_resynthesised_at_its_level_and_pitch`: a 440 Hz sine at −6 dBFS: after 150 ms, one oscillator at 440 Hz within 1 cent, and the output within 0.5 dB of the input's level.
2. `a_chord_gets_an_oscillator_per_note`: `chord` of C4, E4 and G4: at least three oscillators within 5 cents of the three fundamentals.
3. `it_follows_a_glide_without_steps`: `glide_fourth`, `shape` sine, `snap` off. After 150 ms, every 10 ms window of the output is within 10 cents of the input's pitch at a lag of `latency()`, and no window steps by more than the input's change plus 3 cents.
4. `it_follows_a_vibrato_without_steps`: `vibrato_30c_6hz`: within 8 cents per window, with no step over the input's change plus 3 cents.
5. `bins_never_show`: a sine gliding slowly over exactly one bin's width (11.7 Hz at 48 kHz) over 2 s: the oscillator's frequency moves monotonically, and no hop moves it by more than twice the input's change over a hop plus 0.5 cents, including where the input crosses a bin's centre or edge. This proves there is no bin snapping.
6. `snap_steps_only_when_asked`: the same glide with `snap` `chromatic`: the output pitch takes only semitone values (±2 cents) and steps within one hop (256 samples) of where the input, lagged by `latency()`, crosses a quarter-tone; with `snap` off, test 3 holds.
7. `shift_is_continuous`: `shift` swept from 0 to +7 semitones over 2 s on a steady sine: the output glides with no step over 3 cents per window.
8. `freeze_holds`: freeze on a chord, then silence the input: the chord keeps sounding at the same pitches for 2 s within 1 cent.
9. `a_transient_frame_uses_the_parabola`: a pluck's first frame, where the phase estimate is off by more than a bin: the oscillator is born within half a bin of the true pitch.
10. `latency_is_declared`: `latency() == 2304` at 48 kHz, and it scales with the rate.
11. `survives_nonsense`, `stays_bounded` (at most 4.0) and `processing_never_allocates` (10 s of a chord and a glide inside `assert_no_alloc`, the FFT included).
12. The bench `fx/resynth` (512 frames at 48 kHz, a six-note chord, 32 peaks, saw): at most 300 µs a block; printed with the load average.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-wall/src
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-fx -j 4 -- 'resynth'
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-fx and kazoo-wall test passes.
- [ ] `Cargo.lock` changed only by kazoo-fx's new edge to rustfft (paste the diff's size).
- [ ] The module doc cites McAulay and Quatieri, PARSHL, and Laroche and Dolson, and says which frequency estimate is used and why.
- [ ] BENCH.md `## KZ-P12 resynth`: the bench row with the load average, date and sha.
- [ ] Not the builder's: after P05 lands, Cassio with Tom puts `resynth` (saw, `shift` +12) and then `crush` on a guitar input, and Tom plays chords, a slide and the scream. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES and kazoo-fx's real-time contract. The FFT plan, its scratch, the frame buffers, the phase history and the oscillator bank are allocated in `prepare`; `process` allocates nothing, and the no-allocation test covers the FFT call. Every phase difference goes through one `princarg` function. NaN or Inf in a frame clears that frame's peaks and leaves the oscillators fading. No `as` casts clippy pedantic flags. Files under 1000 lines.

## END

```
START KZ-P12 base=<sha> branch=lane/kz-P12 df=<GB> at <time>
END KZ-P12 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: fx/resynth per 512-frame block
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
