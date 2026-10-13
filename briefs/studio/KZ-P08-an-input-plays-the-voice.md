# KZ-P08: an input plays the voice: `t` on an input strip runs the voice tracker on it inside the desk, and the desk sends its frames to every follower and to the wall as the voice

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 8. It joins the input side (P02) to Track V's whole following machinery: V04's tracker and judge, V07b's forwarding and one-voice rule, V09's followers and V10's wall modules, all unchanged.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Change: test 7 no longer needs P05, which this lane does not wait for (P05 waits on C02, C03, P03 and P04, and following an input is the nearer wish); it proves the tap with the EQ, and adds the gate and insert case only if P05 has landed.

Time: about 30 to 40 minutes. Crates: kazoo-mix.

Depends: each of these must have landed before this lane opens:
- KZ-P02 (input strips);
- KZ-P06 (profiles);
- KZ-V07b (the desk forwards voice frames, one voice at a time), and with it V07a and V04.

KZ-P07 need not have landed: it changes only what the tracker gives.

Files (7): `kazoo-mix/src/inputs/track.rs` (new), `inputs/tests.rs`, the strip's file (the tap), `hub/voice.rs` (V07b's routing: a second source kind), `shared.rs` (the request bits), `desk.rs` (`t`), `ui.rs` (help line).

Why: Tom wants sweep picking "driving a synth, almost like an arpeggiator", and heavy slide over the wall, with the synth following "hertz for hertz" as the voice does. Track V already carries a voice from a program (the mouth) to every follower and to the wall. An input on the desk is in the same process as the hub, so it needs no link: the desk can be the singer.

## Outcome

