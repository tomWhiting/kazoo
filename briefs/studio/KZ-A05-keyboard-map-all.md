# KZ-A05: one keyboard map, all three parts in one lane

This lane builds KZ-A05a, then KZ-A05b, then KZ-A05c, in that order, in one worktree. Read all three briefs (`briefs/studio/KZ-A05a-keyboard-map.md`, `KZ-A05b-keyboard-map.md`, `KZ-A05c-keyboard-map.md`) and follow each as written. `kazoo-host` is on main already (b670f5e, with `reports` and `terminal`). Add the `keys` module to it (`pub mod keys;` in its `lib.rs`) and touch nothing else in it. A04 part 2 runs at the same time in another lane, moving every program's terminal setup onto `kazoo-host`; it edits the same `run.rs` files in other places. Keep your edits to the key handling, so Cassio can merge the two lanes.

The time box is 25 minutes. Do a fully, then b, then c. Run each part's checks before starting the next, so whatever is finished is green. Say in the END block which parts are done.
