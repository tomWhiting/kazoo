# KZ-D16b: the tape, the crusher and the fuzz stop doing work nobody hears, bit for bit, and the grimy save fits in under 3 ms

Line numbers are from the sweep that wrote this brief, at `820ccc5`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, except that every cargo command takes the form under **Checks** below.

Read by Cassio, 13 Oct 23:0x. Accepted as written. The branch is whatever `lane.sh open` names (`lane/kz-D16b`); put that name in START and END. The study's estimate was about 3.3 ms, so the 3.0 ms target may be missed; a miss with every null holding still lands, as the definition of done says.

Time: about 75 minutes of a builder's work. Crate: kazoo-fx (tape, crush, fuzz, one noise method). Depends: **KZ-D16a**. Branch from D16a's reviewed head (or from main once D16a has landed). This lane uses D16a's harness (`kazoo-fx/src/drive/nulls.rs`), its switches and its `save_profile` bench, and it does the final measurement of the grimy save for both lanes.

This lane does not touch the live wall: no MCP call, no load, no save, no daemon restart. The save is read from disk, read-only, and rendered headless.

## Why this lane exists (measured 13 Oct 2026, 19:54 to 20:37, by Cassio)

KZ-D16a's first section has the grimy save's profile: a module sum of 4,247.5 µs a callback, 41% of the deadline at real-time priority. tape1 (522 µs), crush2 (248) and the three fuzzes (379, 360, 804) are this lane's. In the save:
- tape1, crush2, fuzz2 and fuzz3 hear only their left jack, which the wall mono-normals to both sides (`kazoo-wall/src/dsp/effect.rs:6`), so both sides hear the same samples.
- tape1's `heads` is cabled from an LFO.
- crush2 sits at `aa` 0, with `bits` cabled.
- fuzz2's `fuzz` knob is cabled; fuzz3 and fuzz4 sit still.

Each of these effects spends work on something no one hears:

- **Tape** (`kazoo-fx/src/time/tape.rs`). The loop at `:475` runs both tracks' whole record chain on a mono input: emphasis, an oversampled saturation, de-emphasis, gap, bump, the makeup followers, and a sinc read per head. Both tracks compute the same thing. Separately, `head_delays` (`:408`) and `Wobble::errors` (`:228`) take 12 cosines a sample, three wobbles times (now plus three heads), even when one head plays. The read loop then skips the silent heads' delays (`gain > 1e-4`).
- **Crush** (`kazoo-fx/src/drive/crush.rs:661` to `:740`, `frame`). With `aa` at 0, the filtered converter is weighted by exactly 0 (`filtered.mul_add(restored − raw, raw)`). It still runs `before.peek` on every tick (four complex exponentials), plus `before.ramp`, `after.step` or `after.ramp`, and `clean_line.ramp` (ten poles) every sample. The sweep measured `aa` 0 and `aa` 1 at the same 207 µs. It also runs both channels on a mono input.
- **Fuzz** (`kazoo-fx/src/drive/fuzz.rs:457`, `frame`). `guitar_volume` (`:469`, two audio tapers) and `db_gain` (`:481`, a `powf`) are computed every frame while the knobs sit still. (D14 already twins the fuzz's channels on mono input.)

## Outcome

After this lane:
1. A tape on a mono input runs one record chain and copies it, a tape reads only the heads that play, a crusher with `aa` off runs no anti-alias filters, a crusher on a mono input without dither runs one channel, and a fuzz recomputes its input and output gains only when their knobs move.
2. Every output (mix and wet, both channels) is identical bit for bit to today's in every case. There is one stated exception: the 250 ms after a crusher's `aa` is switched back on from off (see 2 below), which is bounded and click-free.
3. The grimy save's module sum, measured with D16a's bench on a quiet machine, is at most 3.0 ms (from 4.25 ms on a loaded one). `BENCH.md` records it.

## The design (decided; build exactly this)

### 1. Tape: one record chain while both sides are the same (`time/tape.rs`)

**Split `Track`** (`:242`) into two parts:
- `chain: Chain`, which is `Copy`: emphasis, deemphasis, oversampler, clip, gap, bump, low, level, before, after. Every one of these is already `Copy`.
- What stays per track: `line`, `lifted` (both heap `DelayLine`s) and `hiss_tone`.

**The noise is per track.** `self.noise.sample()` (`:504`) is drawn once per track per sample, in track order. That is the only thing that makes a mono tape's two outputs differ, through `hum`.

**`Tape` gains `sharing: Sharing`**, with `Twinned` and `Apart`, as in `drive/pair.rs`. `reset` sets it to `Twinned`, and `prepare` calls `reset`.

**The test for mono, decided:** at sample i, `clean(input[0][i]).to_bits() == clean(input[1][i]).to_bits()`. It compares the values the tracks actually process, so a NaN on one side and 0.0 on the other still count as the same, because `clean` makes both 0.0. A `-0.0` against a `0.0` counts as different, which is the safe side.

**While `Twinned` and the test holds, per sample:**
- Run track 0 exactly as today.
- For track 1, push track 0's `record` into `tracks[1].line` and its `lifted` into `tracks[1].lifted`. These are two stores, so neither long line ever needs copying.
- Draw track 1's white noise as today, in the same order (after track 0's), run `tracks[1].hiss_tone.lowpass`, and form track 1's `hum` with track 0's `level`.
- Write `output[1][i] = guard(dry.mul_add(x, wet * (echo + hum1)))` and the wet output, with track 0's `echo`.
- Skip track 1's `chain` entirely.