On an input strip, `t` starts tracking: the head shows `voice` (V07b's badge) when the desk holds the voice for it. Every instrument following the voice (`i`, V09) plays it, and the wall's `voice` and `breath` modules (V10) carry its pitch, gate, level and onsets, with the input's profile ranges and onsets. Frames are unquantised: slides, bends and vibrato pass as they are played. A frame leaves the desk for a follower within 4 ms at p95 of being made on the callback. `t` again stops tracking. If the mouth is already singing, the input waits for V07b's one-second quiet, and the head and footer say so.

## The design (decided; build exactly this)

### Building and handing over (`inputs/track.rs`)

- `pub struct InputTracker { tracker: VoiceTracker, feed: JudgeFeed, results: JudgeResults, frames: HeapProd<VoiceFrame> }` and, kept off the callback, `Judge` and `HeapCons<VoiceFrame>`.
- Built on the hub thread (the thread that owns the patchbay), never on the callback:
  - `VoiceTracker::with_settings(rate, profile.tracking())` (P06);
  - V04's `judge::spawn` with the profile's judge frame scaled to the rate, and the input's block size;
  - a frame ring `HeapRb<VoiceFrame>` of `DESK_VOICE_RING = 256` (341 ms at 48 kHz, V07a's `VOICE_RING` and its reason).
- The UI thread cannot reach the patchbay (C01's note). `t` sets a bit in `SharedState::track_requested: AtomicU8` (one bit per input, `MAX_INPUTS = 8`), and the hub reads and clears it each poll (`poll_once`), as C01 does for `forget_requested`.
- The tracker goes to the callback as `PatchCommand::Track { slot, tracker: Box<InputTracker> }`, and comes back as `PatchEvent::Untracked { slot, tracker }`, dropped on the hub thread with its judge joined there. At most one input is tracked at a time: a second `t` on another input first untracks the first. Why one: V07b carries one voice, and two singers would be refused anyway.

### The tap (the strip's render path)

For the tracked input strip, once per callback, on the mono samples after trim and before the gate, EQ and inserts:
1. `feed.push(samples)`;
2. pop every `JudgeResult` into `tracker.judge`;
3. `tracker.process(samples, |frame| …)`, collecting into a fixed array of 64 frames as V08a does, then pushing each to `frames` (full counts `voice_dropped`).

Why after trim and before the gate and inserts: trim sets the level the tracker's gate reads, and P05's gate and the drives would cut tails and bury the fundamental under distortion. The scream through screamer and amp still tracks the clean pickup underneath. All three calls are V04's callback-safe calls.

### Sending (`hub/voice.rs`)

- V07b's `VoiceSource { id: [u8; 16], last: Instant }` takes a desk input as a source with the id `DESK_INPUT_ID = *b"kazoo-desk-input"` (16 bytes), which no instrument's UUID can equal in practice.
- Each hub poll, after the poll loop and before routing notes:
  1. drain the frame ring into batches of at most 32 (`encode_batch`, V07a);
  2. route each batch through the same function V07b uses for an instrument's `Inbound::Voice`, with `DESK_INPUT_ID` as the sender. The same rules apply: it takes the voice if no one holds it or the holder has been quiet for `VOICE_HOLD`; otherwise the frames are refused, counted and noticed once a second ("in/gtr is singing too; kazoo-mouth has the voice").
- `voice_roles` marks the input's strip `voice` when it holds the voice.
- Latency: the hub polls at most every `HUB_TICK` (2 ms, `hub.rs:75`), so a frame waits on average 1 ms in the ring. No syscall is added to the callback to wake the hub, because the callback stays free of everything but `Thread::unpark` (V04's judge already uses that). The budget is therefore 4 ms at p95, against V07b's 2 ms from a mouth whose frames also cross a socket.

### Nothing sticks to notes

The frames go out as the tracker makes them: `snapped_hz == hz` (P06). There is no quantiser on an input. A follower that plays `snapped_hz` (V09a) therefore plays every glide, bend and vibrato. For a snapped line, the wall's `quant` module quantises from `voice.pitch` when Tom wants it.

### Keys and help

`t` on an input strip toggles tracking. On any other strip it does nothing, and the footer says "t tracks an input strip". The help gains: "t (input): this input drives the voice: every instrument following the voice, and the wall's voice modules, play it".

## Not this lane

Chords (P13 to P17). Several tracked inputs at once. Quantising inputs. Any change to V07b's rules.

## Red tests first

`inputs/tests.rs`, with the hub test rig (`hub/tests.rs`'s `Rig`) where links are needed, and P01's `live_pair` for the input. Red for all: none of this exists.

1. `t_tracks_the_input_and_the_desk_sings`: a `guitar` input fed a 220 Hz plucked tone (P06's Karplus-Strong generator), `t`, and a test link following the voice: the link's `drain_voice` receives frames with `voiced` and Hz within 10 cents of 220, and `voice_roles` marks the input's slot `voice`.
2. `untracking_returns_the_tracker_off_the_callback`: `t` again: `PatchEvent::Untracked` arrives, the judge thread has exited within 200 ms, and no frames follow.
3. `a_singing_mouth_keeps_the_voice_until_quiet`: a test link sings first: the input's frames are refused and counted, and one notice names both. After the link is quiet for 1.1 s, the input's frames reach the follower.
4. `frames_are_unquantised`: every forwarded frame has `snapped_hz == hz`.
5. `a_sweep_reaches_the_follower_as_six_onsets`: P06's `sweep` take through a `guitar` input: the follower receives exactly 6 frames with `onset`.
6. `a_glide_and_a_vibrato_reach_the_follower_without_steps`: the perfect fourth over 2 s and ±30 cents at 6 Hz through a `guitar` input: every received voiced frame is within 5 cents of the truth at its `at`, and no frame-to-frame change exceeds the truth's change plus 2 cents.
7. `the_tap_reads_before_the_eq_gate_and_inserts`: with the strip's EQ cutting the tone's band by its full depth, the tracker still reports the tone at its level after trim. If P05 has landed when this lane opens, also with P05's gate on at a threshold above the tone and a C02 `Delaying` insert: the tracker still reports it.
8. `frames_cross_the_desk_quickly`: 500 frames through the ring to a following link: the time from the callback's push to the follower's `drain_voice`. Assert p95 ≤ 6 ms (looser than the 4 ms budget, as V07b's test is, because the gate runs under load). Report p50, p95 and the load average under BENCH.
9. `the_callback_never_allocates_while_tracking`: 2 s of tracking inside `assert_no_alloc`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-mix test passes, V07b's unchanged.
- [ ] Not the builder's: after landing and reinstalling, Cassio with Tom starts `kazoo mix --input gtr=<interface>:1:guitar`, `kazoo mini --follow` and a wall with `voice1.pitch` into a `vco`. Tom sweeps and slides, and Cassio checks that the mini retriggers per swept note, glides with the slide, and that `wall_look` says "The voice is live." The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. The tap calls only V04's callback-safe functions and pushes into a fixed ring. The tracker and judge are built and dropped on the hub thread. `hub.rs` (1226 lines) and `ui.rs` (1891) are already over 1000 lines: never split them. Add only the calls named, put the logic in `inputs/track.rs` and `hub/voice.rs`, and list both files under FOUND with their line counts.

## END

```
START KZ-P08 base=<sha> branch=lane/kz-P08 df=<GB> at <time>
END KZ-P08 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: frames_cross_the_desk_quickly p50 and p95, with load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
