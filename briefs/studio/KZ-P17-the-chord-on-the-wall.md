# KZ-P17: the chord on the wall: a `chord` module gives the played chord's root, third, fifth and top as continuous pitch voltages that glide with a slide, and a `strum` module gives its gate, its change trigger and how sure it is

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 17, the last. It is V10's pattern for the chord. P16 (the arp) may run beside it.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Change: `take` starts each glide from the value being played, not from the old target, so a frame that arrives early or late never makes a step; `progress` holds at 1 when a frame is late; and the first frame after silence sets the pitches at once rather than gliding up from the last chord. Two tests are added for those.

Time: about 30 to 40 minutes. Crates: kazoo-wall, kazoo-mcp (one sentence).

Depends: KZ-P15 (the desk forwards chords; P14's wire) and KZ-V10 (the wall's `voice` modules, whose engine and control-side pattern this lane copies the shape of and shares `Tick` with) must have landed before this lane opens.

Files (9): `kazoo-wall/src/engine/chord.rs` (new), `engine/mod.rs` (the drain and the tick, calls only), `dsp/chord.rs` (new), `dsp/mod.rs` (`Tick` gains `chord`), `catalogue.rs` (two kinds), `daemon/wall.rs` (the follow wish), `protocol/mod.rs` (`ChordLook`), `kazoo-mcp/src/render.rs` (one sentence), `dsp/chord/tests.rs` (new).

Why: on the wall a chord is not an instrument. It is a set of control voltages any module can take: the root into a bass `vco`, all four into four `vco`s through a `seq` or `switch` to arpeggiate on the wall's own clock, the change trigger into an `env`. Unlike the arp's notes (P16), voltages carry the chord's exact pitch, so a slid chord glides on the wall.

## Outcome

