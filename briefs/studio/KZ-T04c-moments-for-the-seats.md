# KZ-T04c: every Claude seat can mark the moment, go to any moment, and read the tree, with three MCP tools that say plainly what happened and how to undo it

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Third of PLAN.md's T04 split in four (T04a the tree thread, T04b the wall, **T04c** the seats' tools, T04d the console's keys).

Read by Cassio, 13 Oct evening. Accepted as written.

Time: about 25 minutes of a builder's work. One mechanism: three tools over T04b's requests. Crates: kazoo-mcp. Depends: T04b (landed): `Request::{Mark, Go, Tree}`, `MarkResult`, `TreeResult`, `LoadResult` for a go, `Event::Marked`.

## Outcome

`wall_mark`, `wall_go` and `wall_tree` are tools every seat has. `wall_mark` names the moment the wall is at. `wall_go` puts the wall at a moment (a mark, a branch, `#<change>` or a commit id) and answers like `wall_load`: the change number, what it did, that every module is new and `wall_look` comes first, how `wall_undo` brings the wall back, and that the next change grows a bud. `wall_tree` lists where the wall is, the branches, the marks and the latest moments, in plain lines, each with what `wall_go` takes to reach it. The server's instructions say what the tree is in three sentences.

## The design (decided; build exactly this)

### Arguments (`tools.rs`, beside `SaveArgs` and `LoadArgs`, `:225`, `:235`; all `#[serde(deny_unknown_fields)]`)

```rust
/// `wall_mark`.
pub struct MarkArgs {
    /// The mark's name: 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a
    /// letter or digit, e.g. `dawn` or `waffles low end 2026-10-13`. Not one
    /// already marked (any case): marks never move.
    pub name: String,
}
/// `wall_go`.
pub struct GoArgs {
    /// The moment: a mark's name, a branch (`trunk`, or a `bud/...` name as
    /// wall_tree lists it), a change as `#<number>` (the wall just after that
    /// change, from wall_log), or a commit id (7 to 40 hex digits).
    pub moment: String,
}
/// `wall_tree`.
pub struct TreeArgs {
    /// How many of the latest moments on the wall's line to list: default
    /// 20, at most 200.
    #[serde(default)]
    pub recent: Option<u32>,
}
```

### The tools (`tools.rs`, in the router beside `wall_load`, `:649`)

- `wall_mark`: `self.read::<MarkResult>(Request::Mark { name }, |r| Ok(render::marked(r)))`. Description, exactly: "Name the moment the wall is at (just after its latest change) so anyone can come back to it with wall_go: 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a letter or digit, e.g. dawn. A mark is a name in the wall's tree (a git tag, mark/<name>); it never moves and is never deleted, and the sound does not change. A name already marked (any case) is refused (mark_exists). Every seat is told. A seat may mark three times a minute (slow_down beyond), and not once the tree holds 1,000 marks (full)."
- `wall_go`: as `wall_load` (`:652`): `ask::<LoadResult>(Request::Go { moment })`, `self.own(result.change.seq)`, `render::went(&result)`. Description, exactly: "Put the whole wall at a moment from its past while it plays on: a mark's name or a branch from wall_tree, a change as #<number> from wall_log (the wall just after it), or a commit id. It is a cut: the master fades out and back in over a few hundredths of a second. Nothing in the past changes: the wall as it is is kept first (as before-go-<time>), the go is one change, and wall_undo with its number brings the wall back exactly to where it was. The first change anyone makes after a go grows a bud, a new branch named for the moment and the time; with no change, nothing grows. Every seat is told. Call wall_look afterwards: every module and cable is new, and changes are refused (wall_replaced) until this seat has looked. A seat may replace the wall (a load, a go, or undoing one) three times a minute (slow_down beyond)."
- `wall_tree`: `self.read::<TreeResult>(Request::Tree { recent }, |r| Ok(render::tree(r)))`. Description, exactly: "The wall's tree, where every change is a moment: where the wall is (its line and the moment), every branch (trunk and the buds, newest first, each with the moment it grew from), every mark and every save's mark, and the latest moments on the wall's line (recent: default 20, at most 200), each with its change number, who, when (UTC) and what. Anything listed is a moment wall_go takes. Anyone may ask, at most ten times in a few seconds."
- The instructions (`:57`) gain, after the saves bullet: "- The wall's past is a tree (git): every change is a moment, and nothing in the past ever changes. wall_tree shows the branches, marks and recent moments; wall_mark names the moment the wall is at; wall_go puts the wall at any moment, and the next change after that grows a new branch (a bud) from there."
- `TOOLS` (`:747`) becomes 22 long, sorted, with the three names.

### What they say (`render/tree.rs`, new; `mod tree; pub use tree::{marked, went, tree};` in `render.rs` beside `mod unheard;`, `:1935`)

- `marked(&MarkResult)`: `Marked the moment the wall is at as "<name>" (<tag>, <place>). wall_go with "<name>" puts the wall back here.` where `<place>` is `after change #<n> on <branch>` / `after change #<n>, off any line` / `moment <short>` by what `at` holds.
- `went(&LoadResult)`: the load's sentence, `render::loaded` (`render.rs:620`), then: ` The next change anyone makes grows a bud: a new branch from this moment.` For a go that undid a go (`change.undoes` is set) instead: ` The wall is back on the line it left.`
- `tree(&TreeResult)`, lines, each moment as `#<n> <short> <at> <seat>: <summary>` (`#<n>` left out when `change` is `None`; summaries through `channel::clean`, `channel.rs:301`, as every summary a seat reads):
  ```
  The wall is on <branch> at <moment>.                 | The wall is at <moment>, off any line<, reached by mark "<m>">: the next change grows a bud.
  The tree has every change up to #<through> (the wall's latest is #<revision>).   [or: "The tree is <n> changes behind the wall." when through < revision]
  Branches (<branch_count>):
  - <name><" (grew from <grew_from>)">: <moment>
  Marks (<mark_count>):
  - "<name>" (<mark | save>): <moment>
  Recent on this line (<n>):
  - <moment>
  wall_go with a mark, a branch, #<change> or a commit id puts the wall there.
  ```
  `and <n> more` after a list cut at `MAX_LISTED`. An empty tree: `The tree has no moments yet: the first change makes one.`

