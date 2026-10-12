# KZ-C05a: every note and parameter event that passes through the hub while a take rolls is written beside the audio take, stamped with the studio frame and the instrument's own stream frame

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

This is the first half of KZ-C05 (record the signal). It is split because the event file and the replay are two mechanisms; this half writes, KZ-C05b replays. Nothing from the plan's C05 row is dropped: between the two files every outcome is here.

Time: about 30 minutes of a builder's work. Crates: kazoo-mix (hub stamping, deck writing), kazoo-core (the `.kzev` reader and writer, since KZ-C05b's player and `kazoo master` both read it). Depends: KZ-C04 and KZ-B06.

## Outcome

While the desk records, the arp's notes to the 303, the keys a musician plays into the DX, and every knob move the desk or an instrument sends as a parameter change are kept next to the audio: `303-001.kzev` beside `303-001.wav`, one record per event, each saying which studio frame it happened on and which frame of the instrument's own stream that was. So a take is not only what was heard but what was played, to the frame, and KZ-C05b can play it back into the same instrument on the same frames.

## The design (decided; build exactly this)

**What is recorded.** Every `NOTE_EVENT` (`kazoo-core/src/ipc/types.rs:36`) the hub reads from an instrument (`Instrument::handle`, `kazoo-mix/src/hub/instrument.rs:239`), every `NOTE_EVENT` the hub sends to one (`route_note`, `hub.rs:833`), and every `PARAMETER_CHANGE` (`types.rs:38`) in either direction once KZ-B06 has made it real (find where B06 reads it in `instrument.rs` and where the hub sends it). An event is filed under the take of the strip whose instrument is its target when it is sent to an instrument, and under the strip of its source when it is read from one; a broadcast note is filed once per target it reaches. A strip that is not armed records no events; the master records none. An event for an instrument whose strip is armed but whose take failed to open is dropped and counted like the audio (`DeckNotice::Failed`).

**Stamps.** On reading or sending, the hub reads the desk's clock once per poll (`clock()`, `hub.rs:553`; `DeskClock::playing_now`, `kazoo-mix/src/studio_clock.rs:225`) and stamps the event `studio_frame = clock.playing_now()`, `stream_frame = instrument.stamp().stream_at(studio_frame)` (`studio_clock.rs:164`) or `u64::MAX` when the stream is not yet placed. The studio frame is as exact as the hub's wake (500 µs today, better after KZ-B01); the stream frame is the hub's view of which frame of its own stream the instrument was rendering then, which is what KZ-C05b needs to put the note back on the same place. Both are kept so neither has to be derived later.

**The file** (`kazoo-core/src/io/events.rs`, new; `pub mod events` in `io/mod.rs`). `.kzev`, little-endian, fixed-size records, append-only:

```
header, 48 bytes:
  magic           [u8; 4]  = b"KZEV"
  version         u32      = 1
  sample_rate     u32
  reserved        u32      = 0
  count           u64      records in the file; 0 while unfinished, written at finish
  instrument_id   [u8; 16] the target strip's instrument id at record start (RegisterMsg::instrument_id)
  stream_offset   u64      studio_frame.wrapping_sub(stream_frame) at the first event (u64::MAX if unplaced)
record, 32 bytes:
  studio_frame    u64
  stream_frame    u64      u64::MAX when the stream was not placed
  kind            u8       0 note on, 1 note off, 2 cc, 3 pitch bend, 4 parameter change
  channel         u8       MIDI channel for notes; 0 for parameters
  a               u8       note or cc number; parameter index (ParameterChangeMsg::param)
  b               u8       velocity or cc value; 0 for parameters
  direction       u8       0 from the instrument, 1 to the instrument
  pad             [u8; 3]  = 0
  value           f32      ParameterChangeMsg::value; 0.0 for notes
  pad2            u32      = 0
```

`EventRecord` (plain `Copy` struct), `encode(&self, &mut [u8; 32])`, `decode(&[u8; 32]) -> Result<Self, EventFileError>` (an unknown kind, a non-finite value or a non-zero pad is an error, named), `EventWriter::create(path, sample_rate, instrument_id) -> io::Result<Self>` (writes the header with `count = 0`), `EventWriter::push(&mut self, record)` (buffered, `BufWriter`), `EventWriter::checkpoint()` (flush and rewrite `count` in place, every second as the WAV header is), `EventWriter::finish()` (count, flush, `sync_all`), `EventFile::read(path) -> Result<(EventHeader, Vec<EventRecord>), EventFileError>` (refuses a bad magic or version; a `count` short of the records on disk is read as the records on disk with `truncated: true` in the header, so a desk killed outright still leaves a usable file). No allocation in `encode`/`decode`/`push`'s record path (the `BufWriter` is sized once at 64 KiB). Nothing here is on an audio thread.

**Hub to deck.** The hub thread owns no file; it hands each stamped event to the deck over a bounded `std::sync::mpsc::sync_channel(4096)` of `TapeEvent { lane, record }` with `try_send` (never a blocking `send` on the hub thread, which is real-time); a `Full` is counted in `HubStats` (`hub.rs:273`) as `lost_events`, shown in the footer's health as `e` beside `u s r` (`strip_detail`, `ui.rs:1355`) when non-zero, and the sidecar's `dropped_events`. The deck (`tape.rs`, C04) drains the channel in its 20 ms loop and appends to the lane's open `EventWriter`; events for a lane that is not rolling are dropped on the floor and counted as `lost_events` too (the hub does not know the roll state; this keeps the hub ignorant of files). The deck learns which lanes are armed and their instrument ids from `DeckCommand::Roll`'s `TakeSpec` (C04), which gains `instrument_id: Option<[u8; 16]>` and `stream: Option<String>`.

