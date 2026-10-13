# KZ-P10: the sidestep: the input made into a synth with no note detection at all, by squaring it up, dividing it down, ring-modulating and folding it, through a resonant filter with envelopes started by its own onsets

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 10, the first of the chord lanes. Tom: "it doesn't need to be perfect ... garbled, bit crushed, ring modded ... it's for fucking around." This lane also starts the new kazoo-fx family `shape` (effects that turn an input into a synth), which P11 and P12 add to.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Changes: the paths without edges (`x` for `dry`, `d` for `ring`, `half` and `fold`) pass through a one-sample delay so they line up with the polyBLEP edges and `latency()` of 1 is true for the whole output; the alias test compares with and without the polyBLEP instead of a fixed 40 dB a naive square might also pass; the output is held to ±4.0, which `stays_bounded` asks of it. Accepted otherwise as written.

Time: about 40 minutes. Crates: kazoo-fx (kazoo-wall in the checks, because the wall offers every kazoo-fx kind).

Depends: nothing in flight. If P03 or P04 is in flight, merge `lib.rs` line by line.

Files (7): `kazoo-fx/src/shape/mod.rs` (new: the family, `KINDS`), `shape/sidestep.rs` (new), `shape/svf.rs` (new: the filter P11 and P12 reuse), `shape/blep.rs` (new: band-limited edges P12 reuses), `shape/testkit.rs` (new, `#[cfg(test)]`: the glide, vibrato, chord and plucked-string signals P11 and P12 reuse), `shape/sidestep/tests.rs` (new), `kazoo-fx/src/lib.rs` (`pub mod shape;`, `.chain(shape::KINDS.iter())` in `catalogue()`, the family in the module doc).

## What is known about how it is done (research, with sources)

