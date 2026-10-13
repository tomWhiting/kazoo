# KZ-P11: octaves by band: 80 complex band-pass filters split the input, each band's phase is doubled for octaves up and halved for octaves down, and the bands are summed, so chords get octaves without any note being found

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 11, the second chord lane, in the `shape` family P10 started.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Change: the doc also states the response time at a bass's lowest band (with `low` at 30 Hz, about 100 ms), so a bass player knows the octave down on a low B blooms rather than speaks. Accepted otherwise as written; the full-rate bank's 600 µs a block is accepted for what it gives.

Time: about 40 minutes. Crates: kazoo-fx (kazoo-wall in the checks).

Depends: KZ-P10 (the `shape` family, `shape/testkit.rs`) must have landed before this lane opens.

Files (4): `kazoo-fx/src/shape/octaves.rs` (new), `shape/octaves/band.rs` (new: one complex band), `shape/octaves/tests.rs` (new), `shape/mod.rs` (one entry in `KINDS`).

## What is known about how it is done (research, with sources)

- **The analogue octave divider** (Boss OC-2; Wikipedia "Octave effect"; The Valve Wizard, "U-Boat"). A flip-flop at half the input's frequency inverts the signal's polarity on alternate cycles. That is amplitude modulation by a half-frequency square, giving f/2 and 3f/2, and the 3f/2 is low-passed away. It works on one note. On a chord, the flip-flop follows whatever crosses zero, and the result is noise.
- **EHX POG.** Its makers say only that it gives "one octave up, two octaves up and one octave down" with "polyphonic", "glitch-free" tracking. No patent or paper describing it was found.
- **The published method.** Etienne Thuillier, "Real-time polyphonic octave doubling for the guitar", MSc thesis, Aalto University, supervised by Vesa Välimäki. Read through its abstract and excerpts; the full PDF refused the fetch. Four candidates were compared against three commercial pedals for roughness and transients. The best, ERB-PS2, splits the input with a constant-ERB-bandwidth bank of complex-valued analysis filters (so each band is analytic, with in-phase and quadrature parts), and doubles each sub-band's instantaneous phase. It reported at most 3 ms of latency in the 3 to 8 kHz bands, much less than the pedals. It covers octave up only.
- **An open implementation that adds octaves down.** `schult/terrarium-poly-octave` (MIT), `util/OctaveGenerator.h` and `util/BandShifter.h`, read in full:
  - 80 bands with centres `480·2^(0.027 n) − 420` Hz (60 Hz to about 1.7 kHz), each with a bandwidth equal to the harmonic mean of its spacings to its neighbours;
  - each band an RBJ-cookbook second-order low-pass at half that bandwidth, shifted to the centre into a complex band-pass by A. J. Noga's method ("Complex band-pass filters for analytic signal generation and their application", DTIC ADA395963, §3.1);
  - octave up as `y·(y/|y|)`, the real part `(a² − b²)/|y|`;
  - octave down as the half-angle `y·(y/|y|)^(−1/2)`, with the sign flipped each time `y`'s phase wraps (real part negative while the imaginary part changes sign), because the square root has two answers and the sign keeps the sub continuous;
  - the second octave down repeats that on the first;
  - it runs at 8 kHz (decimated by 6 from 48 kHz), and its author reports it close to EHX's Sub 'N' Up, short on low-end resonance.

Where the two disagree, decided here:
- **Spacing**: on the ERB scale, as Thuillier found best, not Terrarium's empirical curve. The ERB scale is Glasberg and Moore's (1990): `E(f) = 21.4 · log10(1 + 0.00437 f)`.
- **Rate**: the host rate, not decimated. Decimating and interpolating back costs about a millisecond of linear-phase filtering on a path whose whole budget is 2.7 ms. This Mac can afford the full-rate bank (below).
- **Octave down**: kept from Terrarium, since Thuillier's thesis does not cover it.

## Outcome

`kazoo_fx::find("octaves")` adds two octaves up and two octaves down to anything played into it, chords included, by phase scaling in 80 bands. It follows slides and vibrato sample by sample, because the band's instantaneous phase is the input's. The dry signal passes undelayed. The octaves lag in the low bands by the bands' own response time (about 40 ms at 60 Hz, 10 ms at 1 kHz), because resolving notes a semitone apart near 60 Hz takes that long. A `width` knob trades that lag against roughness.

## The design (decided; build exactly this)

Mono inside, as P10: `(L + R)/2`, processed once, written to both.

### Parameters (in this order)

