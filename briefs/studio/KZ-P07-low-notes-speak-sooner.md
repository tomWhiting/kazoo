# KZ-P07: low notes speak sooner: on guitar and bass, the tracker takes a provisional pitch from the first negative half-cycle, confirmed by a half-period NSDF, so a low B speaks at a period and a half instead of two

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 7. It adds one mechanism to V03's `CycleTracker`, switched on by P06's `CycleSettings::early` for the guitar and bass profiles.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: the arrival times for B0, E1 and E2 both ways, the 1.5-period history the half-period NSDF needs, the 42k multiply-adds, and that test 3's 30% square puts no early lag near its true period whichever lobe is negative. Accepted as written.

Time: about 30 to 40 minutes. Crates: kazoo-core.

Depends: KZ-P06 (settings, profiles and the plucked-string takes) must have landed before this lane opens.

Files (5): `kazoo-core/src/analysis/cycle.rs` (the hook in `push` and at a mark), `analysis/cycle/early.rs` (new), `analysis/cycle/tests.rs`, `voice/profile.rs` (`early: true` for guitar and bass), `analysis/score/tests.rs`.

## Why, and the physics

A pitch needs time, and the time is in periods. V03's tracker reports at its second cycle mark: one mark at the end of the first full cycle, one at the end of the second, and an NSDF over one period that needs two periods of signal. Its budget is 2 periods + 1 ms. That is 10 ms on an A3 and 25 ms on a guitar's low E (82.4 Hz), but 66 ms on a bass's low B (B0, 30.87 Hz, a 32.4 ms period), where a player hears the synth come in late.

The options weighed, with the one decided:
- **Harmonic-first** (lock on the strong second harmonic, which has half the period, then fold down when the fundamental is confirmed). This was rejected. The synth would play an octave high for the first 30 to 60 ms of every low note and then drop: an audible blip on every note.
- **Gate at the onset, pitch after.** This was rejected. A follower then sounds the previous note's pitch, or V09a's `follow_start` jumps to whatever Hz the frame holds, for the time it takes to find the new pitch. A wrong note at full level is worse than a late one.
- **A provisional period from half a cycle (decided).** The first complete negative lobe of the filtered signal, from a downward zero crossing to the next upward one, lasts half a period. It is complete at most one and a half periods after the note starts, and usually one. Twice its length is a candidate period, and an NSDF over half a period at that lag, which needs one and a half periods of history, confirms or refuses it before the second mark. The note then speaks at about 1.5 periods + 2 ms: 50.6 ms for B0 (from 65.8), 38.4 ms for E1 (from 49.5), and 20.2 ms for a guitar's low E (from 25.3). Being wrong costs nothing: a refused candidate leaves V03's path to speak at its usual time.

Hearing the instrument itself is untouched. The tracker reads a copy of the input, so the dry sound and every insert stay at interface latency (P05).

## Outcome

With `early` on (guitar and bass), the first voiced frame of a plucked low note arrives within 1.5 periods + 2 ms on the plucked-string takes. Octave errors among those early frames are 2% of notes or fewer. When V03's normal path takes over at the next mark, it moves the pitch by more than 30 cents in 5% of notes or fewer. With `early` off (voice and line), nothing changes, bit for bit.

## The design (decided; build exactly this)

### The early path (`analysis/cycle/early.rs`), active only while unlocked and `settings.early`

- **Lobes.** On the same filtered signal `f` V03 marks cycles on, keep the time of the last downward zero crossing (interpolated as V03 interpolates upward ones), and whether `f` went below `−θ` (V03's Schmitt arm) since then. At each upward zero crossing that ends an armed negative lobe, `h = t_up − t_down`.
- **Candidate.** `τ0 = 2h`, kept only if `min_period ≤ τ0 ≤ max_period`.
- **When to try.** Once the history since the note's first crossing reaches `1.5·τ0` (wait for it if needed, checking every sample against a stored deadline), evaluate the NSDF at lags `τ0 · {0.80, 0.90, 1.00, 1.11, 1.25}` over a window of `W = round(τ0/2)`. Why the five lags: a waveform with a strong second harmonic has unequal lobes, so `2h` can be off by 10 to 25%. Five lags bound the search, and parabolic refinement around the best (V03's step 3) finds the true period between them.
- **Accept** the best if `n* ≥ EARLY_CLARITY = 0.90`, and run V03's half-period check (`HALF_MARGIN`) on it. Why 0.90, not V03's 0.80: half a period of window is half the evidence, so it must be cleaner to count. A plucked string's first cycles are its cleanest, so 0.90 still accepts them (test 2 proves it).
- **Provisional.** An accepted estimate is set as `estimate()` with a flag `provisional: true` on `CycleEstimate` (a new field, false on every V03 path). The low-pass coefficient is set as for any accepted estimate. At the next mark, V03's own candidates are evaluated as usual. If they choose a period, it replaces the provisional one and the flag clears. If they choose nothing, the provisional estimate stands until `UNLOCK_PERIODS` without an accepted mark unlocks it, as for any estimate. At most one provisional estimate per note: the path rearms only after an unlock.
- **Cost bound.** Five candidates and one half check, each three lags over `W ≤ max_period/2` samples, once per note. At B0 at 48 kHz (`τ0` about 1555 samples, `W` about 778) that is 18 lag evaluations of 3 multiply-adds over 778 samples, about 42k multiply-adds, once per note: about 40 µs on this Mac, inside the one callback where it falls (1.33 ms at 64 frames), and nothing on any other.

