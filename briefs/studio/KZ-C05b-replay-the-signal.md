# KZ-C05b: a strip's recorded notes and parameter changes are sent back to the instrument on it, each on the same frame of the instrument's own stream it was played on

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

This is the second half of KZ-C05 (record the signal); KZ-C05a writes the file, this half replays it. Between the two files every outcome of the plan's C05 row is delivered.

Time: about 30 minutes of a builder's work. One mechanism: scheduled note and parameter delivery, hub to link, by stream frame. Crates: kazoo-core (wire extension, the link's schedule), kazoo-mix (the hub's replayer, the strip switch). Depends: KZ-C05a.

## Outcome

A musician records the DX being played from the keyboard, then presses `P` on the DX's strip and space: the DX plays the take again, itself, from its own notes, landing each one on the frame it was first played on, so the synth's sound (its envelopes, its LFO, whatever knob it is on now) is live and editable while the performance is kept. The same for a knob ride sent as parameter changes. If the instrument is not plugged in, the strip says `no inst` and nothing is pretended.

## The design (decided; build exactly this)

**Wire.** `NoteEventMsg` (`kazoo-core/src/ipc/types.rs:482`) gains `at_frame: u64` after `velocity`: `WIRE_SIZE` stays 36 for the unscheduled form and `SCHEDULED_WIRE_SIZE = 44` is what `encode` writes when `at_frame != SYNC_NOW` (`types.rs:390`); `decode` reads 36 bytes as `at_frame = SYNC_NOW` and 44 bytes with it, as `RegisterMsg::decode` already does for `pace_lead_frames` (`types.rs:238`). `ParameterChangeMsg` (`types.rs:548`) the same: 6 bytes or 14. `PROTOCOL_VERSION` (`types.rs:48`) is unchanged: an old instrument reads the first 36 bytes of a 44-byte note (its `decode` indexes a fixed prefix) and plays it at once, which is what it does today, and the hub reads an old instrument's 36 bytes as now. The hub's `Instrument::handle` (`kazoo-mix/src/hub/instrument.rs:239`) accepts either length and `NoteMessage::payload` (`instrument.rs:63`) becomes `[u8; 44]` with `len`, forwarded as sent.

**Link schedule** (`kazoo-core/src/ipc/link.rs`). `HubLinkAudio` (`link.rs:382`) gains `scheduled: NoteSchedule`, a fixed array of 64 `Option<Scheduled { at: u64, message: HubMessage-sized enum of Note | Param }>` ordered by insertion like `TransportFollower` (`kazoo-core/src/ipc/follow.rs:82`, capacity and `due` shape copied, not shared: a note is not a whole transport state and several may be due on one frame, all delivered in order). The link thread, where it pushes received messages into the callback ring (`link.rs:995`, `self.messages.try_push(message)`), pushes scheduled ones as it does now; it is the callback half that sorts them: `try_recv` (`link.rs:692`) takes a message from the ring and, if it carries `at_frame != SYNC_NOW`, moves it into `scheduled` instead of returning it, then returns the first scheduled message whose `at <= stream_frame()` (`link.rs:606`), oldest first. So an instrument that only ever calls `try_recv` gets a replayed note on the first callback at or after its frame, no more than one block late. New `due_events(&mut self, block_frames: u32) -> DueEvents<'_>`: an iterator of `(offset_in_block: u32, event)` for every scheduled event with `at` in `[stream_frame, stream_frame + block_frames)`, and `offset 0` for any already late; an instrument that calls it inside its render loop places each note on its sample. A full schedule (65th) is counted in `LinkStatus` (`link.rs:352`) as `events_dropped` and the event is delivered at once rather than lost: late beats lost. Both paths allocate nothing.

**One instrument adopts the offset.** The 808 (`kazoo-808/src/audio.rs:310`, `drain_hub`) switches from `try_recv` notes to `due_events(block_frames)` and triggers each drum at its offset within the block (its `DrumMachine::trigger` is called today at block start; find whether it takes a sample offset, and if not, split the block render at each due offset, which its `render_chunk` can do since it already renders in sub-ranges for transport changes). Reason: one real instrument proves the API end to end; the rest adopt it in their own rows (named under FOUND with file:line each).

**The replayer** (`kazoo-mix/src/hub/replay.rs`, new; a `Replayer` owned by the hub `Server`, `hub.rs:468`). The desk loads a take's events on the UI thread (`EventFile::read`, C05a) into `Arc<[EventRecord]>` with its sidecar's `song_beat`, `studio_frame` and `bpm`, and sends `HubCommand::Replay { slot, take: Arc<ReplayTake>, on: bool }` on a new `std::sync::mpsc::sync_channel(64)` into the hub (the hub gains `Hub::command(&self, HubCommand) -> Result<(), HubBusy>`; a full channel is a footer note, never a wait). Each event's place in the song is `beat = song_beat + beats_in(event.studio_frame - take.studio_frame, bpm, rate)` (`follow.rs:31`), computed once at load into `ReplayTake.events: Vec<(f64 beat, EventRecord)>`. Only `direction == 1` records (sent to the instrument) are replayed; `direction == 0` records are the instrument's own output and are kept for the record only (sending the arp its own notes would double them; the 303's copy of them carries `direction == 1`).

Each poll, in `follow_song` (`hub.rs:859`) after the anchors are known: for each strip with a replay `on` and an instrument on it, the replayer converts the current song anchor (`self.song.front()`, the one in force) to a frame for each pending event, `frame = anchor.frame + (beat - anchor.beat) * 60 * rate / bpm` (exact while the anchor is playing; nothing is sent while stopped), and sends every event whose frame is within `instrument.stamp().schedule_need(clock)` (`kazoo-mix/src/studio_clock.rs:104`) of `clock.playing_now()`, as a `NOTE_EVENT` or `PARAMETER_CHANGE` with `at_frame = stamp.stream_at(frame)` (`studio_clock.rs:164`), advancing the strip's cursor. A new anchor (play, locate, loop wrap, tempo change) resets the cursor to the first event at or after `anchor.beat`; events before it are skipped, never sent late. A stream placed afresh (`needs_song`, `hub.rs:800`) re-sends nothing: the frames already sent were correct for the old placement and the link treats a late one as due at once; events not yet sent go out with the new placement. An event whose instrument is absent is counted per strip in `HubStats` as `replay_unsent` and the strip head shows `no inst` in `AMBER`.

**Hub stats and the desk.** `HubSnapshot` (`hub.rs:283`) gains `replayed` and `replay_unsent`; `SharedState` gains per-slot `signal: AtomicBool` (the switch) published for the head badge `sig` in `SAGE` (`draw_strip_head`, `ui.rs:446`). Key `P` on a strip: toggles the switch; the desk loads the strip's latest finished take with an `events` sidecar field (`SessionDir`, C04) and sends `Replay { on }`; a strip with no such take gets the footer note "no signal take on this strip". The help gains the line. The switch is kept in the model so KZ-C08 saves it.

## Not this lane

- KZ-C06 chooses which take plays where on the timeline; when it lands, its take player hands the hub each placed take's `ReplayTake` through the same `HubCommand::Replay` and this lane's "latest finished take" rule gives way to the timeline. State that handoff in `replay.rs`'s module doc.
- KZ-B06 defines what an instrument does with a parameter change; this lane delivers it on a frame and nothing more.
- The other eleven instruments keep `try_recv` and are named under FOUND for their own rows; no edit to them here.
- KZ-C10 records desk control moves; not notes.

## Red tests first

Wire tests (`types.rs` tests at `:585`): `a_scheduled_note_round_trips_and_an_unscheduled_one_is_36_bytes` (both sizes, both decodes), `a_36_byte_note_decodes_with_sync_now`, `a_scheduled_parameter_change_round_trips`.

Link tests (`link.rs` tests from `:1127`, which drive the link against a fake hub socket; copy that rig):

- `a_scheduled_note_comes_out_of_try_recv_on_the_first_call_at_or_after_its_frame`: send a note at stream frame 1000 while the stream is at 0; `try_recv` returns `None` at 0 and 999; at 1000 the note; never again.
- `due_events_gives_each_event_its_offset_in_the_block`: notes at 1000 and 1100; `stream_frame == 896`, `due_events(256)` yields `(104, note)` and `(204, note)`; `due_events(256)` at 1152 yields nothing.
- `a_late_event_is_due_at_once_with_offset_zero`.
- `events_due_on_one_frame_come_out_in_the_order_sent`.
- `a_full_schedule_delivers_at_once_and_counts`: 65 future events; `events_dropped == 1`; the 65th comes out of `try_recv` now.

808 test (`kazoo-808/src/audio.rs` tests): `a_note_due_mid_block_triggers_on_its_sample`: a `NOTE_ON` for a drum at offset 100 of a 256 block; the rendered block is silent before sample 100 and not after (within the drum's attack).

Hub tests (`hub/tests.rs`, `Rig` at `:27`): write the `ReplayTake` by hand in the test (three notes at beats 0, 0.5, 1.0, recorded at 120 BPM) rather than from a file, so the test is about frames:

- `replayed_notes_reach_the_instrument_with_the_frames_the_song_puts_them_on`: client placed (feed audio as `an_instrument_starts_on_the_very_frame_the_desk_does` does, `hub/tests.rs:345`); `Replay { on: true }`; play; the client reads three `NoteEvent`s with `at_frame` values that, converted back through the sync it received (`sync.at_frame` for beat 0 at 120 BPM and 48 kHz), are exactly 0, 12_000 and 24_000 frames after the downbeat.
- `a_new_anchor_resets_the_cursor_and_skips_what_is_behind_it`: stop and play again: the three come again; locate is C06's, so a second play is the test.
- `an_absent_instrument_counts_unsent_and_the_strip_says_so`: disconnect the client; play; `snapshot().replay_unsent == 3`; `channel_readout(slot)` carries the flag the head draws.
- `only_events_sent_to_the_instrument_are_replayed`: a `direction == 0` record is never sent.

Desk test: `P_toggles_the_signal_switch_and_loads_the_latest_take` (tempdir session with a finished take and `.kzev`; the hub end is a test `Receiver<HubCommand>`; the command carries the three events).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p kazoo-mix -p kazoo-808
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core -p kazoo-mix -p kazoo-808 --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src kazoo-mix/src kazoo-808/src
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core
```

## Definition of done

- [ ] Every test above red, then green; every existing test in the three crates green (the link's existing note test at `link.rs:1556` still sends 36 bytes for an unscheduled note).
- [ ] Record a DX take from the keyboard; `P`; play: the DX plays it back; `kazoo-808` plays a replayed drum take on the sample.
- [ ] An old-protocol instrument (a 36-byte decoder) still receives and plays a scheduled note at once, proven by a test that decodes the 44-byte frame with the old prefix.
- [ ] `no inst`, `sig` and the footer note are on screen; help updated; counts in the hub snapshot.
- [ ] The other instruments' `try_recv` sites are listed under FOUND with file:line.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines, so `link.rs` (1645 lines today) loses the schedule to `ipc/link/schedule.rs` or the schedule goes in `ipc/schedule.rs`; never grow it). Real-time rules: `try_recv` and `due_events` run on the instrument's audio callback: fixed arrays, no allocation, no lock; the hub thread sends with `Outbox::queue` as it does for syncs (`hub.rs:885`), no allocation per event (`ReplayTake` is an `Arc` built on the UI thread); a NaN beat or bpm in a sidecar is refused at load with a footer note, never scheduled. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C05b base=<sha> branch=lane/kz-c05b df=<GB> at <time>
END KZ-C05b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