- **Boss SY-1 and SY-300.** Searched (reviews, Boss's own articles, retailer copy, teardown searches): nothing public says how they work. The public claims are "polyphonic", "no special pickup", and a guitar or bass input switch. Nothing here claims to copy them.
- **Roland GR-300** (gr300.com technical description). Per string: an adaptive band-pass, a zero-crossing comparator to a square, a pulse at each edge resetting a sawtooth ramp (time to voltage), a "chopper gate" clipping the ramp's top so every note has the same level, two VCOs locked to the string with one offset by up to an octave, a 24 dB/octave low-pass that tracks pitch, and an envelope from the pickup driving the VCA and the filter, normal or inverted. A "squelcher" suppresses false notes from second harmonics. It needs a hexaphonic pickup, which this track does not use.
- **EHX Micro Synth's sub-octave section, modelled** by F. Esqueda, O. Lähdeoja and V. Välimäki, "Algorithms for guitar-driven synthesis: application to an augmented guitar", SMC 2018, pp. 444-451 (read in full). The chain:
  - smoothing `H⁴(z)`: four one-pole low-passes at 1 kHz, bilinear, about 24 dB/octave (the Micro Synth uses three poles at about 1.1 kHz);
  - two first-order allpasses `A²(z)`, `(a₁ + z⁻¹)/(1 + a₁z⁻¹)` with the same `a₁`, aligning the dry path with the clock;
  - an adaptive Schmitt trigger: a comparator `Vc1 = sgn(x)` and a second `Vc2 = (x < Vx)` against a negative peak follower `Vx` (an RC charging through 220 Ω and discharging through 100 kΩ into 4.7 µF, discretised by the trapezoidal rule, their eq. 7);
  - an S-R latch (high on `Vc1` rising, low on `Vc2` rising), which is the clock at the input's frequency;
  - a D flip-flop dividing it by two;
  - a "processor": either the input times the bipolar sub-octave square (ring modulation: components at f₀ ± f₀/2), or times the unipolar one (every second cycle removed, keeping f₀);
  - a resonant state-variable filter.

  They state it is for a monophonic input, and that with one pickup it follows the strongest periodic component.
- **Boss OC-2** (Wikipedia "Octave effect"; The Valve Wizard, "U-Boat"; a 2001 teardown noting two CD4013s). Flip-flops at half and quarter frequency invert the input's polarity on alternate cycles, inverting at the peak; the 3/2 f₀ product is low-passed away.

## Outcome

`kazoo_fx::find("sidestep")` turns any input into a synth voice. It makes the input's own squared-up wave, its octave and two-octave subs, two processors (ring and half-wave) and a wavefolded copy, mixed by knobs, through a resonant low-pass whose cutoff can track the measured period. The amplitude either follows the input or is an ADSR started by the input's own onsets. There is no note detection anywhere:
- it follows slides, bends and vibrato cycle by cycle, because it is made from the input's own zero crossings;
- a chord never makes it stop or guess, because the square and the subs follow the strongest low partial, while the ring, half and fold paths carry every note of the chord;
- it adds one sample of latency (the band-limited edges).

## The design (decided; build exactly this)

Mono inside: the input is `(L + R)/2`, processed once, and written to both outputs. Why: inputs are mono, and it halves the cost.

### Parameters (in this order; 21, under the wall's 24 knobs)

`low` (20 to 200 Hz, 30; the input high-pass), `high` (200 to 4000 Hz log, 1000; the smoothing cutoff, Esqueda's value), `square`, `sub1`, `sub2`, `ring`, `half`, `fold` (each 0 to 100%; defaults 0, 100, 0, 0, 0, 0), `drive` (1 to 10, 3; into the fold), `cutoff` (40 to 12,000 Hz log, 1200), `reso` (0 to 100%, 30), `track` (0 to 100%, 50), `fenv` (−100 to 100%, 50), `attack` (0.5 to 2000 ms log, 2), `decay` (5 to 5000 ms log, 300), `sustain` (0 to 100%, 60), `release` (5 to 5000 ms log, 200), `env` (stepped `follow`, `adsr`; `adsr`), `sense` (−70 to −20 dB, −45), `dry` (0 to 100%, 0), `level` (−24 to 12 dB, 0). P05 sets `low` and `high` from an input's profile when the effect is put on one. For bass, `high` at 500 Hz keeps the second harmonic from taking the clock.

### Per sample

1. **Front.** `x` = one-pole high-pass at `low` of the input. `s = H⁴(x)`: four bilinear one-pole low-passes at `high`. `d = A²(x)`: two first-order allpasses with Esqueda's `a₁` for the same cutoff (their eq. 3 to 5), so `d` and the clock line up.
2. **Clock.** `Vx` is the negative peak follower of `s` by Esqueda's eq. 7. The time constants are scaled so the charge is 1.03 ms and the discharge is 0.47 s, both as in their circuit and both rate-independent. `Vc1 = s ≥ 0`, `Vc2 = s < Vx`. The latch goes high when `Vc1` rises and low when `Vc2` rises. Each rising edge's time is interpolated between samples (the zero crossing of `s`, linearly), and the period `P` between rising edges is kept.
3. **Dividers.** A toggle on each latch rising edge gives `sub1` (f/2), and a toggle on each `sub1` rising edge gives `sub2` (f/4). Their edges carry the same interpolated times.
4. **Waves.** `square`, `sub1` and `sub2` are ±1 with each edge corrected by a two-sample polyBLEP at its fractional time (`blep.rs`: the residual applied to the sample before and after the edge, hence one sample of latency). `ring = d × sub1_bipolar` and `half = d × (sub1 + 1)/2` (Esqueda's two processors). `fold = sin(π/2 · clamp(drive × d / max(env, 1e-4), −8, 8))`, a sine wavefolder on the input normalised by its envelope, so the fold's colour does not depend on playing level. The fold carries every note of a chord, intermodulated.
5. **Mix.** `w = Σ knob × wave`, with `square`, `sub1` and `sub2` scaled by 0.5 and the others by 1, so equal knobs sound about equally loud.
6. **Envelope and onset.**
   - `env` is a peak follower of `x` (1 ms attack, 30 ms release), in dB as `env_db`.
   - The gate is `env_db ≥ sense`.
   - An onset is a rise of `env_db` of at least 6 dB over its valley (the valley follows down at once and up with an 80 ms time constant, and resets at each onset), with at most one onset per 40 ms. That is V04's rule (`ONSET_RISE_DB`, the valley, the refractory time) with P06's 6 dB for plucked strings and a 40 ms refractory time so a 16th-note picking run at 200 BPM still retriggers.
   - The ADSR (linear attack; exponential decay and release to −80 dB within the times given) restarts on each onset and releases when the gate closes.
   - In `follow`, the amplitude is `env` itself (the input's dynamics pass through).
7. **Filter.** `svf.rs` is a TPT state-variable filter (Zavalishin, *The Art of VA Filter Design*, ch. 4), low-pass out, `k = 2 − 1.98 × reso` (`reso` 100% is just under self-oscillation). The cutoff is `cutoff × 2^(fenv × 6 × adsr)` octaves when `env` is `adsr` (6 octaves at 100%), times `(f_clock / 220)^track`, where `f_clock = rate / P`, smoothed by a 5 ms one-pole. This is the GR-300's pitch-tracking filter, with the pitch measured from the clock and never detected as a note. The coefficient `g = tan(π fc / rate)` is recomputed only when `fc` has moved more than 0.1% since the last computation. The cutoff is clamped to 0.45 × rate.
8. **Out.** `y = level × (filter(w) × amp + dry × x₋₁)`, where `x₋₁` is `x` one sample late (as `d` is, before step 4 uses it, so every path lines up with the edges' one sample), sanitised, held to `[−4.0, 4.0]`, written to both channels.

### Latency and cost

`latency()` is 1 (the polyBLEP). The cost is about 60 to 80 ns a sample on this Mac (two transcendental calls: the fold's `sin`, and `tan` when the cutoff moves), so a 512-frame block is about 40 µs, 0.4% of its 10.7 ms at 48 kHz. The bench must show at most 100 µs.

### The shared test kit (`shape/testkit.rs`, test only)

`glide_fourth(rate)` (220 to 293.66 Hz over 2 s, phase-continuous), `vibrato_30c_6hz(rate)` (220 Hz, ±30 cents at 6 Hz, 2 s), `chord(rate, &[f])` (equal-level harmonic tones, 6 harmonics at 1/h), `pluck(rate, f, seconds)` (Karplus-Strong, loss 0.996, seeded), and `window_pitch(signal, rate, ms)` (interpolated zero crossings after a band-pass around the expected pitch). These are kazoo-fx's own, because kazoo-fx depends on nothing and its tests must not pull in kazoo-core.

## Not this lane

P11's band-split octaves and P12's resynthesis. Garbling: put `ringmod` and `crush` after it in the chain (kazoo-fx has both; nothing new is written for it). Any pitch detection.

## Red tests first

`shape/sidestep/tests.rs`. Red for all: the effect does not exist.

1. `the_sub_follows_a_glide_without_steps`: `glide_fourth` at −12 dBFS, `sub1` 100% and the rest 0, `cutoff` 12 kHz, `reso` 0, `env` `follow`. The output's pitch per 10 ms window (after 50 ms) is half the input's within 5 cents, and no window steps by more than the input's change plus 2 cents.
2. `the_sub_follows_a_vibrato_without_steps`: the same with `vibrato_30c_6hz`.
3. `the_square_is_the_inputs_own_pitch`: `square` alone on a 110 Hz pluck: the window pitch is 110 Hz within 3 cents while the pluck rings above `sense`.
4. `a_low_b_clocks_with_high_at_500`: a 30.87 Hz pluck with `high` 500: the measured period is 30.87 Hz within 0.5% on 95% of cycles.
5. `a_chord_is_never_silenced_and_folds_every_note`: `chord` of C3, E3 and G3 with `fold` alone: the output's spectrum has energy within 20 dB of the strongest component at each of the three fundamentals' frequencies or their folded products (print the table). With `sub1` alone: a finite, non-silent output.
6. `each_onset_restarts_the_envelope`: three plucks 150 ms apart on a still-ringing string, `env` `adsr`: three attacks (envelope rises after each pluck within 3 ms).
7. `the_gate_closes_below_sense`: the input faded to −60 dBFS with `sense` −45: the output releases to below −80 dB within `release` + 5 ms.
8. `the_edges_are_band_limited`: `square` alone on a 1 kHz sine at 48 kHz: the alias energy (bins not within 3 bins of a multiple of 1 kHz) is at least 12 dB lower with the polyBLEP than without it (a test switch), and at least 30 dB under the fundamental with it. Print both figures. That proves the polyBLEP does the work.
9. `latency_is_one_sample`: `latency() == 1`, and with `dry` at 100% and the waves at 0, the output is the high-passed input delayed by one sample within 1e-6.
10. `survives_nonsense`, `stays_bounded` (every output at most 4.0 in magnitude at any setting) and `processing_never_allocates` (10 s inside `assert_no_alloc`).
11. The bench `fx/sidestep` (512-frame blocks at 48 kHz, all waves at 50%): at most 100 µs per block; print it with the load average.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-wall/src
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-fx -j 4 -- 'sidestep'
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-fx and kazoo-wall test passes (the wall's every-kind tests now include `sidestep`).
- [ ] The module doc cites Esqueda, Lähdeoja and Välimäki (SMC 2018), gr300.com, and the OC-2 sources, and says plainly that Boss's method is not public.
- [ ] BENCH.md `## KZ-P10 sidestep`: the bench row with the load average, date and sha.
- [ ] Not the builder's: after P05 lands, Cassio with Tom puts `sidestep`, then `ringmod`, then `crush` on a guitar input, and Tom plays chords, sweeps and slide through it. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES and kazoo-fx's real-time contract (`lib.rs:19-25`): no allocation, lock, I/O or panic in `process`, `set_param` or `reset`, and NaN or Inf in is silence out, with no poisoned state. The latch, dividers and edge times are plain state. Every filter state is flushed of denormals. Knobs glide through the family's `Knobs` pattern (copy `drive/kit.rs`'s use, never its code: import it, and make it `pub(crate)` if it is not). Files under 1000 lines.

## END

```
START KZ-P10 base=<sha> branch=lane/kz-P10 df=<GB> at <time>
END KZ-P10 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: fx/sidestep per 512-frame block
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
