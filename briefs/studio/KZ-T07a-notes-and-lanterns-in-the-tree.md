# KZ-T07a: a note on any moment is a git note in `refs/notes/lanterns` that only ever grows; a lantern is a note with a way back to the seat that lit it and to Tom's spoken words around it; the tree thread writes them, lists them, counts visits, and dims each lantern as the wall moves on from it

Line numbers are from main at `820ccc5` and may have moved; find each item by what it does, never rename. Items from KZ-T01 to KZ-T04d are named as those briefs name them (`tree::git::Importer`, `Planted`, `TreeItem`, `News::Answer`, `Answered`, `HereError`, `here::resolve`, `Line`, `TreePlace`, `MomentView`, `MAX_LISTED`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T04a and T04b have landed (they had not when this was drafted); everything else is accepted as written. Checked: chrono is already a kazoo-wall dependency with `std` (through `clock`), so `parse_from_rfc3339` needs no new crate. A visit's `resume_point` may read a large transcript on the tree thread; the music's commits queue behind it and are never dropped, as T02b decided, so that is acceptable. Tom has been asked whether a lantern's answer may name the file and line numbers of his words (13 Oct evening); `Heard` keeps them here, and T07c follows his answer for what seats see.

Revised by Cassio, 13 Oct 21:19, on Waffles' reading against Tom's 8 Aug lantern definition (MEMORY-AND-LANTERNS.md §3, §3.1): a lantern dims by the changes since it was lit, and visits and added notes no longer brighten it; `ask` opens with the changes since as the past self's briefing; a watcher may visit, answered and not counted.

PLAN.md's T07 ("notes on moments as git notes; lanterns with session and transcript pointers; ask a moment") is four lanes, because it touches more than ten files, split as T04 was: **T07a** (this: the tree thread's side, the note format, the readers), **T07b** (the wall: four requests, allowances, telling seats), **T07c** (the seats' tools, and how a seat finds its own way back), **T07d** (the console: a key to light one, lanterns on the tree list). They run in that order; T07c and T07d may run side by side.

Time: about 45 minutes of a builder's work. One mechanism: the tree thread keeping an append-only note per moment. Crates: kazoo-wall. Depends: KZ-T04a and KZ-T04b, which must have landed before this lane opens. Nothing here is reachable from a seat yet; T07b wires it.

## Tom's design, and what it decides here

From Norn's memory design (`ablative/stack/norn/docs/design/norn-memory/MEMORY-AND-LANTERNS.md`, Tom's rulings of 8 Aug; Tom, 13 Oct 20:2x: "the design for that has already basically happened now, so we're all good"):
- "A memory is a way of storing a note against something, so you can retrieve that note against that thing. A lantern comes with an annotation like the note, but is a pathway back to have a conversation with the previous self through a fork." So: a **note** is text on a moment; a **lantern** is text on a moment plus a way back. Nothing else separates them (§1: "the real discriminator is whether the object carries a resumable session reference").
- Notes grow: "we can update those notes over time so you can see what wound up happening" (§1a.2, §4.2). So: **nothing is ever edited or removed**; a note is appended to, and a note can be added to a lantern (its epilogue).
- Lanterns are declared deliberately, never lit automatically (§1a.3, ruled M3). So: only a request lights one (T07b, T07c, T07d); nothing in the wall lights one on its own.
- A lantern dims; a note does not (§1 table, §3: "a memory is a record … churn must not dim it"; "a lantern is an oracle"). Dimming ranks it lower and never deletes it (§3.1). Time is "position in the work's history … never wall-clock age" (§1a). So: **a lantern dims by the changes the wall has made since it was lit**, and a note never dims. A visit or an added note does not brighten it: the ancestor's world stays frozen at the moment of lighting, so nothing done since makes it know more (§3, Tom's 8 Aug definition; it supersedes the May design's engagement reinforcement, `DESIGN.md`, "Intensity and Decay"). The lantern is lit on the whole wall as it was, so every change since counts. The same changes are the "welcome to the future" briefing that repairs the ancestor (§3.1), and `ask` hands them to it (below).
- The retention hazard (§7.2): "a lantern can open onto nothing … it will look exactly like a live one until someone tries to walk through it." So: every listing says, for each lantern, whether its way back is still there.
- Vectors and resonance (§6) are out of scope here, as the task says. This is the pull half only: a seat or Tom goes looking.

