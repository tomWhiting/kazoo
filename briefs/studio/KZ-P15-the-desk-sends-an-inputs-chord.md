# KZ-P15: the desk sends an input's chord: `C` on an input strip runs the chord follower on it, and the desk forwards its frames to every instrument and wall that follows the chord

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 15. It is P08's shape for the chord. The desk is the source, the hub forwards to followers, and one chord source at a time follows V07b's one-voice rule.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Change: the key is `C`, because `c` is the desk's click (`desk.rs:391`); `C` is free on the desk and in every brief that touches it. Accepted otherwise as written.

Time: about 30 to 40 minutes. Crates: kazoo-mix.

Depends: each of these must have landed before this lane opens:
- KZ-P14 (the chord on the wire);
- KZ-P13 (the follower);
- KZ-P02 (input strips);
- KZ-V07b (the desk's feature message and voice routing, whose code this lane follows).

If KZ-P08 has landed, put the chord tap beside its voice tap; if not, the tap is this lane's own. Find P08's by what it does.

Files (9): `kazoo-mix/src/inputs/chord.rs` (new), `inputs/tests.rs`, the strip's file (the tap), `hub/chord.rs` (new: routing), `hub.rs` (features value, read arms and the drain call only), `hub/instrument.rs` (the two read arms), `shared.rs` (request bits, roles), `desk.rs` and `ui.rs` (`c`, the badges, the help).

Why: Tom wants to "strum a chord and the synth arpeggiates it", and to send chords to the wall.

## Outcome

- The desk announces `FEATURE_VOICE | FEATURE_CHORD`.
- `C` on an input strip starts the chord follower on it, with its profile's range. The head shows `chord` while the desk holds the chord for it.
- Every instrument that asked to follow the chord (`MSG_CHORD_FOLLOW`) gets every frame, and only those instruments get them. Their strips show `follows chord`.
- An instrument may itself send chord frames (any program can be a chord source, as with the voice). A second source at once is refused until the first has been quiet for a second, counted and noticed.
- A frame crosses the desk in under 4 ms at p95.

## The design (decided; build exactly this)

- **The follower on an input** (`inputs/chord.rs`): P13's `spawn_chord(rate, profile, block)` is built on the hub thread on a `C` request bit (`SharedState::chord_requested`, as P08's `track_requested`). It is handed to the callback as `PatchCommand::Chord { slot, feed }` and returned as `PatchEvent::Unchorded`, then dropped and joined on the hub thread. The callback pushes the strip's mono post-trim, pre-gate, pre-insert samples into the feed (P08's reasons: the clean signal is the one to analyse). The chord thread's results ring is read by the hub thread, not the callback, because the frames go nowhere on the callback. One input at a time.
- **Hub** (`hub/chord.rs`, following V07b's `hub/voice.rs`):
  - `MSG_DESK_FEATURES` now carries `FEATURE_VOICE | FEATURE_CHORD`.
  - Read arms: `MSG_CHORD` (checked as `decode_batch` checks; a malformed one is `ProtocolError::Malformed` and drops only its sender) gives `Inbound::Chord(ChordBatch)`, a fixed 516-byte copy. `MSG_CHORD_FOLLOW` gives `Inbound::FollowChord(bool)`.
  - `Instrument` gains `follows_chord: bool`.
  - `Hub` gains `chord: Option<ChordSource { id, last }>` with `CHORD_HOLD = VOICE_HOLD` (1 s, the same reason).
  - Each poll, the input's results ring is drained into batches of at most 8 and routed with `DESK_CHORD_ID = *b"kazoo-desk-chord"` as the sender, through the same function that routes an instrument's `Inbound::Chord`. The rule: it takes the chord if it is free, its own, or the holder has been quiet for `CHORD_HOLD`; it forwards unchanged to every follower but the sender; a failed send puts that follower on `leaving`; otherwise frames are refused, counted in `stats.chord_refused`, and noticed at most once a second ("<name> is playing a chord too; <holder> has the chord"). When the holder leaves, the chord is free.
- **Roles**: `chord_roles: [AtomicU8; MAX_STRIPS]` (0 none, 1 source, 2 follows), drawn in the head beside V07b's voice badge as `chord` or `follows chord`.
- **Keys**: `C` on an input strip toggles; on any other strip, the footer says "C follows the chord of an input strip". The help line: "C (input): this input's chord goes to every instrument following the chord (the arp, the wall)".

### Nothing sticks to notes

Frames pass through the hub byte for byte. The hub never reads a tone's pitch.

## Not this lane

The arp (P16) and the wall (P17). Several chord sources. Changing the voice's rules.

## Red tests first

`inputs/tests.rs` and `hub/tests/` with real sockets and kazoo-core `HubLink`s, as V07b's tests do. Red for all: none of this exists.

1. `the_desk_carries_chords`: a test link sees `desk_carries_chord()` within 200 ms.
2. `shift_c_follows_the_inputs_chord`: a `guitar` input fed an A major strum (P13's take), `C`, and a following link: `drain_chord` gives frames naming A major within 150 ms, and the strip's role is `chord`.
3. `chords_reach_followers_and_no_one_else`.
4. `a_second_chord_source_waits_until_the_first_is_quiet` (with exactly one notice).
5. `the_chord_is_free_when_its_source_leaves`.
6. `a_malformed_chord_message_drops_only_its_sender`.
7. `a_slid_and_a_vibrato_chord_arrive_as_played`: P13's sliding E major (a perfect fourth over 2 s), and its A minor with ±30 cents at 6 Hz, each through an input: every received frame's tone volts equal what the follower produced, bit for bit (a test tap on the results ring records them), so the glide survives the desk.
8. `chords_cross_the_desk_quickly`: p95 at most 6 ms from the results ring to `drain_chord` under load (budget 4 ms); report p50, p95 and the load average under BENCH.
9. `the_callback_never_allocates_while_following_a_chord`.

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
- [ ] `rg 'MSG_CHORD|FEATURE_CHORD' kazoo-mix/src` shows the feature value, both read arms and the forward.
- [ ] Not the builder's: after P16 or P17 lands, Cassio with Tom strums into an input with `C` on, and checks the follower's strip says `follows chord`. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. The tap only pushes into P13's feed. The follower is built and dropped on the hub thread, and the hub's paths use fixed-size buffers. `hub.rs` and `ui.rs` are over 1000 lines: add only the calls named, put the logic in `hub/chord.rs` and `inputs/chord.rs`, and list the long files under FOUND with their line counts.

## END

```
START KZ-P15 base=<sha> branch=lane/kz-P15 df=<GB> at <time>
END KZ-P15 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: chords_cross_the_desk_quickly p50 and p95, with load average
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
