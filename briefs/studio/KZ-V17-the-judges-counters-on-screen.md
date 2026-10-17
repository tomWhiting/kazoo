# KZ-V17: the voice judge's counters reach the screen

Written by Cassio, 17 Oct 2026, about 10:55, and brought up to date about 11:15 after the first Opus review. Decided by Cassio; the reasons are given with each decision. Cassio wrote this lane directly, with no builder.

Place: Track V, after KZ-V16.

Crates:
- kazoo-core: `voice/judge.rs`, `engine/stats.rs`, `engine/handle.rs`, `engine/mod.rs`, and the tests in `engine/judge_shutdown_tests.rs` and `engine/processing/tests.rs`
- kazoo-tui: `status.rs`, `app.rs`, `ui/header.rs`

## Why

The Opus review of KZ-V16 (17 Oct 2026) found that the voice judge counts what it loses, but nothing shows those counts. There are three counters: samples its input ring missed, verdicts the output callback had not drained, and, since V16, pYIN runs that failed. `Judge::stats()` was read only by tests, so a judge falling behind lost pitch with nothing on screen to say so. The engine's other drop counters already reach the TUI's status line and header through `EngineStatsSnapshot`.

## Outcome

- **The judge's counters, shared.** The judge hands out `JudgeCounters`, a cloneable handle on its counters.
  - Reading it takes no lock and allocates nothing.
  - It also keeps the first failed pYIN run's reason, which the worker sets once.
- **Read with the engine's own.** `engine::start` attaches the handle to `EngineStats` (`set_judge`, behind a `OnceLock`, as the mic status and audio health are attached).
  - `EngineStats::snapshot` reads it into three new counters: `judge_samples_dropped`, `judge_results_dropped` and `pitch_runs_failed`. All three count against `is_clean`.
  - `EngineHandle::stats` is the plain snapshot again.
  - `first_pitch_failure` gives the reason.
  - The processing rig attaches its own judge's counters, as the engine does.
- **On screen.** kazoo-tui's counters read "pitch in", "pitch out" and "pyin", placed right after "mic": these change what the synth plays.
  - "pyin" is marked an engine fault, since a validated configuration never fails a run. When it is the worst problem, its message ends with the first failure's reason.
  - The analysis thread's two counters had "pitch" labels, but that thread now feeds only the spectrum and formant display. They are now "scope in" and "scope out", and their docs say so.
- **Tests.**
  - A real judge's ring, offered 4900 samples at once while empty, counts exactly 100 missed, read through `EngineStats`.
  - The counters can be attached only once.
  - The reason is said with "pyin" and with no other counter.
  - The coverage test checks all 20 counters.

## Not in this lane

The mouth runs its own voice judge. Nothing reads that judge's counters, and most of the mouth's own counters in its `Stats` (mic drops and skips, display drops, voice and audio refusals, desk losses) are counted but never shown. KZ-V18 (`briefs/studio/KZ-V18-the-mouths-health-on-screen.md`) gives the mouth a health line of its own, and is the next lane. The offline scorer and the wall's listener treat a failed run as an unvoiced frame. Their reports already show unvoiced frames as missed pitch, and a validated configuration never fails a run.

## Done when

clippy and nextest pass for kazoo-core and kazoo-tui, and an Opus review finds nothing open.
