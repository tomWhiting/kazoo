# KZ-C02: every strip and group bus has an insert chain of kazoo-fx effects, pre-allocated, bypassable, with the whole desk kept in time by latency compensation

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. One mechanism: the engine runs a chain of `kazoo_fx::Effect`s on each strip and bus, and delays everything else to match. Crates: kazoo-mix, kazoo-fx (read only, unless an `Effect` contract gap is found; then the smallest fix, under FOUND). Depends: KZ-C01.

## Outcome

A musician will be able to put a tape delay on the 808's strip, a plate on the pad group, a compressor on the drums bus, and hear them, without the strips drifting out of time with each other when one effect delays its signal and another does not. This lane is the engine only: the effects can be put in, taken out, reordered, bypassed and turned from code and from tests. The knobs and the list on screen are KZ-C03.

## The design (decided; build exactly this)

**Where the chain sits.** Post-EQ, pre-fader, the console insert point. The strip's signal flow comment at `kazoo-mix/src/engine.rs:9` becomes `source ─ trim ─ 3-band EQ ─ inserts ─ comp delay ─ (clip detect) ─ fader·mute ─ pan ─┬─ mix bus / group bus`. A group bus (C01) runs its chain on its stereo sum, then its comp delay, then its fader and balance.

**Block rendering.** kazoo-fx effects are block-based (`kazoo_fx::Effect::process`, `kazoo-fx/src/lib.rs:144`, left and right slices), so the strip stops mixing frame by frame straight into the bus. `ChannelStrip::render_into` (`engine.rs:851`) keeps its block-popping loop but writes post-EQ audio (trim and EQ per frame, as `mix_frame` does at `engine.rs:1053`) into the strip's own `pre: StereoBlock` (two `Vec<f32>` of `MAX_RENDER_FRAMES`, `engine.rs:41`), zeros where there is a gap or an underrun. Then the chain runs once over the whole block, then the comp delay, then a second per-frame loop applies the fader, pan, aux send and meter exactly as `mix_frame` does now. Ramps and the EQ slew (`tick_eq`, `engine.rs:970`) advance once per frame in those loops; `advance_ramps` (`engine.rs:958`) for gaps goes, since gaps are now zeros flowing through the same loops. The tests `ramp_stays_in_step_across_a_gap` (`engine.rs:1714`) and `glide_carries_on_across_render_calls` (`:1739`) must still pass unchanged: the arithmetic is the same.