## Outcome

The tree keeps one more ref, `refs/notes/lanterns`. Each moment (a commit) that anyone has written on carries one git note, a few JSON lines, one entry a line, only ever appended to:

```
{"v":1,"id":"5f3c0e2a91b4d7e8","kind":"lantern","at":"2026-10-13T21:04:11Z","seat":"Cassio","change":6400,"text":"the brass finally breathes","way":{"session":"3ae5c173-4c29-4007-a7b5-faa882f70271","call":"toolu_01MTfyfPU1zNnj4JYT7saCPY","message":"b4d58a77-08d4-463a-8612-eea26e290558","project":"/Users/tom/Developer/projects/deno_rust/kazoo","transcript":"/Users/tom/.claude/projects/-Users-tom-Developer-projects-deno-rust-kazoo/3ae5c173-4c29-4007-a7b5-faa882f70271.jsonl","found":"session"},"words":{"from":"2026-10-13T20:59:11Z","to":"2026-10-13T21:09:11Z"}}
{"v":1,"id":"91d0c3a2b4e5f607","kind":"visit","at":"2026-10-14T09:12:40Z","seat":"Waffles","change":6712,"on":"5f3c0e2a91b4d7e8"}
{"v":1,"id":"0c4b1e9d77a2f310","kind":"note","at":"2026-10-14T09:15:02Z","seat":"Tom","console":true,"change":6714,"text":"it held up the whole next morning","on":"5f3c0e2a91b4d7e8"}
```

The music never changes for a note: no branch, tag or `HEAD` moves, and no change is logged. The tree thread answers two new jobs, in queue order behind every change before them: write an entry (a lantern where the wall is, a note on a moment or on a lantern, or a visit, which is counted and answered with where the lantern's way back starts and where Tom's words around it are), and list the notes and lanterns (near a moment, on a branch, or everywhere). Nothing kazoo does ever pushes the ref, and the tree is set so that no copy of it (`git clone`, `jj git clone`, even `git clone --mirror`) carries it.

## The design (decided; build exactly this)

### The note format, decided: JSON lines, one entry a line, append only

Reasons, against TOML:
1. **A line is an entry.** Git's own note merge strategy for line notes, `cat_sort_uniq`, concatenates, sorts and drops repeated lines. With one self-contained entry per line, two clones' notes on one moment merge into exactly the union of their entries. A TOML `[[entry]]` spans several lines, and that strategy would interleave them into nonsense.
2. **Appending is concatenation.** The new note is the old bytes plus one line. Nothing already written is parsed and re-serialised, so nothing written earlier, by this build or another, can be changed by writing.
3. **The log is JSON lines already** (`~/.kazoo/wall/log.jsonl`), and `Kazoo-Change` is compact JSON (KZ-T02b). One format to read.

### The words (`protocol/tree.rs`, T04a's file)

