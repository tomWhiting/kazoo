# KZ-D19: the wall's guess at what a wall will cost holds on a small wall, a go to a moment of the tree is never refused for it, and a go to a tree moment keeps no extra save

Line numbers are from main at the commit that adds this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 00:1x. Decided by Cassio; reasons beside each decision.

Place: Track D (the wall's deadline, after D15). Time: about 40 minutes. Crates: kazoo-wall (`daemon/wall/cost.rs`, `daemon/wall.rs`, their tests). Runs after KZ-T04d (landed). Runs beside KZ-P11b (amended 15 Oct 07:5x): P11b changes one row of `cost.rs` and the two test helpers below, nothing else here. So this lane's first step is the same helper pin P11b makes, and whichever lands second merges identical lines.

**First step (amended 15 Oct 07:5x, from the P11b fix decision 6):** in `kazoo-wall/src/daemon/wall/tests/cost.rs`, `heavy()` returns `Kind::from_name("screamer")`, and `audio_kind(false)` returns, by name, the kind it picks today (print it once before the change and put it in the END). Each keeps its assertion that the kind has an audio input and output. Reason: the fixtures' arithmetic was built on screamer's row (two screamers and an `out` are 0.2992 of a 48 kHz block). With `heavy()` following "the dearest row", any new or re-measured effect would move this lane's numbers under it, so the before and after this brief asks for would not compare like with like.

## Why

Tom, 14 Oct 2026, 23:59 to 00:05, walking the tree with `g`: "it was changing around, and then it... wasn't changing as much, and then like now when I move around, it's not changing it at all... I really don't think it's machine load... if it is a CPU thing, that absolutely shouldn't be the case." The machine has run at far higher load every night he has played.

What happened: each go is checked by `load_room` (`cost.rs`, `load_projection`). It costs every heard module of the incoming wall at its bench share times `busy()`, the current wall's measured render load over its modules' bench shares, held to 1 to 8. On a small wall the measured load is mostly fixed work (the engine, the sink, the output) that is not in any module's bench share, so `busy()` comes out large (a 9-module wall gave about 4.7 to 8), and every incoming module is costed that many times over. A 141-module wall that had played all evening was refused at "about 133%". Each go to a smaller wall raised the factor, so the walls that could be put up ratcheted down to nothing. Tom's tree now goes as his console (whose loads only warn), so he is unblocked; every seat (the MCP's `wall_go` once T04c/T04d land, `wall_load`) still meets the wrong guess.

Also: every go saves the wall as it was (`before-go-…`), and walking the tree sends a go every second or two (tonight, 30 in a minute). A wall that is already a moment of the tree needs no save to come back: the tree holds it.

## Outcome

1. **The busy factor counts the fixed work apart.** Measure the engine's render load with no modules heard (the fixed cost: take it from the engine's own timing of its non-module work, or from the bench row for the engine and sink, whichever the code already has; say which) and compute `busy` as `(load − fixed) / bench`, held to 1 to 8, and 1 when `load − fixed` is not above zero or the bench is below `BUSY_MIN_BENCH`. The projection then adds `fixed` once, not per module. Decision: the guard keeps its purpose (stopping a seat from overloading the machine), but its arithmetic stops multiplying overhead by module count.
2. **A go never fails for the projection.** A go (`Request::Go`) by anyone is put up whatever the projection says, as a console's load is now (`load_room_as`, the console branch): it is a moment the wall has already played. When the projection passes the guard, the answer's notes say so (the console's wording, "about N% of the wall's time…"). The guard still refuses additions while the wall is actually over it (`render_room`, unchanged), so a heavy moment cannot then be made heavier. `wall_load` of a save by a seat keeps today's refusal, with the corrected factor.
3. **A go from a tree moment keeps no save.** When the wall as it is is exactly a moment of the tree (the tree's HEAD names it and nothing has changed since), a go saves nothing and its answer's `backup` names that moment (`tree:<12-char id>`) instead of a save; undoing the go goes back to that moment. Otherwise it saves as today.

## Tests (write them first; each fails before the change)

- A 9-module wall with a fixed cost of 30% of its load: `busy()` is within 10% of what the same modules give on a 141-module wall measured on the same machine (fixtures: set the engine's measured load and fixed cost directly, as `tests/cost.rs` does).
- From a 9-module wall, a go to a 141-module moment whose corrected projection is under the guard is put up with no note; one whose projection is over is put up with the note; neither is refused, for a console and for a seat.
- A seat's `wall_load` over the guard (corrected) is still refused with today's wording.
- Two goes in a row from tree moments add no save to the store; a go after an unsaved change adds one; undoing a no-save go puts the wall back at the moment named in `backup`.
- The existing cost tests still pass, or each change to one is explained in the END block with the number before and after.

## Amended 15 Oct 2026, 02:3x (Cassio)

4. **A wake is costed the same way.** Waking modules that are on the wall but silent (patching a sleeping seq into a heard path) goes through the same projection. Tonight it refused to wake 17 seqs at once, projecting 30% → 95%. Woken a few at a time, the measured load stayed at 29 to 33%. Each sleeping module is costed at its bench share times the corrected `busy`, never more. A test wakes 17 sleeping `seq`s on a wall measured at 30% with seq bench shares from the table, and the projection stays under the guard.
5. **Reserved seat names hold in any case.** KZ-D17 made the wall refuse seats named `desk` and `kazoo-wall`, but the check is case-sensitive (`daemon/control.rs`, the seat name check), so `Desk` or `KAZOO-WALL` gets through and can pose as the desk in the log. Compare without case, test it with three casings, and keep the refusal's wording.
