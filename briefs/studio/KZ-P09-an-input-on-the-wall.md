# KZ-P09: an input on the wall: an `input` module plays a desk input's sound live into the wall's patch, late by a measured amount it states, for textures; its pitch and gate come from the `voice` module

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 9. Tom wants heavy lap-steel slide over whatever the wall plays. He hears his own playing on the desk at interface latency (P02, P05). The wall follows his pitch through V10's `voice` module once the input drives the voice (P08). This lane adds the third thing: his sound itself inside the wall's patch, through the wall's modules (granular, shimmer, folds moved by cables, loops), which the desk's insert chain cannot do.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Track L turned down a live return module because it would flam; this one is accepted because it keeps to what the flam cannot spoil (washed textures), states its lateness in `look`, and never carries the dry sound, which stays on the desk at interface latency. Checked: test 2's 0.05 step bound holds with a 5 ms crossfade on a 0.5 sine at 220 Hz (at most about 0.019 a sample). Accepted as written.

Time: about 30 to 40 minutes. Crates: kazoo-wall, kazoo-mcp (one sentence).

Depends: each of these must have landed before this lane opens:
- KZ-L09 (the engine's return lines: `ReturnLine`, `Returns`, `drain_returns`), and with it L06 (returns on the wire) and L03 (the `from` request);
- KZ-L07 (the desk sends returns of any strip);
- KZ-P02 (input strips, named `in/<name>`).

Files (7): `kazoo-wall/src/dsp/input.rs` (new), `dsp/mod.rs` (builder registration), `catalogue.rs` (one kind), `daemon/loops.rs` or wherever L03 handles `from` (accepts `input` modules), `protocol/mod.rs` (the module's view in `look`), `kazoo-mcp/src/render.rs` (one sentence), `dsp/input/tests.rs` (new).

## Why it is late, and why it is still worth having

L09 measured the path: the desk plays the wall up to `LINK_BACKLOG_MS` (250 ms) after it renders, and a return comes back stamped with the wall frame it was heard against. A live return is therefore always late, by at least the desk's lead for the wall's stream (`studio_clock.rs:85-96`: device buffer + block + 3 ms) plus the hub tick plus the link's wake. Track L turned a live return module down because it "would flam by the desk's lead", and for a rhythmic part it would. Decided here, with that known:
- An input's dry and insert-processed sound is never taken from the wall. It is heard on the desk at interface latency.
- The wall's copy is for what the desk cannot do: long, washed and moving textures (a shimmer, a granular cloud, a fold opened by an LFO, a loop), where tens of milliseconds sit inside the effect's own smear.
- The module states its lateness in `look`, so no one patches it into a tight rhythm by mistake.

## Outcome

`wall_add input`, then `wall_loop input1 from in/gtr` (L03's `from` request, now accepted for `input` modules), puts Tom's guitar on the wall:
- `out` is his sound, mono, at full level, and `level` is a 0 to 1 envelope.
- `look` says "input1 from in/gtr, heard 38 ms after it is played", or why not ("the desk has no strip called in/gtr yet").
- The delay is the smallest that keeps the line fed, measured and held steady. It grows (crossfaded) when the desk falls further behind, and shrinks only after five quiet seconds.
- A slide comes through as played: the module never touches pitch.

## The design (decided; build exactly this)

### The module (`dsp/input.rs`)

| Kind | Knobs | Inputs | Outputs |
|---|---|---|---|
| `input` | `gain` −24 to +24 dB, default 0 ("the input's level into the patch"); `smooth` 1 to 200 ms log, default 10 ("how fast `level` follows") | none | `out` (audio, "the desk input's sound, late by what look says"), `level` (cv, "its loudness, 0 to 1") |

- The module reads L09's return line for its `from` slot at engine frame `F − D`, where `D` is the module's current delay in frames.
- `out = gain × x`. `level` is a peak follower (instant attack, `smooth` release) mapped as V04's loudness: (dBFS + 60) / 60, clamped to 0..1.
- Both outputs are written every sample and sanitised.

### The delay (`dsp/input.rs`)

- Per block, record the lateness of the newest filled frame: `rendered − filled_to` from L09's line.
- Keep the maximum over the last 2 s in a ring of 2 s / 32-frame sub-blocks (pre-allocated in `new`).
- `target = max_lateness + MARGIN_FRAMES`, with `MARGIN_FRAMES = 2 × SUB_BLOCK` (64 frames): one sub-block for the read and one for the jitter of the next arrival.
- **Start:** silent until 1 s of returns has been seen, then `D = target`, with a 5 ms fade-in.
- **Grow:** when `target > D`, move `D` to `target` at once with a 5 ms crossfade between the old and new read positions (both in the line), and count a `jump`. A frame that is not filled yet reads as L09's `None`: silence, counted as `missing`.
- **Shrink:** when `target < D − 20 ms` for 5 s on end, move `D` to `target` with a 5 ms crossfade. Why only after 5 s: every jump is a 5 ms smear of two copies, and the lateness wobbles from callback to callback, so `D` holds steady unless the saving is worth it.
- `D` is capped at `RETURN_DELAY_SECONDS` (L09's 0.5 s). Past that, `look` says "too late to play live; record it with a loop instead".

### The source (`from`)

L03's `from` request (`wall_loop <id> from <strip>`) accepts a module of kind `input` as well as `loop`, and gives it a return line exactly as L09 gives a loop (one line per source, shared by every reader of the same strip). It is not culled (D03): it reaches an `out` only if patched, and it is a sink of returns either way.

### `look` (`protocol/mod.rs`, `kazoo-mcp/src/render.rs`)

The module's view carries `from`, `heard_ms` (`D` in ms, rounded) and `jumps`. `render::look` adds one sentence per input module: "input1 from in/gtr, heard 38 ms after it is played." The kazoo-mcp `wall_loop` tool's description gains "or sets where an input module listens".

### The catalogue (`catalogue.rs`)

Add `Kind::INPUT` as the next free `Kind` number on main when the lane branches (say which under CHANGED), with group `voice` (beside `voice` and `breath`, V10), and its row in `design/wall/DESIGN.md`'s module table. Appending never renumbers an existing kind.

## Not this lane

Pitch and gate: V10's `voice` and `breath` carry them once P08 makes the input the voice. Playing the dry input from the wall to the desk (never). Recording an input into a loop (L09 already does it with `from in/<name>`).

## Red tests first

`dsp/input/tests.rs`, with `Bench` and a `Returns` view filled by test code through L09's real `ReturnLine` (its write path, not a stand-in). Red for all: the kind does not exist.

1. `it_plays_its_line_late_by_d`: a return line filled 40 ms behind the render head: after 1 s, `out` equals the line's content delayed by `D` and `D` is within one sub-block of 40 ms + 64 frames.
2. `it_grows_with_a_crossfade_when_the_desk_falls_behind`: the lateness steps from 40 to 90 ms: `D` grows once, no output sample-to-sample step over 0.05 on a 0.5-amplitude 220 Hz sine, `jumps == 1`.
3. `it_shrinks_only_after_five_quiet_seconds`: the lateness falls from 90 to 40 ms: `D` holds for 5 s, then shrinks once.
4. `missing_frames_are_silence_and_counted`.
5. `a_glide_and_a_vibrato_come_through_without_steps`: the line carries the perfect fourth over 2 s and ±30 cents at 6 Hz, with one forced grow during the glide: the pitch of every 10 ms window of `out` is within 2 cents of the line's at `F − D`, and outside the jump's window no window steps by more than the truth's change plus 1 cent.
6. `look_says_how_late`: the rendered sentence for 38 ms, and for "no strip called".
7. `survives_nonsense` and `stays_in_range` (`dsp/mod.rs`'s shared tests) and `every_kind_builds` pick up `input`.
8. `the_render_never_allocates`: 10 s with a line, inside `assert_no_alloc`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall kazoo-mcp
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-wall and kazoo-mcp test passes.
- [ ] Not the builder's: the builder never touches the live wall. After landing and reinstalling, Cassio with Tom: the desk with a guitar input, the wall up, `wall_add input`, `wall_loop input1 from in/gtr`, `input1.out` into a `shimmer` into an `out`. Tom plays slide, and Cassio reads `look`'s figure aloud and asks whether the wash sits right. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. On the render thread the module never allocates, locks, formats or panics. The lateness ring is allocated in `new`. Every read goes through L09's `Returns::frame`. `catalogue.rs`, `engine/mod.rs`, `daemon/wall.rs` and `protocol/mod.rs` are over 1000 lines: add only the entries named, and list them under FOUND with their line counts.

## END

```
START KZ-P09 base=<sha> branch=lane/kz-P09 df=<GB> at <time>
END KZ-P09 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