Every value equals what running track 1 would give, because its inputs and state are track 0's.

**The first sample the test fails:**
- `tracks[1].chain = tracks[0].chain` (a `Copy` of under 3 KB).
- `sharing = Apart`.
- Run both tracks fully from that sample on, until `reset`.

Track 1's line, lifted line and hiss filter are already its own, so after the copy its whole state is exactly what it would have reached running itself. The output is identical bit for bit to running both all along, so a switch to stereo mid-stream cannot click: it is not a different sound.

**Why apart until reset**, as D14 decided for `Pair`: two tracks that have heard different samples never return to the same state, so twinning them again would be wrong. Unplugging the right cable leaves the tape at full cost until it is reset, for example by a load. Say this in `Tape`'s doc.

**Why not copy the lines at the hand-off:** a 12.1 s line at 48 kHz is 2²⁰ floats (4 MB). Copying it on the audio thread would cost hundreds of microseconds in one callback. Pushing one sample a frame costs nothing.

`refresh_filters` keeps setting both tracks' gap and bump (every 32 samples), unchanged.

### 2. Tape: only the heads that play (`time/tape.rs:228`, `:408`, `:470`)

- `Wobble::errors` takes `active: [bool; 3]` and returns 0.0 for an inactive head without computing its cosine. `now` is computed once per wobble, as today, but only if any head is active.
- `head_delays` takes the same mask and leaves an inactive head's delay at 0.0.
- The mask is `gains[k] > 1e-4`: the exact predicate the read loop at `:479` already uses, computed once per sample before `head_delays`. A head whose delay is not computed is never read, and every active head's arithmetic is unchanged, so this is bit-exact.
- One head playing costs 6 cosines a sample instead of 12. While the LFO glides the heads between two sets, the heads on either side are computed.

### 3. Crush: no anti-alias filters while `aa` is off (`drive/crush.rs`, `frame`)

- **The skip, decided:** in a frame where `filtered == 0.0` (that is, `weight(aa, 1.0)` is exactly zero, which happens once `aa` is below 6e-8; from a glide, that is about 0.33 s after it is turned off), a channel does not run:
  - `before.peek`;
  - `before.ramp`;
  - the second `quantise`;
  - `after.step` or `after.ramp`;
  - `clean_line.ramp`.

  It also does not update `clean` or `last_restored`. Then `wet = raw` exactly, as `0.0.mul_add(restored − raw, raw)` gives today: `restored` is always finite after the modal flush, so the product is +0 or −0 and the sum is `raw`. Assert that in a test, with a `−0.0` restored.
- **The dither noise keeps its place.** Every tick, each channel draws exactly four samples from `dither_noise`, whatever is skipped: two in the raw `quantise`'s `tpdf()`, two in the held one's. A skipped held `quantise` calls `dither_noise.skip(2)`. So when dither or `aa` returns, the noise sequence is where today's would be.
- **Turning `aa` back on, decided:** on the first frame where `filtered > 0.0` after a skipped frame:
  - the channel's `before`, `after` and `clean_line` states are reset (`Modal::reset`);
  - `clean = 0.0` and `last_restored = 0.0`;
  - then the frame runs as today.

  The filtered converter therefore starts from rest. Its output rises smoothly from 0: an eighth-order lowpass starting from rest has seven zero derivatives. It is weighted by `filtered`, which starts at about 1e-7 and glides up with the knob (20 ms time constant), so the mix moves continuously from `raw` and cannot click.
