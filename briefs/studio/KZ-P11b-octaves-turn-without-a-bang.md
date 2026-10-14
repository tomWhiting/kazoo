# KZ-P11b: octaves by band, finished: turning `low`, `high` or `width` never bangs or drops the level, every path is at unity, the response time is stated truly, and the code is built, tested and measured for the first time

Line numbers are from `lane/kz-P11` at 02e89b6; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-fx -p kazoo-wall`.

Place: Track P, lane 11b. Written by Cassio, 14 Oct 2026, 22:1x, from the Opus review of KZ-P11. This lane **builds on `lane/kz-P11`** (open with `BASE=lane/kz-P11`), then merges main into itself. Decided by Cassio; each decision's reason is beside it.

Time: about 50 minutes. Crates: kazoo-fx (`shape/octaves.rs`, `shape/octaves/band.rs`, `shape/octaves/tests.rs`), kazoo-wall (`daemon/wall/cost.rs`), `briefs/studio/BENCH.md`.

## Why

P11 was written in a round where nothing was compiled. The review modelled the bank in f64 with P11's own equations and found that an ordinary change of `low`, `high` or `width` (P05 sets `low` and `high` from an input's profile, so this happens in normal use) gives a burst up to +21 dB, and levels off by up to 21 dB afterwards. It also found three places where the brief itself was wrong. This lane fixes all of it and then runs everything for the first time.

## Decided, where the brief was wrong

1. **`width` runs from 0.8 to 1.25 (was 0.5 to 2.0), default 1.0.** At 0.5 the bands leave gaps (up1 falls about 20 dB between band centres); at 2 neighbouring bands' doubled phases cancel the centre band's up1. Within 0.8 to 1.25 the bank stays usable, and the lag-against-roughness trade still exists. The test in point D3 below holds it there.
2. **Test 4 uses pure sines.** `testkit::chord` gives each note six harmonics, and down1 of each 2nd harmonic lands exactly on the chord tone, so "nothing within 20 dB at the chord tones" can never hold for it. The test means the octave paths themselves; a local `pure_chord` helper in `tests.rs` (sines only) is what it asserts on. The model gives −42.8, −75.3 and −67.1 dB there, spread 0.4 dB.
3. **The response time is about `2.4 / (π · bw)`**, not `1 / (π · bw)`. That was the band's time constant; the −3 dB rise of this second-order band is about 2.4 of them. The doc says: about 24 ms at 1 kHz, about 100 ms at 60 Hz, about 230 ms at `low`'s 30 Hz floor, with `width` 1. Test 8 allows 30 ms at 1 kHz (24 ms plus a margin).

## The fixes (build exactly these)

- **D1, a redesign never bangs.** At a redesign, zero the spare bank's filter state and phase dividers, run it alone on the input for `3 / (π · bw_min)` (its slowest band's settling; samples counted, no allocation), and only then start the 10 ms crossfade. The spare bank does not run outside a redesign (D9).
- **D2, levels stay at unity after a redesign.** `calibrate` runs on the spare bank inside `retune` (80 band responses, about 20 µs, no allocation). Gains are kept per bank, and each bank's gains are applied before blending.
- **D6, the wall knows its cost.** `kazoo-wall/src/daemon/wall/cost.rs` gains `("octaves", 800_000)` after the `gate` row, with the comment "provisional until fx/octaves is measured" as `sidestep` and `gate` have; then replace it with the measured figure from the bench below, with its measuring date.
- **D7, the down paths never cancel in a crossfade.** `Bank::sample` also answers its complex down1 and down2 sums. At a crossfade's start, the spare bank's down1 sign (±1) and down2 quarter turn (1, i, −1, −i) are chosen to best line up with the active bank's sums, and kept through the fade.
- **D8, every path is at unity.** Next to tests 2 and 3: up2, down1 and down2 each at 100% on a sine at three frequencies across the bank are within 1 dB of the input's level.
- **D10, the wet is only the octaves.** `process_wet` writes the octaves alone (after `tone`, times `level`) to `wet`, as `pitch` and `shift` do.
- **D11, test 9 catches a bang or a dip.** On a single down1 sine, for each of `low`, `high` and `width` changed once: the 2 ms RMS stays within ±3 dB of its level before the change, for 200 ms after it. Keep test 9's chord sweep and finiteness checks.
- **D13, the `follows` helper never panics.** A 10 ms output window with no matching input window is skipped and counted; the test asserts the count is under 5%.
- **D3's test.** `width` at 0.8, 1.0 and 1.25: each path's level across the bank's range, from `Band::response` (no audio needed), stays within 3 dB for up1 and down1 and within 7 dB for up2 and down2 (the review found up2 varies about 7 dB at width 1).
- **Merge main.** Then `briefs/studio/BENCH.md` conflicts (both appended at the end): keep main's sections, then P11's.

## Red tests first

D8, D11, D3's test and the new test 4 and test 8 fail first on `lane/kz-P11` (paste under RED); then all of tests 1 to 9 and these pass (GREEN).

## Bench

`fx/octaves` on a 128-sample block at 48 kHz, steady and during a redesign, under BENCH; the figure goes into `cost.rs` and `BENCH.md`.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-wall/src
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-fx -j 4`.

## Definition of done

- [ ] The three decisions are in the code, the doc and the tests.
- [ ] D1, D2, D6, D7, D8, D10, D11, D13 and D3's test done; RED then GREEN pasted.
- [ ] Bench measured; `cost.rs` and `BENCH.md` carry it.
- [ ] Every check clean, main merged in.

## END