**The chain** (`kazoo-mix/src/inserts.rs`, new). `pub const MAX_INSERTS: usize = 8` (the depth MIXER.md's strip points imply; eight tape delays in series is already a sound design). `InsertChain { slots: [Option<InsertSlot>; MAX_INSERTS], scratch: StereoBlock, latency: usize }`; `InsertSlot { effect: Box<dyn kazoo_fx::Effect>, kind: &'static kazoo_fx::EffectKind, heard: Ramp, latency: usize }`. `process(&mut self, context: &kazoo_fx::Context, block: &mut StereoBlock, frames)`: for each occupied slot in order, the effect processes `block` into `scratch`, then per frame `block = scratch * heard + block * (1 - heard)` with `heard` the slot's bypass ramp (1 heard, 0 bypassed, gliding over the engine's 5 ms `ramp_frames`, `engine.rs:620`), so a bypass never clicks and a bypassed effect keeps running (its tail is there when it comes back; it costs CPU, which the desk shows). `latency` is the sum of `effect.latency()` (`lib.rs:192`) over slots whose `heard` target is 1; it is recomputed when a slot is put, taken, moved, bypassed, or given a stepped parameter (`Curve::Stepped`, `lib.rs:47`; the doc at `lib.rs:184` says latency changes only then).

**Latency compensation.** Each strip and each bus owns `comp: CompDelay`, two `kazoo_fx::dsp::DelayLine`s (`kazoo-fx/src/dsp.rs:85`, allocated once at `MAX_COMP_FRAMES = 8192`, 170 ms at 48 kHz) read at an integer delay (`tap`, `dsp.rs:162`). After any chain change the engine computes, for every strip, `total = chain.latency + (bus chain latency if routed)`; `max_total` over strips in use; each strip delays by `max_total - total`; each bus delays by nothing (its strips arrive aligned). The live input latency therefore grows by `max_total`; the engine exposes `compensation_frames()` and the callback publishes it (`SharedState::set_compensation_frames`) for the header to show as `fx +3.2 ms` when non-zero. A `Put` that would take any `total` past `MAX_COMP_FRAMES` is refused and the effect handed back (`InsertEvent::Refused { why: TooMuchLatency { frames } }`), never silently clipped. The delay change itself is a jump, not a glide: compensation changes only when the engineer edits a chain, and a 5 ms crossfade of two differently-delayed copies would comb; a one-time click on an edit is accepted and documented.

**Commands from the desk** (`inserts.rs`, `InsertBay`): the patchbay pattern (`kazoo-mix/src/patchbay.rs:79`) between the UI thread and the callback: two `ringbuf::HeapRb` SPSC rings of capacity 64, every command answered by exactly one event, the callback taking a command only when the event ring has room (`PatchbayCallback::service`, `patchbay.rs:174`).

```
enum Target { Strip(usize), Bus(usize), Master }   // Master is for KZ-C09; refused here with NoSuchTarget
enum InsertCommand {
    Put   { target, slot, effect: Box<dyn Effect>, kind: &'static EffectKind },
    Take  { target, slot },
    Move  { target, from, to },
    Bypass{ target, slot, bypassed: bool },
    Param { target, slot, index, value: f32 },
}
enum InsertEvent {
    Done  { target, slot },
    Taken { target, slot, effect: Box<dyn Effect> },        // dropped on the UI thread
    Refused { command_kind, target, slot, why: RefuseWhy, effect: Option<Box<dyn Effect>> },
}
enum RefuseWhy { NoSuchTarget, SlotTaken, SlotEmpty, TooMuchLatency { frames }, StripUnused }
```

`Put` into an occupied slot is refused with `SlotTaken` (the desk takes first, then puts); `Take` of an empty slot `SlotEmpty`. `Param` clamps through `kind.params[index].clamp` (`lib.rs:76`) on the UI side before sending and the effect clamps again (`set_param` contract, `lib.rs:138`). The effect is built and prepared on the UI thread: `(kind.build)()` then `effect.prepare(sample_rate as f32)` (`lib.rs:128`, the one method that may allocate), before it is sent. `AudioCallback` (`kazoo-mix/src/callback.rs:30`) gains `with_insert_bay(InsertBayCallback)` and services it in `process` right after the patchbay (`callback.rs:113`); `main.rs` creates the bay beside the patchbay (`main.rs:284`) and hands the desk end to `run_desk` (`main.rs:108`).

**Context.** `kazoo_fx::Context { bpm }` (`lib.rs:106`) is built once per chunk from the song clock's current anchor (`MixerEngine::song().bpm`, `engine.rs:312`); tempo-synced delays follow the desk.

**The desk-side model** (`inserts::model`, same file or `inserts/model.rs`): `InsertModel` is the UI thread's record of what it sent, since it is the only writer: per target, `Vec<Option<InsertEntry { id: &'static str, params: Vec<f32>, bypassed: bool }>>`. `InsertModel::put/take/move_/bypass/param` build the command and update the model only on `Done`; a `Refused` reverts and returns the reason in words for the footer. KZ-C03 draws it; KZ-C08 saves it.

**CPU.** The callback already publishes nothing about load. Add `SharedState::set_callback_load(f32)`: the callback measures its own `Instant` span over the buffer against the buffer's duration (an `Instant::now()` pair is not I/O and does not allocate) and the header shows `cpu 23%`. Reason: inserts are the first thing on this desk that can cost more than the device buffer, and a musician must see it before hearing it.

## Not this lane

- KZ-C03 draws the chain, the picker and the knobs; nothing in `ui.rs` or `desk.rs` changes here beyond the header's `fx` and `cpu` readouts.
- KZ-C09 puts a chain on the master; `Target::Master` exists in the enum and is refused here.
- KZ-C08 saves the model; KZ-C10 automates the parameters.
- kazoo-fx's effects and their latency declarations are KZ-D07 to D10's; do not tune an effect here. If an effect's `latency()` is wrong, write it under FOUND with the measurement and carry on.

## Red tests first

Engine tests in `kazoo-mix/src/engine.rs` (fake clock `RATE = 800`, `engine_with_source` at `engine.rs:1199`). A test-only `Delaying(n)` effect in `kazoo-mix/src/inserts.rs` tests implements `kazoo_fx::Effect`: a pure n-frame delay that declares `latency() == n`, so the fake and the contract it stands for are one thing.

- `an_insert_is_heard_on_its_strip`: put kazoo-fx `eq` (`kazoo_fx::find("eq")`, `lib.rs:208`) with `output` (param index 13 at `kazoo-fx/src/space/eq.rs:100`) at -12 dB; a 0.5 source settles at `0.5 * db_to_gain(-12.0)` within 2e-3.
- `a_bypassed_insert_fades_out_over_the_ramp_and_keeps_running`: bypass the -12 dB EQ; output rises monotonically over 4 frames to 0.5; un-bypass: falls back.
- `inserts_run_in_slot_order`: `Delaying(3)` in slot 0 and `Delaying(5)` in slot 1: an impulse comes out 8 frames late before compensation is applied on a single strip (compensation is zero when one strip is the maximum); with a second, clean strip in use, both impulses leave the master on the same frame.
- `a_group_chains_latency_adds_to_its_strips`: strip on bus 1 with `Delaying(4)`, bus 1 with `Delaying(6)`, clean strip 2: impulses coincide; `compensation_frames() == 10`.
- `too_much_latency_is_refused_and_the_effect_comes_back`: `Delaying(MAX_COMP_FRAMES + 1)`: `Refused { why: TooMuchLatency, effect: Some(_) }` and the chain unchanged.
- `moving_and_taking_slots_keeps_the_rest_in_order`: put A, B, C; move 2 to 0; take 1; the order is C, A.
- `a_stepped_parameter_change_recomputes_latency`: `Delaying` with a stepped `mode` param that doubles its delay; after `Param`, `compensation_frames()` on a two-strip engine doubles.
- `every_catalogue_effect_declares_the_latency_it_has`: for every kind in `kazoo_fx::catalogue()` (`lib.rs:198`) at 48 kHz, an impulse through one strip with the effect at defaults (mix at 100 % where there is a `mix` knob) first exceeds 1e-3 on frame `latency()` (effects whose defaults produce no impulse response, such as a gate-like model, are listed by id in the test with the setting that makes them audible). This is the fake-versus-real check: `Delaying` stands for the contract and every real effect is held to it.
- `nan_from_an_effect_never_reaches_the_bus`: a test effect that writes NaN; output finite.

Bay tests in `inserts.rs`: `commands_round_trip_through_the_callback` (put, done; take, the Box comes back) and `commands_beyond_capacity_wait_for_answers` (as `patchbay.rs:309`).

Callback test (`callback.rs` tests at `:165`): `the_header_readouts_see_compensation_and_load`: after a `Put` of `Delaying(48)` on strip 0 with strip 1 in use, `shared.compensation_frames() == 48`; `shared.callback_load()` is finite and non-negative.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix -p kazoo-fx
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix -p kazoo-fx --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src kazoo-fx/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix engine test green unchanged (the ramp and gap tests prove the block restructure is exact).
- [ ] `kazoo-mix/Cargo.toml` depends on `kazoo-fx = { path = "../kazoo-fx" }`; nothing else added.
- [ ] Put, take, move, bypass and param work on every strip and bus from the bay; every command is answered; every Box comes back to the UI thread.
- [ ] Compensation keeps a clean strip and a delayed strip on the same frame, through a group too; the header shows `fx +n ms` and `cpu n%`.
- [ ] `MAX_INSERTS`, `MAX_COMP_FRAMES` and the per-strip memory added (two `MAX_RENDER_FRAMES` scratch blocks, two `MAX_COMP_FRAMES` delay lines: about 100 KB a strip) are stated in the module doc with the total at `MAX_STRIPS`.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines, so `engine.rs` becomes `engine/mod.rs`, `engine/strip.rs`, `engine/bus.rs` as needed, with `inserts.rs` its own file). Real-time rules: `InsertChain::process`, `CompDelay`, the bay's `service` and everything under `render_block` never allocate, free, lock, format or do I/O; `prepare` is called on the UI thread only; a Box leaves the callback only inside an `InsertEvent`; every sample out of an effect passes `sanitize_sample` before the fader; a NaN parameter is ignored (`ParamSpec::clamp`, `lib.rs:76`). Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C02 base=<sha> branch=lane/kz-c02 df=<GB> at <time>
END KZ-C02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