- **The stated difference:** today the filters were warm at that moment. The difference is the warm filters' zero-input response, which decays at the slowest pole's rate, ω·sin(π/16) with ω = 2π × 0.45 × clock. At a 100 Hz clock that is a time constant of 18 ms, which is −90 dB within 190 ms. A rare quantisation flip of one step adds under −110 dB each.
  - **Tolerance, decided:**
    - bit-exact up to the frame where `filtered` first leaves 0;
    - in the 50 ms after it, the largest sample-to-sample step is at most 1.1 times the reference's largest step over the same span;
    - from 250 ms after it to the end, the RMS null is at most −90 dB.
  - Reason: 250 ms covers the slowest clock (100 Hz) with margin, and −90 dB is under a 16-bit LSB at the levels the fixture plays.
- **Why reset and not keep warm:** keeping them warm is the cost being removed (`before.peek` alone is four complex exponentials a tick), and there is no cheaper exact form.

### 4. Crush: one channel while both sides are the same and there is no dither (`drive/crush.rs`)

- `Crush` gains `sharing: Sharing`, with `Twinned` and `Apart`. `reset` sets it to `Twinned`.
- **The test, decided:** `left.to_bits() == right.to_bits()` and the frame's `dither == 0.0` (the knob is exactly 0, so `tpdf()` is exactly 0 on both sides). With dither, each side's quantisation draws different noise, so the sides differ by design, and the sharing must stop.
- **While `Twinned` and the test holds:** run channel 0 as today. Advance `dither_noise` by channel 1's four draws per tick (`skip(4)`). Return channel 0's out and wet for both sides.
- **The first frame the test fails:** `channels[1] = channels[0]` (`Channel` is already `Copy`; the pads and the line stages go with it), then `sharing = Apart`, then both channels run, until `reset`.
- The noise is in the same place and the state is exact, so this is bit-exact, including a dither turned up mid-stream.
- Extract the per-channel body of the closure in `frame` into `fn convert(channel: &mut Channel, …) -> [f32; 2]` so that the twin path and the apart path run one copy of the code.

### 5. `Noise::skip` (`kazoo-fx/src/dsp.rs:366`)

- `pub const fn skip(&mut self, count: u32)` runs the xorshift step `count` times, leaving the generator where `count` calls to `sample` would. It allocates nothing and cannot fail.
- Why: a draw whose value is thrown away reads as a mistake (and the tree forbids `let _ =`); `skip` says the draw is kept only for the sequence's place.

### 6. Fuzz: input and output gains only when their knobs move (`drive/fuzz.rs:457`)

- `Fuzz` gains:
  - `volume: Retune<1>` and `input_gain: f64`;
  - `level: Retune<1>` and `out_gain: f64`.

  They are built from the defaults in `build`, with `input_gain = guitar_volume(fraction(default INPUT))` and `out_gain = db_gain(default LEVEL) * OUTPUT_SCALE`.
- In `frame`:
  - `if self.volume.due([knob[INPUT]]) { self.input_gain = guitar_volume(fraction(knob[INPUT])); }`;
  - the same for `LEVEL`;
  - `Settings.input` and the output multiply read the cached values.
- `Retune::due` compares bits, so a knob that has not moved by one bit costs a compare, and a moved one is recomputed by the same function on the same value: bit-exact. A cabled knob that moves every frame recomputes every frame, as today.
- The fuzz keeps its 8x rate.

### 7. The switches (D16a's `nulls.rs`)

