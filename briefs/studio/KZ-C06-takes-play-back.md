# KZ-C06: every strip plays its recorded takes back through itself on the transport, the song can be located to a bar, and a loop plays round sample-exactly with every instrument following

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

Time: about 30 minutes of a builder's work. One mechanism: a take player per strip reading pre-loaded audio at song positions, plus a song clock that can start anywhere and wrap. Crates: kazoo-mix only (kazoo-core's clip model is used as is). Depends: KZ-C04 (and uses KZ-C05b's `HubCommand::Replay` when present).

## Outcome

A musician records the 808 and the pad, stops, and presses space: both takes play back from the downbeat through their strips, under the same faders, EQ and inserts, while the instruments stay live on top. `,` and `.` step the song back and forward a bar, `Home` goes to the top, `o` turns the loop on round the bars `(` and `)` were pressed on, and the song goes round without a click, the instruments landing on the loop's first beat with it every time. Takes are placed by the beat they were recorded on, so a take recorded at bar 9 plays at bar 9.

## The design (decided; build exactly this)

**Song clock from any beat.** `SongClock::schedule` (`kazoo-mix/src/song.rs:98`) gains a `from: Option<f64>` beat: `Some(b)` starts the song at beat `b` on that frame (play from a locate point), `None` keeps today's rule (0 when starting, continuous otherwise). `SongAnchor` (`song.rs:24`) is unchanged in shape; `recording` was added by C04. Loop: `SongClock` gains `loop_region: Option<(f64, f64)>` in beats, set through a new anchor field `loop_region` so the hub and the instruments learn it on the same frame as everything else; `advance` (`song.rs:123`) computes `beat = start + (elapsed_beats) ` and, when `loop_region` is on and the beat reaches `end`, lands a wrap: the callback, not `advance`, schedules the wrap as an ordinary anchor `{ frame: wrap_frame, beat: loop_start, playing, bpm }` far enough ahead (`schedule_ahead`, `shared.rs:366`, the same distance every transport change uses), because every instrument must be told the frame its beat jumps on (`song_sync`, `hub.rs:1082`, already converts an anchor to each instrument's frame). The callback does this in `schedule_transport` (`callback.rs:87`): after the revision check, if the clock is playing with a loop and its next wrap frame (`SongClock::next_wrap_frame()`, exact integer arithmetic from the anchor: `frame + ceil((end - beat) * 60 * rate / bpm)`) is within `ahead + frames` of `next_frame` and not yet scheduled, schedule it and announce it to the hub. So the loop wraps on an exact frame and the whole studio moves on it; the test is the same shape as `an_instrument_starts_on_the_very_frame_the_desk_does` (`hub/tests.rs:345`).

**Transport in shared state** (`shared.rs:249`): `locate_beat: AtomicU64` (f64 bits) consumed by the callback with the revision (`Transport` gains `from: Option<f64>`; the desk sets it with play in one revision for a locate-and-play, or alone to move the stopped position, which the callback schedules as a stopped anchor at that beat), `loop_region` (two `AtomicU64`s of f64 bits plus an `AtomicBool`), and the desk's keys: `Home` locate to beat 0; `,` / `.` move the position one bar (4 beats, `BEATS_PER_BAR`, `metronome.rs:10`) back or forward, floor-of-bar, while stopped or playing (playing: a locate-and-play anchor); `(` sets the loop start to the current bar, `)` the loop end to the start of the next bar after the current one; `o` toggles the loop (a loop whose end is not past its start is refused with a footer note). Header (`draw_transport`, `ui.rs:1145`) shows the position as `bar.beat` (`009.3`) after the BPM and `LOOP 005-009` in `SAGE` when on.

