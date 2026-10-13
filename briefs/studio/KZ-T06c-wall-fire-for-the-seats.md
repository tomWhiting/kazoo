# KZ-T06c: every Claude seat can set the tree on fire from any moment at any speed, and put it out, with one tool that says plainly what is burning and how it ends

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Items from T04c, T05c and T06b are named as those briefs name them (`MarkArgs`, `GoArgs`, `render::went`, `render/tree.rs`'s place words, `Request::{Fire, FireOut}`, `FireResult`, `FireOutResult`, `MIN_FIRE_SPEED`, `MAX_FIRE_SPEED`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Third of PLAN.md's T06 split in four (T06a the tree's side, T06b the wall burns, **T06c** the seats' `wall_fire`, T06d the console's `F`). Runs beside T06d: they share no file.

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T06b has landed; the by-hand check is Cassio's with Tom after landing, never the builder's. Accepted otherwise as written.

Time: about 25 minutes of a builder's work. One mechanism: one tool over T06b's two requests. Crates: kazoo-mcp. Depends on T06b, which must have landed before this lane opens.

Files (three):
1. `kazoo-mcp/src/tools.rs`: `FireArgs`, the `wall_fire` tool, the instructions' line, `TOOLS`.
2. `kazoo-mcp/src/render.rs`: `mod fire; pub use fire::{lit, out};` beside T04c's `mod tree;`.
3. `kazoo-mcp/src/render/fire.rs` (new): `lit`, `out`, and their tests.

## Outcome

`wall_fire` is a tool every seat has. With `from` (and `along`, `speed` if wanted) it lights the fire and answers what is burning: `The tree is on fire from mark "dawn" (after change 812) along trunk: 214 moments at 2×. …`. With `out: true` it puts the fire out where it has reached and says where the wall now is. Seats already hear, through the channel, when anyone lights it, when it goes out and when it reaches the tip (T06b).

## The design (decided; build exactly this)

### Arguments (`tools.rs`, beside `GoArgs`; `#[serde(deny_unknown_fields)]`)

```rust
/// `wall_fire`.
pub struct FireArgs {
    /// Where the fire starts: a mark's name, a branch (trunk, or a bud/...
    /// name from wall_tree: the whole branch from its first moment), a
    /// change as #<number>, or a commit id. Leave it out with out: true.
    #[serde(default)]
    pub from: Option<String>,
    /// The branch to burn forward along: by default trunk when it holds the
    /// moment, else the newest bud that does.
    #[serde(default)]
    pub along: Option<String>,
    /// How fast: 0.25 to 64 times as fast as it was played (default 1).
    #[serde(default)]
    pub speed: Option<f64>,
    /// true: put the fire out where it has reached (nothing else is given).
    #[serde(default)]
    pub out: bool,
}
```
Before anything is sent, `fn fire_request(args: FireArgs) -> Result<Request, String>` turns the arguments into `Request::FireOut` or `Request::Fire { from, along, speed }`, and the router answers its `Err` with `bad` (`tools.rs:316`), as the other tools refuse what makes no sense: `out` with any of `from`, `along`, `speed` is `"wall_fire with out: true takes nothing else"`; neither `out` nor `from` is `"wall_fire needs from (a mark, a branch, #<change> or a commit), or out: true"`.

### The tool (`tools.rs`, in the router beside `wall_go`)

- `out`: `ask::<FireOutResult>(Request::FireOut)`, `render::out(&result)`.
- Else: `ask::<FireResult>(Request::Fire { from, along, speed })`; when `result.go` is `Some(go)`, `self.own(go.change.seq)` as `wall_go` does (the seat's own go is not news to it); `render::lit(&result)`.
- Description, exactly: "Set the tree on fire: the wall goes to a moment (crossing over 16 beats, unless it is there already), waits for the bar, then plays every moment that grew after it on a branch again, each at the beats it came after the one before, divided by speed (0.25 to 64, default 1): the smoke is the music. from: a mark, a branch (the whole branch from its first moment), #<change> or a commit id; along: the branch to burn (default trunk when it holds the moment, else the newest bud that does). Nothing is written in the tree: the past is there already, and each moment played is a line in wall_log. Any change anyone makes while it burns (a turn, a cable, a module, an undo, a tempo, a go, a load, moving a module) puts it out where it reached, and that change grows a bud from there; recording, marking and saving do not. It goes out by itself at the branch's tip, and the wall is then on that branch. out: true puts it out. Every seat is told when it is lit, goes out or reaches the tip. Lighting it draws on the same allowance as wall_load and wall_go (three walls put up a minute)."
- The instructions (`:57`) gain, after T04c's tree line: "- wall_fire sets the tree on fire from any moment: the wall plays the branch's moments again at their own beats (any speed), and the first change anyone makes puts it out and grows a bud from where it reached."
- `TOOLS` (`:747`) gains `wall_fire`, sorted (23 after T04c's 22).

### What it says (`render/fire.rs`)

`<place>` is T04c's place words for a `TreePlace` (`render/tree.rs`: `mark "<name>"`, `<branch>`, `the moment after change <n>`, `moment <short>`, with `, after change <n>`), reused, not copied: make the function T04c wrote `pub(super)` if it is private.
- `lit(&FireResult)`: `The tree is on fire from <place> along <along>: <moments> moments at <speed>×.` (speed through `{}` of the `f64`: `1`, `0.25`, `64`), then, when `go` is `Some`, a space and `render::went(go)`'s words (T04c, T05c), else ` The wall was there already.`; then ` It waits for the bar, plays each moment at its beat, and goes out at the tip of <along>, with wall_fire out: true, or with the first change anyone makes, which grows a bud from where it reached. Recording (wall_record) catches the smoke.`
- `out(&FireOutResult)`: `The fire is out at <place> after <played> moments (<left> not played). The wall stays there; the next change grows a bud from it.`; with `left` 0: `… after <played> moments, at the tip.`
- Summaries and names through `channel::clean` (`channel.rs:301`), as every text a seat reads.

## Not this lane

The wall's side (T06b), the console (T06d). The channel's news of fires (T06b).

## Red tests first

Each fails today because there is no `wall_fire`.

1. `tools.rs`: `the_router_holds_every_tool_with_a_description_and_schema` with 23 names.
2. `tools.rs`: `arguments_are_strict` gains: `FireArgs` takes `{"from": "dawn"}`, `{"from": "trunk", "speed": 4}`, `{"out": true}`, and refuses `{"from": "dawn", "rate": 2}`.
3. `tools.rs`: `wall_fire_refuses_what_makes_no_sense`: `fire_request` of `out` with `from`, with `speed`, and of neither, each give their sentence; of `{"out": true}` gives `Request::FireOut`; of `{"from": "dawn", "speed": 2}` gives `Request::Fire { from: "dawn", along: None, speed: Some(2.0) }`.
4. `render/fire.rs`: `a_lit_fire_says_what_burns_and_how_it_ends`: with and without a go, speed 0.25 and 64, exact strings.
5. `render/fire.rs`: `a_fire_put_out_says_where_the_wall_is`: part way, and at the tip.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mcp/src
```

## Definition of done

- [ ] The 5 tests fail first (paste each under RED), then pass.
- [ ] Files: the three above.
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, from a Claude seat, with Tom: `wall_fire` from a mark at speed 4, `wall_log` (the played moments), `wall_fire` with `out: true`, `wall_tree`. Paste the answers.

## Code quality

As PLAN.md's STACK RULES. `render.rs` and `tools.rs` gain only what is named; list `render.rs`'s line count under FOUND.

## END

```
START KZ-T06c base=<sha> branch=lane/kz-T06c df=<GB> at <time>
END KZ-T06c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