`low` (30 to 500 Hz log, 60), `high` (500 to 5000 Hz log, 2000), `width` (0.5 to 2.0, 1.0: a multiplier on every band's bandwidth), `dry` (0 to 200%, 100), `up1`, `up2`, `down1`, `down2` (0 to 200% each; 0, 0, 100, 0; 100% is the band sum at unity), `tone` (500 to 16,000 Hz log, 8000: a one-pole low-pass on the summed octaves), `level` (−24 to 12 dB, 0). P05 sets `low` and `high` from an input's profile.

### The bank (`octaves.rs`, `octaves/band.rs`)

- `BANDS = 80`. Two banks are allocated in `prepare`. When `low`, `high` or `width` has stopped moving for 50 ms, `process` designs the spare bank's coefficients in place (80 bands of a few `sin`/`cos` each, about 20 µs, once) and crossfades to it over 10 ms. A turned knob therefore never allocates and never clicks, and no design runs while the knob is still moving.
- **Centres**: `E` evenly spaced from `E(low)` to `E(high)` in 80 steps, then `f = (10^(E/21.4) − 1)/0.00437`.
- **Bandwidth**: the harmonic mean of the spacings to the neighbours (Terrarium's rule), times `width`.
- **Band filter**: RBJ low-pass, Q = 1/√2, at `bw/2`, shifted to the centre by multiplying its z-plane poles and zeros by `e^(jω_c)` (Noga), giving complex coefficients `d0` (real), `d1`, `d2`, `c1`, `c2`, in Terrarium's transposed direct form II. The state is complex `f32`. Coefficients are designed in `f64` and stored in `f32`.
- **Per band per sample**: `y = s2 + d0·x; s2 = s1 + d1·x − c1·y; s1 = d2·x − c2·y`.
  - Up 1, complex: `u1 = y²/|y|`.
  - Up 2: `u2 = u1²/|u1|`.
  - Down 1: the half angle of `y` (Terrarium's `update_down1`, with `1/√` and `√` written out and no fast-inverse-square-root tricks: `f32::sqrt` is fast on this hardware), with its sign flip at each wrap of `y`.
  - Down 2: the same applied to down 1.
  - A path whose knob is 0 is not computed. A band whose `|y|²` is under `1e-9` (−90 dB) skips its paths and holds its signs.
- **Sum**: each path's real parts over the bands, times one fixed gain per path. The gain is calibrated in `prepare` so that a sine at a band's centre comes out at unity, and test 1 pins it.

### Latency and cost

- `latency()` is 0. The dry path is the input, and the octaves are new sounds whose group delay is part of what they are, like a delay's echo under the `Effect::latency` contract (`lib.rs:193-200`).
- The doc states each band's response time, `≈ 1/(π · bw)`: about 100 ms at 30 Hz (`low` as P05 sets it for bass), about 40 ms at 60 Hz, 10 ms at 1 kHz and 5 ms at 2 kHz with `width` 1 (where ERB/80 spacing gives bands of about 7.5, 32 and 58 Hz). `width` 2 halves those, and adjacent notes then leak into each other's bands and roughen.
- Cost, all paths on and all 80 bands sounding: about 60 flops per band per sample, 4,800 a sample, 2.5 M per 512-frame block. That is about 600 µs on this Mac's scalar cores, 5.6% of the block's 10.7 ms. With silent bands skipped, a single note costs about a tenth of that. The bench must show at most 800 µs per block with all paths on and a six-note chord.

## Not this lane

Pitch shifting by other intervals (kazoo-fx's `pitch` and `shift` do that). The sidestep (P10) and resynthesis (P12).

## Red tests first

`shape/octaves/tests.rs`, using `shape/testkit.rs`. Red for all: the effect does not exist.

1. `one_sine_in_one_band_comes_out_at_unity`: a sine at a band's centre with `up1` 100% and the others 0: the output's level is within 1 dB of the input's, at twice its frequency within 1 cent.
2. `down1_and_down2_are_half_and_quarter`: a 440 Hz sine: `down1` alone is 220 Hz within 2 cents, and `down2` alone is 110 Hz within 2 cents, with no other component within 30 dB of it below 1 kHz.
3. `up2_is_four_times`.
4. `a_chord_gets_its_octaves`: `chord` of A2, C#3 and E3 (110, 138.6 and 164.8 Hz) with `down1` alone: energy within 6 dB of each other at 55, 69.3 and 82.4 Hz, and nothing within 20 dB of them at the chord tones themselves.
5. `the_octave_follows_a_glide_without_steps`: `glide_fourth` with `down1` alone. Every 10 ms window's pitch (after 100 ms) is half the input's within 10 cents, with no window step over the input's change plus 5 cents, and the level never dips more than 6 dB at a band crossing.
6. `the_octave_follows_a_vibrato_without_steps`: `vibrato_30c_6hz` the same, with `up1` alone at twice the pitch.
7. `the_dry_is_not_delayed`: `dry` 100% and every octave 0: the output equals the input within 1e-6, and `latency() == 0`.
8. `the_response_time_is_as_stated`: a 1 kHz tone burst with `up1` alone: the output reaches −3 dB of its steady level within 12 ms (the stated 10 ms plus a margin).
9. `turning_low_high_or_width_never_allocates_or_clicks`: sweeping `width` from 0.5 to 2 over 1 s during a chord inside `assert_no_alloc`: every output finite, and the largest step at most three times the largest in the 100 ms before.
10. `survives_nonsense`, `stays_bounded` (at most 4.0) and `processing_never_allocates`.
11. The bench `fx/octaves` (512 frames, 48 kHz, all paths on, a six-note chord): at most 800 µs a block. Print it with a single-note figure and the load average.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-wall/src
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-fx -j 4 -- 'octaves'
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-fx and kazoo-wall test passes.
- [ ] The module doc cites Thuillier's thesis, Terrarium (MIT, with the files read), Noga, Glasberg and Moore, and the OC-2 sources, and says what it took from each and why it runs at the host rate.
- [ ] BENCH.md `## KZ-P11 octaves`: the chord and single-note rows with the load average, date and sha.
- [ ] Not the builder's: after P05 lands, Cassio with Tom puts `octaves` (`down1` 100%, `up1` 50%) on a guitar input and on a bass input, and Tom plays chords and slides. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES and kazoo-fx's real-time contract. Both banks are allocated in `prepare`. A redesign writes only coefficients into the spare bank, so it allocates nothing. Every band's state is flushed of denormals. The sign flips are plain state. No `as` casts clippy pedantic flags. Files under 1000 lines.

## END

```
START KZ-P11 base=<sha> branch=lane/kz-P11 df=<GB> at <time>
END KZ-P11 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: fx/octaves chord and single-note per 512-frame block
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
