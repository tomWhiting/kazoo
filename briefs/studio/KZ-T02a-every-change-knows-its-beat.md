# KZ-T02a: every change the wall makes carries the beat it landed on, the tempo playing, and the clock that beat belongs to, so a branch's beats form one line the fire can walk

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Read by Cassio, 13 Oct 21:0x. Accepted as written. Opens after KZ-T01 lands (both edit `daemon/wall.rs` and `protocol/mod.rs`), and after KZ-D03b if that is in flight (it edits the daemon).

Time: about 30 minutes of a builder's work. One mechanism: stamping each change with a clock reading on the control thread. Crates: kazoo-wall only. Depends: KZ-T01 (only for ordering of landings; no shared code). This is the first half of PLAN.md's "T02 a commit for every change". It was split off because adding a field to `Change` touches ten files on its own, and the ten-file rule leaves no room for the tree thread beside it. KZ-T02b writes the commits.

Files (ten):
1. `kazoo-wall/src/protocol/mod.rs`: `Change` gains `clock`; `ClockStamp`.
2. `kazoo-wall/src/daemon/wall.rs`: the clock line and its stamp; `#[cfg(test)] mod clock_tests;`.
3. `kazoo-wall/src/daemon/wall/clock_tests.rs` (new).
4. `kazoo-wall/src/daemon/mod.rs`: its test literal (`:772`).
5. `kazoo-wall/src/store.rs`: its two test literals (`:2014`, `:2730`).
6. `kazoo-wall/src/protocol/tests.rs`: its three literals (`:180`, `:324`, `:796`) plus one serde test.
7. `kazoo-wall/src/tui/log.rs`: its test literal (`:224`).
8. `kazoo-wall/src/tui/tests/mod.rs`: its helper literal (`:150`).
9. `kazoo-wall/src/tui/tests/saves.rs`: its helper literal (`:77`).
10. `kazoo-wall/src/daemon/wall/tests.rs`: its literals (`:3328`, `:3658`). The `Some(Change { what: …, .. })` patterns (`:960`, `:1156`, `:1325`) already end in `..` and need no change.

Each literal gains `clock: None`. `daemon/wall.rs:1071` (`resume`) also gains `clock: None`, for the reason below. kazoo-mcp and kazoo-cli build no `Change` (checked).

## Outcome

Every change logged from now on reads, in `log.jsonl` and in every event, like this:

```json
{"seq": 6400, "at": "2026-10-13T21:04:11Z", "seat": "Tom", "what": {…}, "summary": "…",
 "clock": {"beat": 18734.25, "tempo": 120.0, "line": "9c41d07a3be2f610.3"}}
```

`beat` is where in the music the change landed, `tempo` is what was playing, and `line` names the unbroken run of beats that `beat` belongs to. Two changes on the same `line` are `beat₂ − beat₁` beats apart, near enough to replay them in time. Two changes on different lines are not compared by beat at all. A restart, a load, or the desk taking or letting go of the wall's clock starts a new line. Old log lines read as they did, with no `clock`.

## The design (decided; build exactly this)

### The stamp (`protocol/mod.rs`)

```rust
/// Where in the music a change landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockStamp {
    /// The song position, in beats, at which the change could first sound.
    pub beat: f64,
    /// The tempo playing then, in beats per minute.
    pub tempo: f64,
    /// The clock line `beat` is on: `<start>.<n>`.
    pub line: String,
}
```

`Change` (`protocol/mod.rs:651`) gains, last, `#[serde(default, skip_serializing_if = "Option::is_none")] pub clock: Option<ClockStamp>`. Each field gets a doc comment that says exactly what this section says.

### What `beat` means, exactly