```rust
/// One line of a moment's note (see KZ-T07a).
pub struct NoteEntry {
    pub v: u32,                       // 1
    pub id: String,                   // 16 lower-case hex, drawn fresh
    pub kind: EntryKind,
    pub at: String,                   // UTC, YYYY-MM-DDTHH:MM:SSZ
    pub seat: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub console: bool,
    pub change: u64,                  // the wall's latest change when written
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,         // a note's or a lantern's words
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,           // the lantern a visit or an added note is of
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub way: Option<Way>,             // a lantern lit by a Claude seat
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Words>,         // every lantern
}
pub enum EntryKind { Note, Lantern, Visit }          // serde snake_case
/// The way back to the conversation that lit a lantern.
pub struct Way {
    pub session: String,      // the Claude Code session id
    pub call: String,         // the id of the wall_lantern tool call (toolu_…)
    pub message: String,      // the uuid of the transcript entry holding that call
    pub project: String,      // the directory that session runs in (absolute)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>, // CLAUDE_CONFIG_DIR, when the seat's is not the default
    pub transcript: String,   // the session's transcript when lit (absolute)
    pub found: WayFound,      // session | helper | moved (T07c says when each)
}
pub enum WayFound { Session, Helper, Moved }          // serde snake_case
/// The span of Tom's spoken words a lantern points at, in UTC.
pub struct Words { pub from: String, pub to: String }
pub enum Glow { Lit, Dim, Faint }                     // serde snake_case
pub enum Portal { Open, Gone, Words }                 // serde snake_case
/// Tom's words around a lantern in one of Dot's transcript files: where they are, never what.
pub struct Heard { pub path: String, pub seat: String, pub first_seq: u64, pub last_seq: u64,
                   pub first_at: String, pub last_at: String, pub lines: u32 }
/// A note or a lantern as listed.
pub struct NoteView {
    pub entry: NoteEntry,             // kind note or lantern, never visit
    pub moment: MomentView,           // T04a's
    pub visits: u32,                  // visit entries naming it (lanterns)
    pub added: Vec<NoteEntry>,        // notes added to it, oldest first (lanterns)
    pub since: u64,                   // revision − its own `change`, 0 when the revision is lower
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glow: Option<Glow>,           // lanterns only: notes never dim
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<Portal>,       // lanterns only
}
/// A line in a note that is not an entry (written by hand with git notes).
pub struct HandNote { pub moment: MomentView, pub text: String }
pub struct NotesResult {
    pub scope: String,                // "near <moment>", "on <branch>", "everywhere", as said back
    pub revision: u64,
    pub notes: Vec<NoteView>, pub count: usize,
    pub hand: Vec<HandNote>,
}
/// A visit's answer: the lantern, where its way back starts, and where Tom's words are.
pub struct Visited {
    pub lantern: NoteView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<String>,    // the transcript entry to resume at (below)
    pub heard: Vec<Heard>,
    pub dot: String,                  // the directory Dot's transcripts were looked for in
    pub since_lit: u64,               // revision − the lantern's own change
    pub revision: u64,                // the work revision the visit was answered at
}
```

All derive `Debug, Clone, PartialEq, Serialize, Deserialize` (`Eq` where they hold no float). Field order is as written: `serde_json::to_string` writes fields in declaration order, so the same entry is the same bytes. Every type gets a doc comment saying what this section says.

Reading an entry does **not** deny unknown fields. Reason: a newer kazoo may add a field, and an older one must keep listing the line, not drop it. A line with `v` above 1 is listed as it reads.

### The rules an entry keeps (`tree/lantern.rs`, new; `impl NoteEntry { pub fn check(&self) -> Result<(), String> }`)

