# KZ-B06: ParameterChange is real: every instrument applies it, the wall maps it to a knob, the desk can send one

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-core -p kazoo-mix` plus every instrument crate touched. Depends: KZ-A03.

## Outcome

Another process can turn a knob on any instrument. Today every instrument ignores `ParameterChange` and the hub disconnects one that sends it (survey: hub.md); after this lane the message does what its name says.

## The design (decided; build exactly this)

- `kazoo-core/src/ipc`: `ParameterChange { stream: u16, param: u16, value: f32, glide_ms: u16 }` (keep the existing wire layout where it exists; add `glide_ms` at the end with a protocol minor bump; old senders imply 0). The hub no longer disconnects on receipt: it routes a change addressed to an instrument down that instrument's link, and one addressed to the desk to the strip's own parameters (fader, pan, mute, send, trim, EQ gains by index).
- `kazoo-core/src/host/link.rs` (KZ-A03): `drain_events` yields `HubEvent::Parameter(ParameterChange)`.
- Each instrument gains `pub fn parameter_table() -> &'static [ParamSpec]` (`ParamSpec { index: u16, name: &'static str, min: f32, max: f32 }`) listing its existing knobs (read each crate's parameter enum or knob list: 808 per-voice level/tune/decay; 303 cutoff/res/env/decay/accent; cs80, mini, juno, prophet, dx, string, glass their synth params; arp rate/mode/octaves) and applies a change by index with clamping, on its audio thread, through the same path its own keys use. The wall maps `param` to `(slot, knob)` as `slot * 24 + knob` and applies it as a `wall_turn` glide over `glide_ms`.
- `kazoo-mix` can send: a `:set <strip> <param> <value>` console command (the desk has a command line; find it) for the proof.

## Not this lane

Recording or playing back parameter changes (KZ-C05, KZ-C10).

## Red tests first

- `ipc::tests::parameter_change_round_trips_with_glide` and `old_parameter_change_without_glide_decodes_as_zero`.
- `hub::tests::parameter_change_is_routed_not_disconnected` (a test instrument end on a real socket stays connected and receives the change).
- Per instrument: `parameter_table_covers_every_knob` (the table length equals the knob enum's variant count) and `change_out_of_range_is_clamped`.
- Wall: `parameter_index_maps_to_slot_and_knob`.

## Definition of done

- [ ] All red tests then green in every crate touched.
- [ ] `:set 1 0 0.2` on the desk moves the 808's first knob (by hand, say what moved).
