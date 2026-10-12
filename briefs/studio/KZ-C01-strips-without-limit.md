# KZ-C01: the desk grows a strip for every stream that arrives, keeps it when the stream leaves, and has four group buses with their own fader, pan, mute and solo

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. One mechanism: the strip table stops being a fixed array of eight. Crates: kazoo-mix only. Depends: KZ-B03 (named streams on one link) landed on main.

## Outcome

Today the desk is eight strips, fixed, and the ninth instrument is refused with "every strip is taken" (`kazoo-mix/src/hub.rs:727`). After this lane a musician starts the desk and sees no strips, only the master section and the line "no instruments yet". Every stream that plugs in (an instrument, or one of the wall's outs after B04) gets the next strip, named as B03 names it (`808`, `wall/pad`), and the desk scrolls sideways when there are more strips than the terminal is wide, exactly as it banks today. When an instrument leaves, its strip stays on the desk, greyed as disconnected, with its trim, EQ, pan and fader as they were; when the same stream comes back it gets the same strip. `D` on a disconnected strip removes it. Four group buses, `G1` to `G4`, sit between the last strip and the master; `g` on a strip sends it to the next group (none, 1, 2, 3, 4, none); each group has its own fader, pan, mute and solo and feeds the master.

## The design (decided; build exactly this)

**Caps.** `pub const MAX_STRIPS: usize = 64` and `pub const BUSES: usize = 4` in `kazoo-mix/src/shared.rs`, replacing `DESK_CHANNELS` (`shared.rs:25`) in every place it meant the cap. Reason: the audio callback never allocates, and the lock-free cell table (`SlotCell`, `shared.rs:178`) and the callback's snapshot array (`callback.rs:35`) must be sized before the stream starts; 64 strips at 12 columns is 768 columns of desk, more than any terminal and more streams than any session here. Every strip's buffers exist from `MixerEngine::new` (`engine.rs:268`); what grows is the number in use. Cost: one `ChannelStrip` (`engine.rs:666`) holds a 65,536-float block buffer (`engine.rs:750`) plus its EQ and ramps, about 260 KB; 64 of them is about 17 MB, allocated once at start. State that number in the module doc.

**Strip state.** `ChannelStrip` gains `state: StripState` with `Unused`, `Live { key: StreamKey }`, `Disconnected { key: StreamKey }`. `StreamKey` is the string B03 names a strip with (`<instrument>/<stream>`; find the type B03 added to the hub's registration path and reuse it, never a second one). The engine renders strips in slot order up to `strips_in_use` (the highest slot not `Unused`, plus one), skipping `Unused` slots; `render_chunk` (`engine.rs:488`) loops over `&mut self.channels[..self.strips_in_use]`.

**Hub assignment** (`hub.rs:456` `Strip`, `:477` `strips: [Strip; DESK_CHANNELS]`, `:727` first free). `strips` becomes `Vec<StripSeat>` with capacity `MAX_STRIPS`, where `StripSeat { state: Free | Busy | Releasing | Parked(StreamKey) }`. On a stream's registration: a `Parked` seat with the same key is reclaimed first; else the lowest `Free` seat; else a new seat is pushed if `len < MAX_STRIPS`; else refuse with `RefuseReason::DeskFull` as today. On leaving (`hub.rs:933` `remove`), the seat goes `Releasing` as today and, when the callback's `Detached` answer arrives (`hub.rs:977`), to `Parked(key)` instead of `Free`. `HubConfig::reserved` (`hub.rs:100`) is removed: it existed for the demo strip, and the demo plugs in through the link like any instrument (`main.rs:84`). The `PatchCommand::Attach` (`patchbay.rs:33`) carries the key alongside the name so the engine sets `Live { key }`; `Detach` leaves the strip `Disconnected { key }` with its controls untouched (`ChannelStrip::detach`, `engine.rs:784`, must no longer clear the name).

**Removing a strip.** New `PatchCommand::Forget { slot }`: the engine returns the strip to `Unused`, resets its controls to `ChannelControls::DEFAULT` (`controls.rs:227`) and clears its name; the hub turns the seat `Free`. The desk asks for it with `D` on a disconnected strip: the UI thread cannot reach the patchbay (hub-owned), so the desk sets `SharedState::forget_requested(slot)` (an `AtomicU64` bitmask per 64 slots is exactly `MAX_STRIPS` bits) and the hub reads and clears it each poll (`poll_once`, `hub.rs:537`). `D` on a live strip does nothing and the footer says "unplug the instrument first".

**SharedState** (`shared.rs:249`). `slots: [SlotCell; MAX_STRIPS]`, plus `buses: [BusCell; BUSES]` where `BusCell` has `fader_db`, `pan`, `flags` (mute, solo) and a `MeterCell` (`shared.rs:77`); per slot a new `bus: AtomicU8` (0 none, 1..=4) and `state: AtomicU8` (unused, live, disconnected) published by the callback from the snapshot. `strips_in_use: AtomicUsize` published by the callback every buffer. `ChannelReadout` (`shared.rs:223`) gains `state` and `bus`.

**Buses in the engine.** `MixerEngine` gains `buses: [GroupBus; BUSES]`, each with `fader: Ramp`, `pan_left/pan_right: Ramp`, `in_left/in_right: Vec<f32>` of `MAX_RENDER_FRAMES` (`engine.rs:41`), controls `BusControls { fader_db, pan, muted, soloed }` sanitised like `MasterControls` (`controls.rs:338`), and a `MeterAccumulator`. In `render_chunk`: a strip whose `controls.bus` is `Some(b)` writes into `buses[b].in_*` instead of `bus.output` (extend `BusTargets`, `engine.rs:1093`, with the four bus pairs; `mix_frame`, `engine.rs:1053`, picks the target by the strip's bus); the aux send still goes to the aux bus. After the strips, each bus ramps its fader and balance (stereo balance law as `pan_gains` for stereo, `engine.rs:1106`) and sums into `output` before the master loop at `engine.rs:520`. Solo (`engine.rs:482`): `any_solo` covers strips and buses; a strip is audible when it is soloed, or routed to a soloed bus, or nothing is soloed; a bus is audible when it is soloed, or nothing is soloed; mutes apply on top. Every bus gain ramps over the same 5 ms (`RAMP_SECONDS`, `engine.rs:59`).

**Controls.** `ChannelControls` (`controls.rs:202`) gains `bus: Option<u8>` (sanitised to `1..=4` or `None`). New `BusControls` and `BusControl { Pan, Fader }` with `label()` `"PAN"`/`"FDR"`, `adjust`, `reset`, `normalized`, `set_fader_position`, mirroring `MasterControls` (`controls.rs:329`).

**Desk** (`desk.rs`). `Focus` (`desk.rs:24`) gains `Bus { bus: usize, control: BusControl }`. `move_strip` (`desk.rs:593`): right from the last strip in use goes to bus 1, right from bus 4 goes to master, left mirrors it; `jump_to_number` (`desk.rs:552`) keeps 1..9 for strips and drops the "N+1 = master" rule (master is reached with `l`). `strip_meters` and `strip_health` (`desk.rs:263`, `:267`) become `[_; MAX_STRIPS]`, plus `bus_meters: [StereoMeters; BUSES]`. `scroll_to_focus` (`desk.rs:333`) banks over `strips_in_use + BUSES` columns (buses are drawn as strips of `STRIP_WIDTH`, `ui.rs:32`, after the last strip). Keys: `g` cycles the focused strip's bus; `D` forgets a disconnected strip; `m`/`s` on a bus toggle its mute/solo; `R` on a bus resets it. The help (`ui.rs:1417`) gains the three lines.

**UI** (`ui.rs:135` `draw_desk`). `visible` is computed from `strips_in_use + BUSES`; the strip loop draws strips, then buses with a `STEEL` double border and title `G1`..`G4`, head "group", a pan knob, mute/solo, fader and meters (reuse `draw_strip_fader`, `ui.rs:561`, through a `StripView`-like `BusView`). A `Disconnected` strip keeps its name on a `LED_OFF` scribble strip with status `off` in `TEXT_DIM` (`strip_status`, `ui.rs:620`). A strip's head shows `G2` after the status when routed. With no strip in use, the strips area shows the hub's "no instruments yet" line centred (`HubNews::line`, `main.rs:194`, already builds it; pass it in `StatusInfo`). Hit targets (`desk.rs:41`) gain `Bus { bus, control }`, `BusMute { bus }`, `BusSolo { bus }`, `Forget { slot }`.

**Callback** (`callback.rs:109`). Configures `strips_in_use` strips and the four buses from shared state every buffer, publishes `strips_in_use`, each strip's state and bus, and each bus's meters.

## Not this lane

- KZ-B03 owns the registration message, the stream list and the `<instrument>/<stream>` naming; C01 reuses its key type and does not touch `kazoo-core/src/ipc/**`.
- KZ-C02 adds insert chains to strips and buses; leave room (the `GroupBus` and `ChannelStrip` structs are where they go) and add nothing.
- KZ-C04 adds arming and recording; `a` and `r` are reserved and unbound here.
- KZ-C08 saves strips to `session.json`; nothing here is persisted.
- No change to `kazoo-core`, no change to any instrument.

## Red tests first

Engine tests in `kazoo-mix/src/engine.rs` (the `tests` module at `engine.rs:1157`, fake clock at `RATE = 800`, `engine_with_source` at `:1199`):

- `a_new_engine_has_no_strips_in_use_and_renders_silence`: `strips_in_use() == 0`, output all zero, `next_frame` advances.
- `attaching_a_stream_to_a_fresh_slot_brings_the_strip_into_use`: attach to slot 5 of an empty engine; `strips_in_use() == 6`; snapshot 5 is `Live`, snapshots 0..5 are `Unused`.
- `a_detached_strip_keeps_its_name_and_controls`: configure trim +6 dB, detach; snapshot shows `Disconnected`, same name, `channel_controls(slot).trim_db == 6.0`; re-attach with the same key renders at +6 dB after the ramp.
- `forgetting_a_strip_resets_it_and_frees_the_slot`: forget; snapshot `Unused`, controls default, `strips_in_use` falls when it was the last.
- `a_strip_routed_to_a_group_is_heard_through_the_groups_fader`: route strip 0 to bus 1, bus fader -6 dB, push 0.5: output settles at `0.5 * db_to_gain(-6.0)` within 1e-3.
- `group_mute_silences_its_strips_and_group_solo_keeps_them`: two strips, one on bus 1; bus 1 muted silences only that one; bus 1 soloed silences the other.
- `group_balance_biases_a_stereo_group`: pan the bus hard left; right output under 1e-5.
- `sixty_four_strips_fit_and_the_sixty_fifth_is_refused`: `attach_consumer(MAX_STRIPS, ..)` returns `InvalidSlot`.

Hub tests in `kazoo-mix/src/hub/tests.rs` (the `Rig` at `:27`, `connect` at `:67`):

- `the_ninth_instrument_gets_the_ninth_strip`: nine clients; the ninth is `Joined { slot: 8 }`.
- `a_returning_stream_reclaims_its_parked_strip`: connect, drop, connect with the same name; same slot; the notice says so (`Joined` with the same slot).
- `a_forgotten_strip_is_free_for_the_next_stream`: set `forget_requested` on the parked slot; the next new stream takes it.

Desk and UI tests (`desk.rs` tests at `:734` with `key()` at `:740`; `ui.rs` tests at `:1556` with `render_with` at `:1581` and `text` at `:1622`):

- `g_cycles_the_focused_strips_group`: none, 1, 2, 3, 4, none.
- `arrows_walk_strips_then_groups_then_master`: with 3 strips in use, 3 rights reach `Bus { bus: 0 }`, 4 more reach `Master`.
- `an_empty_desk_says_no_instruments_yet_and_draws_four_groups_and_master`: render at 160x40; text contains "no instruments", "G1", "G4", "MASTER", no " 01 ".
- `a_disconnected_strip_is_drawn_dim_with_its_name`: publish a `Disconnected` snapshot named "mini"; text contains "mini" and "off".
- `the_desk_banks_over_strips_and_groups`: 20 strips in use at 100 columns; footer says "of 24".

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green or rewritten for the new count (name each one changed and why in END).
- [ ] A ninth, tenth and sixty-fourth stream each get a strip; the sixty-fifth is refused with "every strip is taken".
- [ ] A stream that leaves keeps its strip, named and dim; the same stream back gets the same strip; `D` removes it.
- [ ] Four groups with fader, balance, mute and solo, ramped, metered, drawn, banked and reachable by keyboard and mouse.
- [ ] `MixerEngine::new` is the only place strip or bus memory is allocated; a comment in the module doc states the per-strip cost and the total at `MAX_STRIPS`.
- [ ] `kazoo-mix --demo` still puts the 808 on strip 1 (through the link, with no reserved seats).

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines, split `engine.rs` into `engine/strip.rs` and `engine/bus.rs` if it passes). Real-time rules: `render_block`, `render_chunk`, `mix_frame`, `PatchbayCallback::service` and `AudioCallback::process` never allocate, free, lock, format or do I/O; a `Forget` returns nothing to free (controls are plain values); every ramp target and every mixed sample goes through `sanitize_sample`; NaN or Inf in a bus control reads as its default (`finite_clamp`, `controls.rs:407`). Red test first, then green, one behaviour per commit. No TODO, no stub, no deferred work.

## END

```
START KZ-C01 base=<sha> branch=lane/kz-c01 df=<GB> at <time>
END KZ-C01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
