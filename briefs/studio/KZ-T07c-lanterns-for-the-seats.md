# KZ-T07c: every Claude seat can light a lantern that leads back to its own conversation at that moment, add notes, list the lanterns, and ask one: kazoo hands back the exact command that forks the past self, and never runs it

Line numbers are from main at `820ccc5` and may have moved; find each item by what it does, never rename. Items from KZ-T04c, KZ-T07a and KZ-T07b are named as those briefs name them; if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 13 Oct evening, line by line. Changes:
- The lane opens only after T07b, T07a and T04c have landed.
- `find_way` reads other conversations' files (a helper's, and in "Moved" up to eight others in the folder). It must only parse a line that contains both the tool suffix `__wall_lantern` and the call's words as JSON would write them; every other line is skipped unparsed. So kazoo never decodes another conversation's content, and the scan stays cheap. T07a's `find_call` does the same filtering; test 5 gains a line of another conversation that would fail to parse, and passes.
- The by-hand proof (a Claude seat on the wall, and running the fork commands) is Cassio's after landing, not the builder's: the builder never touches the live wall and never starts a Claude session.
- Tom has been asked (13 Oct evening) whether a seat may be told the file and lines of his words. Until he answers, build it as written; if he says no, the `<lines> lines in <path>, seq …` line becomes `<lines> lines kept by Dot for <seat>`, a one-line change at review.

Third of PLAN.md's T07 split in four (T07a the tree thread, T07b the wall, **T07c** the seats' tools, T07d the console). Runs beside T07d: they share no file.

Time: about 40 minutes of a builder's work. One mechanism: four tools, and a seat finding its own place in its own conversation. Crates: kazoo-mcp. Depends on T07b, which must have landed: `Request::{Lantern, Note, Visit, Lanterns}`; T07a, which must have landed: `Way`, `WayFound`, `NoteView`, `NotesResult`, `Visited`, `tree::lantern::read::{find_call, commands}`; T04c, which must have landed: the tools file as it left it.

## Outcome

Four tools every seat has: `wall_lantern`, `wall_note`, `wall_lanterns`, `wall_ask`.

When a seat lights a lantern, its kazoo server finds, by itself, the seat's own Claude Code conversation and the exact entry in it that holds this call, and sends them with the words. Nothing is asked of the seat, and nothing it says about itself is trusted. When anyone later asks the lantern, the answer is two commands, ready to paste:

```
cd '/Users/tom/Developer/projects/deno_rust/kazoo' && claude -p --resume 3ae5c173-4c29-4007-a7b5-faa882f70271 --fork-session --resume-session-at 0d1e2f30-4a5b-4c6d-8e7f-901a2b3c4d5e 'How did that moment feel?'
cd '/Users/tom/Developer/projects/deno_rust/kazoo' && claude --resume 3ae5c173-4c29-4007-a7b5-faa882f70271 --fork-session
```

with where Tom's spoken words around the moment are kept (file and lines, never the words), the notes added since, and how far the wall has moved on. Kazoo never starts the conversation.

## The design (decided; build exactly this)

### How a seat knows its own way back (`way.rs`, new; `mod way;` in `lib.rs`)

**What the server can see, checked on this Mac on 13 Oct (Claude Code 2.1.296).** Each seat's `kazoo mcp` process (three running: Cally, Waffles, Buckley, read with `ps eww`) has in its environment `CLAUDE_CODE_SESSION_ID` (the session's id), `CLAUDE_PROJECT_DIR` (the directory the session runs in), and, for Waffles' seats only, `CLAUDE_CONFIG_DIR=/Users/tom/.claude-waffles`. A session's transcript is `<config>/projects/<the project's folder name>/<session>.jsonl`, `<config>` being `CLAUDE_CONFIG_DIR` or else `~/.claude`. The assistant entry holding a tool call is written to the transcript before the tool runs (watched on a live transcript: the entry with the running call was its last line).

