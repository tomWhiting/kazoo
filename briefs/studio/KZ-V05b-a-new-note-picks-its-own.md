# KZ-V05b: a new note picks its own nearest note; hysteresis holds a note, never carries one into the next

Line numbers are from `main` at `6d64a9e` and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place: Track V, a follow-up to V05 (its review's open question). Written by Cassio, 14 Oct 2026, 08:0x. Decided by Cassio under Tom's rule that internal calls are not parked on him.

Time: about 10 minutes. Crates: kazoo-core.

Depends: KZ-V05 (landed). If KZ-P06 is in flight it edits other `voice/` files, not `quantise.rs`; no merge needed.

Files (2): `kazoo-core/src/voice/quantise.rs` (`Quantiser::process`, its doc comment), `kazoo-core/src/voice/quantise/tests.rs`.

## Why

`Quantiser::process` (`quantise.rs:201`) chooses the target with hysteresis on every voiced frame, onsets included: an existing `target` is kept when the centre is within `hysteresis_cents` past the midpoint. So a singer who holds A, stops, and starts a new note at A# −40 cents hears A, not A#. Hysteresis exists to stop a held note flicking between two notes as it wavers; a new note is a new choice and must start on its own nearest note. V05's review found this and left it because V05's brief step 3 said otherwise; this brief replaces that step.

## The design (decided; build exactly this)

- In `process`, the hysteresis arm of the target choice applies only when the frame is not an onset. On an onset the target is `nearest_note()` of the freshly reset centre. Nothing else in the order changes: the onset still resets the centre first (`update_centre`), and the correction still jumps straight to the goal on an onset.
- The doc comment on `process` says, in one sentence, that an onset chooses its own nearest note and hysteresis only holds a note already sounding.

## Not this lane

The snap speed, keep-vibrato, the scale tables, `set()`'s retarget on a scale change (V05 review), and the blend.

## Red tests first

1. `a_new_note_picks_its_own_nearest_note`: chromatic, root C, hysteresis 30 cents, blend 1. Hold A (several voiced frames at A), then one unvoiced frame, then an onset frame at A# −40 cents followed by voiced frames there. The snapped pitch on the onset frame and after is A#, not A. Red on `main` first: paste its failure, which shows A.
2. `hysteresis_still_holds_a_wavering_note`: the same settings, a held A with no onset that wavers to A# −40 cents stays on A. Green before and after (it guards that the fix is only for onsets); say so.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `sysctl -n vm.loadavg` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/voice
```

While iterating, filter nextest to `-E 'test(quantise)'`; run the whole of kazoo-core once at the end.

## Definition of done

- [ ] Test 1 fails first on `main` (paste it), then passes; test 2 passes before and after.
- [ ] Every kazoo-core test passes.

## Code quality

As PLAN.md's STACK RULES. `process` runs on the audio path: no allocation, no panic; `processing_never_allocates` still passes.

## END

```
START KZ-V05b base=<sha> branch=lane/kz-V05b df=<GB> load=<1m> at <time>
END KZ-V05b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
