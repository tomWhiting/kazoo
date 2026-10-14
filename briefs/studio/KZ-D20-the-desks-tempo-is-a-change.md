# KZ-D20: the desk's tempo is kept with the wall: when the desk moves the tempo, the wall logs it as a change (so every moment's tempo is what was played), and a go with the desk linked asks the desk for the moment's tempo

Line numbers are from main at `eab16dd` and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 00:4x, from an Opus agent's diagnosis of Tom's report (the same as KZ-T11a's). Decided by Cassio; reasons beside each decision.

Place: Track D. Time: about 40 minutes. Crates: kazoo-wall (`daemon/wall.rs`, the desk link's tempo path, `change.rs`, `protocol/mod.rs` if a `What` is added, and the tests). Runs after KZ-T11a (both touch `wall.rs`).

## Why

Tom, 15 Oct 2026, about 00:40: going between moments, "it doesn't come back quite sounding the same." One cause: moment 6904's `wall.toml` says 96 BPM, but change 6905's own before-value says the wall was really at 120: the desk had moved the tempo and the wall logged nothing. The tree added a correction commit, `7002f61` (Kazoo-Seq 6904, 120 BPM), whose files are otherwise the same as 6904's. So Tom's arrow steps played 96, 120, 96, 110 BPM between moments that differ by one knob or nothing.

And with the desk linked, a go keeps the desk's tempo, not the moment's (`playing_after`, `wall.rs:3845`; `swap_engine` sends `Command::Tempo` only when not on the desk, `wall.rs:1612`), so one moment plays at whatever tempo the desk has.

## Outcome

1. **The desk's tempo is a change.** When the wall, linked to the desk, hears the desk's tempo move (by more than 0.05 BPM from what the patch holds), it sets the patch's tempo and logs a change by the seat `desk`, `What::Tempo` with the before and after values, summary `the desk moved the tempo to {bpm}`. A run of moves within 2 seconds (a fader being dragged) logs one change, with the first before and the last after, written when the run ends. Decision: one change per gesture, as the console's knob turns are; the tree then holds a moment at the tempo that was played.
2. **A go with the desk linked asks the desk for the moment's tempo**, as `Tom asked the desk for {bpm}` does now (`change.rs:297`), timed with T11a's landing so the tempo and the wall change on the same bar. If the desk refuses or does not answer within 1 second, the go still lands at the desk's tempo, and the go's answer says so (`the desk kept 120 BPM; this moment was made at 96`).
3. **No double logging.** A tempo the wall itself asked the desk for (a seat's `wall_tempo`, a go) is not logged a second time when the desk echoes it.

## Tests (write them first; each fails before the change)

- The desk moves 120 → 96 in one step: one change by `desk`, before 120, after 96; the next commit's `wall.toml` says 96.
- A drag 120 → 100 over 1.5 seconds in 30 messages: one change, before 120, after 100.
- A seat's `wall_tempo` 110 echoed by the desk: one change (the seat's), none by `desk`.
- A go to a 96 BPM moment with the desk at 120: the desk is asked for 96 at the landing bar; with a desk that refuses, the go lands and its answer names both tempos.

## Amended 15 Oct 2026, 02:3x (Cassio), after KZ-D17 landed (`01d544f`)

KZ-D17 already logs desk tempo moves: in `daemon/wall/tempo_sync.rs`, `sync_desk_tempo` logs them under seat `desk` as `What::Tempo { desk: true }`, with the summary "the desk set the tempo X → Y" and a threshold of 0.01 BPM. It also already keeps the asks the wall made (`desk_tempo.asked(bpm)`, at most 8), so an echo is not logged twice. This lane changes that one path and adds no second path:
- **Threshold:** 0.05 BPM (this brief's), replacing D17's 0.01. Change D17's tests that pin 0.01, and say so.
- **Summary:** "the desk moved the tempo to {bpm}" (this brief's), replacing D17's wording everywhere it is read (the MCP's renderers, the tree's lanes, and the console's log). Find every reader by the old wording, and test one.
- **Grouping (Outcome 1):** goes inside `sync_desk_tempo`. The desk moves that a seat's ask reveals first (D17 logs any move it had not yet heard before an ask) close the open run before the ask is logged.
- **Outcome 2's ask on a go** must call `desk_tempo.asked(bpm)`, or the desk's echo is logged as a desk change (D17 review, finding 9).
- **The status line:** a lasting failure to save the tempo takes the desk's status line until a save succeeds, and hides other hub notices meanwhile (`kazoo-mix/src/run.rs:221`). Show the save failure beside the hub notice, not instead of it. Test: a hub notice during a save failure is shown.
