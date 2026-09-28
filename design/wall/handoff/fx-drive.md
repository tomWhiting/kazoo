# Handoff: kazoo-fx drive family (fx-drive)

Written 26 Sep 2026 at the lead's checkpoint call. Nothing is committed. Only
`kazoo-fx/src/drive/` and `kazoo-fx/tests/drive_no_alloc.rs` are touched.

## Build rule

Run every cargo command as `nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo … -j 4`, one at a time,
on kazoo-fx only. While iterating, filter to the family: `cargo test -p kazoo-fx --release --lib drive`.
Run the full crate suite once, at the end, in debug: `cargo test -p kazoo-fx`. That is where
`drive_no_alloc` actually checks allocation.

The crate-wide clippy currently fails only on other families' files (time/, space/). Those are
not ours. To clippy drive alone, use the isolated copy at
`/private/tmp/claude-501/-Users-tom-Developer-projects-deno-rust-kazoo/37fc610d-be0a-4b16-89d4-625e4821c0d9/scratchpad/fxdrive`.
Its `src/drive` is a symlink to the real directory. Run:
`nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo clippy --all-targets -j 4 -- -D warnings`.

## State at checkpoint (all green)

| Gate | Result |
|---|---|
| `cargo test -p kazoo-fx --release --lib drive` | 68 passed |
| `drive_no_alloc` (debug) | passes at 44.1, 48, 96 and 192 kHz |
| clippy `-D warnings` (isolated copy) | clean |
| `ast-grep scan` (drive) | 0 findings |
| rustfmt (drive) | clean |

## Files

- Shared: `kit.rs`, `filter.rs`, `oversample.rs`, `pair.rs` (new), `solve.rs`, `testkit.rs`.
- Effects: `klon.rs`, `screamer.rs`, `fuzz.rs`, `amp.rs`, `fold.rs`, `crush.rs`, `ringmod.rs`,
  `comp.rs`.
- Registry: `mod.rs`.

## Audio audit fixes (done, lead approved)

- **Oversampling to a target rate.** Each effect oversamples to an internal rate of at least
  176.4 kHz: 4x at 44.1/48k, 2x at 88.2/96k, 1x at 176.4/192k. The cap is 8x.
- **Two exceptions run at 352.8 kHz or more.**
  - screamer: the lead decided to keep 352.8k. Its worst spur is about -68 dBc on a -6 dBFS 5 kHz
    tone, at about 20% of a core at 192 kHz. 705.6k would buy -100 dBc for 4x the CPU and was
    rejected.
  - fold: moved up during the review below.
- **The f/2 spur was aliasing, not solver period doubling.** 4987 Hz is 192k/38.5, so the 38th
  and 39th harmonics fold onto f/2. Moving the tone off that frequency or raising the internal
  rate removes the spur.
  - amp: the grid-conduction clamp and the power stage's tanh are now ADAA'd.
  - screamer: runs at 352.8k and models the op-amp's 3 MHz gain-bandwidth.
- **fuzz:** both transistor stages are ADAA'd with closed-form antiderivatives. The worst spur
  went from -45 to -68 dBc.
- **Contract:** added a cross-rate check (level plus a spur limit) to `testkit::conformance`, and
  `no_subharmonics` tests.

## Opus review round 1: 38 findings, all addressed

Reviewer agent id: `a45920ef3ce0d77a4`.

Main changes:

- **Oversampler stage 1:** now 127 taps, Kaiser beta 9. It holds 90 dB rejection at a 44.1k
  host. Later stages are 31 taps. Tests cover factors 2, 4 and 8 at 44.1 and 48k.
- **amp**
  - The valve table spans -12 to +10 V, with linear extrapolation past both ends.
  - The valves are ADAA'd via an exact integral of the table.
  - The tone stack is now Yeh & Smith's equation 1. The mid pot is a real pot with C3 on its
    wiper; this was verified symbolically with sympy. The netlist test uses Yeh's figure 1.
- **klon:** treble, summing lowpass (495 Hz), the 106 Hz feed-forward network and the rails now
  follow Electrosmash. The treble shelf starts at 408 Hz and runs from -8 to +18.2 dB. The
  rails are ±12 V. The gain reaches 40 dB. The node is solved to 1e-13 V.
- **fold:** rewritten on published models, running at 352.8 kHz.
  - Buchla: the 259 cells are computed from the DAFx-17 component table.
  - Serge: the Lockhart Lambert-W closed form (SMC 2017) in a four-stage cascade, with ADAA.
- **crush**
  - Jitter is zero-mean on the clock period.
  - The aa path uses exact continuous-time modal Butterworth filters, with sub-sample capture
    and sub-sample hold steps.
- **comp**
  - Oversampled to the target rate and stereo-linked.
  - The FET colour is memoryless: (y² − ms)/rms.
  - Makeup is capped at 18 dB.
