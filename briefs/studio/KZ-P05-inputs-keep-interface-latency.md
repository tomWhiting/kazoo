# KZ-P05: an input keeps the interface's latency through its own gate and its inserts: it is never delayed to match other strips, its head shows what its chain costs, and range knobs start at its profile

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 5. With this lane, an input is what Tom asked for: his instrument in, through effects, heard at interface latency.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: P01's `bass` range is (29, 500); `n` is free in kazoo-mix; P10 to P12's `low` knobs start at 30 Hz, so a bass input's 29 clamps to 30 through the `ParamSpec`, as the design says. The exemption from compensation is accepted: an input is heard where it is played, which is what Tom asked for. Accepted as written.

Time: about 30 to 40 minutes. Crates: kazoo-mix.

Depends: each of these must have landed before this lane opens:
- KZ-P02 (input strips);
- KZ-P03 (the `phase` knob the warning names);
- KZ-P04 (the `gate` effect);
- KZ-C02 (insert chains and latency compensation);
- KZ-C03 (the insert view and its put path).

Files (8): `kazoo-mix/src/inputs.rs`, `inputs/gate.rs` (new), the inserts engine file C02 made (`inserts.rs`), the strip's file, `controls.rs`, `desk.rs`, `ui.rs` (strip head and help), `inputs/tests.rs`.

Why: C02 delays every strip to the slowest chain, so the mix stays in time. For a live input that rule is wrong. If any other strip carries a linear-phase EQ or a reverb with a pre-delay latency, Tom's own guitar would come back that much later, and a player feels 5 ms. An input must be heard at the interface's latency plus its own chain, nothing more. He also asked for "a named input with gain, a clip light and an optional noise gate"; P02 gave gain and the clip light, and this lane gives the gate.

## Outcome

- Every input strip has its own noise gate in front of its insert chain, off by default. `n` on the strip toggles it, and its threshold is a strip control (`GATE`) moved like trim. Its hold comes from the profile.
- An input strip is never delayed by compensation, and its chain's latency never raises other strips' delay.
- The input's head shows its total latency, the software share plus its heard inserts' `latency()`. The figure is amber over 1 ms of inserts, and the footer names the slow effect and the fix ("amp adds 0.96 ms; set its phase to short").
- An effect with `low` and `high` parameters, put on an input, starts with them at the input profile's range.

## The design (decided; build exactly this)

### The input's gate (`inputs/gate.rs`)

- Each input strip owns a `kazoo_fx::Effect` built from `kazoo_fx::find("gate")` and prepared at the desk's rate before the stream starts (in `MixerEngine::attach_live`'s path), with a pre-allocated stereo scratch. It runs on the strip's post-trim, post-EQ signal, right before the insert chain: `source ─ trim ─ EQ ─ gate ─ inserts ─ fader…`. Why after trim: the threshold then reads against the level Tom set, so turning the gain up does not change what the gate lets through relative to the playing. Why before the inserts: the digital drives add no noise, so gating their input silences their output.
- `ChannelControls` gains `gate_on: bool` (default false) and `gate_db: f32` (−90 to 0, default −55, P04's default), sanitised as the other controls are (`finite_clamp`, `controls.rs`). `StripControl` gains `Gate`, with label `GATE`, reached with the same keys that move between trim, EQ and fader. It is shown only on input strips: for any other strip it is skipped.
- Bypass is a 5 ms ramp between the gated and ungated signal, the engine's `ramp_frames`. The gate keeps running while bypassed, as C02's bypassed inserts do, so turning it on never starts mid-release.
- Hold by profile, set once at attach: `voice` 30 ms, `guitar` 50 ms, `bass` 80 ms, `line` 50 ms. Reasons: bass, because 80 ms is over two periods of B0 (32.4 ms each); guitar and line take P04's default; voice is shorter so a sung phrase's consonants are not held open over a breath.

### Inputs are exempt from compensation (C02's `inserts.rs`)

- In C02's computation (`total = chain.latency + bus chain latency`, `max_total` over strips in use, each strip delayed by `max_total − total`), a strip whose source is live is left out of `max_total`, and its own `CompDelay` is held at 0 frames.
- If an input is routed to a group with a chain, the group's chain latency is still heard on it (it is in the input's path), and it is shown in the head's figure. The group's other strips are compensated as before, among themselves.
- Recording (C04) records what was heard, so an input's take sits where Tom played it against what he heard. A doc comment on the exemption says this, so C04 adds no shift.

