# KZ-C07a: a timeline view with one lane per strip, clips drawn from peak files the disk thread computes, a real time scale in bars and seconds, zoom, scroll and a playhead

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename.

This is the first half of KZ-C07 (timeline view). It is split because drawing the timeline and editing clips on it are two mechanisms; this half draws, KZ-C07b edits. Between the two files every outcome of the plan's C07 row is delivered.

Time: about 30 minutes of a builder's work. Crates: kazoo-mix (the view, the peak reader), kazoo-core (the peak file writer and reader in `io/`, since `kazoo master` and any later tape app read them). Depends: KZ-C06.

## Outcome

A musician presses `Tab` and the desk becomes a tape-and-tracks view: one lane per strip and group, named at the left, each take drawn as a clip with its waveform in it, a ruler above in bars and beats with seconds under it, the loop region shaded, and a playhead that moves while the song plays. `+` and `-` zoom round the playhead from a whole song on screen down to a beat across the terminal; `h`/`l` scroll; `j`/`k` pick a lane; `Tab` goes back to the desk with everything as it was. Nothing waits: a take's waveform appears when its peak file is ready, and until then the clip is drawn plain with its name.

## The design (decided; build exactly this)

**Peak files** (`kazoo-core/src/io/peaks.rs`, new; `pub mod peaks` in `io/mod.rs`). `<take>.peaks` beside the WAV, little-endian:

```
header, 16 bytes: magic b"KZPK", version u32 = 1, frames_per_bin u32 = 256, channels u16, reserved u16 = 0
bin: per channel, min i8 then max i8 (sample * 127, rounded, clamped to -127..=127; NaN/Inf read as 0)
```

Stereo: 4 bytes per bin, 750 bytes a second at 48 kHz. `PeakWriter::create(path, channels)`, `push_frames(&[f32] interleaved)` (accumulates a partial bin across calls, pre-sized state), `finish()` (flushes the partial bin, `sync_all`); `PeakFile::read(path) -> Result<Peaks, PeakFileError>` with `Peaks { frames_per_bin, channels, bins: Vec<[(i8, i8); 2]> }` (mono stored as one channel, read as two equal), and `Peaks::columns(&self, first_frame, frames_per_column, width) -> impl Iterator<Item = (f32 min, f32 max)>` that folds bins into screen columns (min of mins, max of maxes), clamped to -1..=1, for any zoom: coarser than a bin folds, finer than a bin repeats the bin. Reason for i8: a terminal cell has eight vertical steps (`EIGHTHS`, `kazoo-mix/src/ui.rs:67`); 127 levels is far more than it can show and a tenth of the f32 file.

**Who computes them.** The deck thread (C04, `tape.rs`) writes the peak file alongside the WAV as it writes, from the same samples (one `PeakWriter` per rolling lane), so a take's peaks exist the moment it is finished and cost no second read. For a take without one (recorded before this lane, or copied in), the deck builds it on request: `DeckCommand::Peaks { path }` reads the WAV in 65,536-sample pieces through `hound::WavReader` and writes the file, then `DeckNotice::Peaks { path }`; the desk asks when it places a take whose `.peaks` is missing and draws the clip plain until the notice.

**View state** (`desk.rs:257` `Desk`). `view: View { Desk, Timeline }` toggled by `Tab`; `timeline: TimelineState { zoom: Zoom, scroll_beat: f64, lane: usize, peaks: HashMap<PathBuf, Arc<Peaks>> }` (the map is UI-thread only; loaded through `PeakFile::read` in `tick` when a notice arrives or a placed take has a file). `Zoom` is beats per column, from the fixed ladder `[1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8, 16, 32]` beats; `+` steps finer, `-` coarser, each keeping the playhead's column where it was (adjust `scroll_beat`); the starting zoom is the coarsest that fits the session's last clip end in the lane width, or 1 beat per column for an empty session. `h`/`l` scroll a quarter of the width; `H`/`L` a whole width; `Home` the start; `End` the last clip end; `j`/`k` move the lane cursor; the view follows the playhead while playing (scrolls a page when the playhead leaves the right edge) unless the musician scrolled in the last 2 s. Reason for beats, not seconds, as the unit: the song clock is in beats, takes are placed by beat, and a bar grid is what a musician aligns to; seconds are shown as a second ruler row from the tempo (`beats_in`, `kazoo-core/src/ipc/follow.rs:31`, inverted).

