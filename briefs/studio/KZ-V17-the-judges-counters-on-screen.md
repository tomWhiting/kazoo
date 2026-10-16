# KZ-V17: the voice judge's counters reach the screen

Written by Cassio, 17 Oct 2026, about 11:50. Decided by Cassio; the reasons are given with each decision. Cassio wrote this lane directly, with no builder.

Crates:
- kazoo-core: `engine/stats.rs`, `engine/handle.rs`
- kazoo-tui: `status.rs`

## Why

The Opus review of KZ-V16 (17 Oct 2026) found that the voice judge counts what it loses, but nothing shows those counts. There are three counters: samples its input ring missed, verdicts the output callback had not drained, and, since V16, pYIN runs that failed. `Judge::stats()` was read only by tests, so a judge falling behind lost pitch with nothing on screen to say so. The engine's other drop counters already reach the TUI's status line and header through `EngineStatsSnapshot`.

## Outcome

- `EngineStatsSnapshot` gains three counters: `judge_samples_dropped`, `judge_results_dropped` and `pitch_runs_failed`.
  - `EngineHandle::stats` fills them from the engine's judge.
  - `EngineStats::snapshot` leaves them zero, because the judge keeps its own counters.
  - All three count against `is_clean`.
- kazoo-tui's `COUNTERS` gains "judge in", "judge out" and "pyin", so a rise is reported on the status line and the total stays in the header, like every other counter. The coverage test now checks all 20.

## Not in this lane

The mouth runs its own judge and keeps counters in its own `Stats`. Most of those counters (mic drops, display drops, refusals) are not shown either, so the mouth needs its own health display, written as a lane of its own. The offline scorer and the wall's listener treat a failed run as an unvoiced frame. Their reports already show unvoiced frames as missed pitch, and a validated configuration never fails a run.

## Done when

clippy and nextest pass for kazoo-core and kazoo-tui, and an Opus review finds nothing open.
