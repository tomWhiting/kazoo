# KZ-C07b: on the timeline a clip can be selected, moved by a beat or a bar, trimmed at either end, split at the playhead, muted, duplicated and removed, every edit sample-exact through the engine

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

This is the second half of KZ-C07 (timeline view); KZ-C07a draws, this half edits. Between the two files every outcome of the plan's C07 row is delivered.

Time: about 30 minutes of a builder's work. One mechanism: clip edits on the timeline, applied to the take model and carried to the engine through KZ-C06's `TakeBay`. Crates: kazoo-mix only. Depends: KZ-C07a.

## Outcome

On the timeline a musician walks to a lane, `Enter` selects the clip under the playhead (or `n`/`p` the next or previous clip on the lane), `h`/`l` nudge it a beat, `H`/`L` a bar, `[` and `]` trim its start and end to the playhead, `s` splits it at the playhead into two clips, `m` mutes it, `y` duplicates it right after itself, `d` removes it (the take file stays on disk; only the placement goes). Every edit is heard on the next pass of the song exactly where it is drawn, because the drawing and the engine read the same `TakeClip`. The mouse drags a clip sideways and drops it on the beat grid.

## The design (decided; build exactly this)

**Selection.** `TimelineState` (C07a) gains `selected: Option<(lane, ClipId)>`; `Enter` on a lane selects the clip covering the playhead's beat (the later-placed one when several), `n`/`p` the next or previous clip on the lane by start beat, `Esc` clears. The selected clip is drawn in `BRASS` (C07a). Lanes are strips and groups, as drawn.

**Edits** (`kazoo-mix/src/takes.rs`, C06's `TakeModel`, which is the UI thread's record of what is placed; every edit is a pure method on it that also returns the `TakeCommand` to send):

- Move: `h`/`l` by one beat, `H`/`L` by one bar (`BEATS_PER_BAR`, `metronome.rs:10`), never before beat 0. `TakeCommand::Move { slot, id, beat }` (C06).
- Trim: `[` sets the clip's start to the playhead's beat when the playhead is inside the clip (`TakeClip::trim_start`, forwarding to `AudioClip::trim_start`, `kazoo-core/src/mixer/clip.rs:286`, with the beat moved forward by the same frames so the audio stays where it was in time, the console way); `]` sets the end (`trim_end`, `clip.rs:296`). Frames are from beats at the take's recorded tempo, rounded to the nearest frame. New `TakeCommand::Trim { slot, id, source_start, source_end, beat }`: the callback applies all three on the placed clip in one command so a trim is never heard half done.
- Split: `s` at the playhead inside the clip: `TakeClip::split_at(frame, new_id)` (`AudioClip::split_at`, `clip.rs:312`, on both sides; ids come from the desk's `ClipId` counter, monotonic for the session, never reused); `TakeCommand::Split { slot, id, at_frame, new_id }`: the callback splits in place, allocation-free (the two halves share the `Arc`, `clip.rs:320`); the right half becomes the selection.
- Mute: `m` toggles `TakeCommand::Mute` (C06).
- Duplicate: `y` places a copy of the clip (same `ClipData` `Arc`s, same trim and gain, a new id) at the clip's end beat; `TakeCommand::Place` with the copy (built on the UI thread; the 65th on a lane is refused as C06 says, shown in the footer).
- Remove: `d` sends `TakeCommand::Remove`; the `Removed` event's `TakeClip` is dropped on the UI thread; the take file and sidecar are untouched (removing a file is a session operation for KZ-C08's session menu, not a timeline key).
- Gain: `+`/`-` on a selected clip move its gain by 1 dB, `}`/`{` by 6 dB (`AudioClip::set_gain`, `clip.rs:238`, through `Db`), `0` to unity; `TakeCommand::Gain { slot, id, db }`; the clip's name row shows `-6 dB` after the name when not unity.