- Add `Change::TapeTwins`, `Change::TapeHeads`, `Change::CrushQuiet`, `Change::CrushTwins` and `Change::FuzzKnobs`.
- Each new path begins with `#[cfg(test)] if nulls::is_before(Change::…) { … today's code, verbatim … }`. `TapeTwins` and `CrushTwins` force `Apart` from the first sample. `TapeHeads` passes all heads active. `CrushQuiet` never skips. `FuzzKnobs` recomputes every frame.
- **Commit the new cases before any change** (this lane's first commit), so `every_case_prints_its_fingerprint` prints today's hashes for them. At the head, every `before` hash equals that commit's `now` hash.

## Not this lane

- Threading or more cores (KZ-D06).
- The fuzz's 8x and the klon's 4x rates (Tom's to hear).
- Hall, pitch, bbd, vcf, radio and digital.
- Twinning a tape or crusher again after it has gone apart (state cannot rejoin; see 1).
- Anything on the live wall.

## Red tests first

There is no timing assertion in any test, for D16a's reason: this Mac's figures for one save swing 2.6x between core types and with load. The reds are work counters (`#[cfg(test)]` fields counting the work done) and the switches that do not exist yet.

1. `drive::nulls::tests::tape_runs_one_record_chain_for_mono`. A `#[cfg(test)] chain_runs: u64` on `Tape` counts record-chain runs. One second of `Mono` gives exactly 48,000. Then `MonoThenStereo` gives 48,000 for the first second and 2 × 48,000 for the second. Red: 96,000.
2. `drive::nulls::tests::tape_reads_only_the_heads_that_play`. A `#[cfg(test)] cosines: u64` counter. With `heads` settled at step 1 ("2"), one second gives 6 × 48,000. At step 6 ("1+2+3") it gives 12 × 48,000. Red: 12 × 48,000 at step 1.
3. `drive::nulls::tests::the_tape_nulls_bit_for_bit`, against `as_before(&[Change::TapeTwins, Change::TapeHeads])`, with `assert_bit_exact`. Cases:
   - `default`;
   - `the save` (drive 0.8, feedback 0.5425, flutter 0.475, heads 1, hiss 0.15, mix 0.35, time 0.35, wear 0.3, wow 0.25);
   - `heads 3` (heads 3, drive 0.8);
   - `all heads` (heads 6);
   - `heads glide` (heads 1, turned to 6 at 1 s);
   - `time bends` (time 0.35, turned to 1.2 at 1 s);
   - `no hiss` (hiss 0).

   Each runs with `Mono`, `Stereo`, `MonoThenStereo` and `RightSilent`. Red: no switches.
4. `drive::nulls::tests::crush_runs_no_filter_it_does_not_hear`. A `#[cfg(test)] peeks: u64` counter. `aa` 0 from reset gives 0 peeks over a second. `aa` 1 gives one per tick. Red: one per tick at `aa` 0.
5. `drive::nulls::tests::the_crush_nulls_bit_for_bit`, against `as_before(&[Change::CrushQuiet, Change::CrushTwins])`, with `assert_bit_exact`. Cases:
   - `default`;
   - `the save` (bits 9, rate 12,000, aa 0, mix 63.5);
   - `aa on` (aa 1);
   - `dither` (dither 50);
   - `jitter` (jitter 40, rate 500);
   - `aa turns off` (aa 1, turned to 0 at 1 s);
   - `dither turns on` (dither 0, turned to 30 at 1 s).

   Each runs with `Mono`, `Stereo`, `MonoThenStereo` and `RightSilent`. Red: no switches.
6. `drive::nulls::tests::turning_aa_on_does_not_click`. `aa` 0, turned to 1 at 1 s, at clocks 12,000 and 100 Hz, `Mono` and `Stereo`, against `as_before(&[Change::CrushQuiet])`: section 3's three bounds, with the measured step ratio and dB printed. Red: no switch.
7. `drive::nulls::tests::the_fuzz_nulls_bit_for_bit`, against `as_before(&[Change::FuzzKnobs])`, with `assert_bit_exact`. Cases:
   - `default`;
   - `fuzz2` (fuzz 45, bias 50, input 100, level −8);
   - `fuzz3` (fuzz 100, bias 32, level −12);
   - `fuzz4` (fuzz 85, bias 50, level −12);
   - `octave` (octave 1);
   - `low input` (fuzz 10, input 20);
   - `input turns` (input 100, turned to 30 at 1 s);
   - `level turns` (level −12, turned to 0 at 1 s).

   Each runs with `Mono` and `Stereo`. Plus a counter `#[cfg(test)] gains_computed: u64`, which is 0 over a second of still knobs after reset. Red: no switch, and 48,000.
8. `drive::nulls::tests::the_hand_offs_do_not_allocate`. A prepared tape and a prepared crusher each render `MonoThenStereo` across the hand-off in 512-frame blocks under `assert_no_alloc`. Red: no hand-off.
9. `dsp::tests::skip_leaves_the_noise_where_samples_would`. Two generators from one seed: `skip(7)` on one and seven `sample()`s on the other leave equal `next_u32()`. Red: no method.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs, on Tom's Mac. This replaces the STACK RULES' form for this lane. Read `df -h /System/Volumes/Data` and `uptime` before each and put both in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx kazoo-wall
```

Every change here is bit-exact apart from the stated `aa` transient, so no other crate's output moves. nextest on kazoo-fx and kazoo-wall is enough.

**Benches, alternating, on this Mac.** M is D16a's measure commit (its first: the profiler alone, before any sound change; read the sha from D16a's END). B is this lane's base (D16a's head). H is this head. For each of three rounds, check out M, then B, then H, and run the commands below, recording `uptime` before and after each:

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-fx --bench circuit_models -j 4 -- 'fx/(fuzz|crush|tape)$'
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench wall_render -j 4 -- 'wall/kind/(fuzz|crush|tape)$'
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench save_profile -j 4 -- profile "/Users/tom/.kazoo/wall/saves/grimy beat fuzzed siren octave.json" --rt --secs 20
```

**"A quiet machine", decided:** the 1-minute load average is at most 30 at the start of the run. That is the range the study's quiet run had (17 to 30), and it is reachable on this Mac with the other seats' builds idle. A run above 30 is kept and marked "loaded". The target is judged on the median of H's unloaded runs. If no H run is unloaded, the target goes under NOT CONFIRMED with the loaded figures. Nobody waits for the machine to clear.

## Definition of done

- [ ] Reds 1 to 9 fail first (paste the failure), then pass. All existing kazoo-fx and kazoo-wall tests pass.
- [ ] The fingerprint tables from this lane's first commit and from the head are both pasted. Every `before` hash at the head equals the first commit's `now` hash.
- [ ] `turning_aa_on_does_not_click`'s printed step ratios and dB are pasted.
- [ ] `BENCH.md` gains "## KZ-D16b", with:
  - M, B and H shas;
  - each run's load before and after;
  - `fx/*` and `wall/kind/*` medians at M, B and H with the change in %;
  - the grimy save's per-module table at M, B and H (medians of the three rounds, per module);
  - its sum of modules and render CPU mean, p99 and max.

  Its last line reads `grimy save, sum of modules: <M> µs at M, <H> µs at H, quiet runs <n> of 3` and says whether H is at most 3,000 µs.
- [ ] **Target: H's sum of modules is at most 3.0 ms on a quiet machine.** It is expected from the study:
  - tape1 down by about half (one record chain, 6 cosines);
  - crush2 down by more than half (no filters, one channel);
  - fuzz gains off the per-frame path;
  - on top of D16a's dot and klon savings.

  If H misses it on a quiet run, the lane still lands, provided every null holds. The miss and the per-module residual table go under NOT CONFIRMED, with no technique added beyond this brief.
- [ ] Nothing allocates, locks or does I/O on the audio path (red 8), and NaN and Inf still come out as silence: the existing `poison_is_refused` and `silence_stays_silent` conformance tests for tape, crush and fuzz pass. One case is added to red 3: `Mono` with a NaN on the left at 0.5 s, which stays twinned (`clean` makes it 0.0 on both sides) and nulls bit for bit.

## Code quality

Tree rules. The twin state, the counters and the switches are fixed-size fields. The hand-offs copy only `Copy` state; the long delay lines are never copied. File lengths at `820ccc5`:

| file | lines |
|---|---:|
| `tape.rs` | 746 |
| `crush.rs` | 1,121 |
| `fuzz.rs` | 772 |
| `dsp.rs` | 618 |

`crush.rs` is already over 1,000. Add to it only the lines named here (the sharing field, the skip, `convert`, the reset on re-enable), put every new test in `nulls.rs`, and list `crush.rs` under FOUND with its line count. If `nulls.rs` would pass 1,000 lines, make its case tables more compact (one line per case), not a new file. One behaviour per commit, in this order:
1. cases and switches (the fingerprint commit);
2. `Noise::skip`;
3. tape twins;
4. tape heads;
5. crush quiet;
6. crush twins;
7. fuzz gains;
8. BENCH.md.

Files (6, under the cap of ten):
- `kazoo-fx/src/time/tape.rs`
- `kazoo-fx/src/drive/crush.rs`
- `kazoo-fx/src/drive/fuzz.rs`
- `kazoo-fx/src/dsp.rs`
- `kazoo-fx/src/drive/nulls.rs`
- `briefs/studio/BENCH.md`

## END

```
START KZ-D16b base=<sha> branch=lane/kz-d16b df=<GB> load=<1/5/15> at <time>
END KZ-D16b head=<sha> base=<sha> measure=<M sha> pushed=lane df=<GB> load=<1/5/15> at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
NULLS: the fingerprint tables (first commit and head); the aa step ratios and dB
BENCH: the BENCH.md section added, as written; the grimy save's sum of modules at M and H and whether H is at most 3,000 µs
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