Constants, each with its reason in its doc comment:
- `pub const NOTE_TEXT_MAX: usize = 2_000;` characters (Unicode scalar values). A note is a paragraph or two, about 300 words; a longer thing belongs in a file the note names.
- `pub const NOTE_BYTES_MAX: usize = 1 << 20;` bytes in one moment's note. A note is read whole at every append; 1 MiB holds about 400 lanterns with their visits on one moment, past any real use, and keeps one append under a few milliseconds.
- `pub const WORDS_BEFORE: u64 = 300;` and `pub const WORDS_AFTER: u64 = 300;` seconds. Tom speaks in turns seconds to a minute apart (Dot's transcripts on 13 Oct); a lantern is lit in answer to what was said in the minutes before, and Tom's question is also how the moment "made him react", which is after. Five minutes either side holds both, and is short enough that the span is about this moment.
- `pub const LIT_WITHIN: u64 = 400;` and `pub const DIM_WITHIN: u64 = 4_000;` changes. The log made 6,373 changes from 26 Sep to 13 Oct, about 370 a day of playing. A lantern lit within about a day's playing is lit; within about ten days' playing, dim; beyond, faint. Counted in changes, not days, because Tom ruled that a lantern's time is its place in the work's history, never the clock: a week away from the wall dims nothing.

`check` refuses, each with a whole sentence:
- `v` other than 1; `id` not 16 lower-case hex; `at` not the UTC form `change::utc` writes (`change.rs:363`); `seat` failing `valid_name` (`protocol/mod.rs:74`).
- By kind: a `note` has `text` and no `way` or `words`; a `lantern` has `text` and `words`, and no `on`; a `visit` has `on` and no `text`, `way` or `words`. `on`, where present, is 16 lower-case hex.
- `text`: kept exactly as given (never trimmed); 1 to `NOTE_TEXT_MAX` characters; no control character but `\n`; not only white space. This rule is its own function, `pub fn text_problem(text: &str) -> Option<&'static str>`, with the sentences "a lantern's or note's words are 1 to 2,000 characters", "… may hold line breaks but no other control characters" and "… cannot be only spaces", so the wall (T07b) and the console (T07d) check words with the same code.
- `way`: `session` and `message` each a lower-case uuid (8-4-4-4-12 hex); `call` 1 to 128 of `A-Z a-z 0-9 _ -`; `project`, `config` and `transcript` absolute, at most 4,096 bytes, with no control character.
- `words`: both in the UTC form, `from` not after `to`.

### Glow (`tree/lantern.rs`)

- `pub fn glow(since: u64) -> Glow`: `Lit` when `since <= LIT_WITHIN`, `Dim` when `<= DIM_WITHIN`, else `Faint`.
- Visits and added notes never change a lantern's glow. Reason: Tom's 8 Aug definition (§3): an epilogue "briefs the *visitor*; it cannot repair the *ancestor*, whose world-model stays frozen at the moment of lighting" (§4).
- Listing a lantern is not a visit. Reason: a listing shows every lantern near a place, and counting that would keep every listed lantern lit forever.
- `since` is `revision.saturating_sub(change)`; it is also the "welcome to the future" the design asks for (§3.1): how far the wall has moved on since the lantern was lit. T07c says it in words, and `commands` puts it at the head of the question asked.

### Readers that touch files outside the tree (`tree/lantern/read.rs`, new)

These run no git. Each is bounded and returns `ReadError` (a whole sentence) on trouble, never panics.

**Tom's words, by reference.** Dot keeps one transcript per seat at `~/Library/Logs/locutus/<Seat>-transcript.jsonl`, one JSON line per thing said or heard, with `seq`, `at` (ISO 8601 with the local offset, e.g. `2026-10-13T18:19:28+11:00`), `speaker` (`person` for Tom, `locutus` for what the seat said) and `text` (`dot/src/bin/dot-seat/record.rs:38`, `record/transcript_file.rs`). Each seat run rolls the file to `.1` and so on to `.4`, and drops the oldest (`KEEP = 4`, `transcript_file.rs:13`).
- `pub fn dot_dir() -> Option<PathBuf>`: `$KAZOO_DOT_LOGS` when set and not empty (tests), else `$HOME/Library/Logs/locutus` (Dot's `timeline_dir`, `record.rs:290`).
- `pub fn tom_words(dir: &Path, words: &Words) -> Result<Vec<Heard>, ReadError>`: every file in `dir` named `<seat>-transcript.jsonl` or `<seat>-transcript.jsonl.<1 to 4>` (at most 64 files, by name), each read line by line up to 16 MiB. Each line is deserialised into a private struct of `seq`, `at` and `speaker` **only**: the `text` field is never deserialised, so Tom's words are never held by kazoo, not even in memory. Lines with `speaker == "person"` whose `at` (parsed with `chrono::DateTime::parse_from_rfc3339`) lies within `[from, to]` are counted; a file with any gives one `Heard` (its path, the seat from its name, the first and last `seq` and `at`, the count). A line that does not parse is skipped. Sorted by path.
- **Why the window and not a path is kept in the note.** The file that holds a lantern's words today is `.1` after the seat's next run, and gone after five. A path kept at lighting would point at another run's words. The window is resolved when someone visits, and the visit says plainly when the words are no longer kept.

**The way back's starting point.** `pub fn resume_point(transcript: &Path, call: &str) -> Result<Option<String>, ReadError>`: reads the transcript line by line (at most 512 MiB; the largest on this Mac on 13 Oct was 77 MB, a whole night), parses only lines that contain `call` as a substring, and returns the `uuid` of the first entry with `type == "user"` whose `message.content` holds a `tool_result` with `tool_use_id == call`. Reason: that entry is the seat's own moment just after it lit the lantern, with the lantern's answer in front of it; resuming there is talking to the self that lit it. `None` when there is no such entry (the session ended first).

**Finding a call at lighting (used by T07c).** `pub fn find_call(transcript: &Path, tool_suffix: &str, input: &serde_json::Value) -> Result<Option<Call>, ReadError>`, `pub struct Call { pub message: String, pub call: String }`: reads the last 4 MiB of the file (`TAIL_BYTES`; one assistant turn with its thinking is well under 1 MiB in Tom's transcripts), drops the first, partial line, and walks lines from the end, parsing only a line that contains `tool_suffix` (every other line is skipped unparsed, so another conversation's content is never decoded; T07c's reading note): the first entry with `type == "assistant"` whose `message.content` holds an item with `type == "tool_use"`, a `name` ending in `tool_suffix`, and `input == *input` gives `Call { message: <its uuid>, call: <the item's id> }`. Measured 13 Oct on Claude Code 2.1.296: the assistant entry holding a tool call is in the transcript before the tool runs.

**The commands (used by T07c and T07d).** `pub fn commands(way: &Way, at: &str, question: Option<&str>, moved: Moved) -> Commands`, `pub struct Moved { pub lit: u64, pub now: u64 }` (the lantern's `change` and the visit's `revision`), `pub struct Commands { pub ask: String, pub open: String }`:
- `ask`: `cd <project> && [CLAUDE_CONFIG_DIR=<config> ]claude -p --resume <session> --fork-session --resume-session-at <at> <question>`; with no question, the question is `<your question>`. When `now > lit` the question is preceded by the briefing, in the same quoted word: `Since you lit this lantern at change <lit>, the wall has made <now − lit> changes; wall_log lists them, newest first (before: <n> for older pages). `. Reason: the ancestor answers from a world that froze at lighting, and this is the one query that both measures and repairs that (§3.1).
- `open`: `cd <project> && [CLAUDE_CONFIG_DIR=<config> ]claude --resume <session> --fork-session`.
- Every path and the question are quoted for a POSIX shell: wrapped in `'…'`, each `'` inside written `'\''`. The uuids are written bare (they are hex and dashes).
- Reasons: `--fork-session` leaves the lantern's conversation as it was, so a lantern can be visited any number of times. `--resume-session-at` is how Claude Code 2.1.296 resumes a conversation cut at one entry; it is not in `claude --help` and is honoured only with `-p`, so `ask` is a one-question form, and `open` is the whole conversation to talk with, which also knows what came after. `cd <project>` because Claude Code finds a session among the project's own; `CLAUDE_CONFIG_DIR` because Waffles' seats keep theirs in `~/.claude-waffles`.

### Writing a note (`tree/git.rs`)

`pub struct NoteCommit { pub seat: String, pub at: String, pub message: String, pub target: String, pub note: Vec<u8> }` and `Importer::note(&mut self, c: &NoteCommit) -> Result<u64, TreeError>`, beside KZ-T02b's `commit`, writing:

```
commit <branch>
mark :<k>
author <seat> <<slug>@kazoo.invalid> <unix> +0000
committer kazoo-wall <kazoo-wall@kazoo.invalid> <unix> +0000
data <len>
<message>
[from <tip>]              only on this importer's first commit, when the ref has a tip
N inline <target>
data <len>
<the whole new note>

```

The author line through the same function KZ-T02b's change commits use (factor it if it is inline, without changing a byte it writes). The importer is a short-lived one, made by the thread from `Planted { branch: "refs/notes/lanterns", tip: <git rev-parse -q --verify refs/notes/lanterns>, through: None }`; `checkpoint` then checks `refs/notes/lanterns` exactly as it checks a branch, and `finish` ends it. Checked on git 2.47.1 on 13 Oct: `N inline` writes a note `git notes --ref=lanterns show` reads, a second commit with `from` appends a line, and a commit not containing the tip is refused (`Not updating refs/notes/lanterns`, exit 1, ref unchanged). Reason for `fast-import` and not `git notes append`: the same durable path as every other write in the tree (`core.fsync=committed`, the sha checked after the checkpoint), and `git notes append --no-separator` needs git 2.42 where the tree needs 2.36.

### The thread's jobs (`tree/thread.rs`; the work in `tree/thread/lanterns.rs`, new, declared from it)

`TreeItem` gains:

```rust
/// Write one entry (KZ-T07a): a lantern where the wall is, a note, a visit.
/// `dot` is where Dot's transcripts are, for a visit's `heard` (the wall
/// fills it from `dot_dir()`; tests give a temporary directory, because the
/// 2024 edition makes `std::env::set_var` unsafe and the tree forbids that).
/// `count` is false only for a watcher's visit: answered, never written.
Write { ticket: u64, entry: Box<NoteEntry>, to: Target, revision: u64, dot: Option<PathBuf>, count: bool },
/// List the notes and lanterns.
Notes { ticket: u64, scope: Scope, revision: u64 },
pub enum Target { Here, Moment(String), Lantern(String) }
pub enum Scope { Near(Option<String>), Branch(String), Every }
```

`Answered` gains `Wrote(Box<NoteView>)`, `Visited(Box<Visited>)` and `Notes(Box<NotesResult>)`; `HereError` gains `NoLantern(String)`. As every T04a job: the thread checkpoints first if any commit is unacknowledged; while the tree is down a job is answered `Down`, never dropped; the answer carries `through`.

**Write.**
1. `entry.check()`, else `Broken(<its sentence>)` (T07b checks first, so this is a second guard).
2. The target commit. `Here`: the line's commit (T04a's `Line`); none → `UnknownMoment("the tree has no moment yet: nothing has been committed")`. `Moment(m)`: `here::resolve`. `Lantern(id)`: read every note (below) and find the entry with that id and kind `lantern`; its moment is the target, and `entry.on` must equal `id`; none → `NoLantern("no lantern <id>: wall_lanterns lists them")`.
3. A lantern must be `Here`, a visit must be `Lantern`. Anything else is `Broken`. Reason: a lantern's way back is the lighting seat's conversation as it is now, and that belongs to the moment the wall is at now, never to a moment in the past. A note may go anywhere.
With `count` false (a visit only; anything else with it is `Broken`), steps 4 to 6 are skipped and the answer is as step 7 gives it, with `visits` as it was. Reason: a watcher reads the moment and its way back; it writes nothing to the tree (KZ-T07b).
4. Keep the copies private, once per thread run before its first write: `git -C <tree> config --get-all uploadpack.hideRefs`; if no line is `refs/notes/lanterns`, `git -C <tree> config --add uploadpack.hideRefs refs/notes/lanterns`. A failure refuses the write: `Broken("the tree could not be set to keep lanterns out of its copies (<why>); nothing was written")`. Checked on git 2.47.1 on 13 Oct: with that setting, `git ls-remote` and both `git clone --mirror <dir>` and `git clone --mirror file://<dir>` leave the notes behind. Reason: Tom's plan makes the tree "clonable and forkable", and a lantern holds the path of a seat's conversation and points at Tom's words; private by default means a copy carries the music and not those.
5. Read the target's current note (`git notes --ref=lanterns list <target>`, which exits 1 when there is none, then `git cat-file blob <blob>`). New bytes: the old, a `\n` if the old does not end in one, the entry as one compact JSON line, `\n`. Over `NOTE_BYTES_MAX` → `Full("the note on <short> holds 1 MiB already; add to another moment")`.
6. Write it through the notes importer, checkpoint, finish. The commit's message: subject `<seat> lit a lantern on <short>: <text>` / `<seat> noted <short>: <text>` / `<seat> added to lantern <id>: <text>` / `<seat> visited lantern <id>`, with `<text>` cut at 60 characters and line breaks made spaces; a blank line; trailers `Kazoo-Kind: lantern | note | visit` and `Kazoo-Entry: <id>`. A refusal or a sha mismatch at the checkpoint (someone moved the ref) is `Broken("the lanterns' ref moved while this was written (<git's words>); nothing was written: write it again")`. **It never sends `Down` and never touches the line's importer**: the music's commits go on.
7. Answer. A note or a lantern: `Wrote` with its `NoteView` (`visits` 0, `added` empty). A visit: `Visited` with the lantern's `NoteView` as it now reads, `resume_at` (for a lantern with a `way`: `resume_point(transcript, call)`, else `way.message` when the transcript is a file, else `None`), `heard` from `tom_words(dot, words)` with the job's `dot` (`None`, or an error reading, is an empty `heard` with the reason in the daemon log; a visit never fails because Dot's files cannot be read), `dot` as that directory's path (empty when `None`), `since_lit` (saturating) and `revision` (the job's). A note added to a lantern: `Wrote` with the lantern's `NoteView`.

**Reading every note.** `git notes --ref=lanterns list` (pairs `<blob> <commit>`; no ref yet means no notes), then the blobs through one `git cat-file --batch`. At most `MAX_NOTED = 100_000` moments are read (more than seven years of a note a day from every seat); past it the listing says so in `scope` ("… the first 100,000 noted moments"). A blob over `NOTE_BYTES_MAX` (made by hand) is not read and is listed as a `HandNote` "this note is over 1 MiB and is not read". An empty line is skipped (`git notes append` writes one between paragraphs). Every other line parses as a `NoteEntry` that passes `check`, or is a `HandNote` with the line (cut at `NOTE_TEXT_MAX`).

**Notes.** Commits in scope, then the views:
- `Near(m)`: `m` resolved (`None`: the line's commit). The moment, then up to `NEAR = 100` first-parent ancestors (`git rev-list --first-parent -n 101 <c>`), then, when the place has a branch, up to 100 of its later commits on that branch (`git rev-list --first-parent --ancestry-path --reverse <c>..refs/heads/<branch>`, the first 100). Ordered by distance from the moment (0, then 1 back, 1 on, 2 back, …). `NEAR` is 100 because that is about a quarter of a day's playing either side, and a screen of moments.
- `Branch(b)`: `refs/heads/<b>` must exist (else `UnknownMoment("no branch \"<b>\": wall_tree lists them")`); its first-parent commits, newest first.
- `Every`: every noted commit; lanterns first by glow (lit, dim, faint), then newest lit; then notes, newest first.
- Within one moment, entries in the order written. Visits and added notes are folded into their lanterns (across every note read, so an added note written on another moment by hand still counts) and are not listed on their own.
- `portal`: a lantern with a `way` is `Open` when `way.transcript` is a file, else `Gone`; one without is `Words` (lit by the console, or by a seat whose way could not be found: Tom's words are its only way back).
- Moments as T04a's `MomentView`s, through one `git log --no-walk=unsorted` of the commits, as T04a's `List` reads them. At most `MAX_LISTED` views, with `count` the total.

## Not this lane

- The requests, allowances, error codes and events (T07b). The tools and finding a seat's own way (T07c). The console (T07d). The little tree (T08a, T08b).
- Resonance, vectors, passive surfacing of lanterns (Norn §6): out of scope for kazoo.
- Removing, editing or moving any entry: never.
- Running a Claude conversation: kazoo never starts one (T07c says why).

## Red tests first

`tree/lantern/tests.rs` (declared from `tree/lantern.rs`) for the format and readers; `tree/thread/lanterns/tests.rs` (declared from `tree/thread/lanterns.rs`) for the jobs, each on a tree planted with `tree::git::plant` in a `tempfile::tempdir()` and driven as T04a's tests drive one (they need `git` 2.36 or later). Every fixture transcript is written by the test; nothing reads a real one. Each fails today because there is no `tree::lantern` and no `Write` or `Notes` job.

1. `entries_read_back_byte_for_byte`: one entry of each kind, written and read: the same bytes; each refusal in "the rules an entry keeps", one case each, with its sentence.
2. `unknown_fields_and_newer_lines_are_kept`: a line with an extra field and one with `v: 2` are listed, not dropped.
3. `glow_counts_changes_not_days`: since 0, 400, 401, 4,000, 4,001 → lit, lit, dim, dim, faint; a visit and an added note leave `since` and the glow as they were.
4. `toms_words_are_counted_never_held`: a Dot directory with `Buckley-transcript.jsonl`, `.1`, `.4`, a `.5` (ignored), `Buckley-timeline.log` (ignored); lines inside and outside the window, at `+11:00`, `person` and `locutus`: exact `Heard`s. Every fixture line's text is the sentinel `NEVER-READ-OUT`: the `Debug` of the result does not contain it.
5. `a_call_is_found_in_the_transcript_tail`: the right entry among a different input, another tool, a user entry, and a partial first line at the 4 MiB boundary.
6. `the_resume_point_is_the_answer_to_the_call`: and `None` with no answer.
7. `commands_quote_exactly`: a project with a space and a `'`, a config dir and none, a question with `'` and a newline: the exact strings; with `moved` 812 → 812 no briefing, 812 → 1,000 the briefing (`188 changes`) inside the same quoted word, and with no question the briefing then `<your question>`.
8. `notes_grow_and_never_change_the_music`: a lantern, a note, a visit and an added note on one moment: `git notes --ref=lanterns show <c>` is four lines in that order; `refs/notes/lanterns` has four commits, each the parent of the next; every branch, tag and `HEAD` is as before; no change was logged.
9. `a_lantern_is_lit_where_the_wall_is`: on `trunk`, and at a moment after a go (a `What::Go` offered by hand, as T04a's tests make one): the note is on that moment.
10. `lanterns_go_nowhere_but_here_and_visits_only_to_lanterns`: `Lantern` with `Moment`, and a visit to a note's id: `Broken`; an unknown id: `NoLantern`; a note with `count` false: `Broken`; a visit with `count` false is answered, and the note's bytes and `visits` are unchanged.
11. `notes_list_near_a_moment_on_a_branch_and_everywhere`: distances and order, 205 notes listing 200 with `count` 205, a missing branch.
12. `a_lost_way_is_said`: the transcript removed: `Gone`; a lantern with no `way`: `Words`; a visit then has `resume_at: None`.
13. `lanterns_never_leave_the_tree`: after two writes, `uploadpack.hideRefs` holds `refs/notes/lanterns` once; `git ls-remote <tree>` and `git clone --mirror <tree>` show no `refs/notes/lanterns`.
14. `a_full_note_and_a_hand_note`: a note at 1 MiB refuses the next entry (`Full`); a line added with `git notes --ref=lanterns append` by the test is listed as a `HandNote`.
15. `a_moved_ref_refuses_the_write_and_the_music_goes_on`: the test moves `refs/notes/lanterns` to an unrelated commit between the read and the write (through a `#[cfg(test)]` hook the thread calls between steps 5 and 6): `Broken`; the next change still commits on `trunk`, and `tree.error` is not set.
16. `lanterns_run_only_these_git_commands`: read `tree/thread/lanterns.rs`, `tree/lantern.rs` and `tree/lantern/read.rs` with `include_str!`; every git subcommand is one of `notes` (only as `notes --ref=lanterns list`), `cat-file`, `rev-list`, `rev-parse`, `log`, `config` (only `--get-all uploadpack.hideRefs` and `--add uploadpack.hideRefs refs/notes/lanterns`); `read.rs` runs none; `push` and `--force` appear nowhere. KZ-T02b's own command test still passes (`git.rs` gains only stream text).

## Bench

On a tree of 20,000 commits made by an `Importer` in a test helper, with 1,000 entries over 300 moments: one `Write` and one `Notes { scope: Every }`, each timed, printed under BENCH. Expected under 50 ms and 100 ms; over twice that, say so under FOUND.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tree.rs kazoo-wall/src/tree kazoo-wall/src/protocol
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 16 tests fail first (paste each under RED), then pass.
- [ ] `NoteEntry`, `EntryKind`, `Way`, `WayFound`, `Words`, `Glow`, `Portal`, `Heard`, `NoteView`, `HandNote`, `NotesResult`, `Visited`, the six constants, `text_problem`, `glow`, `dot_dir`, `tom_words`, `resume_point`, `find_call`, `Call`, `commands`, `Commands`, `NoteCommit`, `Importer::note`, `TreeItem::{Write, Notes}`, `Target`, `Scope`, `Answered::{Wrote, Visited, Notes}` and `HereError::NoLantern` exist as named.
- [ ] Files: `protocol/tree.rs`, `tree.rs` (`pub mod lantern;`), `tree/lantern.rs`, `tree/lantern/read.rs`, `tree/lantern/tests.rs`, `tree/git.rs`, `tree/thread.rs`, `tree/thread/lanterns.rs`, `tree/thread/lanterns/tests.rs`: nine. No new crate.
- [ ] BENCH lines written.

## Code quality

As PLAN.md's STACK RULES. Each new file under 1000 lines. `tree/thread.rs` gains only the two items, the answers and the dispatch to `lanterns.rs`. Every `git` child is waited for. No file outside the tree is ever written; the readers open files read-only. Nothing logs or returns the `text` of a Dot transcript line.

## END

```
START KZ-T07a base=<sha> branch=lane/kz-T07a df=<GB> at <time>
END KZ-T07a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the write and the listing, and the tree they ran on
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
