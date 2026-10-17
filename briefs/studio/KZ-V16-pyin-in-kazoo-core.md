# KZ-V16: pYIN moves into kazoo-core, so `cargo install kzoo` resolves without `--locked`

Written by Cassio, 17 Oct 2026, about 10:30. Decided by Cassio; the reasons are given with each decision. Cassio wrote this lane directly, with no builder (memory `do-it-directly`).

Place: Track V. Crates:
- kazoo-core: `analysis/pyin/` (new: `mod.rs`, `yin.rs`, `prior.rs`, `hmm.rs`, tests), `analysis/pitch.rs`, `analysis/pitch/tests.rs`, `analysis/score/wav.rs`, `Cargo.toml`
- the workspace `Cargo.toml` and `Cargo.lock`

## Why

kzoo 0.1.0 went to crates.io on 17 Oct 2026, but a plain `cargo install kzoo` fails to resolve. The `pyin` crate 1.2.0 hard-depends on `ndarray-npy`, which pulls in `zip` 0.5, and every `zip` 0.5 release is yanked. A fresh resolution refuses yanked versions, so only `cargo install --locked kzoo` works. Tom asked for the crates.io release, and the install it promises should be the one people type.

## Outcome

- kazoo-core has its own pYIN, ported from `pyin` 1.2.0 (MIT, Jeongmin Liu), which was itself translated from librosa 0.9.1 (ISC). Both licences are kept in the module's header. kazoo-core now depends on `realfft` 3.5 for its FFTs. `pyin`, `ndarray`, `ndarray-npy`, `ndarray-stats`, `statrs`, `zip` and the rest are gone from the dependency graph.
- **Same results, bit for bit.** The port does the crate's arithmetic in the crate's order:
  - row and threshold sums in ndarray's eight-lane order;
  - the beta prior as the 100 values statrs gave the crate, kept as bits;
  - nothing fused that the crate rounded twice.

  A fixture holds the crate's output for 7 signals under 4 settings, 2541 frames in all: the voice judge, a scored take, pYIN's defaults, and fine bins on a short frame. On every frame the port gives the same voicing, pitch and voicing probability, compared bit for bit.
- **Where the crate panicked, the port refuses or reads sensibly:**
  - `Settings::grid` does its arithmetic in floats before any cast, so a tiny fmin or resolution is refused rather than saturating. It also refuses resolutions under 0.001, more than `MAX_STATES` states, and a band as wide as the range. The crate's own check let that last one through, and then `transition_local`'s assertion fired.
  - `track` refuses a signal shorter than a frame.
  - Non-finite samples and a non-finite padding value read as silence.
  - A candidate below the lowest bin goes to the lowest bin, as librosa clips it.
- **Faster.** Decoding searches each state's band. The best way in from outside the band is the first state outside it, with the states sorted once per frame by their score at nil probability. A test checks this against the dense search: random observations with ties, NaN, nil and certainty, odd and even widths, and bands covering the whole range. A second test checks scores that round together. Only the frame before and the frame being decoded are kept. A judge run of three frames at 48 kHz, release:
  - the crate: 1.3 to 1.8 ms CPU;
  - the port: 0.19 to 0.34 ms, measured on a sine, noise and silence.

  The fixture test runs in 10 s in debug, against 96 s before.
- **The ceiling.** The detector's 64 MiB transition-table ceiling becomes `MAX_STATES`, 2896 states, inside `Settings::grid`. That is exactly where the crate's table passed 64 MiB, so the configurations accepted are unchanged, apart from the band-as-wide-as-the-range case above. The refusal names the state count.
- **Failed runs.** A pYIN run that fails is counted, not hidden: `PitchDetector::failed_runs` and `last_failure` record it, and the voice judge adds it to `JudgeStatsSnapshot::pitch_runs_failed`, read through `Judge::stats()`. The frame reads as unvoiced. Nothing shows the judge's counters on screen yet, this one or the two it already had. A run cannot fail under a validated configuration, so the counter is there to be honest about a failure, not because one is expected.

## Decisions

1. **A module in kazoo-core, not a crate of its own.** A new crate name on crates.io takes a slot under the new-crate rate limit and adds another name to keep. Only kazoo-core uses pYIN.
2. **f64 only, plain slices, no ndarray.** kazoo uses `PYINExecutor<f64>` alone. ndarray would bring back most of the graph this lane removes.
3. **Bit-for-bit, not "within a tolerance".** A near-tie in the Viterbi can flip on one ulp, so "close" is not "the same". The sums follow ndarray's order and the beta table is statrs's own values. A test checks the table against Beta(2, 18)'s closed form to 1e-13.
4. **No reflect padding.** kazoo pads with zeros only. Reflect padding would be dead code.
5. **The ceiling moves into the tracker.** The wav scorer builds trackers too, and a tracker should never be asked for an unbounded grid.

## Done when

- These tests pass: the fixture, bit for bit; banded against dense; the model against the crate's construction, bit for bit; the ported crate tests (librosa's Boltzmann values, `transition_local`, the Viterbi example, the triangle window); and the refusal, ceiling, below-the-grid and padding tests.
- The workspace passes clippy and nextest. The only failures are the two known R04 failures, whose output is identical to main's.
- `cargo tree` finds no `zip` or `ndarray`, and a fresh resolution with no lockfile succeeds.
- An Opus review finds nothing open.
- Release 0.1.1, the release Tom asked for on 17 Oct 2026, goes to crates.io with `cargo install kzoo` working.
