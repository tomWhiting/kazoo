# KZ-T05g: a walk through the tree is told to the seats as one line, not one notification a go

Line numbers are from main at `dfbdb64` and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-mcp`.

Written by Cassio, 15 Oct 2026, 00:5x. Decided by Cassio; reasons beside each decision. Split out of KZ-T05f (its Outcome 5), which no longer carries it, so it can land at once: it touches kazoo-mcp only.

Place: Track T. Time: about 30 minutes. Crates: kazoo-mcp (`src/channel.rs` and its tests). No dependency; runs beside anything that does not touch kazoo-mcp.

## Why

Tom, 15 Oct 2026, about 00:52, walking the tree with the arrows (a go every second or less): "we really need to do something about that... the things giving you all those notifications... if I'm skipping around, you're gonna get thousands of those... everybody who's got it is gonna get thousands of those things." Tonight 22 goes in 22 seconds gave every seat 22 notifications.

Cause: a go is a whole-wall replacement, `Event::Loaded`, and `News::take` (`channel.rs:158-167`) sets `urgent`, which sends at once (`due`, `channel.rs:242-245`), with the go's change line beside it. Every go of a walk is therefore its own notification to every seat.

## Outcome

1. **A walk is one line.** Goes (`Event::Loaded` whose change is a `What::Go`, by a seat other than this one) are folded, per seat that made them, until that seat has made no go for `WALK_REST` = 5 seconds: then one line, `Tom walked the tree: 22 moments, now at the moment after change 6825 (140 modules, 189 cables, 96 BPM)`, using the last go's own summary words for where it now is. A single go alone reads as today (`Tom went to …`), told when it has rested.
2. **The first go of a walk still says at once that the wall is stale**, one line: `Tom is moving the wall through the tree: what this seat knew of it is stale` (urgent, as today). Later goes of the same walk add no notification until the walk's line. Decision: a seat must know at once that its view is stale (its changes are refused with `wall_replaced` until it looks again, so it is never unsafe, only wasteful); after that, more of the same says nothing new.
3. **Everything else is unchanged:** a `wall_load` of a save is told at once as today; faults stay urgent; other changes keep the 20-second window; a walk's line goes out in the window's next notification, or alone if nothing else is due. A go by this seat itself is never news to it (as today).
4. **Nothing is lost:** every go is still in the wall's log and `wall_log` lists them one by one; the folded line says how many.

## Tests (write them first; each fails before the change)

- 22 goes by Tom one second apart: one urgent notification at the first (the stale line), then exactly one more 5 seconds after the last, `Tom walked the tree: 22 moments, now at …` naming the last go's place; nothing in between.
- One go alone: one urgent stale line at once, then `Tom went to …` after 5 seconds (or the go's line in the stale notification itself; say which and test it).
- Goes by Tom and by Cally interleaved: one walk line each.
- A `wall_load` of a save between goes is told at once, as today, and splits the walk in two.
- A seat's own goes give it nothing.