Keys that clash with C07a's view keys (`h`/`l` scroll, `[`/`]` are free on the timeline since tempo nudge is a desk-view key) resolve by selection: with a clip selected, `h`/`l`/`H`/`L` move the clip and `Esc` returns them to scrolling; the footer says which (`clip 808-001 · h l move · Esc deselect`).

**Mouse.** Drag on a `Clip` hit target moves it: the beat under the pointer minus the grab offset, snapped to the nearest beat at zooms of a beat per column or coarser and to the nearest sixteenth when finer; the engine is told once on release (`Move`), the drawing follows the pointer during the drag from a `dragging: Option<ClipDrag>` on `TimelineState`, released as `release_drag_if_moved` does for faders (`desk.rs:501`) when the view changes under it.

**Engine side.** C06 reserved `MAX_TAKES_PER_STRIP` slots per strip and places, removes, moves and mutes; this lane adds `Trim`, `Split`, `Gain` to `TakeCommand` and their application in the strip (each is a few field writes on the callback; `Split` pushes into the reserved `Vec`, refused with `TooManyTakes` when full, and then nothing changes). A command naming an id that is not on the strip is `Refused { why: NoSuchClip }`.

**Footer and help.** The footer's detail on the timeline shows the selected clip: name, lane, start bar.beat, length in bars and beats, gain, `muted`; help gains the clip keys.

## Not this lane

- KZ-C08b makes every edit here undoable; the `TakeModel` methods return what they changed so C08b can invert them, but no undo log exists here.
- Crossfades, fades, clip colours, lane reordering and ripple edits are not in the plan; not here.
- No change to kazoo-core; a clip-model gap is a FOUND.

## Red tests first

Take model tests (`takes.rs`): `move_trim_split_duplicate_and_remove_change_the_model_and_name_their_command` (one test per edit, asserting the returned `TakeCommand` and the model after), `trim_start_keeps_the_audio_where_it_was_in_time` (beat moves by the trimmed frames at the take's tempo), `a_split_gives_the_right_half_a_fresh_id_and_both_share_the_audio` (`ClipData::shares_audio_with`, `clip.rs:125`), `edits_never_move_a_clip_before_beat_zero`.

Engine tests (`engine.rs` tests, fake clock, `ClipData` built in the test): `a_trimmed_take_plays_only_its_window` (trim 4 frames off each end of a 16-frame take: 8 frames heard, the right ones), `a_split_take_plays_both_halves_without_a_gap_or_a_double` (output identical before and after the split), `clip_gain_scales_the_take`, `a_command_for_an_unknown_clip_is_refused`.

Desk tests (`desk.rs` tests, `key()` at `:740`, with the `TakeBay` callback end serviced by hand): `enter_selects_the_clip_under_the_playhead_and_n_p_walk_the_lane`, `with_a_clip_selected_h_l_move_it_and_esc_returns_them_to_scrolling`, `brackets_trim_to_the_playhead_only_inside_the_clip`, `s_splits_at_the_playhead_and_selects_the_right_half`, `y_duplicates_after_the_clip_and_the_sixty_fifth_is_refused_in_the_footer`, `d_removes_the_placement_and_keeps_the_file` (tempdir: the WAV is still there), `a_drag_snaps_to_the_grid_and_sends_one_move_on_release`.

UI tests: `a_selected_clip_is_drawn_bright_with_its_gain` and `the_footer_describes_the_selected_clip`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green.
- [ ] Select, move, trim, split, mute, duplicate, gain and remove by keyboard; move by mouse; each heard on the next pass where it is drawn.
- [ ] Every refusal in the footer; files never deleted from the timeline.
- [ ] Footer and help updated.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: every command applied on the callback is field writes on pre-allocated clips; `Split` never allocates (shared `Arc`, reserved `Vec`); the `Arc` of a removed or refused clip returns to the UI thread; beat-to-frame conversion rounds through integer arithmetic once per edit on the UI thread, and the callback receives frames. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C07b base=<sha> branch=lane/kz-c07b df=<GB> at <time>
END KZ-C07b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
