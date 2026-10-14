# KZ-V09b: the mouth shows the voice: a scope, a spectrum and the latency it adds

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-mouth` (and `-p kazoo-core` if the latency figures come from there).

Place: Track V, after KZ-V09a (the panel board). Written by Cassio, 14 Oct 2026, 20:0x, from Tom: "I'd like to see an oscilloscope of the voice and that kind of stuff as well... just more"; and "maybe a little bit of delay, more delay than need, but that could also be the AirPods". Decided by Cassio.

Time: about 40 minutes. Crates: kazoo-mouth (`engine.rs`, `ui/*`, `run.rs`). Depends: KZ-V09a (the header's latency placeholder and the voice strip).

## The design (decided; build exactly this)

1. **Scope.** The voice strip gains a scope beside the pitch trail: the mic signal over a window of four periods of the tracked pitch (20 ms when unvoiced), triggered on the rising zero crossing nearest the start of the window, so a held note stands still. The synth's output is drawn over it in a second colour, so the voice and the sound it makes are seen together. Drawn in braille cells (ratatui `Canvas` with `Marker::Braille`), at least 40 cells wide when the terminal allows. `o` toggles the scope between voice-and-synth, voice only and synth only.
2. **Spectrum.** Under the scope, a 32-band log-spaced spectrum (50 Hz to 10 kHz) of the mic, with the tracked fundamental's band marked. `k` hides or shows it.
3. **Capture without harm.** The engine copies the last 4096 mic samples and the last 4096 output samples into preallocated rings the UI reads (the existing display path or a new SPSC ring, preallocated, no lock, no allocation on the callback); the FFT runs on the UI side.
4. **Latency readout.** The header's latency field shows, in milliseconds, the sum of the mic's input latency, the input resampler's delay (V11's `delay_frames`), the engine's block (frames ÷ rate), the effect chain's reported latency (0 for slots that report none) and the output device's latency, each as cpal or the host reports it; and beside it `in X / dsp Y / out Z`. Where a figure is unknown it shows `?` for that part and the sum is marked `≥`. `L` opens a one-line breakdown. This settles "is it the AirPods" by showing the output part on its own.

## Red tests first

1. `a_held_note_stands_still`: a 220 Hz sine through the scope's trigger, frame after frame: the drawn trace's first crossing is at the same column every frame.
2. `the_scope_draws_both`: with voice-and-synth, the buffer holds both colours; with voice only, one.
3. `the_spectrum_marks_the_fundamental`: a 440 Hz sine lights the band containing 440 Hz highest, and that band is marked.
4. `capture_never_allocates`: the engine's render with capture on inside `assert_no_alloc`.
5. `latency_adds_its_parts`: a fake device report of 5 ms in, 2.7 ms resampler, 2.7 ms block, 0 fx, 10 ms out shows `20.4 ms` and the breakdown; an unknown out shows `≥ 10.4 ms`.

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
- [ ] Tom's AirPods figure read off the header after landing (NOT CONFIRMED until then).

## END

```
START KZ-V09b base=<sha> branch=lane/kz-V09b df=<GB> load=<1m> at <time>
END KZ-V09b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
