# KZ-V07b: the desk carries the voice: it says so, forwards voice frames to the instruments that follow, keeps one voice at a time, and shows who sings and who follows

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-mix. Time: about 30 minutes. Depends: KZ-V07a (the wire and the instrument side) and KZ-B03b landed (B03b edits `hub.rs` and `hub/instrument.rs`).

Read by Cassio, 13 Oct evening. Changes: the checks take the lane form, the by-hand check after V08a is Cassio's and Tom's, and the once-a-second refusal notice is kept on the `Instrument`. Opens only after V07a has landed.

Why: Tom wants his voice "to drive all the engines we have". V07a gave instruments a way to send and receive voice frames. This lane is the desk in the middle: the one place that knows which program is singing and which programs are listening. Today the desk disconnects an instrument for any message type it does not know (`kazoo-mix/src/hub/instrument.rs:260`). Notes go to everyone or to one UUID (`hub.rs:916-935`), and nothing subscribes.

## Outcome

On registration, the desk tells every instrument it carries the voice. An instrument that asks to follow gets every voice frame from the program singing, and only those instruments get them. A second program singing at the same time is refused, and the refusal is counted and shown, until the first has been quiet for a second. Strip heads show `voice` on the singer's strip and `follows` on each follower's strip. A frame crosses the desk in under 2 ms at p95 on a quiet machine.

## The design (decided; build exactly this)

1. **Features after registration** (`hub.rs:824-860`). After `MSG_REGISTERED` is sent and flushed, send `MSG_DESK_FEATURES` with `FEATURE_VOICE`. If that send fails, treat it like a failed `REGISTERED` send: the same refusal path, with the reason "could not tell it what the desk carries".
2. **Reading** (`hub/instrument.rs:224-261`). Two new arms:
   - `MSG_VOICE`: `need(4)`, then check the count and length as V07a's `decode_batch` does. Malformed input is `ProtocolError::Malformed { msg_type, reason }` (new variant), and the instrument leaves with `LeaveReason::Protocol` as today. Otherwise the result is `Inbound::Voice(VoiceBatch)`, where `VoiceBatch { len: usize, bytes: [u8; 1284] }` is a copy of the payload. The hub thread is not an audio callback, but the copy is fixed-size anyway so the read path does not allocate.
   - `MSG_VOICE_FOLLOW`: `need(1)` gives `Inbound::Follow(bool)`. A byte other than 0 or 1 is `Malformed`.
3. **Who follows.** `Instrument` gains `follows_voice: bool`, set by `Inbound::Follow` and false on registration.
4. **One voice at a time.** `Hub` gains `voice: Option<VoiceSource { id: [u8; 16], last: Instant }>` and `pub const VOICE_HOLD: Duration = Duration::from_millis(1000)`. On `Inbound::Voice` from instrument X:
   - X takes the voice if `voice` is `None`, or is X, or its `last` is older than `VOICE_HOLD`. A second of silence means the first singer has stopped. A second is longer than any breath between phrases from a mouth that is still running, since the mouth sends frames continuously while it runs, gate low or high.
   - When X has the voice, set `last = now` and forward the payload unchanged as `MSG_VOICE` to every instrument with `follows_voice` whose id is not X's. A send that fails puts that follower on `leaving`, as `route_note` does.
   - When X does not have the voice, drop the frames and add the batch's frame count to `stats.voice_refused`, a new `AtomicU64` in the hub's stats beside `dropped_blocks`. The first refusal per X per second (an `Instrument` field `voice_refused_at: Option<Instant>`, so no table is needed) is also a `HubNotice::VoiceRefused { name, holder }` (new), which reads "<name> is singing too; <holder> has the voice".
   - When the holder leaves (`remove`), `voice = None`.
   - Route voice batches after the poll loop in the same pass as notes (`hub.rs:908-910`), in the order they arrived.