## Not this lane

The wall's side (T04b), the tree thread (T04a), the console (T04d). Notifications of marks and goes from other seats are T04b's (`channel.rs`).

## Red tests first

In `tools.rs`'s tests and `render/tree.rs`'s own `#[cfg(test)] mod tests`. Each fails today because the tools and `render::tree` do not exist.

1. `the_router_holds_every_tool_with_a_description_and_schema` (`tools.rs:770`) with the 22 names, unchanged otherwise.
2. `arguments_are_strict` (`:802`) gains: `MarkArgs` needs `name` and refuses `overwrite`; `GoArgs` needs `moment`; `TreeArgs` takes `{}` and `{"recent": 50}` and refuses `{"limit": 5}`.
3. `a_mark_says_its_name_tag_and_place` (the three `<place>` forms, exact strings).
4. `a_go_says_how_to_undo_it_and_that_a_bud_will_grow` and the undone form.
5. `the_tree_reads_line_by_line_within_its_bounds`: on the wall's line, off a line by a mark, the tree behind, `and 5 more` at 205 marks, the empty tree; every summary cleaned.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mcp/src
```

## Definition of done

- [ ] The 5 tests fail first (paste each under RED), then pass.
- [ ] Files: `kazoo-mcp/src/tools.rs`, `kazoo-mcp/src/render.rs` (two lines), `kazoo-mcp/src/render/tree.rs`: three.
- [ ] By hand after the landing reinstall, from a Claude seat: `wall_tree`, `wall_mark`, `wall_go` to the mark, `wall_undo` of the go. Paste the four answers.

## Code quality

As PLAN.md's STACK RULES. `render.rs` (1,935 lines) and `tools.rs` gain only what is named above; list `render.rs`'s line count under FOUND.

## END

```
START KZ-T04c base=<sha> branch=lane/kz-T04c df=<GB> at <time>
END KZ-T04c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
