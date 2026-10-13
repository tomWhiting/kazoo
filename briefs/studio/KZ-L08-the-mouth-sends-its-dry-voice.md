# KZ-L08: kazoo-mouth puts Tom's own voice, dry, on the desk as a second strip, `mouth/dry`, sample-aligned with the synth it drives

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 20 minutes of a builder's work. One mechanism: a second named stream from the mouth. Crates: kazoo-mouth only (`src/engine.rs`, `src/run.rs`, and V08b's screen line). Depends: KZ-V08a (the crate), KZ-V08b (its screen), KZ-B03a (named streams on the link) and KZ-B03b (the desk seats them).

Read by Cassio, 13 Oct evening. Changes: the checks are written out in the lane form; the live check is Cassio's and Tom's; and, decided here, the desk seats a `dry` stream muted so the laptop speakers cannot feed back into the mic (one small kazoo-mix change, below; KZ-L07 taps before mute so loops still hear it). Opens only after V08a, V08b, B03a and B03b have landed.

## Why

V08a's mouth sends the desk its synth only: "when the desk link is connected, send the stereo block with `send_audio`" (`KZ-V08a-the-mouth.md`, engine step 5). V07a's voice stream is control frames (pitch, gate, loudness), not sound. A loop that records Tom singing needs his voice itself (Tom: "I'd record something as a loop and have it work in and out over time"). The desk is where every sound meets (`kazoo-mix-is-the-hub`), so the voice goes there as its own strip. Tom can then hear it, set its level, record it (C04), and send it back to a loop on the wall (KZ-L07, KZ-L09).

## The design (decided; build exactly this)

- The link: `LinkConfig::new("kazoo-mouth", 2, rate, block)` (V08a) gains `streams: vec![StreamConfig { name: "main".into(), channels: 2 }, StreamConfig { name: "dry".into(), channels: 1 }]` (B03a's `LinkConfig.streams`).
  - Stream 0 stays the synth and stays the clock (B03b).
  - Stream 1 is `dry`, mono.
  - The desk then labels them `mouth/main` and `mouth/dry` (B03b's `strip_label`; a two-stream instrument is labelled per stream).
  - Reason for mono: the mic is one channel (V08a opens it with 1 channel asked). Sending two equal channels would double the bytes for nothing, and B03b pans a mono stream like any mono strip.
- The callback (`MouthEngine::render`, V08a step 5): after `send_audio` for the synth, and only when the link is connected, call `hub.send_stream_audio(1, frames, &mic[..frames])`. `mic` is the very block the tracker and the synth used in this callback (V08a step 2: the newest mic block, sanitised on entry). So on the desk, the dry voice and the synth it drives play on the same studio frames, sample for sample.
  - A `false` (the stream not yet seated, or the desk refused it) is counted in `dry_unsent: u64` and shown on the screen. It is never discarded silently.
  - `--silent` (V08a) still sends `dry`: silent means the mouth makes no sound of its own, and the voice is Tom's, not the mouth's.
- The `DeskOut` test trait (V08a) gains `fn send_stream_audio(&mut self, stream: u16, frames: u32, samples: &[f32]) -> bool`, implemented for `HubLinkAudio` and the fake.
- The mic's own latency, from the time Tom's sound reaches the mic to when its block is popped, is not compensated here. That is the input buffer plus at most one output block, a few milliseconds at the sizes V08a uses. It is the same for the synth, so Tom hears the two together. A loop that records `mouth/dry` places it where the desk played it, and the loop's `nudge` knob (KZ-L02, KZ-L09) moves it by ear if Tom ever hears it late.
- The screen (V08b): the link line becomes "on the desk: mouth/main strip 3, mouth/dry strip 4". When `dry` is refused it reads "on the desk: mouth/main strip 3; dry refused: every strip is taken" (B03b's reason, from `HubLink::last_stream_refusal`). `dry_unsent` shows when it is above zero.

### The dry strip is seated muted (kazoo-mix, decided 13 Oct by Cassio)

Tom plays the wall on the MacBook's own speakers. A dry mic strip at unity on those speakers feeds back into the same mic: a howl, and a loud one. So:
- When the desk seats a stream named `dry` (B03b's seating, wherever it builds the strip's `ChannelControls`), the strip starts with `muted: true`. Every other stream is seated as today.
- The strip head already shows mute. The help gains one line: "a `dry` strip (a mic, as the mouth sends it) starts muted so the speakers cannot feed back into the mic; unmute it with headphones on".
- The desk's meter still moves for a muted strip (the engine renders it), and a return still carries it (KZ-L07 taps before mute).
- Red test 6, in `kazoo-mix/src/hub/tests.rs`: `a_dry_stream_is_seated_muted`. An instrument registers `main` and `dry`: `dry`'s strip is muted, `main`'s is not.

This adds kazoo-mix to the lane's crates (one condition where a stream's strip is seated, one help line, one test).

## Not this lane

The desk sending `mouth/dry` back to the wall (KZ-L07) and the loop recording it (KZ-L09). Any processing of the dry voice: it is the mic, sanitised, nothing more. A gate or EQ is the desk's strip's job.

## Red tests first

In kazoo-mouth's engine tests, with V08a's fake `DeskOut`. Each fails today because the mouth has one stream.

1. `the_dry_voice_goes_out_on_stream_1_with_the_same_block_the_synth_heard`. Feed a known mic block. The fake records a `send_stream_audio(1, n, samples)` whose samples equal the mic block bit for bit, in the same callback as `send_audio`.
2. `silent_still_sends_the_dry_voice`.
3. `a_refused_dry_stream_is_counted_not_dropped_silently`. The fake returns `false`, and `dry_unsent` counts every block.
4. `the_link_registers_main_and_dry`. Check the `LinkConfig` built by `run`'s start-up function, which must be a pure function so it can be tested: two streams, `main`/2 and `dry`/1, in that order.
5. The callback with `dry` sending still allocates nothing: V08a's `assert_no_alloc` render test, unchanged, passes.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mouth -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mouth -p kazoo-mix --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mouth/src kazoo-mix/src
```

## Definition of done

- [ ] The five tests fail first where they can (paste each under RED; test 5 is a regression guard and passes before and after), then pass.
- [ ] Not the builder's: after landing, Cassio starts the desk and `kazoo mouth`, checks the desk shows `mouth/main` and `mouth/dry` with `mouth/dry` muted, and asks Tom to sing and watch `mouth/dry`'s meter move while `mouth/main` plays the synth. The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. One `send_stream_audio` call on the callback, which B03a makes allocation-free.

## END

```
START KZ-L08 base=<sha> branch=lane/kz-L08 df=<GB> at <time>
END KZ-L08 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
