# KZ-T05f: a crossing takes as long as the moments are far apart, and between branches it follows the tree, down to the fork and back up; a walk of goes is told to the seats as one line

Line numbers are from main at the commit that adds this brief and may have moved; find each item by what it does, never rename. Names from KZ-T05a to T05e are as those briefs name them (`DEFAULT_CROSS_BEATS`, `MAX_CROSS_BEATS`, `CrossPlan`, `crossing_beats`, `cross`); use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall -p kazoo-tree`.

Written by Cassio, 15 Oct 2026, 00:2x. Decided by Cassio; reasons beside each decision. **This amends T05b to T05e:** where they fix a crossing at `DEFAULT_CROSS_BEATS` (16), a go that names no beats now takes the length below. A go that names its beats (a seat's `beats`, the console's `[` `]`) keeps them.

Depends: KZ-T05b (the crossing) has landed. Time: about 50 minutes.

## Why

Tom, 15 Oct 2026, about 00:20, walking the tree with the arrows (KZ-tree's follow): "it's meant to sort of like transition between things. It's currently just skipping completely. I think the transition should be based on how far apart the changes are... how long it takes to transition should be based on... how far the change is. Maybe even when going between branches, follow the tree back down and back over." And: "it should probably not give you updates for every single one of those things."

## Outcome

1. **How far apart two moments are.** `distance(a, b)` is the number of moments on the tree's path between them: from `a` down to their last common moment and up to `b` (along one limb, the count between). The tree gives it from the commits (`git rev-list --count a...b` with `--left-right`, or the wall's own tree thread's index; say which).
2. **The crossing's length follows the distance.** With no beats named: `beats = clamp(2 · log2(1 + distance), 1, MAX_CROSS_BEATS)` (1 change away: 2 beats; 15 away: 8; 255 away: 16). Decision: a log keeps a near step quick and a far journey long without a jump across the whole tree taking minutes.
3. **Between branches, the crossing follows the tree.** When `a` and `b` are on different limbs, the crossing goes in up to three legs: from `a` to their fork moment, then from the fork to `b`, each leg one crossing (T05b's machinery) whose length is given by its own distance (point 2), the second starting as the first ends, so the wall sounds the way the tree is shaped. A leg of distance 0 is skipped. A go that names its beats splits them over the legs in proportion to their distances.
4. **A go mid-crossing turns, not cuts.** A go while a crossing runs starts from where the wall is (the knobs where they have glided to), not from the crossing's end: the new crossing's plan is made from the wall as it sounds now. Decision: walking the tree with the arrows sends a go every second or so; each must bend the path, never jump.
5. **A walk is one line to the seats:** split out to KZ-T05g (kazoo-mcp only), to land at once. Not in this lane.
6. **The tree shows the crossing.** While a crossing runs, kazoo tree draws a moving ✦ along the path it follows, from the moment it left toward the one it is going to, at the crossing's pace (it reads the crossing's start, legs and beats from the wall's answer to the go, or from `wall_look`; say which).

## Tests (write them first; each fails before the change)

- `distance` along one limb, across a fork, and 0 for the same moment.
- With no beats named: 1 change away crosses in 2 beats, 15 in 8, 1,000 in 16; named beats are kept.
- Between two buds of one fork: the crossing has two legs that meet at the fork moment (the knobs at the leg boundary equal the fork's), lengths per point 2.
- A go during a crossing starts from the glided values (a knob halfway through its glide starts the new crossing from halfway).
- kazoo tree: during a crossing the ✦ is drawn between the two moments, and is gone when it ends.
