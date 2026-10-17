# KZ-V18: the mouth says what it loses

Written by Cassio, 17 Oct 2026, about 11:15. Decided by Cassio; the reasons are given with each decision.

Place: Track V, after KZ-V17. Crates: kazoo-mouth (`engine.rs`, `run.rs`, `controls.rs`, `ui`), tests.

## Why

The Opus review of KZ-V17 (17 Oct 2026) found two things about the mouth, which is Tom's first instrument:
- it counts what it loses but shows almost none of it;
- nothing reads its voice judge's counters.

The mouth's `Stats` counts these without showing them:
- mic samples dropped (`run.rs`) and skipped (`engine.rs`);
- display frames dropped;
- voice and audio refusals;
- desk losses.

Only the effect faults reach the screen, through `controls.rs`. The session's `Judge` counts samples missed, verdicts dropped and pYIN runs failed, and nothing reads them. A judge falling behind is heard as pitch that lags or drops out, with nothing on screen to say why.

## Outcome

- The mouth shows a health line the way kazoo-tui's engine header does: each non-zero counter by a short name, worst first. When a counter rises, the status line says what was lost and how much.
- The counters are:
  - every loss counter in the mouth's `Stats`;
  - the session judge's three counters, read through `Judge::counters()` (KZ-V17);
  - the first pYIN failure's reason, said with its counter.
- Reading the counters takes no lock and allocates nothing on the audio thread. The UI thread reads them when it syncs stats.
- Tests:
  - every counter is covered, with a coverage test like kazoo-tui's;
  - a rise is reported once;
  - the judge's counters reach the line.

## Done when

clippy, nextest, fmt and ast-grep pass for kazoo-mouth, and an Opus review finds nothing open.
