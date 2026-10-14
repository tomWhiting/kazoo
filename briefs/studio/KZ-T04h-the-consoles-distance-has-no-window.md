# KZ-T04h: the console works out how far a moment is from the wall by walking limbs, as kazoo tree does, with no window of newest moments

Line numbers are from main at `55efc8f` and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 07:0x, from the Opus review of KZ-T04g (finding 3). Decided by Cassio; the reason is beside each decision.

Place: Track T. Time: about 30 minutes. Crates: kazoo-wall (`tree/thread/here/ancestry.rs`, `tui/app/tree_distance.rs`, the tree listing's protocol and their tests). Depends on KZ-T04g (landed).

## Why

KZ-T04g's console tree list says how far each moment is from the wall. At first it shipped the tree's whole ancestry to the console on every listing: about 680 KB per listing, 0.15 s on the tree thread, about once a second. The review cut that to the newest 4096 moments. A moment further back now reads "further from the wall than the tree counted".

Tom wants no limits on the tree ("we shouldn't limit the number of branches if we can avoid it, I don't want them being dropped"). A distance that stops being known past a window is the kind of limit he means. It can be avoided. kazoo tree already does this in O(limb depth) (`kazoo-tree/src/history.rs`, `History::path_between`, as landed in 6e52b78): it lists the wall's limb chain as (limb, position) pairs up to the root, then walks the other moment's limbs up until it meets that chain. Back and on are then differences of positions.

## Outcome

1. **The listing carries limbs, not ancestry.** Each row carries its limb and its position on it. The listing carries each limb's `from` (its limb and position) once. The console works out the distance the way `path_between` does. Its size grows with the number of rows and limbs, not with the tree's depth.
2. **No window.** Every moment listed has a distance. The "further from the wall than the tree counted" wording and the 4096 constant are removed.
3. **The same words as kazoo tree**, as T04g made them. A test reads both on the same history and compares them.

## Tests (write them first; each fails before the change)

- On a tree of 10,000 moments on the trunk plus a bud, the oldest trunk moment's distance from a wall on the bud is exact: back to the fork, then on.
- A listing's encoded size does not grow when the trunk grows from 1,000 to 100,000 moments with the same rows shown and the same limbs.
- The console's words for five moments match `kazoo_tree`'s `path_between` words on the same history: the wall's own, back on its limb, on along a bud, across a fork, and on no path.