### The head (`ui.rs`, P02's input head)

- The latency figure becomes `P01 latency_frames + gate (0) + Σ heard inserts' latency()` on the strip, plus any group chain's. It is published per slot by the callback (C02 already recomputes chain latency on every chain change; publish it in P02's `InputCell::latency_frames`).
- When the inserts' part is over `INPUT_INSERT_WARN_MS = 1.0` (reason: past 1 ms an insert costs more than a quarter of the software share at 64 frames), the figure is amber. The footer names the slowest heard insert by its catalogue name and adds "set its phase to short" when that effect has a `phase` parameter (P03), or "this effect is slow by design" when it has none.
- `G` in the head (dim when off, bright when on) shows the gate.

### Range knobs start at the profile (`inputs.rs`, C03's put path)

When C03's `InsertModel::put` builds an effect for an input strip, any parameter named `low` is set to `profile.range_hz().0` and any named `high` to `.1`, each clamped through its `ParamSpec`, before the `Put` is sent. That is the only place a profile touches an effect. P10, P11 and P12 name their range knobs `low` and `high` for this.

## Not this lane

Tracking (P08). The gate effect itself (P04). Changing C02's rule for any strip that is not an input.

## Red tests first

In `inputs/tests.rs` and the inserts engine's tests (fake clock `RATE = 800` where C02's tests use it, 48 kHz where a profile or a time in ms matters). Red for all: none of this exists.

1. `an_input_is_never_delayed_to_match_a_slow_strip`: an instrument strip with C02's test `Delaying(48)` insert and an input strip with no inserts: an impulse on the input leaves the master on the same callback frame it would without the other strip, and the instrument strip is not delayed for the input either (`compensation_frames() == 0` with only these two).
2. `an_input_does_not_raise_the_others_delay`: two instrument strips (`Delaying(4)` and none) and an input with `Delaying(40)`: the instruments are aligned with each other at 4, and the input's 40 does not enter `max_total`.
3. `the_gate_runs_before_the_inserts`: gate on at −40 dB, a `Delaying(0)` probe insert that records its input: below the threshold the probe sees silence (after release), above it the probe sees the signal.
4. `gate_toggle_ramps_and_never_clicks`: toggling `n` during a 220 Hz tone: the largest step within 5 ms of the toggle is at most three times the largest step in the 100 ms before it.
5. `the_hold_comes_from_the_profile`: a `bass` input's gate holds 80 ms (± 1 sample at 48 kHz) and a `voice` input's 30 ms.
6. `the_head_counts_the_chain_and_warns_over_a_millisecond`: an input with `Delaying(96)` at 48 kHz (2 ms): the published latency is the P01 figure plus 96, the head draws it amber, and the footer names the effect. With `Delaying(24)` it is not amber.
7. `low_and_high_start_at_the_profile`: a test `EffectKind` with params `low` and `high`, put on a `bass` input: `low == 29`, `high == 500`; put on an instrument strip, they keep their defaults.
8. `a_glide_and_a_vibrato_come_through_the_gate_and_inserts_without_steps`: the perfect fourth over 2 s and ±30 cents at 6 Hz at −20 dBFS, through an input with the gate on (−55 dB) and kazoo-fx `eq` flat as an insert. Every 10 ms window's pitch is within 2 cents of the input's, and no window steps by more than the input's change plus 1 cent.
9. `the_callback_never_allocates_with_gates_and_inserts`: two gated inputs with inserts for 1 s inside `assert_no_alloc`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-mix test passes, C02's compensation tests unchanged for non-input strips.
- [ ] The exemption's doc comment says why, and says that C04 needs no shift.
- [ ] Not the builder's: after landing, Cassio with Tom puts gate, screamer (`short`) and amp (`short`) on a guitar input, and Tom plays the scream trick, with a long reverb on another strip to prove the guitar is not delayed by it. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. The gate and its scratch exist before the stream starts. Nothing in the render path allocates, locks, formats or does I/O. Every gate control is sanitised, and a NaN reads as the default. `ui.rs` and `engine.rs` are over 1000 lines: add only the calls named, and put the logic in `inputs/` files.

## END

```
START KZ-P05 base=<sha> branch=lane/kz-P05 df=<GB> at <time>
END KZ-P05 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