5. **Roles on the desk.** `SharedState` (`kazoo-mix/src/shared.rs`) gains `voice_roles: [AtomicU8; STRIPS]` (find the strip count constant the desk uses), where 0 is none, 1 is the voice and 2 follows. The hub thread writes them whenever a role changes: the source changes, an instrument follows or stops following, or an instrument leaves. `draw_strip_head` (`ui.rs:446-484`) draws `voice` (bright) or `follows` (dim) in the head's free space, right-aligned, in the head's existing palette. The desk's help text (`draw_help`, `ui.rs:1489`) gains one line: "voice: the strip singing into the desk; follows: an instrument playing that voice (press i on the instrument)".
6. **The health line.** If the desk has a per-instrument health report (`MSG_DESK_HEALTH`, 0x23), add nothing to its wire. The refusal reaches the user through the notice in step 4 and the role badges in step 5.

## Not this lane

- The instrument side (V07a).
- Choosing among several voices by name, or a desk command to hand the voice over. One voice and "first singer keeps it until quiet" is the whole rule.
- Recording the voice (the C05a follow-up).

## Red tests first

These go in `kazoo-mix/src/hub/tests.rs`, or in its `tests/` folder beside it, with real sockets as the existing hub tests use. The test instruments are kazoo-core `HubLink`s (V07a), so the test exercises both halves. Red for all of them: the hub knows none of these messages.

- `the_desk_says_it_carries_the_voice`: a test link registers. `desk_carries_voice()` is true within 200 ms.
- `voice_reaches_followers_and_no_one_else`: A sings, B follows, C does not follow. 100 frames from A reach B in order through `drain_voice`. C gets none, and A gets none of its own.
- `following_is_not_a_protocol_error`: B sends `follow_voice(true)` then `follow_voice(false)` and stays connected. After that, frames from A no longer reach B.
- `a_second_singer_is_refused_until_the_first_is_quiet`: A sings, then D sings at the same time. Only A's frames reach B, and `voice_refused` counts D's frames. Exactly one `VoiceRefused` notice is raised. A stops for 1.1 s, D sings, and D's frames reach B.
- `the_voice_is_free_when_the_singer_leaves`: A sings and then disconnects. D's frames reach B at once, without waiting for `VOICE_HOLD`.
- `a_malformed_voice_message_drops_only_its_sender`: a raw socket sends `MSG_VOICE` with count 40. That connection leaves with a protocol reason, and A and B stay connected.
- `the_strip_heads_say_voice_and_follows`: with A singing and B following, `voice_roles` is 1 for A's strip and 2 for B's. A `TestBackend` draw of the desk shows `voice` in A's head and `follows` in B's.
- `voice_crosses_the_desk_quickly`: 500 frames from A to B, one every 1.33 ms. Measure the time from `send_voice` to `drain_voice` with the links at production priority. Assert p95 ≤ 5 ms, and report p50, p95 and the load average under BENCH. The budget is 2 ms at p95 on a quiet machine; the assert is looser because the gate runs under load, as B01's wake test learned.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] All red tests are red first (paste the output), then green. Every existing kazoo-mix test is green.
- [ ] `rg 'MSG_VOICE|MSG_VOICE_FOLLOW|MSG_DESK_FEATURES' kazoo-mix/src` shows the send after REGISTERED, both read arms, and the forward.
- [ ] Not the builder's: after V08a lands, Cassio starts the desk, then `kazoo mouth`, then `kazoo mini --follow`, checks that the mini's strip head says `follows` and the mouth's says `voice`, and asks Tom to sing. The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The desk's audio callback is untouched. The hub thread's new paths use fixed-size buffers. `hub.rs` is 1225 lines and `ui.rs` is 1891, already over 1000: never split them. Put the routing in a new `hub/voice.rs`, keep the additions to `hub.rs` and `ui.rs` to the calls named, and list both files under FOUND with their line counts.

## END

```
START KZ-V07b base=<sha> branch=lane/kz-V07b df=<GB> at <time>
END KZ-V07b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: voice_crosses_the_desk_quickly p50 and p95, with load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
