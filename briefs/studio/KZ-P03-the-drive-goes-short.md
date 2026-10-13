# KZ-P03: every circuit drive can run its oversampler short (polyphase IIR halfbands), so a screamer into an amp on a live input costs under half a millisecond, not two

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 3. The effect chain on an input is the centre of Tom's use, and the circuit drives are the effects a guitar, a bass or a screamed-into pickup goes through first.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: the 45.4-sample round trip, each stage's `tbw` (0.083, 0.29, 0.40), that all seven models use the oversampler, and `amp.rs` at 956 lines. Change: test 4 allows 1.5 dB above 8 kHz, where an allpass halfband's phase departs most from linear and moves the harmonics the drive makes up there; 0.5 dB stays the bound below 8 kHz. The `phase` knob on seven drives is accepted: appended last, it keeps every saved patch.

Time: about 40 minutes. Crates: kazoo-fx (kazoo-wall in the checks only, because its catalogue takes every kazoo-fx parameter as a knob).

Depends: KZ-D16a (it rewrites `Halfband::dot` in `kazoo-fx/src/drive/oversample.rs`, the file this lane extends) must have landed before this lane opens. If KZ-D16b is in flight (it touches fuzz and crush), it must have landed too.

Files (10): `kazoo-fx/src/drive/oversample.rs`, `drive/oversample/iir.rs` (new, with its tests), `drive/pair.rs`, and one parameter and one match arm each in `drive/klon.rs`, `screamer.rs`, `fuzz.rs`, `amp.rs`, `fold.rs`, `ringmod.rs`, `comp.rs`. That is ten, against the under-ten guide: seven of them are a two-line edit each. Say so under CHANGED.