**Hub knowledge of arming.** The hub reads `SharedState::channel_controls(slot).armed` (C04) each poll, so it stamps and forwards only armed strips' events: the channel sees no traffic for an unarmed desk. Since the hub reads armed state and the deck reads roll state, an event is kept exactly when both hold; the window between `r` and the deck opening the file is covered by the deck buffering `TapeEvent`s with `studio_frame >= the first audio block's start_frame` for a lane whose `Roll` has arrived but whose file is still opening (a 4096-entry `VecDeque` per lane reserved once).

**Sidecar.** C04's `<take>.json` gains `"events": "303-001.kzev"` and `"event_count": n`, `"dropped_events": n`; a take with no events has `"events": null`.

**Direction of a parameter change from the desk.** When KZ-C10 (later) or any desk code sends a `PARAMETER_CHANGE` to an instrument, it goes through the hub's send path and is stamped there with `direction = 1`, so this lane needs no hook into C10.

## Not this lane

- KZ-C05b plays the file back: the `at_frame` wire extension, the link's schedule, the hub's sender.
- KZ-B06 defines how an instrument applies a parameter change; this lane records the messages as they pass and does not interpret `param` or `value`.
- KZ-C04 owns the audio lanes and the sidecar's other fields; KZ-C08 lists takes.
- An instrument that does not send its own knob moves as `PARAMETER_CHANGE` records no parameters from itself; say which under FOUND, do not add it to the instrument.

## Red tests first

Event file tests in `kazoo-core/src/io/events.rs`:

- `a_record_round_trips_byte_for_byte`: every kind, `direction` both ways, `value` negative and large; `encode` then `decode` equal; the 32 bytes match a hand-written expected array for one record (so the layout is pinned, not just self-consistent).
- `a_file_written_then_read_back_is_sample_exact`: write 1000 records into a tempdir, `finish`, `read`: header fields and every record equal.
- `an_unfinished_file_reads_what_is_on_disk_and_says_so`: write 10, `checkpoint`, write 5 more, drop without `finish` (simulate a kill: `std::mem::forget` the writer after flushing through a raw `File` clone is not allowed; instead write the 5 directly with `File::write` after the checkpoint): `read` gives 15 records and `truncated == true`.
- `bad_magic_version_kind_and_pad_are_refused_by_name`: four files, four distinct `EventFileError`s whose `Display` names the field.
- `a_non_finite_value_is_refused`: `value = NaN` in a record: `decode` is `Err`.

Hub tests in `kazoo-mix/src/hub/tests.rs` (`Rig` at `:27`; `receive` at `:99`):

- `notes_between_instruments_are_stamped_and_handed_to_the_deck_when_the_target_is_armed`: two clients, the second's strip armed; the first sends a `NOTE_ON` to the second (`send_note_event`, `kazoo-core/src/ipc/client.rs:379`); a test `Receiver<TapeEvent>` in place of the deck gets one record with `kind == 0`, `direction == 1`, `studio_frame` within one device buffer of `shared.studio_frame()`, and `stream_frame != u64::MAX` once the second client's stream is placed (feed audio first as `an_instrument_starts_on_the_very_frame_the_desk_does`, `hub/tests.rs:345`, does).
- `an_unarmed_strips_events_never_reach_the_channel`.
- `a_full_event_channel_is_counted_not_waited_on`: a `sync_channel(1)` never drained; 10 notes; `hub.snapshot().lost_events == 9` and the hub thread's poll period did not stretch (the rig's `until` keeps running).

Deck tests in `kazoo-mix/src/tape.rs` (tempdir): `events_land_in_the_rolling_takes_file_and_the_sidecar_names_it` (roll a lane, push 3 events and 2 audio blocks, stop: `.kzev` with 3 records, sidecar `events` set, `event_count == 3`); `events_for_a_lane_that_is_not_rolling_are_counted_and_dropped`; `events_that_arrive_before_the_file_opens_are_kept` (push events, then `Roll`; all are in the file).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src kazoo-mix/src
nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core
```

## Definition of done

- [ ] Every test above red, then green; every existing test in both crates green.
- [ ] The `.kzev` layout above is pinned by a byte-exact test and documented in `events.rs`'s module doc with the table.
- [ ] Record a take with the arp driving the 303: `303-001.kzev` beside `303-001.wav`, sidecar naming it; `event_count` equals the notes the arp sent.
- [ ] Lost events are counted in the hub's snapshot, shown in the footer, and written in the sidecar; the hub thread never blocks on the channel.
- [ ] A desk killed while recording leaves a `.kzev` that reads to its last checkpoint.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: nothing here touches the audio callback; the hub thread (real-time scheduled, `hub.rs:362`) only stamps and `try_send`s, allocating nothing per event (the `TapeEvent` is `Copy`); every file error reaches the desk in words; counts never wrap silently (`saturating_add`). Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C05a base=<sha> branch=lane/kz-c05a df=<GB> at <time>
END KZ-C05a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
