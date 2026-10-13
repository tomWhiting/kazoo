# KZ-P04: a noise gate in kazoo-fx, with no lookahead and no chatter on a bass's lowest note, so heavy gain stays quiet between notes

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 4. Each input strip gets one of these in front of its inserts (P05), and it is an ordinary effect anywhere else: an insert on any strip, or a module on the wall.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: with the 10 ms detector release a B0 at −40 dBFS falls below the closing level between its peaks, and the 50 ms hold, longer than the 16 ms between |x| peaks, is what keeps it open (test 4 proves it). Accepted as written.

Time: about 30 minutes. Crates: kazoo-fx (kazoo-wall in the checks, because the wall offers every kazoo-fx kind as a module).

Depends: nothing in flight. If KZ-P03 is in flight, the two lanes both touch `drive/mod.rs`: merge line by line, and nothing else is shared.

Files (3): `kazoo-fx/src/drive/gate.rs` (new, with its tests), `kazoo-fx/src/drive/mod.rs` (`mod gate;` and `gate::KIND` in `KINDS`, `:35-44`), and any kazoo-wall test that counts the catalogue (find it by what it does).

Why: Tom's old trick was screaming into the guitar's pickups through heavy distortion. At that much gain, the hum and hiss of the pickup and preamp between notes come up as loud as the notes. kazoo-fx has no gate (`drive/mod.rs:35-44`; the fuzz's "gated" sound, `fuzz.rs:33`, is a starved bias, not a gate). The digital drives add no noise of their own, so a gate in front of them silences them between notes completely.

## Outcome

`kazoo_fx::find("gate")` builds a stereo-linked noise gate. It opens within its attack of the signal crossing the threshold and stays open through a held note, including a 31 Hz low B whose peaks come 32 ms apart. It closes after its hold and release once the signal stays below the threshold minus the hysteresis, down to its range, and it adds no latency.

## The design (decided; build exactly this)

### Parameters (`PARAMS`, in this order)

| Name | Range | Default | Curve | Reason for the default |
|---|---|---|---|---|
| `threshold` | −90 to 0 dB | −55 | linear | a single-coil's hum through a preamp sits around −60 to −70 dBFS at a sensible gain; a soft pick attack is above −40 |
| `hysteresis` | 0 to 20 dB | 6 | linear | it closes 6 dB under where it opened, so a note decaying through the threshold does not flutter |
| `attack` | 0.05 to 20 ms | 0.3 | log | fast enough not to blunt a pick, slow enough not to click: 0.3 ms is about 15 samples |
| `hold` | 0 to 500 ms | 50 | linear | longer than one period of the lowest bass note (B0, 32.4 ms), so the gaps between its peaks never start a release |
| `release` | 5 to 2000 ms | 150 | log | a guitar's tail fades rather than stops |
| `range` | −90 to 0 dB | −90 | linear | closed means silent; less is a downward expander for those who want the room |
| `key` | 10 to 500 Hz | 20 | log | a high-pass on the detector only: raise it to ignore mains hum (50 or 100 Hz) without thinning the sound |

### Per sample

1. Detector: `d = max(|hp(L)|, |hp(R)|)`, with `hp` two one-pole high-passes at `key` in series (`dsp::OnePole`, `kazoo-fx/src/dsp.rs:183`; 12 dB an octave). Why two (corrected by Cassio, 14 Oct 06:3x, after the builder found test 5 could not pass): one pole at 120 Hz takes only 8.3 dB off 50 Hz, so a −45 dBFS hum reads −53 and opens a −55 gate; two take 16.6 dB, and the hum reads −61.6. With `key` at 20 Hz the two take 1.3 dB, so the same hum opens it, as test 5 asks. and a peak follower `env = max(d, env · r)` with a 10 ms release (fast, because the hold does the bridging; a slow detector would make the hold meaningless).
2. State: open when `env ≥ threshold`. While open, every sample with `env ≥ threshold − hysteresis` restarts the hold counter. When the counter runs out, release starts, and it moves back to open the moment `env ≥ threshold` again.
3. Gain, in dB: towards 0 dB at `−range / attack` dB per second while open, and towards `range` at `range / release` dB per second while releasing. It is held at 0 dB during the hold. It is linear in dB, so the release sounds like a fade, and at most 90 dB of travel always takes exactly `release`.
4. Output: both channels multiplied by `10^(g/20)` (computed through `exp2` with a precomputed factor, never `powf` per sample).

Latency 0 (`latency()` stays the trait default), and no lookahead. Why: a lookahead costs its length on every input's round trip, and P05's rule is interface latency only. The 0.3 ms attack is the price, and on a plucked or screamed note it is inaudible.

Knob changes glide as the family's other knobs do (`kit.rs`'s `Knobs`). `reset` closes the gate and zeroes the detector.

## Not this lane

The gate's place on an input strip, its key and its per-profile hold (P05). A side-chain input from another strip.

## Red tests first

`drive/gate.rs` tests (use `drive/testkit.rs` where it helps). Red for all: the effect does not exist.

1. `it_opens_within_its_attack`: silence, then a 1 kHz sine at −20 dBFS: the gain reaches −1 dB within 0.5 ms of the first sample above threshold.
2. `it_holds_then_releases_to_its_range`: the sine stops: the gain is 0 dB for the hold (50 ms ± 1 sample), then reaches −90 dB at 150 ms ± 2 samples later.
3. `hysteresis_stops_flutter`: a 220 Hz tone decaying exponentially through the threshold (−3 dB a second): the gate goes open, release, closed once, and never reopens.
4. `the_lowest_bass_note_does_not_chatter`: a B0 (30.87 Hz) sine at −40 dBFS for 2 s: the gain never drops below −0.1 dB after the attack.
5. `the_key_ignores_hum`: 50 Hz at −45 dBFS alone, with `key` at 120 Hz and threshold −55: it stays closed. With `key` at 20 Hz, it opens.
6. `range_is_the_floor`: `range` −20: closed output is the input at −20 dB within 0.01 dB.
7. `a_glide_and_a_vibrato_pass_untouched`: the perfect fourth (220 to 293.66 Hz) over 2 s, then ±30 cents at 6 Hz for 2 s, at −20 dBFS. After the attack, the output equals the input exactly (null below −120 dB), so the gate never follows the pitch, never pumps and never steps.
8. `survives_nonsense` and `stays_bounded`: NaN, ±Inf and 1e30 in, and NaN parameters: outputs finite, state not poisoned, and a clean tone afterwards gated normally.
9. `processing_never_allocates`: 10 s through `process` inside `assert_no_alloc`.
10. `the_catalogue_offers_it`: `kazoo_fx::find("gate")` builds, has seven parameters in the order above, and the wall's every-kind tests pick it up. If KZ-C02's `every_catalogue_effect_declares_the_latency_it_has` has landed, add `gate` to its list of effects that need a setting to be heard (`range` 0 dB), as that test asks.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-fx -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-fx -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-fx/src kazoo-wall/src
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-fx and kazoo-wall test passes.
- [ ] Each parameter's reason is in its doc comment.
- [ ] Not the builder's: after P05 lands, Cassio with Tom puts the gate before screamer and amp on a guitar input at full gain and checks it is silent between notes and that no note's attack is lost. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES and kazoo-fx's real-time contract (`lib.rs:19-25`). No allocation, lock, I/O or panic in `process`, `set_param` or `reset`. Every NaN or infinite input is silence, and the state is never left poisoned. File under 1000 lines.

## END

```
START KZ-P04 base=<sha> branch=lane/kz-P04 df=<GB> at <time>
END KZ-P04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
