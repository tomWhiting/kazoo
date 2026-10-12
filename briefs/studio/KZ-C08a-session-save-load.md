# KZ-C08a: the whole desk is saved in `session.json`, written atomically on every change and restored on start, so a session reopens as it was left

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

This is the first half of KZ-C08 (session save, load, undo). It is split because saving and undoing are two mechanisms; this half saves and loads, KZ-C08b undoes. Between the two files every outcome of the plan's C08 row is delivered.

Time: about 30 minutes of a builder's work. Crates: kazoo-mix only. Depends: KZ-C07b.

## Outcome

A musician quits, comes back with `kazoo-mix --session jam`, and the desk is as they left it: every strip named and set, its group, its inserts with their knobs, its takes on the timeline, the tempo, the loop, the position, the zoom. Instruments that are not running show as disconnected strips and reclaim their strip when they start. Nothing has to be saved by hand: half a second after the last change the file is written, and `S` writes it now and says so. A session file that cannot be read is moved aside and named in the header, never overwritten and never guessed at.

## The design (decided; build exactly this)

**The file.** `~/.kazoo/sessions/<name>/session.json` (`SessionDir`, C04). Pretty-printed `serde_json` (both already dependencies of kazoo-mix since C04), one object:

```json
{
  "version": 1,
  "name": "jam",
  "sample_rate": 48000,
  "transport": { "bpm": 120.0, "position_beat": 16.0, "loop": { "start": 4.0, "end": 12.0, "on": true }, "click": false },
  "strips": [
    { "slot": 0, "stream": "kazoo-808/main", "name": "808",
      "controls": { "trim_db": 0.0, "eq": { "low_db": 0.0, "mid_db": 0.0, "high_db": 0.0 }, "aux_send": 0.2, "pan": 0.0, "fader_db": -3.0, "muted": false, "soloed": false, "armed": false, "bus": null, "signal": false },
      "inserts": [ { "slot": 0, "id": "tape", "bypassed": false, "params": [0.375, 0.4, 1.0, 50.0] }, null, null, null, null, null, null, null ],
      "takes": [ { "take": "808-001", "clip": 12, "beat": 0.0, "source_start": 0, "source_end": 480000, "gain_db": 0.0, "muted": false } ] }
  ],
  "buses": [ { "bus": 1, "controls": { "fader_db": 0.0, "pan": 0.0, "muted": false, "soloed": false, "armed": false }, "inserts": [ ... ], "takes": [] }, ... four ... ],
  "master": { "fader_db": 0.0, "aux_return": 0.7, "takes": [] },
  "timeline": { "zoom_beats_per_column": 1.0, "scroll_beat": 0.0 },
  "next_clip_id": 13,
  "saved_at": "2026-10-13T10:02:11Z"
}
```

