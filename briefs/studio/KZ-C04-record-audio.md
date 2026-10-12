# KZ-C04: arm a strip, press record, and its post-fader audio and the master are written as WAV takes by a disk thread, starting on the very transport frame the desk starts on

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. One mechanism: a tape deck thread fed by pre-allocated block rings from the callback, started and stopped on scheduled studio frames. Crates: kazoo-mix (the deck, the keys, the session folder), kazoo-core (only if a WAV helper is missing; `DiskRecorder` at `kazoo-core/src/io/recorder.rs:27` is expected to serve as is). Depends: KZ-C01.

## Outcome

A musician presses `a` on the 808's strip and on the pad group, then `r`. The header turns red with `● REC`, the beat lights run, and from the downbeat the 808's strip, the pad group and the master are each going to their own WAV file in the session's `takes/` folder. `r` or space stops; the files are finished and named on the footer: `808-001.wav, pad-001.wav, master-001.wav`. If the disk ever falls behind, the strip shows `drop 1.2k` in amber and the take's sidecar says how many frames were lost; the file is still the right length, with silence where the audio was lost, so nothing ever plays back early. Nothing on the audio thread waits for the disk, ever.

## The design (decided; build exactly this)

**Session folder.** `~/.kazoo/sessions/<name>/` with `takes/` under it; `$KAZOO_SESSIONS_DIR` overrides the parent, as `$KAZOO_WALL_STATE_DIR` does for the wall (`kazoo-wall/src/paths.rs:44`). The name comes from `kazoo-mix --session <name>` (`parse_args`, `kazoo-mix/src/main.rs:56`, `USAGE` at `:35`; if KZ-A06 has already moved this binary under `kazoo mix`, the flag goes on that subcommand, found by what it does); default `scratch`. Reason for the flag now: a take must have a home before KZ-C08 gives the session a file, and a musician who records twice must find both takes. Names are `[a-z0-9-]` up to 32 characters, else refused before the terminal is taken over, in words. The folder is created at start; failure to create it is a start-up error, not a silent desk. New file `kazoo-mix/src/session_dir.rs`: `SessionDir { root, takes }`, `SessionDir::open(name) -> io::Result<Self>`, `next_take_number(slug) -> io::Result<u32>` (scans `takes/` for `<slug>-NNN.wav`, returns the highest plus one, 1 when none; runs on the UI thread at record start, never on the callback), `take_path(slug, n, ext)`.

**Take names.** `<slug>-NNN.wav` where `slug` is the strip's name lower-cased with every run of characters outside `[a-z0-9]` replaced by `-`, trimmed of `-`, cut to 24 bytes on a character boundary, or `strip-<n>` when empty; groups `g1`..`g4`; the master `master`. Numbering is per slug and per session folder and never reuses a number (a deleted file's number stays spent: `next_take_number` also reads `<slug>-NNN.json` sidecars, so a sidecar left behind holds the number).

**WAV format.** 32-bit float, stereo, the desk's sample rate, written with `hound` exactly as `DiskRecorder::start` does (`recorder.rs:81`) and as the wall's recorder does (`kazoo-wall/src/daemon/record.rs:406`); TAPE.md names it as the primary format. Reason: no take can clip on disk whatever the fader did, it is the desk's own precision, and it round-trips sample-exact through `read_wav` (`kazoo-core/src/io/file.rs:80`). Post-fader strip audio is stereo (after pan), so every take is stereo; a mono source panned centre is written as the two equal channels it is.

**Sidecar.** Every take has `<slug>-NNN.json` beside it, written at finish, so a take describes itself even without KZ-C08's session file. kazoo-mix gains `serde = { version = "1", features = ["derive"] }` and `serde_json = "1"` (both already in `Cargo.lock` through kazoo-wall).

```json
{ "version": 1, "take": "808-001", "target": "strip", "name": "808", "stream": "kazoo-808/main",
  "sample_rate": 48000, "channels": 2, "bpm": 120.0, "song_beat": 0.0, "studio_frame": 1234567,
  "frames": 480000, "dropped_frames": 0, "finished": true, "recorded_at": "2026-10-13T09:12:33Z" }
```