**Takes on the timeline.** `kazoo-mix/src/takes.rs` (new): `TakeClip { id: ClipId, left: AudioClip, right: AudioClip, beat: f64 }` using `kazoo_core::mixer::clip::{AudioClip, ClipData, ClipId}` (`kazoo-core/src/mixer/clip.rs:156`, `:51`, `:26`): `ClipData` is mono, so a stereo take is two `ClipData`s and two `AudioClip`s with the same id, position, trim and gain, kept in step by `TakeClip`'s methods (`set_beat`, `trim_start`, `trim_end`, `split_at`, `set_gain`, `set_muted` forward to both). `AudioClip::read_into` (`clip.rs:344`) sums allocation-free; `waveform_overview` (`clip.rs:277`) serves KZ-C07. Reason: the model is reused whole, with its tests, and nothing in kazoo-core changes.

**Loading.** On the UI thread (never the callback): `takes::load(path, desk_rate) -> Result<TakeClip, TakeLoadError>`: `read_wav` (`kazoo-core/src/io/file.rs:80`), de-interleave, and if the file's rate differs from the desk's, `resample_mono` each side (`kazoo-core/src/io/resample.rs:35`); the sidecar (C04) gives `song_beat`, `bpm` and `finished`; an unfinished take is refused with "take still open". `ClipData::new(samples, "808-001", Some(path), file_rate)`. Positions are in beats, converted to frames on the callback from the anchor in force (`SongAnchor::beat_at`, `song.rs:49`, inverted: `frame_of(beat) = anchor.frame + (beat - anchor.beat) * 60 * rate / bpm`, rounded to the nearest frame, exact for the tempo the take was recorded at).

**Handing takes to the callback.** A `TakeBay`, the patchbay pattern again (`patchbay.rs:79`; C02's `InsertBay` is the template): `TakeCommand::{Place { slot, take: TakeClip }, Remove { slot, id }, Move { slot, id, beat }, Mute { slot, id, muted }}`, `TakeEvent::{Placed, Removed { take: TakeClip }, Refused { why, take }}`. Each strip holds `takes: Vec<TakeClip>` with capacity `MAX_TAKES_PER_STRIP = 64` reserved at engine creation (a `TakeClip` is two `Arc`s and a few words; the samples live in the `Arc`), so `Place` never allocates on the callback and the 65th is refused with `TooManyTakes`. The `Arc` is dropped on the UI thread when a `Removed` comes back. On the UI thread the desk's `TakeModel` mirrors what is placed (for KZ-C07 to draw and KZ-C08 to save).

**Playing through the strip.** In `ChannelStrip::render_into` (`engine.rs:851`), after the live source's post-EQ block is written (C02's `pre` block) and before the chain, each take whose span covers any of the chunk's frames sums its samples into `pre`. `AudioClip` positions are absolute sample positions, so the strip sets each take's position to `frame_of(take.beat)` whenever a new anchor lands (once per anchor, on the callback, one `set_position` per take, a few words each) and then calls `left.read_into(chunk_start_frame, &mut pre.left[..frames])` and the same for the right, which already handles a chunk that starts before, inside or after the take. While the song is stopped, takes are silent; while it loops, the anchor at the wrap repositions them. Takes do not go through trim or EQ (they were recorded post-fader; running them through the strip's EQ and fader again is the console's "tape return through the channel", which is what a musician expects when they touch the fader on playback; trim is skipped because it is an input gain). The recorded tap (C04) stays the live source alone, so an overdub never re-records the take under it.

**What the player drives.** KZ-C05b's replay: when a take is placed and the strip's `signal` switch is on, the desk sends `HubCommand::Replay` with that take's events (C05b's `ReplayTake`), and the "latest finished take" rule in C05b gives way: the timeline decides. If C05b has not landed, place audio only and note it under FOUND.

