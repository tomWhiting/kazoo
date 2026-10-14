# KZ-T04g: the console's tree list goes on Enter, `:go` offers the tree at once, and the daemon's tests never touch a real tree

Line numbers are from main at `f44489e` and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block follow PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio on 15 Oct 2026 at 02:2x, from the Opus review of KZ-T04d (findings 4 and 5) and Tom's ruling at 02:05. Cassio made the decisions; the reason is beside each one.

Place: Track T. Time: about 35 minutes. Crates: kazoo-wall (`tui/app/tree.rs`, `tui/app/command.rs`, `tui/draw/tree.rs`, `tree/git.rs`, the daemon test support, and their tests). Depends on KZ-T04d, which has landed.

## Why

Tom, 15 Oct 2026, about 02:05, on the tree: "the arrow keys moving them around probably shouldn't move you straight away. I think like move the arrow keys plus enter kind of thing. And as you move along... you get a little bit of information on it."

kazoo tree now works that way: arrows look, and Enter goes. The console's tree list (`B`) asks a second question before going: Enter opens "go to …?" and `y` sends the go. Earlier, Tom said a walk "with auto confirmation" was what he wanted. A go can always be undone with `z`. So the extra question is a step he did not ask for, and the two views should behave the same.

## Outcome

1. **Enter goes.** In the tree list, Enter sends the go for the row under the cursor straight away, with the same request and wording as `y` gives now. The "go to …?" question is removed from the list. It stays on `:go <place>` typed at the command line, where a typo is likelier. The key line (`draw/tree.rs`) reads `j k move · ⏎ go there · z undo · Esc closes`.
   - Decision: one keypress, as in kazoo tree. `z` is the safety net.
2. **Moving says where you are.** As the cursor moves, the list's status line says how far the row is from the wall, in the same words as kazoo tree. Examples: "3 changes back from the wall", "12 changes from the wall: 4 back to where they part, 8 on", "the wall is here". Count from the parents the TreeResult carries. If it does not carry them, add the parent commit to each row; that is a one-field change on both sides, so say so.
3. **`:go <Tab>` offers the tree at once.** Opening `:` fetches the tree as well as the saves, so the first Tab after starting offers moments without `B` having been opened first. Update the test at `tui/tests/command.rs:41`, which pinned the old behaviour, to the new one, and say so.
4. **The daemon's tests never touch a real tree.** The daemon's test support sets the tree's folder and the reels folder to the test's own temporary folder. That means `KAZOO_TREE_DIR` and the reels root `~/.kazoo/reels` (`tree/git.rs:177`). This holds whatever the environment of the person running the tests says. A test proves it: with `KAZOO_TREE_DIR` set to a folder outside the test, a go and a mark leave that folder untouched.

## Tests (write them first; each must fail before the change)

- In the tree list, Enter on a row sends that row's go. No question is asked, and `y` and `n` do nothing in the list.
- Moving the cursor sets the status line to the distance words above, for a row behind the wall, one ahead on a bud, and the wall's own row.
- After a fresh start, `:go ` then Tab offers a moment, with `B` never opened.
- With `KAZOO_TREE_DIR` pointing at a sentinel folder, the daemon test support's go and mark leave the sentinel folder empty.
