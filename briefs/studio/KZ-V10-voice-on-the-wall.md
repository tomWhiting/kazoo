# KZ-V10: the voice on the wall: a `voice` module (pitch, snap, gate, level) and a `breath` module (breath, onset, sure), fed from the desk

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-wall, kazoo-mcp (one sentence in `render.rs`). Time: about 30 to 40 minutes. Depends: KZ-V07b (the desk forwards voice frames to followers) and KZ-V05 (the shared scale tables). May run beside KZ-V08b and V09b to V09d.

Read by Cassio, 14 Oct 00:1x. Changes: the checks take the lane form; the live-wall check is Cassio's and Tom's after landing; and the two new `Kind` numbers are the next two free on main when the lane branches (22 and 23 if nothing has taken them since `fa3c8d8`; say which under CHANGED). If Track T's birth marks (KZ-T01) have landed, `voice` and `breath` are ordinary modules to the tree and need nothing more. At most two lanes build at once. Opens only after V05 and V07b have landed.

Why: Tom asked to "sing the melody into the synth ... not just on the wall, but i'd like voice to able to drive all the engines we have". On the wall the voice is not an instrument. It is a set of control voltages any module can take: pitch into a `vco`, gate into an `env`, level into a `vca`, breath into a filter. Today the wall drops everything the desk sends except the clock (`kazoo-wall/src/engine/mod.rs:1072-1076`), and no module takes anything from outside the patch except `speak`.

## Outcome