**Default placement.** On the desk, `t`? No: `t` is tap tempo. New key `T` on a strip: "take" opens the strip's take list (newest first, `j`/`k`, `Enter` places the take at its recorded beat, `d` removes a placed one, `Esc` closes), drawn in the master section's rectangle like the inserts panel (C03). A take just recorded (C04's `Finished` notice) is placed automatically at its recorded beat on its strip, so stop-then-space plays it back with no further keys.

## Not this lane

- KZ-C07 draws lanes, clips and waveforms, and moves, trims and splits clips on screen; the `TakeClip` methods are here so C07 only wires keys.
- KZ-C08 saves placements; KZ-C10 automates.
- Pre-roll, count-in and punch regions are not in the plan's rows; not here.
- kazoo-core's clip model is used, never edited; a gap in it is a FOUND.

## Red tests first

Song clock tests (`song.rs` tests at `:157`): `play_can_start_from_a_beat`, `a_loop_wraps_on_an_exact_frame` (120 BPM at 48 kHz, loop 4..8 beats: the wrap frame is start + 96_000; `advance(frame - 1)` is just under 8.0, `advance(frame)` after the wrap anchor lands is 4.0), `a_stopped_locate_moves_the_position_without_playing`.

Engine tests (`engine.rs` tests, fake clock `RATE = 800`): `a_placed_take_plays_from_its_beat_through_the_fader` (a 16-frame take at beat 1 at 60 BPM and 800 Hz is frames 800..816 after the play anchor; before them silence, during them the samples times the fader, after them silence; `ClipData` built in the test), `takes_are_silent_while_stopped`, `a_loop_wrap_repositions_takes` (a take at beat 0 is heard again after the wrap anchor), `a_take_is_not_recorded_into_the_strips_own_take` (arm, play with a placed take, the lane's blocks are the live source only), `the_sixty_fifth_take_is_refused_and_handed_back`, `removing_a_take_hands_its_arcs_back` (`Arc::strong_count` falls to the test's one).

Takes tests (`takes.rs`, tempdir): `a_take_loads_sample_exact_at_the_desks_rate` (write a stereo WAV with `write_wav`, `io/file.rs:125`, load, compare `samples()` bit for bit), `a_take_at_another_rate_is_resampled_and_its_length_scales`, `an_unfinished_take_is_refused_in_words`, `split_and_trim_keep_both_sides_in_step`.

Callback test (`callback.rs` tests at `:165`): `a_loop_wrap_is_announced_to_the_hub_ahead_of_its_frame`: loop on, play; the patchbay's `next_song` yields the wrap anchor with `beat == loop_start` and `frame >= next_frame + ahead` before the engine reaches it.

Hub test (`hub/tests.rs`): `every_instrument_wraps_with_the_desk_on_the_loops_frame`: the impulse test at `:345`, with a one-bar loop: the second impulse (on the sync whose beat is the loop start, received after the first) comes out of the desk exactly one bar of frames after the first.

Desk and UI tests: `locate_keys_move_by_bars_and_home_goes_to_the_top`, `loop_points_are_set_from_the_position_and_a_bad_loop_is_refused`, `the_header_shows_the_position_and_the_loop`, `T_lists_takes_and_enter_places_one` (tempdir session with two finished takes; the `TakeBay`'s callback end serviced by hand).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

## Definition of done

- [ ] Every test above red, then green; every existing kazoo-mix test green.
- [ ] Record two strips, stop, space: both play back through their strips from the downbeat; faders and inserts act on them; the instruments stay live.
- [ ] `,` `.` `Home` `(` `)` `o` work; the loop goes round with no click and the 808 lands on its first beat each time (the hub test proves the frame).
- [ ] `T` lists and places takes; a finished take is placed by itself.
- [ ] `MAX_TAKES_PER_STRIP` and the player's per-strip cost are stated in `takes.rs`'s module doc.

## Code quality

Rules of the tree (`sgconfig.yml`, `ast-grep/rules/`: no-discarded-result, no-empty-error-arm, no-ignored-error-if-let, no-lint-bypass, no-result-ok; `unsafe_code = forbid`; clippy all, pedantic, nursery, `-D warnings`; files under 1000 lines). Real-time rules: loading, resampling and `ClipData::new` run on the UI thread; the callback only `read_into`s shared `Arc` samples and sets positions; an `Arc` is never dropped on the callback (every `Remove` hands it back); wrap and locate arithmetic is integer where it reaches a frame and `sanitize`d where it reaches a sample; a NaN beat from a sidecar is refused at load. Red first, one behaviour per commit, no TODO, no stub.

## END

```
START KZ-C06 base=<sha> branch=lane/kz-c06 df=<GB> at <time>
END KZ-C06 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