- `wall_add chord` gives four outputs, `root`, `third`, `fifth` and `top` (1 V/oct, 0 = C4, continuous, detune included). `top` is the seventh in a four-note chord, the root an octave up in a triad, and the fifth an octave up in a power chord.
- `wall_add strum` gives `gate` (high while a chord sounds), `change` (a 5 ms trigger when the chord's name changes) and `sure` (the confidence, 0 to 1).
- While either is on the wall, the wall follows the desk's chord. `wall_look` says whether a chord is live and names it ("A minor is sounding").
- Between frames (10.7 ms apart) every pitch output glides linearly to the next frame's value, so a slide comes out as a smooth ramp, not a staircase.

## The design (decided; build exactly this)

### The engine (`engine/chord.rs`): V10's `VoiceNow` shape

- `ChordNow { last: ChordFrame, next: ChordFrame, progress: f32, change_left: u32, since: u64 }` is on `Engine`.
- `take(&f)` sets `last` to the pitches being output now (the interpolation at the current `progress`, not the old `next`), stores `f` as `next`, and resets `progress` to 0. Why: a frame arriving before the last glide finished would otherwise make the output jump back. If the previous frame was not sounding and `f` is (a strum after silence), `last` is set to `f` too, so the new chord starts at its own pitches instead of gliding from the last one. If `f.changed`, it sets `change_left` to `ceil(5 ms × rate / SUB_BLOCK)`.
- `tick()` gives `ChordTick { root, third, fifth, top: f32, gate, change, live: bool, sure: f32 }`. Each pitch is interpolated between `last` and `next` by `progress`, which advances by `SUB_BLOCK / CHORD_HOP` per sub-block and holds at 1 until the next frame (a late frame holds the last pitches; it never extrapolates). `CHORD_HOP` is P13's hop in frames at the wall's rate.
- `STALE_MS = 250` (V10's reason): no frame for 250 ms means gate low, `live` false, and the pitches held.
- `drain_hub` gains `drain_chord(&mut hub.chord, |f| chord_now.take(&f))`, as V10 added `drain_voice`.
- `dsp::Tick` gains `pub chord: ChordTick` (`Copy`, default zeros and false), set in `render_sub_block` beside `tick.voice`. Every `Tick { .. }` literal in tests gains it.

### Which tone is which output

From the frame's kind and tones (P13 orders tones from the root up by the kind's intervals):

| Kind | root | third | fifth | top |
|---|---|---|---|---|
| major, minor, sus2, sus4, dim, aug | tone 0 | tone 1 | tone 2 | tone 0 + 1 V |
| 7, maj7, m7 | tone 0 | tone 1 | tone 2 | tone 3 |
| power | tone 0 | tone 1 (the fifth) | tone 1 | tone 0 + 1 V |

For a sus chord, `third` carries the suspended tone: it is the second voice either way, and the name is in `look`. Each tone's `volts` already holds its detune (P13), so nothing here rounds.

### The modules (`dsp/chord.rs`)

| Kind | Knobs | Inputs | Outputs |
|---|---|---|---|
| `chord` | `octave` stepped −3 to 3, default 0 ("octaves added to every pitch"); `smooth` 0 to 100 ms, default 0 ("extra glide on top of the frame interpolation") | none | `root`, `third`, `fifth`, `top` (cv, pitched) |
| `strum` | none | none | `gate` (gate), `change` (gate), `sure` (cv) |

Outputs are clamped to ±`CV_LIMIT` (pitches) and 0..=1 (`sure`), and written every sample. Why two modules: `MAX_OUTPUTS` is 4 (V10's argument).

### Catalogue, following and look

- `Kind::CHORD` and `Kind::STRUM` are the next two free `Kind` numbers on main when the lane branches (say which under CHANGED), in a group named `chord`. They get rows in `design/wall/DESIGN.md`, and V10's catalogue tests gain them (`only_ports_that_carry_pitch_say_so` gains `chord.root`, `chord.third`, `chord.fifth`, `chord.top`).
- `daemon/wall.rs`: after each `send_tables`, `wants_chord` is whether any module is `CHORD` or `STRUM`. When it changes, call `follow_chord(wants_chord)` with V10's three outcomes and the log line "the desk cannot carry chords; chord and strum stay at rest".
- `protocol::Snapshot` gains `#[serde(default)] chord: ChordLook { wanted, live, desk_carries, name }` (`name` is set on the engine side through `Shared`, as V10's `live` is: root and kind as two `AtomicU8`s). `render::look` adds one sentence when `wanted`: "A minor is sounding.", "No chord is playing into the desk." or "The desk cannot carry chords."

### Nothing sticks to notes

The outputs are continuous voltages taken from the measured peaks and interpolated between frames. The only discrete things are the name and the `change` trigger.

## Not this lane

Arpeggiating inside the module (patch the wall's `clock`, `seq` or `switch` to do that). Quantising (the wall's `quant`). The arp (P16).

## Red tests first

Red for all: none of this exists.

- `dsp::chord::tests` (with `Bench` setting `tick.chord`):
  - `the_chord_module_plays_the_tick`: an A minor tick gives `root`, `third`, `fifth` and `top` at A2, C3, E3 and A3 in volts within 1e-4.
  - `octave_moves_every_pitch`.
  - `strum_change_is_a_trigger`.
  - `survives_nonsense`, `stays_in_range` and `every_kind_builds` pick up both kinds.
- `engine::chord::tests`:
  - `frames_are_interpolated_not_stepped`: two frames 10.7 ms apart, 50 cents apart: the `root` output over that time is a straight ramp within 0.5 cent of linear, with no step.
  - `a_slid_chord_glides_without_steps`: frames from P13's sliding E major (a perfect fourth over 2 s): every output's sample-to-sample change is at most the truth's slope plus 0.1 cent per sub-block, and no output ever steps.
  - `a_vibrato_chord_glides`: ±30 cents at 6 Hz on every tone: each output follows within 3 cents of the frames' values, interpolated.
  - `a_change_inside_a_batch_is_never_lost`.
  - `an_early_or_late_frame_never_steps`: frames arriving at 0.5 and at 1.7 hops after the one before, mid-glide: no output sample differs from the one before by more than the larger of the two glides' per-sample slopes.
  - `a_strum_after_silence_starts_at_its_own_pitches`: an A minor, 300 ms of not sounding, then a D major: the first sub-block after the D major frame is at D major's pitches within 1e-4 V.
  - `a_quiet_desk_lets_the_gate_fall` (250 ms, pitches held).
- `catalogue::tests`: the updated tests above.
- `daemon::wall::tests::the_wall_follows_the_chord_while_a_chord_module_is_up` (V10's recording stand-in for the follow call).
- kazoo-mcp `render::tests::look_names_the_chord`, one case for each sentence.
- `the_render_never_allocates_with_a_chord`: 10 s of frames inside `assert_no_alloc`.

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
- [ ] The lane touches the 9 files listed plus `design/wall/DESIGN.md` (two rows): say so under CHANGED.
- [ ] Not the builder's: the builder never touches the live wall. After landing and reinstalling, Cassio with Tom: the desk with a guitar input (`C` on), the wall with `chord` into four `vco`s, `strum.gate` into an `env`, and `wall_look`; Tom strums and slides a chord. The builder lists this under NOT CONFIRMED.

## Code quality

As PLAN.md's STACK RULES. On the render thread, `drain_chord`, `ChordNow` and both modules never allocate, lock, format or panic. Values arrive sanitised (P14) and are clamped again. `daemon/wall.rs`, `catalogue.rs`, `engine/mod.rs` and `protocol/mod.rs` are over 1000 lines: never split them. Add only the calls and entries named, and list them under FOUND with their line counts.

## END

```
START KZ-P17 base=<sha> branch=lane/kz-P17 df=<GB> at <time>
END KZ-P17 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