**Decision: the server finds the way itself; the seat passes nothing.** The tool takes no session argument. Reason: a session id a model types can be wrong, and a wrong way back looks exactly like a right one until someone walks through it, which is the failure Tom's design names first (`MEMORY-AND-LANTERNS.md` §7.2: "absence that presents as presence"). Finding the call itself in the transcript proves the session is this one, now.

```rust
/// What this server's process was given about its seat, read once at start.
pub struct Seen { pub session: Option<String>, pub project: Option<PathBuf>,
                  pub config: Option<PathBuf>, pub home: Option<PathBuf>, pub cwd: PathBuf }
impl Seen { pub fn from_env() -> Self }   // the variables above, HOME, and the working directory
pub enum NoWay { NotClaude, NoTranscript(String), NotFound(String), Unreadable(String) }  // each a sentence
pub async fn find_way(seen: &Seen, input: &serde_json::Value) -> Result<Way, NoWay>
```

`Seen::from_env` reads with `std::env::var_os` only (reading is safe; tests build a `Seen` by hand and never set a variable: the 2024 edition makes `set_var` unsafe, and the tree forbids that). It is read once in `run.rs`'s start and kept on the `Seat`. `find_way`, in order, every file read in `tokio::task::spawn_blocking`:

1. `session` a lower-case uuid, else `NotClaude` "this seat is not a Claude Code session (no CLAUDE_CODE_SESSION_ID)". `config` is `seen.config`, else `<home>/.claude`. `project` is `seen.project`, else `seen.cwd`.
2. The transcript: the first `<config>/projects/<dir>/<session>.jsonl` that is a file, looking at most 4,096 folders (765 on this Mac on 13 Oct). None: `NoTranscript` "Claude Code keeps no conversation <session> under <config>/projects".
3. **Session.** T07a's `find_call(transcript, "__wall_lantern", input)`, where `input` is `{"words": <words>}` exactly as the call carried it (the arguments are strict, so that is the whole input). Found: `Way { session, call, message, project, config: seen.config, transcript, found: Session }`.
4. **Helper.** Not there: the newest 16 files by modified time in `<transcript's folder>/<session>/subagents/` (a helper agent's own transcript; helpers share the seat's servers). Found in one: the way is the seat's own session, `call` the helper's call, and `message` the `uuid` of the last entry of the session's transcript (its place while its helper worked), `found: Helper`. Reason: `claude --resume` resumes a session, never a helper; the self the helper served is the way back.
5. **Moved.** Not there either: the session id the process was started with may be old (`/clear` starts a new conversation and leaves the servers running). The newest 8 other `*.jsonl` files in the transcript's folder modified in the last 10 minutes, each through `find_call`. Found: that file's stem is the session, `found: Moved`.
6. Each of 3 to 5 is tried every 100 ms for up to 2 s before the next. Reason: the call's entry is written before the tool runs, but through a write that may still be buffered; two seconds is far past any flush and short of the seat's 10 s wait (`ANSWER_WITHIN`, `link.rs:37`).
7. Nowhere: `NotFound` "this call is not in this seat's conversation (<session>) or a helper's, so its way back cannot be proven". A read error: `Unreadable` with the path and the error.

**A seat with no way still lights its lantern.** `wall_lantern` sends `way: None`, and the answer says why it has no way back and that Tom's words are its only way. Reason: the seat chose to light it; losing its words to a missing environment would be worse than a lantern that says plainly what it lacks.

### Arguments (`tools.rs`, beside T04c's `MarkArgs`; all `#[serde(deny_unknown_fields)]`)