Every field is a plain serde struct in `kazoo-mix/src/session.rs` (new): `Session`, `TransportSave`, `StripSave`, `BusSave`, `InsertSave`, `TakeSave`, `TimelineSave`, with `#[serde(deny_unknown_fields)]` off (a newer file's extra field is kept through `#[serde(flatten)] extra: serde_json::Map`, so a load-then-save by an older desk never loses it) and every numeric field checked after parse: non-finite or out-of-range values are replaced by the control's default through the same `sanitized` methods the desk uses (`ChannelControls::sanitized`, `controls.rs:240`) and reported in the header as `session.json: 3 values out of range, defaults used`. Insert params are by index in `EffectKind::params` order (`kazoo-fx/src/lib.rs:98`), which the catalogue says is stable (`lib.rs:92`); an unknown effect id is kept in the file, shown as `unknown: <id>` in the slot and refused to the engine. `pan` is the `Pan` value (`kazoo_core::Pan::value`). The master's `takes` is for KZ-C09's renders placed back, empty here. The reason for JSON over the design doc's TOML split (`design/terminal-daw/DESIGN.md` "Session Model"): the wall's saves are JSON with serde (`kazoo-wall/src/store.rs`), the deck's sidecars are JSON (C04), one format across the studio, and one file is one atomic write.

**Writing.** `Session::save(&self, dir)`: serialise, write `session.json.tmp`, `sync_all`, rename over `session.json`, then `fsync` the directory, exactly as the wall's store does (`kazoo-wall/src/store.rs:4`, copy its sequence; `File::open(dir)?.sync_all()` is the directory flush on macOS and Linux). A write error is a header alert `could not save session.json: <err>` that stays until the next successful save; the desk keeps running. Never written from the audio thread or the hub thread.

**Autosave.** The desk (`desk.rs:257`) gains `dirty_since: Option<Instant>`; every model change (controls, inserts, takes, transport settings, zoom) sets it; `tick` (`desk.rs:312`) saves when `dirty_since` is 500 ms old, or at once on `S`, or on quit. Fader rides mark dirty on every nudge but the write waits for the half second of quiet. Reason for 500 ms: a ride is one save, a kill loses at most half a second of edits. The model that is saved is the UI thread's: `SharedState` controls (`channel_controls`, `shared.rs:417`), `InsertModel` (C02), `TakeModel` (C06), the transport from `SharedState::transport` plus loop and position, `TimelineState` (C07a). The header shows `saved` for 2 s after each write in `TEXT_DIM`.

**Loading.** At start, before the terminal is taken (`main.rs:76`): `SessionDir::open(name)` then `Session::load(dir)`: no file is a new session; a file that fails to parse is moved to `session.broken-<unix>.json` (the wall's rule, `store.rs:608`) and the desk starts empty with the header alert naming the file; a `version` above 1 is refused the same way ("written by a newer desk"). Applying a loaded session, on the UI thread after the engine and hub are up: strips are created in their slots as `Disconnected { key: stream }` through a new `HubCommand::Reserve { slot, stream, name }` (the hub seats them `Parked(key)`, C01, so a returning instrument reclaims its slot; the `HubCommand` channel is C05b's, added here if C05b has not landed), controls stored into `SharedState`, inserts built, prepared and `Put` through the `InsertBay` (C02), takes loaded (`takes::load`, C06; a missing WAV is a header note `take 808-001 missing` and the clip is kept in the model as missing, drawn hatched, so a file restored later comes back on the next load), and placed through the `TakeBay`; transport tempo, loop, click and a stopped locate to `position_beat` set in `SharedState`; the timeline state restored; `next_clip_id` restored so ids never repeat. A sample rate in the file that differs from the device's is shown in the header (`session at 44100, desk at 48000: takes resampled`) and takes are resampled at load (C06 already does).

**Session name in the header.** `draw_device_info` (`ui.rs:1083`) shows `jam` before the device facts.

## Not this lane

- KZ-C08b adds undo and redo; the save is of the current state only.
- KZ-C09 adds the master chain and renders to `session.json`; the `master` object has room.
- KZ-C10 adds `automation` to each strip's save.
- KZ-A07's `kazoo studio up <name>` reads the same folder name; nothing here depends on it.
- No change to kazoo-core, kazoo-fx or any instrument.

## Red tests first

Session tests (`session.rs`, tempdir):

- `a_session_round_trips_through_json_field_for_field`: build a full `Session` (two strips with inserts and takes, a bus, loop on), save, load, equal; the JSON text contains the keys above (pin the schema with a `serde_json::Value` comparison against an expected literal for one strip).
- `a_save_is_atomic`: inject a write failure after the `.tmp` is written (a read-only directory): `session.json` is unchanged and the error names the path.
- `out_of_range_values_become_defaults_and_are_counted`: `fader_db: 1e9`, `pan: NaN`, `bpm: -1`: load succeeds, values are the defaults, the report says 3.
- `an_unknown_insert_id_is_kept_and_named`.
- `an_unknown_field_survives_a_load_and_a_save`.
- `a_broken_file_is_moved_aside_and_named`: write `{`; load gives an empty session and `session.broken-<unix>.json` exists with the bytes.
- `a_newer_version_is_refused_without_touching_it`.

Desk tests (`desk.rs` tests): `every_change_marks_the_session_dirty_and_it_saves_after_half_a_second_of_quiet` (nudge a fader five times 100 ms apart with a fake clock passed to `tick`: no save; 500 ms later: one save; the file's `fader_db` is the last value), `S_saves_now_and_quit_saves`, `loading_a_session_restores_strips_inserts_takes_and_transport` (a tempdir session with a WAV and sidecar; after `apply`, `SharedState` controls match, the `InsertBay` saw a `Put` of `tape`, the `TakeBay` a `Place` at beat 0, the transport is at 120 with the loop on, and the hub end received `Reserve` for the strip).

UI test: `the_header_names_the_session_and_says_saved_and_broken`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green.
- [ ] Set up a desk with inserts and takes, quit, start again with the same `--session`: everything back; start the instrument: it takes its old strip.
- [ ] `session.json` matches the schema above; the schema is in `session.rs`'s module doc.
- [ ] Autosave, `S`, save on quit; atomic write; broken file moved aside and named; header shows the name, `saved`, and alerts.
- [ ] A missing take is named and kept, never dropped from the file.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: the audio callback and the hub thread know nothing of the file; every value loaded passes the same sanitising the desk's own keys do before it reaches `SharedState`; every file error is shown, none swallowed; the temp-file-rename-fsync sequence is the only way the file is written. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C08a base=<sha> branch=lane/kz-c08a df=<GB> at <time>
END KZ-C08a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
