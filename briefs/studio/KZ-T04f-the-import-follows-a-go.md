# KZ-T04f: `kazoo wall tree import` follows a go: the wall it puts up is the moment's own wall, the line grows on as a bud from that moment, and an undone go comes back from its backup, so a re-planted tree is the tree the wall grew

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Place: Track T. Written by Cassio, 14 Oct 2026, 22:1x, from the Opus review of KZ-T04b (finding 7). Decided by Cassio.

Time: about 40 minutes. Crates: kazoo-wall (`tree/rings.rs`, `tree/rings/evidence.rs`, `tree/rings/model.rs`, their tests).

## Why

KZ-T04b is the first lane that writes real `What::Go` changes. The import (`kazoo wall tree import`, KZ-T03) replays the wall's log into a fresh tree. It handles `What::Load` by reading the save that went up (`rings.rs`, `fn change`, the `What::Load` arm; `evidence.rs`, `incoming`), but sends `What::Go` to `applied`, which marks the model unknown. So re-planting a tree from any wall that has gone back once stops following the wall exactly from that change on, and every later moment is missing from the re-planted tree. Tom's rule is that nothing is lost.

## The design (decided; build exactly this)

1. **The importer keeps where each change went.** `seq_commit: HashMap<u64, String>`: each change's seq to the commit the import wrote for it (after `commit`), and `branch_of: HashMap<String, String>` from commit to the branch it was written on. Reason: a go names its moment by `to.change` (and `to.commit`, which is the *original* tree's commit and may not exist in the new one); the seq is the same in both.
2. **A go's incoming wall is the moment's own.** In `fn change`'s match, a `What::Go { to, .. }` arm: with `to.change = Some(seq)` held in `seq_commit`, the incoming wall is that commit's tree, read as the import reads a kept file (`git show <commit>:wall.toml` and `modules/`, through the tree's own `git_command`, as the importer already reads blobs), adopted with `Found::Load` so births are kept as a load's are. The commit's module, cable and tempo counts must equal the go's `modules`, `cables`, `tempo`; a difference is the model unknown, with the reason "the wall change <seq> went to change <to> but the moment's wall was not what the go put up (<what differed>)".
3. **The line grows on as a bud.** After a go, the importer's next commit grows a branch named as the live writer names one (`Writer::grow`, `tree/thread/here.rs`): `refs/heads/bud/<from>/<time>`, `<from>` the go's `to.mark` as its ref part, or the first 12 hex of the commit it grew from, `<time>` the go's `at` compacted (`compact_time`), with `unused` for a taken name. The go's own commit is the bud's first commit, its parent that moment's commit. HEAD follows the line, as the live writer's does.
4. **Neither held nor provable is said, not guessed.** With `to.change` `None`, or a seq the import never wrote (a go to a moment before the log begins), fall back to the load's proof path (`incoming`, the next kept file); when that fails too, the model is unknown with the reason "the wall change <seq> went to a moment the import does not hold (<to>)", as for a load that was not kept.
5. **An undone go comes back from its backup.** A change whose `undoes` is a go is handled as an undone load is today (the backup save, `incoming`'s undone-load path), and the line goes back to the branch the go left (`branch_of` of the go's `from.commit`'s new commit, through `seq_commit` of `from.change`); when that cannot be found, the line goes on from the backup as a new bud, and the gap lines say so.

## Red tests first

In `tree/rings/tests.rs` (or the file the import's tests live in), each fails on main first:

1. `an_imported_go_grows_a_bud_from_its_moment`: a log with 10 changes, a go to change 4, then 3 changes: the new tree has `trunk` with 10 commits and one bud of 4 (the go and 3), grown from the commit for change 4; the wall at the bud's first commit has exactly change 4's modules, cables and tempo; the model stays known to the end.
2. `an_imported_go_then_undo_comes_back_to_the_trunk`: as 1, then an undo of the go and 2 changes: the 2 grow on `trunk`.
3. `a_go_to_a_moment_the_import_does_not_hold_is_said`: a go with `to.change` before the log's first change and no kept file after it: the model is unknown from that change, with the sentence of point 4.
4. `the_imported_tree_matches_the_live_tree`: drive a `Wall` with a tree attached (as T04b's tests do) through changes, a mark, a go and more changes; import the same wall's log into a second tree; both trees have the same branches by shape (names may differ only in `<time>`'s collision suffix), the same commit count per branch, and the same `wall.toml` and `modules/` at every commit.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 4 tests fail first (RED), then pass (GREEN).
- [ ] A go and an undone go are followed; nothing is guessed, and what cannot be proved is said.
- [ ] Every check clean.

## END