```rust
/// `wall_lantern`.
pub struct LanternArgs {
    /// Your words about this moment, 1 to 2,000 characters (line breaks
    /// allowed): what you were doing, what you felt, what you learned.
    pub words: String,
}
/// `wall_note`.
pub struct NoteArgs {
    /// The note, 1 to 2,000 characters (line breaks allowed).
    pub words: String,
    /// The moment to note: what wall_go takes (a mark, a branch, #<change>,
    /// a commit id). Leave out for the moment the wall is at.
    #[serde(default)] pub moment: Option<String>,
    /// A lantern's 16-hex id from wall_lanterns, to add to it. Not with moment.
    #[serde(default)] pub lantern: Option<String>,
}
/// `wall_lanterns`.
pub struct LanternsArgs {
    /// Near this moment (what wall_go takes). Default: where the wall is.
    #[serde(default)] pub near: Option<String>,
    /// On this branch (trunk, or a bud/... name from wall_tree).
    #[serde(default)] pub branch: Option<String>,
    /// Every lantern in the tree, brightest first.
    #[serde(default)] pub every: bool,
}
/// `wall_ask`.
pub struct AskArgs {
    /// The lantern's 16-hex id, from wall_lanterns.
    pub lantern: String,
    /// Your question, put into the command quoted for the shell. Leave out
    /// for a command that ends in '<your question>'.
    #[serde(default)] pub question: Option<String>,
}
```

### The tools (`tools.rs`, in the router beside T04c's `wall_mark`)

- `wall_lantern`: `let way = way::find_way(&self.seen, &json!({"words": args.words})).await;` then `self.read::<NoteView>(Request::Lantern { words, way: way.as_ref().ok().cloned() }, |r| Ok(render::lit(r, way.as_ref().err())))`. Description, exactly: "Light a lantern on the moment the wall is at (just after its latest change): your words about it, kept with a way back to this very conversation, so that later anyone (Tom, another seat, you) can ask the self you are now how this moment felt, through a fork of this conversation. Light one on purpose, at a moment that matters: something finished, something that worked, something learned. Tom's spoken words in the five minutes either side are part of its way back, by reference to Dot's transcripts; kazoo never copies them. It changes nothing in the sound. Lanterns live only in the tree on this Mac (git notes, refs/notes/lanterns): nothing kazoo does pushes them, and a copy of the tree leaves them behind. A lantern dims as the wall moves on without anyone visiting it, and is never removed. Every seat is told. A seat may light, note or visit three times a minute (slow_down beyond)."
- `wall_note`: `Request::Note { words, moment, lantern }`, `render::noted`. Description, exactly: "Add a note to a moment, to a lantern, or, with neither, to the moment the wall is at: words that grow on it over time, so anyone can later see what came of it. A note added to a lantern is its epilogue. Notes never change the sound, are never changed or removed, and never dim. moment takes what wall_go takes (a mark, a branch, #<change>, a commit id); lantern takes a lantern's id from wall_lanterns; not both. 1 to 2,000 characters. Private as lanterns are: nothing kazoo does pushes them, and a copy of the tree leaves them behind. Every seat is told."
- `wall_lanterns`: `Request::Lanterns { near, branch, every }`, `render::lanterns`. Description, exactly: "The notes and lanterns: near a moment (by default the moment the wall is at: that moment, up to 100 before it and up to 100 after it on its line), on a branch, or every lantern in the tree, brightest first; give at most one of near, branch and every. Each lantern says who lit it and when, its words, the notes added to it, how often it was visited, how bright it still is (lit, dim or faint: it dims as the wall makes changes with no one visiting it; about 400 changes is a day's playing), and whether its way back is still there. wall_ask visits one. Anyone may ask, at most ten times in a few seconds."
- `wall_ask`: `Request::Visit { lantern }`, then `render::asked(&visited, question.as_deref())`. Description, exactly: "Visit a lantern and get its way back: the exact command that asks the seat that lit it, as it was at that moment, through a fork of its conversation, and the command that opens that whole conversation (which also knows what came after); where Tom's spoken words around the moment are kept (Dot's transcripts: the file and the lines, never the words); the notes added to it; and how far the wall has moved on since. Kazoo never starts the conversation: whoever asks runs the command in a terminal, and it costs a Claude session. The visit is counted and brightens the lantern; no one else is told. question goes into the command, quoted for the shell."
- The instructions (`:57`) gain, after T04c's tree bullet: "- Lanterns: wall_lantern lights one on the moment the wall is at, with your words and a way back to this conversation; wall_note adds a note to any moment or lantern; wall_lanterns lists them; wall_ask gives the command that asks the self that lit one how the moment felt. They live only in the tree on this Mac and change nothing in the sound."
- `TOOLS` (`:747`) becomes four longer than T04c left it (22 → 26, or whatever T05/T06 left, plus four), sorted.
- `Seat` gains `seen: Seen`, filled from `Seen::from_env()` once at start (`run.rs`). A test builds a `Seat` with a `Seen` made by hand.

