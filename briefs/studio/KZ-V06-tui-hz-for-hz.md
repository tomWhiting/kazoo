# KZ-V06: kazoo-tui's armed tracks follow the voice frame, hertz for hertz, instead of a 46 ms pYIN average

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core (`engine/`, `synthesis/pitch_tracked.rs`). Time: about 30 minutes. Depends: KZ-V04. May run beside KZ-V05 (no shared files). If KZ-A03b is in flight it also edits `engine/mod.rs` (`start_desk_link`, `:366-384`); run after it or keep to the lines named here.

Read by Cassio, 13 Oct 23:2x. Changes: the checks take the lane form (cargo lock, four jobs), and the by-hand listen is Cassio's and Tom's after landing, not the builder's. Opens only after V04 has landed and A03b has landed or been closed (both edit `engine/mod.rs`).

Why: the voice synth Tom has today is kazoo-tui's engine (`kazoo mouth` runs it until V08a). Its armed tracks get their pitch from `drain_analysis_results` (`kazoo-core/src/engine/processing.rs:865-903`): the newest pYIN estimate, which after V01 arrives every 10.7 ms about 50 ms late, then a 20 ms portamento (`synthesis/pitch_tracked.rs:128`). With V04's tracker on the callback, the same tracks can follow within two cycles of the note.

## Outcome

In kazoo-tui, an armed track's synth gets a new pitch every 64 samples from `VoiceTracker`, on the output callback, from the same mic block it plays; the judge (pYIN plus onsets) runs on its own thread; the analysis thread keeps only the spectrum and formants it draws; portamento defaults to 3 ms; the pitch display shows the tracker's pitch and confidence. Playback of clips still drives synths from the clip audio, as today.

## The design (decided; build exactly this)

1. **Judge thread instead of pitch on the analysis thread.**
   - `engine/analysis_thread.rs`: remove the `PitchDetector` and the `OnsetDetector` (`:74-83`, `:109-113`, `:125-127`) and `pitch_prod` from `AnalysisIo`; `run` loses its `pitch_detector` parameter. It keeps spectrum and formants.
   - `engine/mod.rs` `start`: replace `PitchDetector::new(..)` (`:183`) and the pitch ring (`:207`) with `voice::judge::spawn(judge_config(&config.pitch, sample_rate), buffer_size, OnsetConfig { onset_fft_size: config.onset_fft_size, onset_threshold: config.onset_threshold })`: the caller's range and threshold reach the judge, and an invalid range is still a caller error at start. The `JudgeFeed` and `JudgeResults` go into `ProcessingIO`; the `Judge` handle goes into `ThreadHandles` and is joined beside the analysis thread at shutdown (find where `ThreadHandles.analysis` is joined in `engine/handle.rs` and do the same).
   - `EngineConfig::onset_fft_size` and `onset_threshold` stay and now reach the judge through `OnsetConfig`. `AnalysisConfig` (`analysis_thread.rs:22-35`) loses its two onset fields, since the analysis thread no longer detects onsets.
2. **The tracker on the callback.** New file `engine/processing/voice.rs` (child module of `processing.rs`, like `block_tests.rs`), because `processing.rs` is 1932 lines and gains only calls:
   - `ProcessingState` gains `voice: VoiceTracker` (built in `ProcessingState::new` at the engine's rate; allocation there, at engine start) and `last_frame: VoiceFrame`.
   - `pub(super) fn feed_voice(io, state, block: &[f32])`: `io.judge.push(block)`; drain `io.judge_results` into `state.voice.judge(..)`; `state.voice.process(block, |f| state.last_frame = f)` (capture into a local, then assign: no borrow of `state` inside the closure); then, if `state.last_frame.voiced`, the armed-track loop that is today at `processing.rs:890-902` sets `set_pitch(state.last_frame.hz)` (same playback/armed rule).
   - `render_segment` (`processing.rs:492-498`): `feed_analysis` keeps feeding the analysis ring (spectrum, formants); the pitch part of `drain_analysis_results` (`:866-868`, `:890-902`) is deleted; call `feed_voice` with `&state.mic_block[..num_read]` when not in playback (the `feed_analysis` rule, `:813-821`), and from `feed_clip_analysis`'s place with the clip buffer during playback (`:826-837`), so clip playback drives synths from clip audio as today (one block later, exactly as today's ordering).
   - The display (`processing.rs:1082`, `frame.pitch = state.latest_pitch`) takes `PitchEstimate { frequency: last_frame.voiced.then_some(last_frame.hz), voiced_probability: last_frame.confidence, midi_note: .. }`; `latest_pitch` goes.