- **ringmod:** both flavours use the transformer-rounded carrier; the transformer corner is
  max(9k, 3·f). The carrier phase is f64.
- **Structure:**
  - `pair::Pair<C>` wraps six of the effects.
  - `kit::Retune` redesigns filters at most every 4 samples; at 16, the new click test caught the
    Klon treble sweep stepping.
  - Knob snapping uses `to_bits`.
  - `kit::one_pole`, `kit::log_cosh` and `flush64` are shared helpers.
  - `Iir3` clears its state when its order changes.
  - There is a `rising_root_within` variant of the root solver.
- **Contract (`Limits`)**
  - Checks at 44.1, 48, 96 and 192k.
  - Guitar unity within ±2 dB at every rate.
  - Spur limits at default and hot settings.
  - Re-prepare mid-stream.
  - A click test for every knob; crush `bits` is exempt.
  - No knob extreme pins a -12 dBFS guitar on the ceiling.

### Hot-setting spur limits (measured value at 44.1k)

These are relaxed limits. Report them to the lead as trade-offs.

| Effect | Settings | Measured | Limit | Note |
|---|---|---|---|---|
| amp | gain 100, master 100, cab off | -41.6 dBc | -40 | 2x internal rate would give -49.9 at twice the CPU; kept 1x |
| fold | folds 100 | ≈ -45 dBc | -40 | at 352.8k |
| crush | aa on, clock 32k | -58 dBc | -55 | real image above host Nyquist folding |
| comp | FET, ratio 20, attack 0.1 ms | -72 dBc | -70 | |
| fuzz | hot | -54.7 dBc | -50 | |
| screamer | `no_subharmonics` | -66 dBc | -60 | its aliasing floor |
| amp | `no_subharmonics` | | -90 | passes |

### CPU

Re-measured under machine load about 67, so the numbers are inflated about 2x and noisy.

- At 48k: klon 15%, screamer 30%, amp 20%, fold 7%, comp 9%.
- Re-measure on an idle machine.

## Opus review round 2 (verification): findings received after the checkpoint

Reviewer `a45920ef3ce0d77a4` confirmed most fixes as sound, including:

- the halfband stages;
- the Yeh & Smith coefficients, term by term;
- the Klon formulas;
- the Buchla cells and the Lockhart model with its antiderivative;
- the subharmonic targeting (screamer f/2 at or below -166 dBc at every rate);
- the crush clock and Modal integrals;
- the comp FET spurs (-71 to -72 dBc);
- the no-alloc test at four rates, clippy and ast-grep.

None of the findings below is fixed yet.

### Major

1. **filter.rs `Iir3::design`: clearing state on an order change is a new click.**
   - The Yeh stack at bass 0 with middle 100 has b3 = a3 = 0 exactly, so the order drops to 2.
     Moving bass off 0 then wipes the state.
   - Measured on a 110 Hz tone, bass 0 to 100: largest sample-to-sample step 0.0112 during the
     glide against 0.0002 at rest.
   - Fix: never clear the state (the stale term the clear was meant to prevent is harmless), or
     fix the order by structure rather than by coefficient values. Add a click test at the stack's
     corners.
2. **Undeclared latency that differs by host rate.**
   - klon, fuzz, ringmod, comp: about 70 samples at 44.1/48k, about 63 at 96k, 0–2 at 192k.
   - screamer, fold, amp: about 74 at 44.1/48k, about 72 at 96k, and 65/64/18 at 192k.
   - Fix: add `Effect::latency()` in host samples and have the wall compensate. That changes
     lib.rs, so it is the lead's call; at minimum, flag it to wall-engine. Consider a shorter
     stage for comp.
3. **amp at hot settings (gain 100, master 100, cab off) still aliases.**
   - Worst spur: -43 dBc at 48/96/192k; -41.6 dBc at 44.1k with a 0.5 input and -40.2 with a 1.0
     input.
   - f/2 at -50 to -53 dBc.
   - Fix: run it at 352.8k, like screamer and fold, or use second-order ADAA. Add a subharmonic
     check at hot settings.
4. **crush with aa off (the default) still depends on the host rate.**
   - Measured: -19.8 dB at 44.1k, -20.5 at 48k, -28.6 at 96k, -33.4 at 192k.
   - The held steps still land on the host grid.
   - Fix: place the raw step edges at sub-sample positions (BLEP-corrected), or narrow the doc's
     claim to the aa path.
5. **klon is dark at noon.**
   - Measured at gain 0, treble 50: -9.5 dB at 3 kHz, -15 dB at 6 kHz, -20 dB at 10 kHz.
   - A 1 kHz tone at -12 dBFS comes out -5 dB.
   - Check Electrosmash's whole-pedal response. Either the R20/C13 495 Hz lowpass belongs to one
     path only, or the treble stage's gain at noon is different.
