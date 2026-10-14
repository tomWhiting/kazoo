# KZ-V15: the voice comes through: a vocoder blend on the mouth

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-mouth`.

Place: Track V, after KZ-V09a. Written by Cassio, 14 Oct 2026, 20:0x, from Tom: "a bit of a vocodery kind of thing with the actual... so if you were singing, that sort of came through as well." Decided by Cassio.

Time: about 35 minutes. Crates: kazoo-mouth (`synth.rs`, `engine.rs`, `controls*`, the SOURCE panel). Depends: KZ-V09a. Uses kazoo-core's existing `synthesis::Vocoder` (and `VocoderCarrierMode`); read it first.

## The design (decided; build exactly this)

1. A **voice** control on the SOURCE panel, 0 to 1, default 0 (today's sound, bit for bit). It sets how much of the mouth's output is the vocoder: the mic as modulator, the mouth synth's own oscillator (before its filter) as carrier, through kazoo-core's `Vocoder`, crossfaded equal-power with the plain synth path. At 1 the words and vowels are heard on the synth's pitch.
2. A **bands** control: 8, 16 or 24 vocoder bands, default 16; a **sibilance** control 0 to 1, default 0.3, passing the mic's high-passed (above 5 kHz) noise through so consonants stay clear.
3. The vocoder is built at engine start with every band count preallocated (or the largest, with fewer used); changing bands or voice never allocates, never clicks (gains ramp over 10 ms).
4. The effects chain follows the blend, as it follows the synth today.

## Red tests first

1. `voice_zero_is_todays_sound`: with voice 0, the output is bit for bit the output before this lane for a fixed input take.
2. `voice_one_carries_the_vowels`: an /a/ then /i/ synthetic vowel pair (two formant sets) as the mic at a steady pitch: at voice 1 the output's spectral envelope moves between the two formant sets (first two formants within 15% of the input's), at voice 0 it does not.
3. `changing_bands_never_allocates_or_clicks`: switching 8→24 mid-note inside `assert_no_alloc`, and no sample-to-sample step above the steady-state maximum step × 2.
4. `nonsense_mic_is_survived`: NaN and Inf in the mic at voice 1 give finite output.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `uptime` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mouth --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mouth --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mouth
```

## Definition of done

- [ ] Each red test fails first (paste it), then passes; every kazoo-mouth test passes; clippy, fmt and ast-grep exit 0.
- [ ] Tom sings through it after landing (NOT CONFIRMED until then).

## END

```
START KZ-V15 base=<sha> branch=lane/kz-V15 df=<GB> load=<1m> at <time>
END KZ-V15 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