3. **Portamento.** `PitchTrackedSynth::DEFAULT_PORTAMENTO_MS` 20 → 3 (`pitch_tracked.rs:128`). Reason: pitch now changes every 1.3 ms by small steps; 3 ms smooths the steps, while 20 ms was a 20 ms lag on every note, larger than the whole tracker budget. The parameter's range is unchanged; a user who wants glide still turns it up.
4. **No other behaviour changes.** MIDI note-on and pitch-bend (`processing.rs:1361-1394`) still set pitch on armed tracks; the desk's NOTE_ON still plays (`:591-619`).

## Not this lane

Publishing the voice on the desk (V07a, V08a — kazoo-tui never publishes). The quantiser (kazoo-tui plays direct). Moving kazoo-tui to `kazoo tui` (V08a).

## Red tests first

In `engine/processing/block_tests.rs` (the `Rig`, `monitor_mic` and `render` helpers are at `:19-37`); a helper `sing(rig, hz_curve)` pushes a sine following a pitch curve into the mic ring block by block. Red reasons stated per test.

- `an_armed_track_follows_a_note_change_within_two_periods`: one armed `PitchTracked` track, 128-frame blocks at 48 kHz, 220 Hz for 300 ms then 330 Hz: the track's synth target frequency (read through the track's layer; add a `#[cfg(test)]` accessor if none exists) is 330 Hz within 5 cents no later than `2/330 s + 1 ms + one block` after the change. Red: today the pitch comes from pYIN, tens of ms later.
- `the_display_shows_the_trackers_pitch`: after 200 ms of 440 Hz, the next display frame's `pitch.frequency` is 440 within 5 cents and `voiced_probability ≥ 0.9`. Red: today it is the pYIN estimate (V01 made it right in Hz, but it arrives on the judge's schedule; assert also that the display updated within 2 blocks of the tracker locking, which today's path cannot do).
- `playback_still_drives_synths_from_the_clip`: a recorded clip of a 330 Hz sine played back with no mic input: the playing track's synth reads 330 within 5 cents within 50 ms of playback start. Guard (should pass before and after; list under NOT CONFIRMED as "not red").
- `a_busy_session_never_allocates_or_frees_on_the_audio_thread` (`block_tests.rs:367`): extended with mic input that sings `steps`; must stay green.
- `analysis_thread::tests::run_drains_input_and_exits_when_producer_dropped` (`analysis_thread.rs:144`): updated to the new signature (no pitch ring); stays green.
- `the_judge_thread_is_joined_at_shutdown`: start and drop an engine in the existing engine test style if one opens a device in tests; if engine tests cannot open a device in CI, test the `ThreadHandles` join path directly with a spawned judge. Red: no judge thread.
- `pitch_tracked::tests::default_portamento_is_three_ms`: `PARAMS[PARAM_PORTAMENTO].default == 3.0`. Red: 20.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core -p kazoo-tui --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core -p kazoo-tui --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/engine kazoo-core/src/synthesis/pitch_tracked.rs
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`. Read `df -h /System/Volumes/Data` before each command and put it in START and END.

## Definition of done

- [ ] All red tests fail first (paste under RED), then pass; every existing kazoo-core and kazoo-tui test passes.
- [ ] `rg 'PitchDetector|OnsetDetector' kazoo-core/src/engine` finds only the judge spawn (paste).
- [ ] `processing.rs` grew by no more than 20 lines (report before/after line counts); the new logic is in `processing/voice.rs`.
- [ ] Not the builder's: after landing, Cassio installs and asks Tom to hum a slide into `kazoo mouth` (still kazoo-tui at this point). The builder lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The output callback (`process_block`) still never allocates, frees, locks or does I/O: the tracker is allocated at engine start, the judge feed is a ring push plus `Thread::unpark`, judge results are `Copy`. No `#[allow]`; the closure captures a local, never `state`. Files: `processing.rs` is already over 1000 lines — never split it; add only the calls named; list it under FOUND with its line count.

## END

```
START KZ-V06 base=<sha> branch=lane/kz-V06 df=<GB> at <time>
END KZ-V06 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked; processing.rs line count before and after
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