6. **comp FET colour still thumps on the envelope.** `(y² − ms)/rms` is just y² highpassed at
   about 8 Hz. Fix: use a Hilbert pair, `(y² − ŷ²)/2`, normalised by `√(y² + ŷ²)`.

### Minor

7. **Hot limits are lax.** amp -40, fold -40 (the Buchla at folds 100 measures -45), fuzz -50
   (fuzz 100 at the default bias measures -52.6).
8. **`Knobs::step` walks through subnormals on a glide to 0.** The `arrived` test is relative, so
   it never fires near 0. Add an absolute tolerance scaled by the parameter's range.
9. **crush `Modal` state is never flushed**, so it runs on subnormals after silence.
10. **`Retune` keeps its `wait` across prepare() and reset().** Clear it in both.
11. **fold:**
    - Level runs from -2 dB at folds 0 to -9.4 dB at folds 100; not matched.
    - The Serge output `tanh` is not ADAA'd.
    - The Serge path is 2 samples late and the Buchla path 0.5, so a model crossfade combs.
12. **filter.rs doc** still says crush's anti-aliasing is a warped Iir3.
13. **The click metric misses clicks.**
    - It goes deaf on distorted output.
    - It moves one knob at a time, and only at 48 kHz.
    - Fix: use a second-difference metric, and test combinations of knobs at their corners.
14. **Unity is judged on a bass-heavy guitar.** A 1 kHz tone at -12 dBFS comes out at klon -5.1,
    screamer -3.1, fuzz -6.9, amp -7.2 dB. Add a mid-weighted or pink signal to the unity check.

The reviewer could not measure CPU: at load 70–94, back-to-back runs of the same effect varied
up to about 5x.

## Next steps

1. Fix every round-2 finding above, with tests. Take #2 (latency) to the lead first: it touches
   lib.rs, which is outside the family.
2. Send the reviewer (`a45920ef3ce0d77a4`, or a fresh Opus reviewer briefed with this file) a
   round-3 verification.
3. Re-run the gates:
   - drive tests (release, filtered);
   - clippy on the isolated copy;
   - ast-grep and rustfmt on the drive files;
   - the full `cargo test -p kazoo-fx` once, in debug.
4. Re-measure CPU on an idle machine: scratch bench at 48 and 192k.
5. Report to the lead:
   - the gate lines;
   - the relaxed hot limits above and the amp 2x option;
   - the CPU figures.


## Update: round-2 fixes done; round-3 verification in flight

All six major and eight minor round-2 findings are fixed, each with a test that failed first
where that applied. The lead's decisions were applied as follows.

**Major findings**

- **Iir3.**
  - It no longer clears its state on an order change, and is now direct form I (better behaved
    when a knob sweeps).
  - The amp's pots keep 0.01 % of their track at each end, so the tone stack stays third order.
  - The click test now measures the second difference and throws each knob with the others at
    default, all at minimum and all at maximum.
- **`Effect::latency()`** added to lib.rs.
  - Oversampler stage 1 is cut to the host rate. Latency is exact: (4m+1) samples at each stage's
    higher rate.
  - `Pair::latency(extra)` adds the half samples from ADAA.
  - The contract test `latency_is_declared` cross-correlates the proper build against the naive
    build at 44.1, 48, 96 and 192 kHz.
  - Notified: the lead (to relay to fx-time and fx-space) and wall-engine.
- **amp** now runs at 352.8 kHz; `no_subharmonics` runs at hot settings too. Fixing that exposed
  a bug: `subharmonic_db` had been ignoring its settings.
- **crush:** the raw staircase goes through an exact modal line stage at 20 kHz with sub-sample
  steps. Stray products are now at least 45 dB down at every rate, aa on or off.
- **klon:** the 495 Hz lowpass is on the drive path only, and the path shares are rebalanced, so
  noon is transparent.
- **comp:** the FET colour now uses a HIIR-style quadrature pair (16 coefficients).

**Minor findings**

- All spur limits are the measured worst plus 3 dB.
- Knob glides snap near zero instead of crawling through subnormals.
- Modal state is flushed.
- `Retune::settle` is called from every prepare and reset.
- fold:
  - A 300 ms level match keeps its loudness steady.
  - The Serge output `tanh` is ADAA'd.
  - The two paths are aligned at a 2.5-sample delay.
  - It runs at 352.8 kHz.
- The unity check now uses a guitar and a formant voice, both within ±2 dB at every rate.
- `Retune::EVERY` is 1: the click test showed 4 still bends a fast sweep of the Klon's treble.

**Gates:** 75 drive tests pass in the isolated crate. The real crate is briefly broken by
time/tape.rs, another family's work in progress. `drive_no_alloc`, clippy, ast-grep and fmt are
all clean.

**Next:** get round 3 back from the reviewer (`a45920ef3ce0d77a4`), fix anything it finds, run
the full `cargo test -p kazoo-fx` once the crate builds, then report to the lead.