`wall_add voice` puts a module up with four outputs: `pitch` (1 V/oct, 0 = C4, the direct pitch), `snap` (the mouth's quantised pitch), `gate`, and `level` (loudness 0..1). `wall_add breath` gives `breath` (0..1), `onset` (a 5 ms trigger for each new note or re-sung syllable) and `sure` (confidence 0..1). While either module is on the wall, the wall asks the desk to follow the voice, and both modules play the mouth's frames as they arrive. `wall_look` says whether the voice is live. The wall's `quant` and the mouth now quantise from one scale table.

## The design (decided; build exactly this)

### Why two modules, not one with five outputs

`MAX_OUTPUTS` is 4 (`kazoo-wall/src/lib.rs:70`). The limit is built into the room's output memory (`engine/room.rs:50`, `:85`), the cable table (`engine/tables.rs:67`, `:120`), the perc adapter's lane destructure (`adapters/perc.rs:132`), a test that every effect has exactly four outputs (`catalogue.rs:1508`), and the console's panel layout. Raising it is a wall-wide change for one module. The split also follows meaning: `voice` carries what is played, and `breath` carries how it is played.

### The engine: one voice for the whole wall (`engine/voice.rs`, new; `engine/mod.rs` calls it)

- `pub struct VoiceNow { frame: VoiceFrame, since: u64, onset_left: u32 }`, a field on `Engine`, built at engine creation.
  - `take(&mut self, f: &VoiceFrame)` stores the frame, sets `since = 0`, and if `f.onset` sets `onset_left = ceil(ONSET_PULSE_MS · rate / 1000 / SUB_BLOCK)` sub-blocks.
  - `tick(&self) -> VoiceTick` reads the current values.
  - `advance(&mut self, frames: usize)` adds to `since` and counts `onset_left` down once per sub-block.
- `pub const ONSET_PULSE_MS: f32 = 5.0`: long enough for every gate input on the wall to see a rising edge, and short enough that two syllables 60 ms apart (V04's refractory time) give two triggers.
- `pub const STALE_MS: f32 = 250.0`: when no frame has arrived for 250 ms, `gate` is low and `live` is false, and the last pitch is held. This matches V09a's `LIVE_MS`.
- `drain_hub` (`engine/mod.rs:1061-1081`) gains `kazoo_core::host::link::drain_voice(&mut hub.voice, |f| voice_now.take(&f))`. To avoid borrowing `self` inside the closure, copy into a local `VoiceNow` and write it back. Frames from one drain are taken in order, so a batch never loses an onset.
- `dsp::Tick` (`dsp/mod.rs:54-63`) gains `pub voice: VoiceTick`, a `Copy` struct with fields `pitch`, `snap`, `level`, `breath`, `sure` (all `f32`) and `gate`, `onset`, `live` (all `bool`). Its `Default` is zeros and false. `render_sub_block` (`:1097`) sets `tick.voice = voice_now.tick()` beside `follow_desk()`, and then calls `voice_now.advance(SUB_BLOCK)`. Every `Tick { .. }` literal in tests gains `voice: VoiceTick::default()`, or uses the constructor if one exists.
- Values in the tick are clamped: `pitch` and `snap` to ±`CV_LIMIT` (`dsp/mod.rs:350`), and the rest to 0..=1. They are already sanitised by V07a's decode.

### The modules (`dsp/voice.rs`, new; builders in `dsp::native`)

| Kind | Knobs | Inputs | Outputs |
|---|---|---|---|
| `voice` | `octave` stepped −3..=3, default 0 ("octaves added to pitch and snap"); `smooth` 0..=100 ms linear, default 1 ("glide time of pitch, snap and level") | none | `pitch` (cv, pitched, "the voice's pitch, 1.0 per octave above C4, as sung"), `snap` (cv, pitched, "the mouth's quantised pitch"), `gate` (gate, "high while a note is sung"), `level` (cv, "loudness, 0 to 1") |
| `breath` | `smooth` 0..=100 ms, default 1 | none | `breath` (cv, "how breathy, 0 to 1"), `onset` (gate, "a 5 ms trigger at each new note or re-sung syllable"), `sure` (cv, "how sure the pitch is, 0 to 1") |

- Per sample, each continuous output moves toward its tick value with a one-pole of time constant `smooth`, where 0 means it jumps. Why a default of 1 ms: frames arrive every 1.33 ms, and 1 ms rounds off their staircase without a lag anyone can hear. `gate` and `onset` are written as 1.0 or 0.0 with no smoothing.
- `octave` adds to `pitch` and `snap` before the clamp.
- `reset` clears the smoothers.
- Both modules write every sample of every output, and never allocate.

### The catalogue (`catalogue.rs`)

- Add a new `voice_kinds()` with both entries, chained after the measuring kinds in `native_kinds()` (`:968`).
- Add `Kind::VOICE` and `Kind::BREATH` as the next two free `Kind` numbers on main (22 and 23 at `fa3c8d8`). Appending never renumbers an existing `Kind` constant.
- Update `catalogue.rs:1529-1550`: native kinds now end `SCOPE, READOUT, VOICE, BREATH`.
- `group()` (`:465-471`) puts both in a group named `voice`.
- `the_constants_name_the_wall_s_own_kinds` (`:1412-1440`) gains both names.
- `only_ports_that_carry_pitch_say_so` (`:1553-1564`) becomes `["seq.pitch", "quant.out", "voice.pitch", "voice.snap"]`.
- `design/wall/DESIGN.md`'s module table (about `:50-60`) gains both rows.

### One scale table (`catalogue.rs:850-880`, `dsp/quant.rs:27-48`)

- `SCALES`, `SCALE_NAMES` and `NOTE_NAMES` become `pub use kazoo_core::voice::quantise::{SCALES, SCALE_NAMES, NOTE_NAMES};`.
- `dsp/quant.rs`'s private `quantise` is replaced by `kazoo_core::voice::quantise::nearest` (V05 moved the identical rule there).
- Every existing `quant` test passes unchanged. That is the proof the rule did not move.

### Following, on the control side (`daemon/wall.rs`)

- After each `send_tables` (`daemon/wall.rs:1495`), compute `wants_voice`: is any module's kind `VOICE` or `BREATH`? When it differs from the last value, call `follow_voice(wants_voice)` on the wall's `HubLink` (the UI half kept beside the audio half that `split_link`, `daemon/control.rs:453-460`, gives the engine).
  - `Err(Unsupported)` writes one line to the wall's log: "the desk cannot carry the voice; voice and breath stay at rest".
  - `Err(NotConnected)` needs nothing, because V07a sends the wish when the desk appears.
  - `Err(Full)` retries at the next `send_tables`, by leaving the last value unchanged.
- `protocol::Snapshot` gains `#[serde(default)] pub voice: VoiceLook { wanted: bool, live: bool, desk_carries: bool }`.
  - `live` comes from an `AtomicBool` the engine sets in `Shared` (`engine/mod.rs:200-252`) from `tick.voice.live`.
  - `kazoo-mcp`'s `render::look` adds one sentence when `wanted`: "The voice is live." or "No voice is singing into the desk." or "The desk cannot carry the voice."
  - Every `Snapshot { .. }` literal in tests gains `voice: VoiceLook::default()`.

## Not this lane

- Quantising on the wall: `snap` is the mouth's, and the wall's own `quant` module is there for anything else.
- Several voices.
- A wall module that sends a voice.

## Red tests first

Red for all of them: none of this exists.

- `dsp::voice::tests` (with `Bench`, setting `tick.voice` directly; add `Bench::voice(VoiceTick)` if the bench has no way to set the tick):
  - `the_voice_module_plays_the_tick`: pitch 0.75 and gate high give `pitch` = 0.75 after 10 ms with `smooth` at 1 ms (within 1e-4), `gate` = 1.0 on every sample, and `snap` and `level` likewise.
  - `octave_moves_pitch_and_snap_only`.
  - `smooth_is_a_time_constant`: a step from 0 to 1 with `smooth` at 20 ms reaches 0.632 ± 0.02 at 20 ms. With `smooth` at 0 it reaches 1 on the first sample.
  - `breath_onset_is_a_trigger`.
  - `survives_nonsense` and `stays_in_range` (`dsp/mod.rs:668`, `:704`) for both kinds.
  - `every_kind_builds` (`:771`) picks both up automatically and must pass.
- `engine::voice::tests`:
  - `frames_become_the_tick`: three frames taken, then `tick()` gives the newest frame's values.
  - `an_onset_inside_a_batch_is_never_lost`: onset in frame 1 of 3, and `tick().onset` is true for exactly `ceil(5 ms / sub-block)` sub-blocks.
  - `a_quiet_desk_lets_the_gate_fall`: no frame for 250 ms gives `gate` false and `live` false, with pitch held.
- `catalogue::tests`: the three updated tests above, and `the_wall_and_the_mouth_share_one_scale_table` (pointer-equal or value-equal to `kazoo_core::voice::quantise::SCALES`).
- `daemon::wall::tests::the_wall_follows_the_voice_while_a_voice_module_is_up`, with a recording stand-in for the link's follow call: add `voice1` gives `follow(true)`; add `breath1` changes nothing; remove `voice1` changes nothing; remove `breath1` gives `follow(false)`.
- `kazoo-mcp` `render::tests::look_says_whether_the_voice_is_live`, one case for each of the three sentences.

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

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-wall and kazoo-mcp test passes, the `quant` tests unchanged.
- [ ] The lane touches 11 files: `catalogue.rs`, `dsp/mod.rs`, `dsp/voice.rs`, `dsp/quant.rs`, `engine/mod.rs`, `engine/voice.rs`, `daemon/wall.rs`, `protocol/mod.rs`, `kazoo-mcp/src/render.rs`, `design/wall/DESIGN.md`, plus the tests file. The two over the ten-file guide are one-line edits (`quant.rs`, `DESIGN.md`). Say so under CHANGED.
- [ ] Not the builder's: the builder never touches the live wall (no MCP call, load, save or daemon restart) and never opens Tom's mic or speakers. After landing and reinstalling, Cassio, with the desk and `kazoo mouth` running:
  - adds `voice`, patches `voice1.pitch` into a `vco`'s pitch and `voice1.gate` into an `env`, and asks Tom to sing;
  - adds a `readout`, patches `voice1.snap` into it, and checks that `wall_probe` names the snapped note;
  - checks that `wall_look` says "The voice is live."

  The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. On the render thread, `drain_voice`, `VoiceNow` and both modules never allocate, lock, format or panic. Values come in sanitised and are clamped again. `daemon/wall.rs` (4656 lines), `catalogue.rs` (1813), `engine/mod.rs` (1513) and `protocol/mod.rs` are already over 1000 lines: never split them. Add only the calls and entries named here, put the logic in `engine/voice.rs` and `dsp/voice.rs`, and list those files under FOUND with their line counts.

## END

```
START KZ-V10 base=<sha> branch=lane/kz-V10 df=<GB> at <time>
END KZ-V10 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