`song_beat` is the song position (`SongAnchor::beat_at`, `kazoo-mix/src/song.rs:49`) on the take's first frame; `studio_frame` is that frame; `frames` is the file's length; `finished` is false while the file is open (KZ-C06 ignores unfinished takes and says so). `recorded_at` is UTC from `SystemTime` formatted with the same civil-date arithmetic the wall uses (`kazoo-wall/src/change.rs:355`); copy that function into `session_dir.rs`, no new dependency.

**Lanes.** `kazoo-mix/src/tape.rs` (new) owns the deck. At start `TapeDeck::new(sample_rate)` builds one lane per possible target: `MAX_STRIPS + BUSES + 1` (C01: 64 + 4 + 1 = 69) `audio_block_ring`s (`kazoo-core/src/audio_transport.rs:339`) with `AudioRingConfig::new(BufferId(lane), 2, 256, blocks)` where `blocks = ceil(sample_rate / 256)`: one second of stereo per lane, 384 KB at 48 kHz, 26 MB in all, allocated once. Reason: nothing is allocated when a stream arrives or when record starts, and one second is fifty times the deck's drain interval. The producers go into the engine (`MixerEngine::with_tape_lanes(Vec<AudioBlockProducer>)`, moved in before the stream starts, indexed by `Target`); the consumers stay on the deck thread. The ring is the one that already exists for the hub's audio (`push_block`, `audio_transport.rs:159`): each block carries its studio `start_frame`, so the deck can tell a gap from a drop without a word from the callback.

**The tap point.** C02's block restructure gives each strip a post-fader stereo block per chunk (the second per-frame loop writes it before summing into the bus); this lane keeps that block in `strip.out: StereoBlock` and, when the strip is armed and the deck is rolling at that frame, pushes `out[from..]` as one `AudioBlock` whose `start_frame` is the studio frame of its first sample. If KZ-C02 has not landed when this lane starts, add the `out` block here (two `MAX_RENDER_FRAMES` buffers per strip) and note it under FOUND; do not wait. Groups push their post-fader sum the same way; the master pushes the limited output after the soft limiter (`render_chunk`, `engine.rs:552`), what the speakers get. Pushing happens once per chunk after the chunk's samples are final, never per frame.

**Arming.** `ChannelControls` (`kazoo-mix/src/controls.rs:202`) and `BusControls` (C01) gain `armed: bool` (a flag bit in `SlotCell::flags`, `shared.rs:186`). `a` toggles it on the focused strip or group (`handle_key`, `desk.rs:348`); the head shows `●` in `RED` after the status when armed (`draw_strip_head`, `ui.rs:446`); a hit target `Arm { slot }` beside mute and solo. The master is always recorded when anything is: a session with no armed strip and `r` records the master alone (the plan says "and the master"; a mixdown pass is the commonest take).