### Into the frame (`voice/tracker.rs`, V04's fusion)

A provisional estimate is a voiced frame like any other, so the gate opens on it (V04's rule: voiced and above `gate_on_db`). V04's judge cannot have ruled yet (its frame is 85 ms for bass, P06), so no fold applies, and the confidence is the estimate's clarity. Nothing else in V04 changes.

### Profiles (`voice/profile.rs`)

`early: true` for guitar and bass, `false` for voice and line. Reasons, as doc comments:
- guitar and bass: plucked strings start with their cleanest cycles, and their low notes are where the time is;
- voice: a sung onset starts breathy and its first lobes are irregular, so V03 and V04 were tuned without this and keep their scoreboard;
- line: unknown sources, so the safe path.

## Not this lane

Any change to V03's normal path or to V04's fusion beyond reading `provisional`. Running the tracker on an input (P08).

## Red tests first

In `analysis/cycle/tests.rs` and `analysis/score/tests.rs`, on P06's `instrument_suite(48_000)`. Red for all: the path and the flag do not exist.

1. `a_low_b_speaks_in_a_period_and_a_half`: `bass_open` with bass settings: B0's first frame within 50 cents arrives no later than `1.5 × 32.4 + 2 = 50.6 ms` after the pluck; E1's no later than 38.4 ms; and the same notes with `early` off arrive no sooner than 2 periods (proof that the path is what moved them).
2. `the_early_estimate_is_rarely_wrong`: over every note of `bass_open` and `guitar_open`, at 3 pluck positions each (the Karplus-Strong excitation shaped to emphasise the 2nd harmonic at one of them): early frames an octave off in ≤ 2% of notes; and the later V03 estimate moves the pitch by more than 30 cents from the provisional one in ≤ 5% of notes.
3. `a_refused_candidate_costs_nothing`: a square wave at 31 Hz with a 30% duty (lobes unequal, NSDF at every early lag below 0.9 by construction of the test, checked first): the first pitch arrives exactly when V03's path would give it, and no provisional frame was emitted.
4. `voice_and_line_are_unchanged`: V03's and V04's whole test and scoreboard set with voice settings, bit for bit (compare frame sequences on `score::suite` against a run with `early` hard-off).
5. `a_note_that_starts_mid_glide`: a plucked note whose delay glides from its first sample (a perfect fourth over 2 s): the provisional pitch is within 30 cents of the truth at its time, and later frames follow the glide with no step larger than the truth's change plus 2 cents.
6. `a_vibrato_note_stays_smooth_through_the_handover`: a bass E1 with ±30 cents at 6 Hz from the pluck: no frame-to-frame change over 10 cents at the provisional-to-confirmed handover.
7. `the_early_path_never_allocates`: 10 s of `bass_open` inside `assert_no_alloc`.
8. Scoreboard `early_meets_its_targets`: `bass_open` and `guitar_open` onset latency max ≤ 1.5 periods + 2 ms per note; cents median ≤ 5 after 100 ms; octave ≤ 2%. Print the instrument table with and without `early`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-core/src/voice
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo run --release -p kazoo-core --example voice_score -j 4
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-core test passes.
- [ ] BENCH.md `## KZ-P07 low notes`: the instrument table with `early` off and on, the load average, date and sha.
- [ ] `EARLY_CLARITY` and the five lags have their reasons as doc comments.
- [ ] Not the builder's: after P08 lands, Cassio with Tom plays a low B on the 5-string into the desk with `kazoo mini --follow`, with this path on and then off (a test build), and asks Tom whether the synth now comes in with the note. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. The early path runs inside `push` on the audio callback: no allocation, lock, format or panic, and every index masked as in V03. All arithmetic on sanitised samples, and any non-finite result means no candidate. No `as` casts clippy pedantic flags. Files under 1000 lines.

## END

```
START KZ-P07 base=<sha> branch=lane/kz-P07 df=<GB> at <time>
END KZ-P07 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the instrument table, early off and on
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