**Drawing** (`kazoo-mix/src/ui/timeline.rs`, new, called from `draw` (`ui.rs:129`) when `desk.view == Timeline`; `ui.rs` becomes `ui/mod.rs` if C03 did not already do it). Layout top to bottom: the same header (`draw_header`, `ui.rs:1049`); a two-row ruler (bars and beats on the first row as `5` at each bar line and `·` at beats when zoomed in enough to show them, seconds on the second as `0:12` at nice intervals chosen as `choose_tick_interval` does in kazoo-tui, `kazoo-tui/src/ui/timeline.rs:463`, copied into kazoo-mix as a pure function with its tests); one lane per strip in use and per group with any take, `LANE_ROWS = 3` rows each (the terminal's height permitting; lanes scroll with the lane cursor when they do not fit), a 14-column name gutter at the left (`808`, `G2 pad`, the focused lane's name in `BRASS`); the master lane last when it has takes; the footer as today (`draw_footer`, `ui.rs:1215`, with the timeline's keys). A clip is a run of columns from its start beat to its end beat (frames to beats through the take's recorded tempo), drawn with its name on the top row (cut to its width) and the waveform on the rows below from `Peaks::columns` using `EIGHTHS` from the lane's centre line (two rows: min below, max above), `SAGE` on a `PANEL_FOCUS` background, muted clips in `TEXT_DIM`, the selected clip (C07b) in `BRASS`. Overlapping clips on a lane draw later-placed on top. The loop region is a `PANEL_FOCUS` band across the ruler with `(` and `)` markers; the playhead is a `RED` `│` down every lane at the song position, read from the shared studio frame and the song anchor in force (`SharedState::studio_frame`, `shared.rs:382`; `MixerEngine::song` published by the callback into `SharedState` as `song_anchor` (frame, beat bits, bpm bits, playing): add that publish in `callback.rs:142` beside `set_studio_frame`). A recording take in progress is drawn as a growing `RED` clip from its first frame to the playhead, named `rec`.

**Hit targets.** `Lane { lane }` (click focuses the lane), `Clip { lane, id }` (click selects, for C07b), `Ruler { beat }` (click locates the song there: a stopped locate, C06's shared `locate_beat`), `ZoomIn`, `ZoomOut` on two `[+]` `[-]` cells at the ruler's right end; wheel over the lanes scrolls, wheel with the pointer over the ruler zooms.

**Help** (`ui.rs:1417`): a `Timeline` group with every key above.

## Not this lane

- KZ-C07b selects, moves, trims, splits, mutes and deletes clips on this view; here a click selects and nothing moves.
- KZ-C06 owns placement and the take model; C07a reads `TakeModel` and never changes it.
- KZ-C08 saves zoom and scroll with the session; KZ-C10 adds automation rows under a lane.
- kazoo-tui's timeline (`kazoo-tui/src/ui/timeline.rs`) is not changed; one pure function is copied with attribution in its doc comment, not shared, since kazoo-tui is not a dependency of kazoo-mix.

## Red tests first

Peak file tests (`kazoo-core/src/io/peaks.rs`): `a_peak_file_round_trips_bin_for_bin` (1000 bins of known values, byte-exact header pinned by a hand-written array), `a_partial_bin_is_kept_across_pushes_and_flushed_at_finish` (push 100 then 300 frames at 256 per bin: 2 bins, the second holding frames 256..400), `columns_fold_bins_at_every_zoom` (coarser: min of mins; finer: repeated; past the end: `(0.0, 0.0)`), `nan_and_inf_read_as_zero`, `bad_magic_and_version_are_refused_by_name`.

Deck tests (`tape.rs`, tempdir): `a_take_gets_its_peak_file_as_it_is_written` (roll, push blocks with a known ramp, stop: `.peaks` exists with the right bin count and values), `peaks_are_built_on_request_for_a_take_without_them` (write a WAV with `write_wav`, `DeckCommand::Peaks`, notice, file equal to what the rolling path produces for the same samples).

Timeline model tests (`kazoo-mix/src/ui/timeline.rs` or a `timeline_state.rs` with the pure parts): `zoom_steps_the_ladder_round_the_playhead` (the playhead's column is the same before and after `+`), `scroll_follows_the_playhead_while_playing_unless_the_musician_scrolled`, `beats_and_seconds_rulers_agree_at_the_tempo` (120 BPM: bar 3 is at 4.0 s), `the_starting_zoom_fits_the_session`.

UI tests (`ui.rs` tests at `:1556`, `render_with` at `:1581`, `text` at `:1622`): `tab_switches_to_the_timeline_and_back_keeping_focus`, `the_timeline_draws_a_lane_per_strip_with_its_takes_named` (two strips with placed takes from a tempdir with peak files: text contains both names, the clip names, bar numbers and `0:00`), `a_clip_without_peaks_is_drawn_plain_and_filled_in_after_the_notice`, `the_playhead_and_loop_are_drawn_where_the_song_is` (song anchor at beat 8, loop 4..12: the `│` column and the band's columns are where the zoom puts them), `the_timeline_renders_at_many_sizes_without_panicking` (the size loop from `renders_at_many_sizes_without_panicking`, with 0, 1 and 70 lanes).

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
- [ ] `Tab` shows the timeline: lanes, named clips with waveforms, bars and seconds, loop band, moving playhead, a growing `rec` clip while recording; `Tab` back.
- [ ] Zoom from the whole session to a beat across the screen, scrolling and following, by keys and by wheel; a ruler click locates.
- [ ] Every recorded take has a `.peaks` file; a take without one gets one without the desk pausing.
- [ ] The `.peaks` layout is pinned by a byte-exact test and documented in `peaks.rs`'s module doc.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: the callback adds one atomic publish (the song anchor) and nothing else; peaks are computed and read on the deck and UI threads only; a peak file error is a footer note and a plain clip, never a panic; `columns` is bounds-safe for any zoom and any width, including zero. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C07a base=<sha> branch=lane/kz-c07a df=<GB> at <time>
END KZ-C07a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