**Record transport.** `SharedState` (`shared.rs:249`) gains `recording: AtomicBool` and bumps `transport_revision` on change (`set_recording`, like `set_playing` at `shared.rs:410`); `Transport` (`shared.rs:287`) gains `recording`. The callback's `schedule_transport` (`callback.rs:87`) already schedules every revision change `ahead` frames on (`shared.schedule_ahead().max(2 * frames)`); it now passes `recording` into `MixerEngine::schedule_transport` (`engine.rs:306`), and `SongAnchor` (`song.rs:24`) gains `recording: bool`. The engine's deck state is `Rolling { from: u64 } | Stopping { at: u64 } | Idle`: when the song clock lands an anchor with `recording` (`SongClock::advance`, `song.rs:123`), the engine starts pushing from exactly that frame (a chunk straddling it pushes its tail from the anchor's frame); when an anchor with `recording == false` lands, it pushes up to that frame and stops. So a take starts and ends on the frames the whole studio moves on, the same frames the instruments are told (`song_sync`, `hub.rs:1082`), and the 808's downbeat is the take's first sample, which is the proof `an_instrument_starts_on_the_very_frame_the_desk_does` (`hub/tests.rs:345`) already gives for play. The hub's `transport_state` (`hub.rs:1041`) reports `TRANSPORT_RECORDING` (`kazoo-core/src/ipc/types.rs:62`) while recording, so instruments that show it can; `apply_transport_request` (`hub.rs:1110`) maps an instrument's `TRANSPORT_RECORDING` request to `set_recording(true)` and `set_playing(true)` instead of play alone (`hub.rs:1115`).

**Keys.** `r`: when stopped, sets playing and recording in one revision (the song starts at beat 0, as `SongClock::schedule` does for a play, `song.rs:101`); when playing, punches in (recording true, same play); when recording, punches out (recording false, playing stays). Space while recording stops both. Header (`draw_transport`, `ui.rs:1145`): the button reads ` ● REC ` in `BG` on `RED` while recording, and the footer's right side shows the open takes' names while rolling and the finished names for 6 s after.

**Deck commands.** UI thread to deck thread over `std::sync::mpsc::sync_channel(64)` (kazoo-mix uses std channels for hub notices, `hub.rs:351`; no crossbeam here): `DeckCommand::Roll { takes: Vec<TakeSpec { lane, path, sidecar: TakeSidecar }> , from_frame }`, `DeckCommand::Stop { at_frame }`, `DeckCommand::Shutdown`. The desk sends `Roll` the moment it sets `recording` (it knows the anchor frame only once the callback schedules it, so `from_frame` is `None` and the deck learns the first frame from the first block's `start_frame`; `Stop` likewise ends at the last block it sees after the stop before the ring runs dry for 100 ms, and the sidecar's `frames` is what was written). Opening the files happens on the deck thread. A take whose file cannot be opened is reported (`DeckNotice::Failed { lane, why }`) and its lane's blocks are consumed and discarded so the ring never fills; the strip shows `rec fail` in `RED`.

**Deck loop.** Every 20 ms (`thread::sleep`; the deck is not real-time and B01 is not about this thread) and on every command: for each rolling lane, `pop_block` (`audio_transport.rs:277`) into a 8192-sample scratch until `Empty`; the first block's `start_frame` fixes the take's `studio_frame`; a block whose `start_frame` is past `next_expected` writes `start_frame - next_expected` frames of zeros first and adds them to `dropped_frames` (the callback pushed and the ring was full: `push_block` returned `Full`, counted by the engine in `SharedState::record_dropped(lane)`, an `AtomicU64` per lane the strip head shows as `drop 1.2k` in `AMBER` via `compact_count`, `ui.rs:1401`); a block whose `start_frame` is before `next_expected` is a bug and is reported as `DeckNotice::Fault`. Samples are written through `DiskRecorder::write_samples` (`recorder.rs:107`, which sanitises). Every second the header is brought up to date as the wall's recorder does (`HEADER_EVERY`, `record.rs:41`): hound's `flush` is not that; use `WavWriter::flush` plus writing the sidecar with `finished: false` and the frames so far, so a desk killed outright leaves a readable take. A file reaching `500_000_000` frames (`MAX_FILE_FRAMES`, `record.rs:54`) is finished and the take continues in `<slug>-NNN.2.wav` with its own sidecar pointing back (`"continues": "808-001"`).

**Finish.** `DiskRecorder::finish` (`recorder.rs:129`), then the sidecar with `finished: true`, written to `.json.tmp`, `sync_all`, renamed over, as the wall's store does (`kazoo-wall/src/store.rs:4`). `DeckNotice::Finished { take, path, frames, dropped_frames }` to the desk for the footer. On quit, `main.rs` sends `Shutdown` and joins the deck with `join_worker` (`kazoo-mix/src/worker.rs:11`) after the stream is dropped, so the last blocks are written.

## Not this lane

- KZ-C05 writes the note and parameter events beside the take; the sidecar's `events` field is added there.
- KZ-C06 plays takes back; KZ-C07 draws them; KZ-C08 lists them in `session.json`.
- KZ-C09's master chain: the master lane records what the speakers get; when C09 lands that is after the chain, which is the intent (TAPE.md "master post-tape").
- No change to any instrument; no change to the wire protocol.

## Red tests first

Deck tests in `kazoo-mix/src/tape.rs` (a `tempfile::tempdir()` session folder; `tempfile = "3"` as a dev-dependency, as kazoo-core has it):

- `a_take_is_the_exact_samples_pushed_from_its_first_frame`: push three 256-frame blocks starting at studio frame 1000 with a ramp signal; `Roll`, `Stop`, `Shutdown`; `read_wav` (`io/file.rs:80`) gives 768 frames equal to the pushed samples bit for bit (`to_bits`), sidecar `studio_frame == 1000`, `frames == 768`, `finished == true`, `dropped_frames == 0`.
- `a_gap_in_the_ring_is_written_as_silence_and_counted`: blocks at 0 and at 512 (256 missing): the file has 768 frames, 256..512 are zero, sidecar `dropped_frames == 256`.
- `a_file_that_cannot_be_opened_is_reported_and_its_lane_keeps_draining`: `takes/` replaced by a file; `DeckNotice::Failed`; pushing 2 s of blocks never fills the ring (the lane's `push_block` never returns `Full`).
- `take_numbers_never_reuse_a_spent_number`: `808-001.wav` and a lone `808-002.json`: next is 3.
- `slugs_are_safe_file_names`: `"Wall / Pad #2"` is `wall-pad-2`; `"ヤマハ"` is `strip-7` for slot 6; 40 characters cut to 24 on a boundary.
- `an_unfinished_sidecar_is_written_every_second`: roll, push, wait 1.2 s, read the sidecar: `finished == false`, `frames > 0`.

Engine tests in `kazoo-mix/src/engine.rs` (fake clock `RATE = 800`, lanes from `audio_block_ring` in the test, consumers popped by the test):

- `recording_starts_on_the_scheduled_frame_not_the_buffer_boundary`: schedule an anchor with `recording` at frame 10 inside a 16-frame render; the lane's first block has `start_frame == 10` and 6 frames; nothing was pushed before it.
- `recording_stops_on_its_frame_and_the_last_block_is_short`.
- `only_armed_strips_and_the_master_are_pushed`: two strips, one armed: its lane and the master lane have blocks, the other lane is empty.
- `a_full_lane_counts_a_drop_and_never_blocks`: a lane of 2 blocks never popped; after 4 chunks `record_dropped(lane) == 2` and the render returned.
- `the_master_take_is_what_the_limiter_puts_out`: a 1.5 source: the master lane's samples equal the output buffer's.

Hub test (`hub/tests.rs`, `Rig` at `:27`): `an_instruments_record_request_starts_the_desk_recording`: a `TRANSPORT_RECORDING` request (`HubIpcClient::send_transport_request`, `kazoo-core/src/ipc/client.rs:358`); `shared.transport().recording` within the rig's wait; the next `TransportSync` the client reads has `state == TRANSPORT_RECORDING`.

Desk and UI tests (`desk.rs` tests at `:734`; `ui.rs` tests at `:1556`): `a_arms_the_focused_strip_and_r_rolls_and_punches` (stopped + `r`: playing and recording; `r`: recording false, playing true; space: both false), `the_header_shows_rec_and_the_strip_shows_its_arm_and_drops` (text contains `● REC`, `●` on the armed strip, `drop 1.2k` when `record_dropped` is 1200).

Session dir test: `the_session_name_is_checked_before_the_terminal_is_taken`: `Bad Name!` refused with the rule in the message; `scratch` creates `takes/`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

(Add `-p kazoo-core` to the first two only if `kazoo-core/src/io/**` was touched.)

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green.
- [ ] `kazoo-mix --session jam` creates `~/.kazoo/sessions/jam/takes/`; `a` on a strip, `r`, play a few seconds, `r`: `<slug>-001.wav` and `master-001.wav` with sidecars, sample-exact against the engine's output in the tests, and the downbeat on the first sample (the hub test proves the frame).
- [ ] Punch in and out while playing; stop with space while recording; quit while recording leaves finished files.
- [ ] A lane that falls behind shows `drop n` on the strip and the sidecar's count; the file length is still exact.
- [ ] Header `● REC`; footer names the takes; help gains `a` and `r`.
- [ ] Memory added (26 MB of lanes at 48 kHz) stated in `tape.rs`'s module doc.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: the callback's only contact with the deck is `push_block` on a pre-allocated ring and an atomic counter; it never opens, writes, formats a name, reads a clock or waits; a full ring is counted and the chunk goes on; every pushed sample is already sanitised by the engine; the deck thread alone does I/O, and every I/O error reaches the desk as a notice in words and a mark on the strip. Nothing is dropped silently: `record_dropped` and the sidecar's `dropped_frames` agree with the zeros in the file. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C04 base=<sha> branch=lane/kz-c04 df=<GB> at <time>
END KZ-C04 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