Why: every oversampling drive delays its input by its linear-phase halfbands. At 48 kHz the first stage is 71 taps (`first_stage`, `oversample.rs:210-216`: transition (48 − 40)/96 = 0.083, 82 dB by Kaiser's formula, m = 17), and the round trip at 8x is `(4·17+1)/2 + (4·7+1)/4 + (4·7+1)/8 = 45.4` host samples (`round_trip`, `oversample.rs:442-447`), padded to 46: 0.96 ms. A screamer into the amp is 1.9 ms, and that is half of the whole software budget (P01: 2.7 ms at 64 frames). The halfbands are linear-phase so that wet and dry stay phase-aligned on the wall and in mastering. A live input does not need that; it needs the time back.

## Outcome

Each of klon, screamer, fuzz, amp, fold, ringmod and comp gains a last parameter `phase`, stepped, `linear` (default, unchanged bit for bit) or `short`. With `short`, the oversampler's halfbands are two-path polyphase allpass IIR filters with the same stopband (90 dB from the image of 20 kHz). Each model's `latency()` is then at most 12 samples at 48 kHz (0.25 ms), measured as group delay at 1 kHz. Its magnitude response and aliasing match `linear`'s within the bounds below, and it costs no more CPU.

## The design (decided; build exactly this)

### The filter (`drive/oversample/iir.rs`)

A halfband built as two allpass branches, `H(z) = ½ [A0(z²) + z⁻¹ A1(z²)]`, each branch a cascade of first-order sections in `z²`, `(a_i + z⁻²) / (1 + a_i z⁻²)`. This is the structure of Valenzuela and Constantinides (1983) and of Laurent de Soras' HIIR library, and the elliptic design MathWorks documents for IIR halfbands ("fewer coefficients, non-linear phase"). Interpolation runs the two branches on the low-rate input and interleaves them. Decimation runs them on the even and odd samples and averages: one multiply-add per coefficient per low-rate sample in each direction.

**Coefficients**, computed in `configure` (off the audio thread), never typed in. Use the elliptic-modulus construction HIIR's `PolyphaseIir2Designer` uses (Vaidyanathan, *Multirate Systems and Filter Banks*, ch. 5):
1. The transition band `tbw` is normalised to the high rate, so the passband edge is `0.25 − tbw/2` and the stopband edge is `0.25 + tbw/2`. The stopband attenuation is `A = 90 dB`.
2. From `tbw`, compute the elliptic modulus `k` and its nome `q` (a short series). From `q` and `A`, compute the smallest number of coefficients `N` that meets the attenuation.
3. Compute the `N` coefficients from `q` by the designer's product series.
4. Write the designer from the published method, with a doc comment naming the method and its sources. Test it against the attenuation it promises (test 1), not against a copied table.

**Band edges per stage**, the same rule the FIR stages use. A stage that runs at `2R` passes to `p = min(20 kHz, 0.45 R)` and must reject from `R − p`, where the image of `p` lands. So `tbw = (R − 2p) / (2R)`, never below 0.02 (as `first_stage` clamps). At 48 kHz that is 0.083 for the first stage, (96 − 40)/192 = 0.29 for the second, and 0.40 for the third.

**Group delay.** The section `(a + z⁻²)/(1 + a z⁻²)` delays DC by `2(1 − a)/(1 + a)` samples of the high rate. `latency()` for `short` is the round trip's group delay at 1 kHz, computed exactly from the coefficients at `configure` (evaluate the phase of the up-down cascade at 1 kHz and 1 kHz + 1 Hz and difference), rounded to the nearest whole host sample, plus the `extra` half samples the antiderivative models add, divided by the factor as today. Why 1 kHz: it is in the middle of where a guitar's and a voice's energy is. The delay is not constant across frequency (that is what non-linear phase means), and it rises toward 20 kHz. A host aligning a dry path is right at 1 kHz and close below 5 kHz. Write that in the doc of `latency()` for `short`.

**No pad.** `whole_latency`'s Thiran pad (`oversample.rs:349-361`) exists to make a linear-phase delay whole. It is not applied for `short`, because it would add up to 3.5 high-rate samples back.

### The switch

- `pub enum Phase { Linear, Short }`.
- `Oversampler::configure(factor, base_rate, extra)` keeps its signature and means `Phase::Linear`. A new `configure_with(factor, base_rate, extra, phase)` is the general form, and `configure` calls it.
- `Oversampler` holds both stage kinds pre-allocated (the FIR `stages` and an `iir: [IirHalfband; MAX_STAGES]`), so switching never allocates. Switching resets the histories.
- `Pair` (`pair.rs:137-138`) passes the phase through. The model files read their `phase` knob and, when it changes, call the pair's reconfigure from `set_param`'s stepped path (as each does for any stepped knob that changes latency, per the `Effect::latency` contract, `kazoo-fx/src/lib.rs:193-200`).
- Each model's `PARAMS` gains, last, `ParamSpec { name: "phase", min: 0.0, max: 1.0, default: 0.0, unit: "", curve: Curve::Stepped { labels: &["linear", "short"] } }`. Appending keeps every existing index and every saved patch as it was. The wall shows one more knob on these seven kinds; find any wall test that pins a kind's knob count and update it in its own commit, named under CHANGED.

## Not this lane

The input strip's choice of `short` and its latency readout (P05). The lofi, tape and pitch effects that also oversample: they are not on a live input's path, and they keep `linear`. Any change to the models' sound beyond the phase response.

## Red tests first

In `drive/oversample/iir.rs` tests and each model's tests. Red for all: nothing here exists.

1. `each_iir_stage_meets_its_stopband`: at host rates 44.1, 48 and 96 kHz and factors 2, 4 and 8, evaluate each stage's magnitude on 4096 points: at least 90 dB down from `R − p` to `R`, and within 0.01 dB of unity from 0 to `p`.
2. `the_iir_round_trip_is_short`: at 48 kHz, the group delay at 1 kHz of the 8x round trip is at most 12 host samples; print the figure for 2x, 4x and 8x under BENCH.
3. `latency_is_the_group_delay_at_1_khz`: a 1 kHz tone burst through each model at `short` with its other knobs at defaults (mix 100% where there is one): the burst's envelope centre arrives within ±1 sample of `latency()`.
4. `short_sounds_like_linear`: D07's null helper cannot be used, because the phase differs. Instead, each model at `linear` and `short` on D01's fixture (`kazoo_fx::bench::fixture(48_000.0)`) at defaults and at drive or gain 100. The magnitude spectra, in third-octave bands from 31.5 Hz to 16 kHz over the whole fixture, agree within 0.5 dB in every band up to 8 kHz and within 1.5 dB above it. Print the largest difference per model, below and above 8 kHz.
5. `short_aliases_no_worse`: a 9 kHz sine at −1 dBFS through screamer (drive 100) and amp (gain 100). The energy at non-harmonic frequencies (every bin further than 3 bins from a multiple of 9 kHz) is within 3 dB of `linear`'s.
6. `linear_is_unchanged`: each model's output at `phase = linear` equals its output before this lane bit for bit (D16a's `as_before` switch, or a stored hash of the fixture's output taken first, in its own commit).
7. `a_glide_and_a_vibrato_come_through_without_steps`: the amp at `short`, gain 30 (a clean-ish setting where the fundamental dominates), on the perfect fourth over 2 s (220 to 293.66 Hz) and on ±30 cents at 6 Hz. The output's fundamental per 10 ms window (zero crossings after a 1 kHz low-pass) follows the input's within 2 cents at a lag of `latency()`, with no window step larger than the input's own change plus 1 cent.
8. `switching_phase_never_allocates_or_clicks`: toggling `phase` every 100 ms during a 220 Hz tone inside `assert_no_alloc`. Every output sample is finite, and the largest step within 2 ms of a switch is at most three times the largest step in the 100 ms before it (`testkit.rs`'s `knob_jumps_do_not_click` pattern).
9. `short_costs_no_more`: the bench `fx/<model>/short` for screamer and amp is at most its `linear` row, measured in the same run. Print both and the load average under BENCH.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-wall/src
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-fx -j 4 -- 'screamer|amp'
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-fx and kazoo-wall test passes.
- [ ] BENCH.md `## KZ-P03 short phase`: each model's `latency()` at 48 kHz for `linear` and `short`, and the screamer and amp bench rows for both, with the load average, date and sha.
- [ ] The designer's doc comment names its method and sources.
- [ ] Not the builder's: after P05 lands, Cassio with Tom puts screamer then amp at `short` on a guitar input, and Tom plays the scream trick through it. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. `process`, `set_param` and `reset` never allocate: both stage kinds exist from `new`. Coefficients are computed in `configure`, which runs from `prepare` or from a stepped knob change. The design's series terms are bounded loops (at most 20 terms, with the convergence stated). Every allpass state is flushed of denormals with the family's `flush64` (`kit.rs`). `amp.rs` (956 lines) must stay under 1000: add only the parameter and the arm. Files under 1000 lines.

## END

```
START KZ-P03 base=<sha> branch=lane/kz-P03 df=<GB> at <time>
END KZ-P03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: each model's latency linear and short; screamer and amp bench rows, linear and short
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
