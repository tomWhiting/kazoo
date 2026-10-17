# KZ-V18: the mouth says what it loses

Written by Cassio, 17 Oct 2026, about 11:15. Decided by Cassio; the reasons are given with each decision.

Place: Track V, after KZ-V17. Crates: kazoo-mouth (`health.rs`, `engine.rs`, `run.rs`, `controls.rs`, `ui.rs`, tests) and kazoo-core (`voice/judge.rs`: `Judge::is_finished`; `ipc/link/voice.rs`: `HubLinkAudio::carries_voice`).

## Why

The Opus review of KZ-V17 (17 Oct 2026) found two things about the mouth, which is Tom's first instrument:
- it counts what it loses but shows almost none of it;
- nothing reads its voice judge's counters.

The mouth's `Stats` counts these without showing them:
- mic samples dropped (`run.rs`) and skipped (`engine.rs`);
- display frames dropped;
- voice and audio refusals;
- desk losses.

The effect faults and refusals are read into the app (`controls.rs`) but never drawn either. The synth fault is the one that is seen: it ends the session with "mouth synth refused its parameter table; output muted" (`run.rs`), and V18 leaves that as it is. The session's `Judge` counts samples missed, verdicts dropped and pYIN runs failed, and nothing reads them. A judge falling behind is heard as pitch that lags or drops out, with nothing on screen to say why.

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

## As built (17 Oct 2026), after the first Opus review

- **A row of its own.** The health row sits in the header, straight after the title, so a short screen cuts the link, mic and latency rows first. It is wrapped to the width as the header is, red while a loss is rising and amber once settled. It never shares the footer or the message slot, so it neither hides a message nor is hidden by one, and the footer's keys stay whole at 80 columns. When a long reason wraps to several rows, the panels below move down while the rise is said and back when it settles; at 80 by 24 everything still fits.
- **One rise at a time.** A rise is said for ten seconds (`RISE_LIFETIME`). Later rises in that time add to it ("40 mic samples dropped"), and the ten seconds start again from the last one. When more than one counter rose, the totals ride along ("… (+2 others) · lost so far: …"), so the others stay in sight however long the losses keep rising. After that the row holds the totals ("lost so far: mic 28 · pyin 1"), worst first.
- **The voice judge.** A judge whose thread has stopped is said first, in red, for as long as the mouth plays, and the end-of-session report says so too. The first pYIN failure's reason is said with a failed run, after it when it is the worst, at the end when it is not.
- **The order.** What is heard comes before what is seen:
  1. mic, mic skip: a gap in the sound and in the judge's input both;
  2. desk audio: the desk link's dropped blocks, heard as silence when on the desk;
  3. pitch in, pyin, pitch out: the judge;
  4. fx: effect faults;
  5. desk msgs, desk voice, desk gone, desk sync: what only the desk misses;
  6. fx refused, screen.
- **Counted only when it is a loss.**
  - **"desk voice"** counts a frame only when the desk is connected and carries the voice (`HubLinkAudio::carries_voice`) and its queue had no room. Playing direct, or to a desk too old for the voice, is not a loss; a test plays direct and counts none.
  - **"desk audio" and "desk msgs"** are the link's own `blocks_dropped` and `messages_dropped`, read from `LinkStatus` once a frame on the UI thread. That read takes the link's status lock; the audio thread takes none. A lost backlog counts once, and the label says so.
  - **"desk gone"** is what the engine counted as "audio refused": a block sent as the desk went away.
  - **"fx refused" and "pyin"** are marked engine faults.
  - **Grammar.** Each counter has a singular and a plural.
- **Nothing missed.** `Losses::read` names every field of `Stats`, so a counter added there must be decided on. A test sets all 13 counters and checks the totals' exact order.
- **At the end.** After the session stops (the judge joined, the desk read just before the link goes), the session's losses are said on stderr once the terminal is back. So are the link's exit report and the loop writers' messages: written before, while the screen was still up, they vanished with it.
- **Smaller.**
  - The mic callback's drop count uses `saturating_sub`.
  - `MESSAGE_LIFETIME` is named.
  - The latency read leaves the event loop.

## Done when

clippy, nextest, fmt and ast-grep pass for kazoo-mouth, and an Opus review finds nothing open.