Why kazoo returns the command and never runs the fork: a fork is a whole Claude session, with its cost, its permissions and the seat's own configuration; the wall daemon and the seat's server must never start a model session of their own; and the asker (a seat with a shell, or Tom in a terminal) is the one who wants the conversation, so the asker runs it and talks to it.

### What they say (`render/lanterns.rs`, new; `mod lanterns; pub use lanterns::{lit, noted, lanterns, asked};` in `render.rs` beside T04c's `mod tree;`)

Every seat's or note's text through `channel::clean`'s rules for control characters, but not cut (the listing is asked for); times as `13 Oct 21:04 UTC`; a moment as `#<n> (<short>)`, or `<short>` with no change number; `<id>` the 16-hex id.

- `lit(&NoteView, Option<&NoWay>)`: `Lit lantern <id> on <moment>: “<text>”.` then, with a way: ` Its way back is this conversation (session <session><", found as a helper's call" | ", found by its call after a /clear">); Tom's spoken words from <from> to <to> are part of it.`; without: ` It has no way back to this conversation (<NoWay's sentence>); Tom's spoken words from <from> to <to> are its only way.` Then ` wall_ask <id> asks it later. It is kept only on this Mac.`
- `noted(&NoteView)`: a note on a moment: `Noted <moment>: “<text>”. Notes are never changed and never dim.`; a note added to a lantern (the view is the lantern's; its last `added` is this one): `Added to lantern <id> (on <moment>): “<text>”. The lantern is touched again and glows lit.`
- `lanterns(&NotesResult)`, lines:
  ```
  <scope>: <n> lanterns and <n> notes<, and <n> lines written by hand>.
  - lantern <id> (<lit | dim | faint>) on <moment>, lit by <seat><" at the console"> <at>: “<text>”; visited <n> times; <n> notes added; way back <open | gone | none: Tom's words only>
    - added by <seat> <at>: “<text>”                (each added note, oldest first)
  - note on <moment> by <seat> <at>: “<text>”
  - by hand on <moment>: “<text>”
  and <n> more.
  wall_ask with a lantern's id gives its way back; wall_note adds to a moment or a lantern.
  ```
  None: `<scope>: no notes or lanterns yet. wall_lantern lights one where the wall is.`
- `asked(&Visited, Option<&str>)`:
  ```
  Lantern <id> (<glow>) on <moment>, lit by <seat> <at>: “<text>”. Visited <n> times.
  Added since: <seat> <at>: “<text>”; …                  (left out with none)
  Since it was lit, the wall has made <since_lit> changes; since anyone was last here, <since_last>.
  The way back (kazoo does not run it; it costs a Claude session):
  - ask the self at that moment: <ask>
  - talk with the whole conversation (it also knows what came after): <open>
  Tom's spoken words from <from> to <to> UTC, kept by Dot (kazoo never reads them out):
  - <lines> lines in <path>, seq <first> to <last> (<first_at> to <last_at>)
  ```
  `<ask>` and `<open>` from T07a's `commands(way, resume_at, question)`. The way-back block is instead `The way back is gone: Claude Code no longer keeps <transcript> (it removes conversations after cleanupPeriodDays, 30 days unless set).` for `Portal::Gone`, and `Lit at Tom's console: there is no conversation to go back to; Tom's words are its way.` for `Portal::Words`. Tom's words with no `heard`: `Tom's spoken words from <from> to <to> UTC: none are kept now in <dot> (Dot keeps each seat's last five runs, or none were spoken then).`
  Before the `ask` line, when the way is `Helper`: `(lit by one of the seat's helpers: the command asks the seat as it was while its helper worked)`.

## Not this lane

The tree thread (T07a), the wall (T07b), the console (T07d). Running any Claude command: never. Reading the text of Tom's words: never.

## Red tests first

In `tools.rs`'s tests, `render/lanterns.rs`'s own `#[cfg(test)] mod tests`, and `way/tests.rs` (declared from `way.rs`). Every transcript is a fixture the test writes into a `tempfile::tempdir()` laid out as `<config>/projects/<folder>/<session>.jsonl`; nothing reads a real one. Each fails today because the tools, `render::lit` and `way` do not exist.

1. `the_router_holds_every_tool_with_a_description_and_schema` (`tools.rs:770`) with the four names added, unchanged otherwise.
2. `arguments_are_strict` (`:802`) gains: `LanternArgs` needs `words` and refuses `session`; `NoteArgs` takes `{words}`, `{words, moment}`, `{words, lantern}`; `LanternsArgs` takes `{}` and refuses `{"limit": 5}`; `AskArgs` needs `lantern`.
3. `the_way_is_this_seats_own_conversation`: the call's entry is the fixture's last line: `found: Session`, `message` that entry's uuid, `call` its tool_use id, `config` `None` for a `Seen` without one and `Some` with one.
4. `a_helpers_call_is_its_sessions_way`: the call only in `<session>/subagents/agent-x.jsonl`: `found: Helper`, `message` the main transcript's last uuid.
5. `a_cleared_seat_finds_its_new_conversation_by_its_call`: the call only in another file of the folder, modified now: `found: Moved`, that session; one modified 11 minutes ago is not looked at.
6. `a_call_written_late_is_waited_for`: the line appended 300 ms after `find_way` starts: found; never appended: `NotFound` after 2 s (the test measures between 2.0 and 2.5 s).
7. `no_session_no_way_and_the_lantern_is_still_lit`: a `Seen` with no session: `NotClaude`; the tool sends `way: None` (check the request the fake wall gets) and the answer says why.
8. `the_answers_read_line_by_line`: exact strings for `lit` (each `found`, and no way), `noted` (both forms), `lanterns` (lit, dim, faint, gone, by hand, `and 5 more`, empty), `asked` (open with a question holding `'`, helper, gone, console, no words heard).

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mcp/src
```

## Definition of done

- [ ] The 8 tests fail first (paste each under RED), then pass.
- [ ] Files: `kazoo-mcp/src/tools.rs`, `kazoo-mcp/src/render.rs` (two lines), `kazoo-mcp/src/render/lanterns.rs`, `kazoo-mcp/src/way.rs`, `kazoo-mcp/src/way/tests.rs`, `kazoo-mcp/src/lib.rs` (one line), `kazoo-mcp/src/run.rs` (the `Seen` read once at start): seven.
- [ ] Not the builder's: after landing and reinstalling, Cassio, from a Claude seat on the wall, calls `wall_lantern` ("first lantern, by hand"), `wall_lanterns`, `wall_note` on it and `wall_ask` with a question; then runs the `ask` command once and the `open` command once (and quits it), and records whether the forked self answered as it was at the moment (it should not know of the `wall_lanterns` call that followed). That is the one proof that `--resume-session-at` with `--fork-session` does what T07a's commands assume. The builder lists it under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `render.rs` and `tools.rs` gain only what is named above; list `render.rs`'s and `tools.rs`'s line counts under FOUND. No blocking file read on the async runtime's threads: every read in `way.rs` runs in `spawn_blocking`. Nothing here writes any file.

## END

```
START KZ-T07c base=<sha> branch=lane/kz-T07c df=<GB> at <time>
END KZ-T07c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