- `beat` is `self.engine.shared().beat()` (`engine/mod.rs:262`), read on the control thread with one relaxed atomic load at the moment the change is numbered (`Wall::new_change`, `daemon/wall.rs:2076`). The engine stores it after every 32-frame sub-block (`engine/mod.rs:1130` to `:1135`): the song position at the end of the last sub-block rendered. That is where the next sub-block starts, so it is the earliest beat at which the change can sound.
- The change reaches the engine through its command channel (`apply` then `mirror`, `daemon/wall.rs:2092`) and lands at the start of the first sub-block rendered after that. That is within one device buffer of `beat`: 512 frames at 48 kHz is 10.7 ms, or 0.021 beats at 120 BPM. A frame-exact stamp is KZ-W02's.
- Nothing on the audio thread changes. The engine already publishes `beat` and `bpm`. This lane only reads them.
- `beat` is the song position the wall plays at. That is its own clock (starting from the engine's configured beat, 0 at a daemon start), or the desk's song position while the wall follows a playing desk (`engine/mod.rs:1082` to `:1094`, `follow`). A rebuilt engine carries the beat on (`EngineConfig::beat`, `engine/mod.rs:421`), so a device rebuild does not break a line.
- `tempo` is `shared().bpm()` (`engine/mod.rs:256`): the tempo playing, which is the desk's while following. For a `What::Tempo` change it is the tempo before the change lands.

### What `line` means, exactly

A clock line is a run of changes whose beats lie on one continuous song position. Between any two stamps on one line, the beat only moves forward with the music.

- **`<start>`** is 16 lower-case hex digits from `crate::chance::fresh_wall_seed()` (`chance.rs:27`), drawn once in `Wall::new` (`daemon/wall.rs:1163`). So every daemon run, on any machine, has its own start. A restart is a new start: the engine begins again at beat 0.
- **`<n>`** is a decimal count from 0 within the run, raised by one when, and only when, one of these happens:
  1. **The wall is replaced**: a load, a new wall, or an undo of a load. `Wall::replace` (`daemon/wall.rs:3420`) breaks the line before it numbers its `What::Load` change (`:3446`, `let change = self.new_change(seat, what, source.undoes())`). So the load change is the first stamp on the new line. A load puts a different wall up mid-music: its beats belong to the new piece.
  2. **The desk takes or lets go of the clock**: `shared().following_desk()` (`engine/mod.rs:268`) differs from the value at the last stamp. The song position then switches between the wall's clock and the desk's.
  3. **The beat went back**: the beat read is below the last stamp's beat on this line (by more than `1e-9`). The desk has no locate today (PLAN.md, "Where we are"). This catches one the day it has, and any rebuild that starts elsewhere, with no change to the engine.
- A tempo change does **not** break the line. Beats still count beats, and a replay across it walks beats at each stamp's tempo.
- `ClockLine::check` also runs on every `Wall::tick` (`daemon/wall.rs:4383`, every 20 ms at most). So a desk joining and leaving between two changes still breaks the line, whether or not the flag is back to its old value by the next change.

**Why a start id and a count, not a time.** A reader needs one thing from `line`: whether two stamps share a beat line. Equal strings mean yes. Different strings mean no.

**Across lines, the stated rule for replay (T06 builds it):** within a line, a replay waits `beat₂ − beat₁` beats. Across a line break, it waits the UTC difference of the two changes' `at`, converted to beats at the earlier stamp's tempo, at most 16 beats (4 bars).
- A replay must neither stall for the hours a restart can span, nor run two walls' changes into each other.
- Four bars is the default crossing length (PLAN.md, Track T, "The crossing"), so a fire passes a break as a crossing would.

Write this rule into `ClockStamp`'s doc comment, so the reader of the log has it.

**Changes with no stamp**: those `resume` logs as the daemon starts (`daemon/wall.rs:1045`). They are made before any engine exists, so there is no beat to read. Every log line from before this lane has none either. `clock: None` means "no beat known", never "beat 0".

### The code (`daemon/wall.rs`)

```rust
/// The clock line every change is stamped on (see `ClockStamp`).
#[derive(Debug)]
struct ClockLine {
    start: u64,
    n: u64,
    /// The last stamp's beat and whether the desk had the clock then.
    last: Option<(f64, bool)>,
}
```

- `fn new(start: u64) -> Self`.
- `fn break_line(&mut self)`: `n += 1`, `last = None`.
- `fn check(&mut self, beat: f64, following: bool)`: breaks the line on rules 2 and 3 against `last`, then sets `last = Some((beat, following))`. A non-finite `beat` breaks the line and stores nothing, so a stamp is never made from a NaN.
- `fn stamp(&mut self, beat: f64, tempo: f64, following: bool) -> Option<ClockStamp>`: `check`, then the stamp, with `line = format!("{:016x}.{}", start, n)`. It returns `None` when `beat` or `tempo` is not finite, and says so once in the daemon log.
- `Wall` gains `clock_line: ClockLine`, made in `Wall::new` from `fresh_wall_seed()`. `Wall::new_change` reads `let shared = self.engine.shared();` and sets `clock: self.clock_line.stamp(shared.beat(), shared.bpm(), shared.following_desk())`. `new_change` becomes `&mut self`; its two callers (`next_change` `:1931`, `pay_owed_all` `:2051`) already hold `&mut self`. `replace` calls `self.clock_line.break_line()` before its `new_change`. `tick` calls `check` with the shared values.
- `ClockLine` is about 50 lines and lives in `daemon/wall.rs` beside `Wall`. A file of its own would be an eleventh file. KZ-T02b may move it into `tree/` if that lane needs it there.

## Not this lane

- Commits and trailers (KZ-T02b). Frame-stamped replay (KZ-W02). The fire (T06).
- Showing the beat in `wall_log`, the console's log pane or the MCP.
- Any engine change. When Track C gives the desk a locate, rule 3 already breaks the line on a jump back. A jump forward on a locate must then break it too, and that lane's brief must say so; write it under FOUND.

## Red tests first

In `kazoo-wall/src/daemon/wall/clock_tests.rs`. It is a child of `daemon::wall`, so it reaches `ClockLine` and `Wall` privately. It builds its own `Wall` as `tests.rs:16` does. Each fails today because neither `ClockStamp` nor `ClockLine` exists.

1. `a_change_carries_the_beat_and_tempo_it_landed_on`. Build a `Wall` and its `Engine`. Render 48,000 frames with `engine.render(&mut buffer, 2)` (as `daemon/wall/tests.rs:549` drives it), so the shared beat is past 0, then turn a knob. The change's `clock.beat` equals `shared().beat()` read just before the request, bit for bit, and `clock.tempo` is 120.
2. `beats_on_one_line_only_go_forward`. Two turns with rendering between: same `line`, and the second beat is greater by the rendered frames' beats (± 1e-9).
3. `a_restart_starts_a_new_line`. Two `Wall`s made one after the other (a simulated restart): their first changes' `line`s differ in `<start>`, and both `<n>` are 0.
4. `a_load_starts_a_new_line_and_is_its_first_stamp`. Use a `Wall` with a store in a temp dir, as the existing load tests do (find them by `Request::Load`). Make a turn on line `s.0`, then a load. The `What::Load` change is on `s.1`, and the next turn is on `s.1`.
5. `the_desk_taking_the_clock_breaks_the_line`. Run `ClockLine::check` / `stamp` on `(10.0, false)`, `(10.5, true)`, `(11.0, true)`, `(11.5, false)`: lines `.0`, `.1`, `.1`, `.2`.
6. `a_beat_going_back_breaks_the_line`. Stamp `(20.0, false)` then `(19.0, false)`: the lines differ.
7. `a_tempo_change_keeps_the_line`. Stamp at 120 BPM, then at 90: same line.
8. `no_beat_from_a_nan`. `stamp(f64::NAN, …)` is `None`, and the next finite stamp is on a new line.
9. `old_lines_read_without_a_clock` (in `protocol/tests.rs`). A logged line from `~/.kazoo/wall/log.jsonl`'s shape with no `clock` key reads with `clock: None`, and writes back with no `clock` key.

## Checks

As PLAN.md's STACK RULES, run on Tom's Mac under the shared lock:
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4`
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings`
- `cargo fmt --all --check`
- `ast-grep scan kazoo-wall/src/protocol kazoo-wall/src/daemon kazoo-wall/src/store.rs kazoo-wall/src/tui/log.rs kazoo-wall/src/tui/tests`
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`

kazoo-mcp reads `Change` through serde only. An added optional field cannot break it. Do not build it here; the pooled gate does.

## Definition of done

- [ ] The 9 tests fail first (paste each under RED), then pass.
- [ ] `ClockStamp`, `Change::clock`, `ClockLine` and `Wall::clock_line` exist as named, and the doc comments state the meaning of `beat` and `line` and the replay rule across lines.
- [ ] Nothing in `engine/` changed (`git diff --stat` shows no `engine/` path).
- [ ] Under DONE, paste one stamped log line from a test wall.

## Code quality

As PLAN.md's STACK RULES. `daemon/wall.rs` is over 1000 lines already. Add to it only `ClockLine` and the calls named here. The audio thread is untouched: the control thread does two atomic loads per change and per tick.

## END

```
START KZ-T02a base=<sha> branch=lane/kz-T02a df=<GB> at <time>
END KZ-T02a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
