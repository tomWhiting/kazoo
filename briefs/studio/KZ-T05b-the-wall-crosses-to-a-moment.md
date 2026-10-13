# KZ-T05b: going to a moment is a crossing, not a cut: over 16 beats the modules both walls share (by birth) glide every knob, the modules only the moment has fade in, the ones it lacks fade out and are then taken away, and every cable that differs crossfades, while the wall plays on and anyone can keep changing it

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Items from KZ-T01 to KZ-T04d, KZ-D03b and KZ-T05a are named as those briefs name them (`Birth`, `Module::birth`, `What::Go`, `TreePlace`, `Source::Moment`, `Source::Back`, `Asked::Back`, `replace`, `kind_share`, `is_sink`, `load_room_as`, `Route::glide`, `Command::TempoGlide`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Second of PLAN.md's T05 split in three (T05a the engine's glides, **T05b** the wall crosses, T05c the seats and the console choose the length).

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T04a–d, D03b and T05a have landed; the by-hand listen on Tom's wall is Cassio's with Tom after landing, not the builder's (the builder never touches the live wall). The six decisions below are taken as written (Cassio decides technical calls and tells Tom; he may overrule any): 16 beats by default, 0 to 64; the tempo glides; a re-sourced input dips to silence at the half; shared modules keep running; an unaffordable crossing becomes a cut; loads stay cuts.

Time: about 75 minutes of a builder's work. One mechanism: the wall moving its engine from one wall to another by birth mark, over time, with every fade timed in frames and finished by the tick. Crates: kazoo-wall, and kazoo-mcp for one test literal. Depends: T04a, T04b, T04c, T04d (a go, its undo, `What::Go`), KZ-D03b (`kind_share`, `is_sink`, `load_room_as`) and T05a (`Route::glide`, `Command::TempoGlide`), all of which must have landed before this lane opens.

Files (ten):
1. `kazoo-wall/src/protocol/mod.rs`: `What::Go` gains `beats`; `DEFAULT_CROSS_BEATS`, `MAX_CROSS_BEATS`.
2. `kazoo-wall/src/change.rs`: a go's words say its crossing; its test literals gain `beats`.
3. `kazoo-wall/src/daemon/wall.rs`: `Rack`'s leaving slots; `Wall::fades`, `cross_plan`, `crossings_sent`; `replace`, `mirror`'s `Go` arm, `send_tables`, `assign_lines`, `swap_engine`, `load_engine`, the `Remove` arm, `tick`, `request_as`'s one comparison; `mod cross;`.
4. `kazoo-wall/src/daemon/wall/cross.rs` (new): `CrossTiming`, `CrossPlan`, `Fades`, `plan_crossing`, `crossing_load`, `crossing_beats`, `cross`, `cross_tick`.
5. `kazoo-wall/src/daemon/wall/cross/tests.rs` (new).
6. `kazoo-wall/src/daemon/wall/moments.rs`: a go asks for the default crossing; the undo of a go crosses back over the beats its go took.
7. `kazoo-wall/src/daemon/wall/unheard.rs`: a leaving sink is still a sink.
8. `kazoo-wall/src/tree/thread/here/tests.rs`: its `What::Go` literals gain `beats: 0.0`.
9. `kazoo-wall/src/tui/tests/tree.rs`: its `What::Go` literal gains `beats: 0.0`.
10. `kazoo-mcp/src/render/tree.rs`: its test's `What::Go` literal gains `beats: 0.0`.

Before the first commit, `rg -n 'What::Go \{' kazoo-wall kazoo-mcp` and check that every struct literal (not a pattern ending in `..`) is in one of these files. If one is elsewhere, it is an eleventh file: add `beats: 0.0` there all the same (a field cannot be left out of a literal), and say so under FOUND.

## Outcome

