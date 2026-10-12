# KZ-A05: one keyboard map, all three parts in one lane

Tom wants the whole of track A finished by 08:30 on 13 Oct. This lane builds KZ-A05a, then KZ-A05b, then KZ-A05c, in that order, in one worktree. Read all three briefs (`briefs/studio/KZ-A05a-keyboard-map.md`, `KZ-A05b-keyboard-map.md`, `KZ-A05c-keyboard-map.md`) and follow each as written. A04 runs at the same time in another lane and also creates `kazoo-host`. Create `kazoo-host` exactly as A05a describes for that case: the manifest A04 specifies, with `pub mod keys;` in `lib.rs`. Add only the `keys` module; do not write `terminal` or `reports`. Cassio merges the two lanes.

The time box is 25 minutes. Do a fully, then b, then c. Run each part's checks before starting the next, so whatever is finished is green. Say in the END block which parts are done.