`wall_go` (and the console's go) no longer cuts. The wall goes on playing, and over 16 beats (4 bars) it becomes the moment:
- a module both walls have (the same birth mark and the same kind) stays where it is in the engine, running, and every knob glides to the moment's value;
- a module only the moment has is built on the control thread, put in the engine with its knobs at the moment's values, and its cables bring it in from silence;
- a module only the wall had stays in the engine while its cables take it down to silence, and is taken out once it is silent;
- a cable both walls have (same source module and port, same destination module and jack) keeps its delay line and glides to the moment's amount; a cable only one wall has fades in or out; an input whose source changes fades the old source out over the first half and the new one in over the second;
- the tempo glides to the moment's over the same time, unless the desk keeps it.

The log says it in one line: `Tom went to mark "dawn", after change 812: 41 modules, 51 cables, 120 BPM; crossing over 16 beats; the wall as it was is saved as "before-go-20261013-201500"`. The patch, `patch.json`, `look` and the tree are the moment's from that change on: a seat's change during the crossing is made to the moment's wall and grows its bud (T04a), while the engine finishes the crossing underneath. A go with `beats` 0 is today's cut, unchanged. A crossing the machine cannot afford (both walls at once past the load guard, or no memory for both) is made a cut and says so.

## The design (decided; build exactly this)

### The numbers (`protocol/mod.rs`, beside `What`; `cross.rs`)

- `pub const DEFAULT_CROSS_BEATS: f64 = 16.0;` Four bars of 4/4: long enough to hear one wall become the other as a passage, short enough that a seat waiting for it is not waiting long (8 s at 120 BPM). Tom's PLAN.md decision (Track T, "The crossing").
- `pub const MAX_CROSS_BEATS: f64 = 64.0;` The longest knob glide a seat can ask for (`MAX_GLIDE_BEATS`, `patch.rs:33`), so no crossing is slower than the slowest turn; and both walls render at once for its whole length, which is what the guard pays for, so it is bounded.
- `const CROSS_MARGIN_SECONDS: f64 = 0.25;` (in `cross.rs`). A fade is finished by the engine `glide` frames after it takes the table in; the control side knows only the frames rendered when it sent it (`Shared::frames`, `engine/mod.rs:337`), and the engine takes a command in at the start of its next callback, at most one device buffer later (512 frames, 1,024 above 96 kHz, `daemon/audio.rs:21`; a backend's default buffer is a few thousand frames at most). A quarter second covers any of them, so nothing is taken out of the engine before its fade has reached silence.

### What the log records (`protocol/mod.rs`, `What::Go`; `change.rs`)

- `What::Go` (T04a) gains, after `tempo`:
  ```rust
  /// Beats the wall took to cross to the moment, at the tempo playing as it
  /// began (`DEFAULT_CROSS_BEATS` unless asked otherwise); 0 is a cut. A
  /// line written before crossings reads as 0, as every go then was.
  #[serde(default)]
  beats: f64,
  ```
  Always written. Reason it is in the change: the undo of a go crosses back over the same beats (below), the fire (T06) reads it, and the log must say how the wall got there.
- One `What::Go` for the whole crossing, logged as it begins, as T04b logs every go. Nothing is logged when it ends: the patch is the moment's from the go's change on, and the crossing is how the engine sounds getting there, as a turn's glide is one change and not its steps (KZ-T02b). The tree is as T04a made it: the go writes no commit and moves `HEAD`; the first change made after the go, during the crossing or after it, is the bud's first commit, holding the moment's wall plus that change, at the moment it is made.
- `change.rs`, T04a's `went` (`describe_with`, `:48`): when `beats > 0`, after the tempo and before `; the wall as it was`, add `; crossing over <beats>`, `<beats>` written by `format::beats` (`format.rs:98`: `16 beats`, `1 beat`, `0.5 beats`). With `beats` 0 the words are T04a's, byte for byte, so T04a's tests stand with `beats: 0.0`.

### Matching by birth (`cross.rs`, `plan_crossing`)

The wall up now is `self.patch` (with `self.rack` saying which slot each module is in); the moment's is `incoming`, as T04b's `go` makes it (`Patch::load` of the moment's files, births included, T01).
- A module `d` of `incoming` **matches** a module `o` of the wall up now when `o.birth == d.birth` (T01's `Birth`, `Eq`; never `None` on a wall, T01), `o.kind == d.kind`, and, for a speaker (`Builder::Adapted(Adapter::Speak)`, as `Wall::insert` tests it, `daemon/wall.rs:1489`), also `o.id == d.id` and the words the two walls hold for it (`Patch::speech`) are equal. Births are unique on each wall (T01), so a match is one to one. Never by id.
- **Same birth, another kind** (it can happen only through a migration, e.g. T03's version-1 `delay` made `digital` with its id and wall kept, so its found mark is the same): two modules. The old one fades out and the new one fades in. Reason: a module's engine processor is built for its kind (`dsp::build`, `daemon/wall.rs:1498`); there is nothing to glide between a delay's state and a digital delay's.
- **Speakers** match only with the same id and the same words. Reason: the speech player keeps a speaker's phrase feed and words by module id (`Speech::attach`, `forget`, `put_back`, `daemon/speech.rs:194`, `:203`, `:303`), and words are not a knob to glide. Same id and words is the common case (a crossing between moments of one lineage, and every step of a fire, T06), and costs nothing; anything else fades the old voice out and brings the new one in with its own words.
- Every other module of `incoming` is **born** (only the moment has it); every other module of the wall up now is **leaving**.
- A matched module keeps its engine slot, its tag and its running processor (an LFO's phase, a sequencer's step, a reverb's tail), and takes the moment's id. That continuity is the point of matching by birth. Its processor was seeded from the old wall's seed and id (`chance::module_seed`, `daemon/wall.rs:1501`) and keeps that state; a born module is built with `module_seed(incoming.seed, d.id)`, exactly as a cut would build it; the wall's seed is the moment's from the go on. So a crossing does not sound bit for bit like a cut to the same moment; nothing else about the patch differs.

### Cables (`plan_crossing`)

Key each cable by where it goes: `(the birth of the module it feeds, the jack name)`, for the cables of both walls (`Cable::to_module`, `to_jack`; the jack name as `CableRecord::to` writes it after the `.`). A jack takes one cable on each wall (`patch.rs:639`). For each key, with `old` the cable on it now (if any) and `new` the moment's (if any), and `N` the crossing's frames:
1. `old` and `new` from the same source (the same birth on the from side, both modules matched or both the same module, and the same output port): **matched**. The cable keeps its delay line and tag (`self.lines`, keyed by cable id, is re-keyed to `new`'s id), so its history and delay glide carry on. Its route has `glide: N`, so a different amount glides from where it is; the same amount changes nothing.
2. `old` and `new` from different sources: **re-sourced**. `old` leaves over `N / 2` (its route stays in the table with amount 0 and `glide: N / 2`, `gone_at = start + N / 2 + margin`). `new` **waits** until that `gone_at`, then comes in over `N / 2` on a fresh line. Reason: a jack holds one cable (`CableTable`, `engine/tables.rs:80`), so the two cannot both sound; out over the first half and in over the second is the crossfade a jack can hold, and it is the same for audio, a pitch CV and a gate (a gate cable fading through 0.5 switches there, which is a gate's crossfade).
3. Only `old`: it **leaves** over `N` (amount 0, `glide: N`, `gone_at = start + N + margin`).
4. Only `new`: it **enters** over `N` (a fresh line and tag from `assign_lines`, `glide: N`, so the engine starts it silent, T05a).

A cable into or out of a born or leaving module falls under 3 or 4 by its key, so those modules fade by their cables alone. Every sound reaches the master through a cable into an `out` (T05a, "Why these two"), so a module whose cables have faded is silent: no per-module gain or inserted VCA is needed, and the patch and the tree's files never hold anything the crossing made up.

A leaving cable's route is frozen as it was last sent (`slot`, `port`, `line`, `tag`, `delay`, `gate`, from `self.rack`, `self.lines`, `self.timing.delays` and `carries_gate` before the patch changes) with `amount: 0.0` and its glide. Its line and tag stay reserved until it is gone, so `assign_lines` never gives them to a new cable meanwhile.

### Knobs and tempo (`plan_crossing`)

- For each matched module and each knob of its kind (`kind.spec().knobs`), when the moment's target differs from the target now (`f32` bits): `Command::Knob { slot, knob, target, glide_frames: N }` (`engine/mod.rs:134`). The engine glides from wherever the knob is, along its travel (`Glide`, `:470`), including a knob a seat's turn is still gliding. A cable into a knob's jack is a cable like any other (above).
- An effect knob that moves is also put in `self.turned` exactly as `mirror`'s `Turn` arm does (`daemon/wall.rs:1841–1858`), so its latency is measured again once it has settled. Factor those lines into `fn note_turned(&mut self, module: &str, knob: usize, from: f64, to: f64, frames: f64)`, called by both, with no change to what they do.
- Tempo: unless the desk keeps it (`on_desk`, `:1447`), when `incoming.tempo` differs from the tempo playing, `Command::TempoGlide { bpm: incoming.tempo, frames: N }` (T05a). Reason: a tempo that jumps mid-passage is a cut in time; a ritardando or accelerando over the crossing is the transition. The desk keeping the tempo is T04b's rule for a go (`playing_after`, `:3389`), unchanged.
- `N` is `beats × 60 / bpm × rate` frames, with `bpm` the tempo playing as it begins (`self.engine.shared().bpm()`) and `rate` the engine's: at least 1, at most `u32::MAX`. The crossing's length is fixed in frames as it begins; a tempo gliding under it, or the desk moving the tempo, does not stretch it. Reason: every fade must end at a known frame so the tick can take out what has gone.

### The plan and its timing (`cross.rs`)

```rust
/// How long each part of a crossing takes, in frames.
pub(super) struct CrossTiming {
    /// Every knob that moves, unless `knob` names it.
    pub knobs: u32,
    /// Knobs with a glide of their own (T06's steps), by (module id in the
    /// moment, knob index).
    pub knob: HashMap<(String, usize), u32>,
    /// Cables in and out (re-sourced ones take half each). 0: at once.
    pub fades: u32,
    /// The tempo. 0: at once.
    pub tempo: u32,
}
```
`CrossTiming::go(frames)` sets `knobs`, `fades` and `tempo` to `frames` and `knob` empty. T06 makes steps with `fades: 0` and `tempo: 0` and its own knob glides; with `fades: 0` a crossing is the moment's changes made at once (born modules in at their amounts, leaving ones and their cables out in the same table, re-sourced inputs swapped), which is what a logged patch, add or remove does today, with no hush.

`CrossPlan` holds what `cross` needs once the patch is the moment's, all computed from the wall up now and `incoming` before the patch changes: the matched modules as `(old id, new id, slot)`; the born module ids in `incoming`'s order; the leaving modules as `(slot, kind)`; the knob commands; the matched cables as `(old id, new id)` with their glide; the leaving routes as `(to slot, jack index, from slot, frozen route, line, tag, gone after)`; the entering cables `(new id, glide)`; the waiting cables `(new id, enter after, glide)`; the tempo glide; and the counts for the words (below). Offsets are frames from the start.

### Doing it (`cross`, called from `mirror`)

- `replace` (`daemon/wall.rs:3420`), for `Source::Moment` and `Source::Back` (T04b), which gain `beats: f64`:
  1. After D03b's `load_room_as` and before `keeping`: `let (beats, plan) = self.crossing_beats(&incoming, source_beats, &mut notes);` (below). For every other source, `beats` is 0 and there is no plan.
  2. `What::Go { …, beats }`.
  3. Just before `self.patch = incoming` (`:3485`), with the plan made: `self.cross_plan = plan` (`Option<Box<CrossPlan>>`, a new `Wall` field).
  4. `mirror`'s `Go` arm (T04a: `What::Go { .. } => self.swap_engine()`) becomes: take `self.cross_plan`; `Some(plan)` with `beats > 0` → `self.cross(*plan)`; else `self.swap_engine()`. A `What::Go` with no plan (beats 0, or from a path that made none) is the cut, as today.
- `cross(&mut self, plan: CrossPlan)`, on the control thread, in this order:
  1. `start = self.engine.shared().frames()`. When `timing.fades > 0`: `self.restart_load_watch()` (`:1325`), a wall put up being measured afresh as after a swap, and `self.crossings_sent += 1`.
  2. Speakers: `self.speech.forget(id)` for each leaving speaker (`speech.rs:203`), before any born speaker of the same id attaches.
  3. The rack: each leaving module's slot moves to `Rack::leaving` (below); matched modules are re-indexed under their new ids (`Rack::reindex`); born modules are placed by `self.insert(module)` (`:1473`: `dsp::build` or `build_speaker` on this thread, `Command::Insert` to the engine; a born speaker's words through `put_back_words`, `:1463`). `reserve_for` (`:1397`) is called first for the born modules and the entering cables. Nothing is built on the audio thread: the engine only takes the boxed processor in (`Engine::insert`, `engine/mod.rs:961`).
  4. `self.lines` is rebuilt: each matched cable's `(line, tag)` under its new id; leaving and waiting cables' are not in it (the fades keep the leaving ones reserved); entering ones get fresh lines in `assign_lines`.
  5. What is kept by module id (`readings`, `wiring`, `fresh_from`, `settled`, `settling`, `faults.unreported`, `faults.last_report`) keeps the entries of matched modules whose id did not change, and drops every other. Reason: a step of a fire keeps every id, and its scopes must go on reading; a module renamed or gone has no reading that is still its own.
  6. `self.fades` takes the plan's leaving routes, leaving modules, waiting and entering cables and matched glides, with every offset made absolute from `start` (`gone_at`, `enter_at`, `done_at` in frames rendered).
  7. `self.send_tables()` (with the fades, below), then every knob command, then the tempo glide.
  8. With `timing.fades == 0` (a fire's step, T06) nothing goes into the fades: leaving cables are simply not in the table step 7 sends, re-sourced and new cables go in at once with `glide: 0`, and right after that table each leaving module is taken out (`Command::Remove`, `rack.gone`), as `mirror`'s `Remove` arm takes a module out today. So a step costs what the logged change it replays cost, and leaves no fades for the next step to wait on. `restart_load_watch` is not called for such a crossing and `crossings_sent` is not raised: it puts up no wall, it is a change (T06b costs each step against the guard itself).
- `Fades` (`cross.rs`), a `Wall` field, empty but during a crossing:
  - `cables: Vec<Leaving>` (`to_slot`, `jack`, `from_slot`, `route`, `line`, `gone_at`), `modules: Vec<(usize, Kind, u64)>` (slot, kind, gone_at: the latest `gone_at` of its cables, or `start + margin` when it has none), `waiting: Vec<(u32, u64, u32)>` (cable id, enter_at, glide), `glides: HashMap<u32, (u32, u64)>` (cable id → glide, done_at, for entering and matched cables), `end: u64` (the latest of everything).
  - `fn glide_of(&self, cable: u32) -> u32` (0 when not there), `fn waits(&self, cable: u32) -> bool`, `fn lines(&self) -> impl Iterator<Item = u32>` (leaving lines), `fn slots(&self)`, `fn sinks(&self)` (leaving slots whose kind `is_sink`, D03b), `fn edges(&self)` (leaving cables as `(from_slot, to_slot)`), `fn forget_slot(&mut self, slot: usize)` (drops every leaving cable from or to it), `fn is_empty(&self)`.
- `send_tables` (`:1516`), with fades:
  - `slots`, `edges` and `outs` also hold the fades' slots, edges and leaving `out` slots, so the render order keeps every leaving module that is still heard through its leaving cables. `heard_order` (`unheard.rs:11`) takes the fades' sinks as sinks too (one `.chain(self.fades.sinks())`): a leaving `out` still plays into the master while its inputs fade.
  - The patch's cable loop (`:1573`) skips a cable that `waits`, and gives each route `glide: self.fades.glide_of(cable.id)`.
  - After it, each leaving route goes into the table at its `(to_slot, jack)` when that jack is still empty, with `amount: 0.0` and its glide. A jack a seat has plugged since (below) keeps the seat's cable.
  - Sending a table again mid-crossing (any seat's change, a re-timing) restarts no fade: the amounts are the same, so the engine's glides run on (T05a).
- `assign_lines` (`:1672`) counts `self.fades.lines()` as taken.
- `Rack` (`:247`) gains `leaving: BTreeMap<usize, u32>` (slot → tag): not free, not indexed by id, `id_at` and `id_of` `None` (so a fault from a leaving module is "gone since", `tick`, `:4414`, and `unheard` never lists it). `fn leave(&mut self, id: &str) -> Option<usize>` moves a placed module there; `fn gone(&mut self, slot: usize)` frees it; `fn reindex(&mut self, renamed: &[(String, String, usize)])` takes every matched module out of the index first and then puts each back under its new id, so ids that swap between the walls never collide; `empty()` (`:298`) also empties `leaving` into the slots it returns.
- `cross_tick(&mut self)`, called from `tick` (`:4383`) right after `settle`, when the fades are not empty, with `now = self.engine.shared().frames()`:
  1. Leaving cables whose `gone_at <= now` are dropped; waiting cables whose `enter_at <= now` move to `glides` with their glide (they go in on a fresh tag, so they start silent, T05a); glides whose `done_at <= now` are dropped. Waiting and glide entries for cable ids no longer on the patch (a seat unplugged them) are dropped.
  2. Leaving modules whose `gone_at <= now` and that no leaving cable still touches are due.
  3. If anything changed: `send_tables()`, then `Command::Remove { slot }` and `rack.gone(slot)` for each due module (tables first, as `mirror`'s `Remove` arm does, `:1872–1881`).
  4. When the fades are empty: `restart_load_watch()` (the old wall is gone; measure what is left) and `retime_now()` (`:1812`).

### While it crosses

- **A seat's change.** The patch is the moment's, so every request is answered against it as on any wall, and logged, and committed by T04a as the bud's commit. In the engine: a turn's `Command::Knob` takes over that knob's glide (the newest glide wins, as now); a cable plugged into a jack a leaving or waiting cable holds replaces it at once (the leaving route is no longer put in, the waiting one is gone from the patch), as any plug replaces; an add is placed as now; a remove frees its slot, and `mirror`'s `Remove` arm calls `self.fades.forget_slot(slot)` right after `rack.free` (`:1877`), so no leaving cable reads from or writes into a slot a later add may reuse. Every fade not touched finishes on time.
- **Another go during a crossing.** The plan is made from the wall up now (the patch and the rack, matched and born modules alike) against the new moment; the fades under way stay in `self.fades` with their own `gone_at`s and finish on their own. A cable still waiting from the first crossing is "the cable on that jack" for the second: matched, it keeps its `enter_at`; not matched, it is dropped from the waiting list and never enters. A new cable for a jack that a leaving cable still holds waits until that cable's `gone_at` (rule 2's wait applies to every leaving cable on the jack, not only the one this crossing made). Nothing is cut.
- **A cut during a crossing** (a load, a new wall, a go with `beats` 0, a crossing made a cut): `swap_engine` (`:1413`) also sets `self.fades = Fades::default()`; `rack.empty()` returns the leaving slots too, so they are removed under the hush with the rest.
- **The audio restarting during a crossing** (`attach`, `:1252`, `load_engine`, `:1370`): `load_engine` sets `self.fades = Fades::default()` (its `Rack::default()` forgets the leaving slots with the old engine); the new engine gets the moment's wall at once. The crossing ends as a cut would, because the old engine is gone.
- **A daemon restart** starts on `patch.json`, which is the moment's wall plus any later change: nothing of a crossing is kept or needs to be.
- **Undo during or after a crossing.** Undoing the go (T04b's `Asked::Back`) crosses back over the beats the go took: `moments.rs` reads `beats` from the logged `What::Go` it undoes and passes it in `Source::Back`; from the wall up now, under the "another go" rule above. A go that was a cut is undone as a cut. Undoing a change a seat made during the crossing is an undo like any other. Undoing a change made before the go is refused as T04b refuses it (replaced since).
- **The clock line** breaks at the go, as T04b breaks it for every go (KZ-T02a): the go's change is the first stamp on the new line. A tempo gliding across the crossing does not break it (KZ-T02a, rule list: a tempo change never does).

### What it costs, and when it is a cut instead (`crossing_beats`, `crossing_load`)

`fn crossing_beats(&self, incoming: &Patch, asked: f64, notes: &mut Vec<String>) -> (f64, Option<Box<CrossPlan>>)`:
- `asked` 0 (or not finite, or below 0: never sent, T05c checks it): `(0.0, None)`.
- Make the plan with `CrossTiming::go(N)`.
- **Memory.** Both walls' modules are in the engine at once: `engine.reserve_slots(rack.slots_for(born))` and `engine.reserve_lines(lines + entering + waiting + leaving)` must both succeed (they send larger rooms ahead, as every add does). If either fails: a cut, with the note `"the machine has no memory to hold both walls at once, so the wall cut to the moment instead of crossing"`.
- **The load guard (KZ-D03b).** While crossing, every module of the wall up now still renders (matched and leaving alike), and the born ones render too. So `crossing_load(&plan)` is `shared().render_load()` plus `kind_share(kind, rate)` (D03b, `daemon/wall/cost.rs:105`, `pub(super)`) of every born module, heard or not. Reason for not asking which are heard: D03b's `heard_modules` is private to `cost.rs`, and counting an unheard born module at its bench cost errs toward a cut, which is the safe side; born modules nobody hears are rare (a moment's wall was played as it is). When the guard is on (`load_watch.on`) and that is past `LOAD_GUARD`: a cut, with the note `"both walls at once would take the wall to about <p>% of its time rendering, past the <g>% it keeps to, so it cut to the moment instead of crossing"`. A cut was already costed by D03b's `load_room_as` at the top of `replace` (and refused for a seat, or noted for Tom's console, as D03b decided), so the cut that replaces a crossing never costs more than the go was allowed to. This holds for Tom's console too: crossing is never worth an overload, and the console still gets its moment.
- Else `(asked, Some(plan))`, with the note `"the wall crosses to it over <b> beats (<s> s at <bpm>): <m> modules glide, <i> fade in, <o> fade out, <c> cables crossfade"` (`<c>` counts leaving, entering, waiting and matched-with-a-new-amount cables).
- `request_as` (`:2262`): a wall put up is not "additions not yet taken in" whether it was swapped or crossed: `let (held, swaps, crossings) = (…, self.crossings_sent)` and `if self.swaps_sent == swaps && self.crossings_sent == crossings { self.note_held(held) }`.

### Off the audio thread, and NaN

Everything in this lane runs on the control thread. The audio thread does what T05a gave it: glide a cable's amount and the tempo, and glide knobs as it always has. Every number sent is finite: targets come from a loaded patch (`Patch::load` keeps knobs in range), amounts from `CableTable::set`'s clamp, frames from a clamped `f64`; `N`'s computation turns a NaN into 1 frame through `max(1.0)` before the cast.

## Not this lane

- The request's `beats`, `wall_go`'s argument, the console's words and keys: T05c. Until it lands every go crosses over `DEFAULT_CROSS_BEATS` and its undo over the beats its go took.
- Loads crossing. A load of a save stays a cut: PLAN.md asks for crossings between moments; a load is not one. (For Tom: see below.)
- The fire (T06), which uses `cross` with `CrossTiming` for its steps.
- Any change to the engine (T05a), the tree thread, or the master's hush.

## Red tests first

In `daemon/wall/cross/tests.rs`, with a real `Wall` on a `Store::peek` state directory and its `Engine` (built as `daemon/wall/tests.rs:16`), driven by calling `replace` with `Source::Moment { to, from, beats }` and made-up `TreePlace`s (no tree needed: `cross` never touches it), and by rendering the engine in 512-frame blocks under `assert_no_alloc` (write the same helper as `daemon/wall/tests.rs:1407`) with `tick` between blocks. The two walls are made from one: `incoming = Patch::load(&wall.patch().to_file())` then changed by `Patch` calls (a knob, a module added or removed, an id renamed by editing the file), so births are shared exactly where the test says. Levels are read from the rendered master in windows of 2,048 frames. Each fails today because a go always cuts.

1. `shared_modules_stay_and_glide_every_knob`: a `vco` (`level` 0.2 → 0.8) and an `lfo` (`rate` changed) in both walls, the `vco`'s id renamed in the moment: after the go, each keeps its rack slot and tag, the engine has taken no `Insert` for it (the retire ring sends back no module during the crossing), `EngineControl::knob` reads between the two targets at `N / 2` and exactly the moment's target after `N`, and `rack.slot_of` finds it by its new id only.
2. `a_module_only_the_moment_has_fades_in_from_silence`: an `out` alone, then a moment with a `vco` into it, 4 beats at 120 (96,000 frames): the first window's peak is under 0.01, the window at the half within 25% of half the last window's, the last window full; no window's peak is more than 1/8 of full above the one before.
3. `a_module_the_moment_lacks_fades_out_then_goes`: the reverse: the level falls the same way, the master never jumps, and after `N` plus the margin and one tick the `vco`'s slot is free in the rack, its line is free, and the engine sent its processor back over the retire ring.
4. `an_input_that_changes_source_crosses_through_silence_at_the_half`: `vco1` and `vco2` (different `freq`) in both walls, `out1.left` fed by `vco1` then by `vco2`: the window at `N / 2 + margin` is under 0.05 of full, the first half carries `vco1`'s frequency only and the second `vco2`'s only (count zero crossings per window), and the two are never summed (no window's peak above one source's).
5. `a_cable_whose_amount_differs_glides_on_its_own_line`: amount 1.0 → 0.25: the level glides, the cable keeps its line and tag, and the engine's line history is not emptied (no dropout at the start).
6. `the_same_birth_in_another_kind_is_two_modules`: a file edited to give a `vcf` the birth of a `vco` that leaves: one fades out and one fades in.
7. `speakers_match_only_with_their_id_and_words`: the same speaker and words: kept, no `forget`; other words: faded out and the new one's words put back.
8. `a_seats_changes_during_the_crossing_are_made_to_the_moment`: half way, a turn on a matched module (by its new id), an unplug of a waiting cable, a plug into a jack a leaving cable holds, and the removal of a matched module whose input still has a leaving cable, then an add that takes its slot: each is answered and logged as on any wall; the fades not touched end on time; the added module reads nothing from the removed one's leaving cable.
9. `a_go_during_a_crossing_carries_on_from_where_the_engine_is`: A → B, then at `N / 4` B → C: A's leaving modules go at their own `gone_at`; no window jumps; at the end the rack holds exactly C's modules under C's ids and `fades` is empty.
10. `a_cut_or_a_new_engine_mid_crossing_ends_it_cleanly`: a load mid-crossing: every slot, leaving ones included, is removed under the hush and `fades` is empty; and `attach` of a new engine mid-crossing: `fades` empty, the moment's wall in the engine.
11. `the_tempo_glides_unless_the_desk_keeps_it`: 120 → 90: `shared().bpm()` near 105 at the half, 90 at the end; with a desk link connected (copy `a_desk`, `daemon/wall/tests.rs:3433`, into this file: it is private to that module), no tempo glide is sent and the tempo stays the desk's.
12. `a_go_of_no_beats_is_todays_cut`: `beats: 0.0`: `swaps_sent` rises, the hush is sent, the change's words are T04a's exactly, and `fades` is empty.
13. `a_crossing_past_the_guard_or_without_memory_is_a_cut_and_says_so`: the guard on, `set_render_load(0.7)`, a moment with heavy born modules (D03b's dearest kind): `What::Go` has `beats: 0.0`, the note says so with the percentage, the hush was sent. The memory branch cannot be reached on a real engine without exhausting the machine, so `crossing_beats` takes its decision through a pure `fn cross_or_cut(asked: f64, room: bool, guard_on: bool, projected: f32) -> Option<&'static str>` (the reason for a cut, or none), and this test also checks it for no room, the guard off, exactly at the guard, and just past it.
14. `the_go_and_its_words_say_the_crossing`: the logged `What::Go` has `beats: 16.0`; a JSON line from T04a's era with no `beats` key reads `0.0`; the summary holds `; crossing over 16 beats`; the `LoadResult` notes hold the counts sentence.
15. `nothing_is_built_or_freed_on_the_audio_thread`: tests 2, 3, 4 and 9 render every block under `assert_no_alloc` (the helper fails the test on any allocation or free).
16. With a planted tree (as T04b's tests plant one; `git` 2.36 or later): `undoing_a_go_crosses_back_over_its_beats`: a go of 16 beats, then its undo: the undo's `What::Go` has `beats: 16.0` and crosses (no hush); the wall afterwards equals the wall before the go (`fingerprint`); and `a_change_during_the_crossing_is_the_buds_first_commit`: a turn at `N / 2` makes one commit on `bud/<moment>/<time>` whose parent is the moment and whose files are the moment's plus the turn.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/daemon kazoo-wall/src/protocol kazoo-wall/src/change.rs kazoo-wall/src/tree kazoo-wall/src/tui/tests kazoo-mcp/src/render
```
Doc tests, because `What::Go` and `Rack` docs change: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 16 tests (18 functions) fail first (paste each under RED), then pass; every test that was green is green.
- [ ] `DEFAULT_CROSS_BEATS`, `MAX_CROSS_BEATS`, `What::Go::beats`, `CrossTiming`, `CrossPlan`, `Fades`, `plan_crossing`, `crossing_load`, `crossing_beats`, `cross_or_cut`, `cross`, `cross_tick`, `note_turned`, `Rack::{leave, gone, reindex}`, `Wall::{fades, cross_plan, crossings_sent}` exist as named, each constant with its reason.
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, on Tom's wall with Tom there, does this: mark "before cross", turn three knobs and add a module, `wall_go` "before cross", listen, then `wall_undo` of the go. Say what was heard at the start, the half and the end of each, and paste the two log lines.
- [ ] Under BENCH: the control thread's time for `plan_crossing` plus `cross` between the evening wall (KZ-T01's fixture, 85 modules) and the same wall with ten knobs changed, three modules added and three removed (mean of 100, release build); and the render load during and after such a crossing on a test daemon (the guard's own reading), with `uptime`'s load average.

## Decisions for Tom (Cassio, please read these to him)

1. A crossing's default is 16 beats; seats and the console may ask 0 (a cut) to 64 (T05c).
2. The tempo glides across the crossing (a ritardando or accelerando), rather than jumping at the start or the end.
3. An input that changes source goes quiet at the half (old out over 8 beats, new in over 8), because one jack holds one cable.
4. Shared modules keep running through a crossing (their phase, step and tails), so a crossing to a moment never sounds bit for bit like a cut to it; a cut (beats 0) is still there for that.
5. A crossing that would overload the wall becomes a cut, for Tom's console too.
6. Loads of saves stay cuts. If Tom wants loads to cross too, it is a small lane after T05c (a save's modules carry their births since T01).

## Code quality

As PLAN.md's STACK RULES. `cross.rs` under 1000 lines (about 600 expected); if it nears it, `Fades` goes to `cross/fades.rs` (an eleventh file: say so). `daemon/wall.rs` (4,691 lines before T04) gains only the fields, `Rack`'s four methods, the `replace` lines, the `mirror` arm, `send_tables`' overlay, `assign_lines`' line, the two clears, the `Remove` arm's line, `tick`'s call, `request_as`'s comparison and `note_turned`: list its line count under FOUND. Everything that allocates is on the control thread. No `as` cast clippy pedantic flags without the tree's comment. No `unwrap` outside tests.

## END

```
START KZ-T05b base=<sha> branch=lane/kz-T05b df=<GB> at <time>
END KZ-T05b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: plan and cross times; render load during and after; load averages
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
