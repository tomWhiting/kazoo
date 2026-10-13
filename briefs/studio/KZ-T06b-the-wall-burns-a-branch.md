# KZ-T06b: set the tree on fire: the wall plays a branch again from any moment, each moment at the beats it came after the last (any speed from a quarter to 64 times), and the first change anyone makes puts the fire out and grows a bud where it had reached

Line numbers are from the sweep that wrote this brief, at `820ccc5` (code as at `d2b10b4`), and may have moved; find each item by what it does, never rename. Items from KZ-T01 to T06a are named as those briefs name them (`Stamp`, `beats_between`, `ClockLine`, `Change::clock`, `What::Go`, `TreePlace`, `Source::Moment`, `Asked`, `Waiting`, `tree_answers`, `TREE_ANSWER_WITHIN`, `GO_TRIES`, `CrossTiming`, `plan_crossing`, `crossing_load`, `cross_plan`, `fades`, `load_room`, `What::Fired`, `FiredGlide`, `fired_files`, `Walk`, `Step`, `StepHead`, `TreeItem::{Walk, Steps}`, `Answered::{Walked, Steps}`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Second of PLAN.md's T06 split in four (T06a the tree's side, **T06b** the wall burns, T06c the seats' `wall_fire`, T06d the console's `F`).

Read by Cassio, 13 Oct evening, line by line. Changes: the lane opens only after T06a has landed; the by-hand listen is Cassio's with Tom after landing; and the note at the end (fire steps do not dim lanterns) applies if T07b has landed first. The six decisions below are taken as written and will be read to Tom. Accepted otherwise as written.

Time: about 75 minutes of a builder's work. One mechanism: the fire, a clock on the control thread that walks the tree's moments and plays each one at its beat. Crates: kazoo-wall, and one arm in kazoo-mcp. Depends on T06a and everything it depends on, which must have landed.

Files (seven):
1. `kazoo-wall/src/protocol/mod.rs`: `Request::{Fire, FireOut}`, `FireResult`, `FireOutResult`, `Event::Fire`, `FireNews`, the speed constants.
2. `kazoo-wall/src/daemon/wall.rs`: `Wall::fire`; routing; putting the fire out before a change; `tick`'s call; `close`; `puts_up_wall`.
3. `kazoo-wall/src/daemon/wall/fire.rs` (new): `Fire`, `fire`, `fire_out`, `burn`, `burn_step`, `offset`, `walk_on`, `put_out`.
4. `kazoo-wall/src/daemon/wall/fire/tests.rs` (new).
5. `kazoo-wall/src/daemon/wall/moments.rs`: `Asked::{Fire, Steps}` and their answers in `tree_answers`.
6. `kazoo-mcp/src/channel.rs`: `Event::Fire` is news; a step is not, one by one.
7. `kazoo-wall/src/tui/app/mod.rs`: `on_event`'s `Event::Fire` arm; a step that puts up a wall of its own is taken as a load is.

## Outcome

Tom's image: "set the tree on fire ... all the smoke that comes off it is the music". A seat or the console lights the fire at a moment: the wall goes there (a crossing over 16 beats, unless it is there already), waits for the bar line, and then plays every moment that grew after it on the branch, in order, each at the beats it originally came after the one before, divided by the speed. A turn glides as it glided then; a cable, a module, a tempo arrives as it did; a load cuts as it cut. Nothing is written in the tree: the past is there already, and the tree's place follows the fire (T06a). Every seat sees each moment as a change in the log ("Tom played moment a1b2c3d4e5f6 again on trunk: Cassio turned …"), and is told when the fire is lit, when it goes out and when it reaches the tip.

The first change anyone makes while it burns puts it out where it reached, and that change grows a bud from there: the old line is never burnt on with new changes in it, and the past never changes. Reaching the tip, the wall is back on that branch, and the next change carries the branch on. A recording, a mark or a save does not put it out: recording the fire is the point.

## The design (decided; build exactly this)

### Requests, answers, news (`protocol/mod.rs`)

```rust
/// Set the tree on fire: put the wall at the moment `from` (as a go does,
/// crossing over DEFAULT_CROSS_BEATS, unless it is there already) and play
/// every moment after it on the branch `along` again, each at the beats it
/// came after the one before, divided by `speed`: a `FireResult`. `from` is
/// anything a go takes; a branch named alone burns from its first moment.
/// `along` left out: trunk if it holds the moment, else the newest bud that
/// does. Any seat but a watcher; it draws on the seat's allowance of walls
/// put up, as a go does. Any change while it burns puts it out (see
/// `Wall::douse`).
Fire {
    from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    along: Option<String>,
    /// 0.25 to 64; left out, 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    speed: Option<f64>,
},
/// Put the fire out where it has reached: a `FireOutResult`. Any seat but
/// a watcher.
FireOut,
```
`op()`: `"fire"`, `"fire_out"`. `is_change()`: both (watchers are refused them, `control.rs:997`, and they count against the flood guard). `acts_on_modules()`: neither.

```rust
pub const DEFAULT_FIRE_SPEED: f64 = 1.0;
pub const MIN_FIRE_SPEED: f64 = 0.25;
pub const MAX_FIRE_SPEED: f64 = 64.0;
pub struct FireResult { pub from: TreePlace, pub along: String, pub moments: u32, pub speed: f64,
                        #[serde(default, skip_serializing_if = "Option::is_none")] pub go: Option<LoadResult> }
pub struct FireOutResult { pub at: TreePlace, pub played: u32, pub left: u32 }
/// Event: the fire was lit, went out, or reached the tip. Not logged (its
/// steps are). Sent to every subscriber.
Fire { seat: String, news: FireNews },
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FireNews {
    Lit { from: TreePlace, along: String, moments: u32, speed: f64 },
    Out { at: TreePlace, played: u32, left: u32, why: String, cause: FireOut },
    Done { at: TreePlace, along: String, played: u32 },
}
/// Why a fire went out, for a console to colour it (`why` says it in words).
#[serde(rename_all = "snake_case")]
pub enum FireOut { Changed, PutOut, LitAgain, Stopped, Tree, Log, Guard, Broken }
```
Speed's reasons, in the constants' doc comments: below a quarter, a gap capped at 16 beats (below) becomes 64, the longest any glide is (`MAX_GLIDE_BEATS`, `patch.rs:33`); above 64 times, a 2-beat glide at 120 BPM lasts 16 ms, under the 20 ms between ticks, so the moments only bunch: nothing more can be heard. A speed that is not a number is `bad_request` "a fire's speed is a number from 0.25 to 64 (1 is as it was played)"; any other is held to the range, as a glide is (`patch.rs:575`).

### Lighting it (`fire.rs`, `Wall::fire`)

1. As T04b's `go`: no tree or the tree down is `internal` with T04b's sentences; `from` 1 to `MAX_MOMENT` bytes with no control character, else `bad_request`. Send T06a's `Walk { ticket, from, along }`; pending, `Asked::Fire { made, tries, from, along, speed }` (T04b's staleness rule: if `self.made` moved before the answer, ask again, up to `GO_TRIES`, then `slow_down`).
2. The `Walked` answer:
   - A fire already burning is put out first, `why` "lit again by <seat>".
   - `walk.here.commit == walk.from.commit`: the wall is there; no go, and the seat's allowance of walls put up comes back (nothing was put up).
   - Else a go, exactly T04b's step 4 with T05b's crossing: `Patch::load(&walk.start_wall)`, `self.replace((seat, console), incoming, &Source::Moment { to: walk.from, from: walk.here, beats: DEFAULT_CROSS_BEATS }, notes)`. Its refusal is the fire's refusal. Its `LoadResult` goes in `FireResult::go`. A seat that wants a cut first goes with `beats` 0 (T05c) and then lights the fire where the wall is.
   - `self.fire = Some(Fire { … })`, with `prev` = `walk.start` (the moment the wall is at), the moment ids, `speed`, the phase `Settling`; ask `Steps` for the first `FIRE_AHEAD` ids.
   - Answer `FireResult`; the outcome's event is `Event::Fire { seat, news: Lit { … } }`.
3. A refusal from the tree comes back as T04b maps `HereError`s.

### The fire's clock (`fire.rs`, `burn`, called from `tick` right after T05b's `cross_tick`)

- `Settling`: while `self.fades` is not empty (the go's crossing), nothing. Then the clock starts: `walked = 0`, `last_beat = shared().beat()`, and the first wait is the bar phase (below). Phase `Burning`.
- Each tick, `walk_on(walked, last_beat, beat_now) -> (walked, last_beat)`: `d = beat_now − last_beat`; a `d` that is not finite, below 0 or above `MAX_FIRE_GAP_BEATS × 4` is taken as 0 (the clock jumped: a restart of the engine's beat, the desk taking the clock or locating; a jump is not time passing). Reason: the fire counts the music's beats as they pass, at whatever tempo plays, so a tempo change mid-fire (its own steps, or the desk's) slows or hastens it as it would the band.
- **Each moment's due beat** is the previous one's plus `offset(prev, next) / speed`, where `offset` (in beats) is KZ-T02a's rule, with KZ-T03's imported moments under it:
  - both stamped on the same clock line (`beats_between(&prev.stamp, &next.stamp)` is `Some(d)`): `d`, at least 0;
  - otherwise (a restart, a load, the desk taking the clock between them; or a moment without a beat, which every moment KZ-T03 imported is): the UTC gap between their `at`s (whole seconds, at least 0), in beats at the earlier moment's tempo (`prev.stamp.tempo`, or when it has none, `prev.tempo`, the tempo its wall held), at most `MAX_FIRE_GAP_BEATS`.
  - `pub(super) const MAX_FIRE_GAP_BEATS: f64 = 16.0;` KZ-T02a's cap: a fire must not stall for the hours a restart can span, and four bars is the crossing's length, so a fire passes a break as a crossing would.
- **The bar phase.** The first moment's due is `wait + offset(start, first) / speed`, where `wait` puts the start on the bar phase it had: with `start.stamp.beat = Some(b0)`, `wait = (b0 − beat_now).rem_euclid(FIRE_BAR_BEATS)`; with no beat, `(−beat_now).rem_euclid(FIRE_BAR_BEATS)`, the next bar line. `pub(super) const FIRE_BAR_BEATS: f64 = 4.0;` The wall counts in 4/4 from beat 0 (its clock and the desk's song position), so at speed 1 every moment on one clock line lands on the beat of the bar it landed on first; the wait is under a bar.
- **Playing what is due**: while the front of `ahead` is due (`walked ≥ due`) and fewer than `MAX_STEPS_PER_TICK` have played this tick, `burn_step` it. `pub(super) const MAX_STEPS_PER_TICK: usize = 16;` Each step is a logged line, a plan and some engine commands on the control thread (well under a millisecond each, T05b's BENCH); 16 in a tick keeps a tick under a few milliseconds and plays 800 moments a second, more than the log's busiest second ever held (30) at 16 times speed. When more are due the fire falls behind and catches up in order; it never skips one.
- **Reading ahead**: when no `Steps` job is out, fewer than `FIRE_REFILL_AT` steps are ahead and ids remain, ask `Steps` for the next `FIRE_AHEAD`. `pub(super) const FIRE_AHEAD: usize = 32;` and `pub(super) const FIRE_REFILL_AT: usize = 16;`: two ticks' worth of moments at the most a tick plays, so a batch read (under 30 ms, T06a's BENCH) is back before the moments ahead run out. A due moment not read yet waits for its read (the fire is behind, and plays it the moment it comes). A `Steps` job unanswered past `TREE_ANSWER_WITHIN` (T04b, 5 s) puts the fire out, `why` "the tree did not give the next moments within 5 s".
- After the last id has played, the fire is done: `Event::Fire { news: Done { at, along, played } }`, `self.fire = None`.

### Playing a moment (`burn_step`)

1. `let (incoming, notes) = Patch::load(&step.wall)` (`patch.rs:1319`); an `Err` puts the fire out with `why` "the moment <short> cannot be put up: <its sentence>". The notes (a knob or kind this build no longer has) are counted and said once per fire in the daemon log.
2. `replaces = step.head.wall != prev.wall` (the file's `wall` number: it moves exactly when the original put up a wall of its own, KZ-T01/T03).
3. The glides: every `What::Turn` in `step.change` (walking `What::Chain`s in order) gives `FiredGlide { module, knob, beats: (glide_beats / speed).clamp(0.0, MAX_GLIDE_BEATS) }`.
4. The plan, unless `replaces`: `plan_crossing(&incoming, CrossTiming { knobs: 0, knob: <each glide's (module id, knob index) in frames at the tempo playing>, fades: 0, tempo: 0 })` (T05b). With `fades: 0` the moment's cables, modules and tempo arrive at once, as the original change made them; knobs not turned by the moment's change but different (a gap or settle moment) move in the engine's shortest glide (1 ms, `MIN_GLIDE_SECONDS`).
5. **The guard** (KZ-D03b), when it is on: for a step that does not `replace`, `crossing_load(&plan)` (T05b) past `LOAD_GUARD`; for one that does, D03b's `load_room(&incoming)` refusing (its sentence carries the percentage; `load_room` is `pub(super)` in `daemon/wall/cost.rs`, `load_projection` is not, so use `load_room`'s answer and never reach past it). Then a seat's fire goes out before the moment, `why` "the next moment (<short>) would take the wall to about <p>% of its time rendering, past the <g>% it keeps to" (for a replacing step, `load_room`'s own sentence); a fire lit by Tom's console plays it, and says so once per fire in the daemon log (`kazoo-wall: Tom's fire played <short> at about <p>% of the wall's time, past the <g>% guard`). Reason: as D03b decided for loads, Tom's moments are his to put up; a seat's fire must not knock the wall over for everyone.
6. The change: `What::Fired { commit, parent: prev.commit, along, change: (kind is change).then_some(stamp.seq), by, said, glides, replaces, tip: (this is the last id), modules, cables, tempo: self.playing_after(&incoming, &mut notes) }`, by the seat that lit the fire. `next_change` then `record` (`daemon/wall.rs:1931`, `:1954`), as `commit` does; a log that cannot take it puts the fire out with the log's sentence, and nothing of the moment is made.
7. `incoming.last_seq = change.seq`; `incoming.wall = if replaces { change.seq } else { self.patch.wall }`. Then `self.cross_plan = plan`, `self.fired_files = Some(step.files)` (T06a), `self.patch = incoming`, `let (change, moved) = self.apply(change)` (which mirrors it: a crossing with no fades, or a cut for `replaces`, T06a), `self.dirty = true`, and `Event::Change` (and the fingerprints' event, if any) go into `self.told`, as `tell_paid` tells the wall's own changes (`:2011`).
8. `prev = step.head`, `played += 1`.

The flood guard is not drawn on (the steps are the fire's, not the seat's requests). The clock line (KZ-T02a) breaks at a step that `replaces`, as at a load (T06a), and runs on through the rest.

### What puts it out (`Wall::douse`, in `request_as`)

- In `request_as` (`daemon/wall.rs:2195`), after the flood guard and the allowance and before `answer` (`:2260`): if a fire burns and the request is one of `Turn`, `Patch`, `Unpatch`, `Add`, `Remove`, `Arrange`, `Group`, `ChainAdd`, `Undo`, `Tempo`, `Speak`, `Load`, `New`, `Go`, take the fire out of `self.fire` and hold it. After `answer`: an `Err` (refused, so nothing changed) puts it back as it was; an `Ok` puts it out, `why` "<seat> changed the wall (<op>): the next change grows a bud from here". `FireOut` is not in the list: its own handler, `fire_out`, puts the fire out (`why` "put out by <seat>") and answers `FireOutResult`. A request answered later (a go, the undo of a go) is `Ok` when asked, so it puts the fire out then, even if the tree refuses it afterwards: the seat has said it wants the wall elsewhere. Nor is `Fire`: a new fire puts the old one out only once its walk is answered ("lit again by <seat>", above), so a refused lighting leaves the old fire burning. Nothing else is done: the wall is at the last moment played, the tree's place is there (T06a), and the change just made is the bud's first commit (T04a).
- Not: `Record` (recording the fire is the point), `Save`, `ChainSave`, `DeleteSave`, `Mark`, `Monitor`, and every read. Reason for `Arrange` and `Group`: they are not logged changes, but the next moment's files would put the rack back as that moment had it, silently undoing Tom's move; so they put it out too.
- `put_out(&mut self, why: String, cause: FireOut)` is the one way a fire ends early, and each place names its cause: a change `Changed`, `fire_out` `PutOut`, a new fire `LitAgain`, `close` `Stopped`, the tree down or slow `Tree`, the log refusing a step `Log`, the guard `Guard`, a moment that cannot be put up `Broken`.
- `close` (`:3865`) puts it out, `why` "the wall stopped"; a fire never survives a restart. The tree down (`TreeHealth::error`, KZ-T02b) puts it out at the next tick, with the error.

### What the fire cannot promise (precision, said plainly)

- **When each moment plays.** A moment is played at the first tick (at most 20 ms) after its due beat, and is heard from the next device buffer (at most 10.7 ms at 512 frames and 48 kHz): up to about 31 ms late, 0.06 beats at 120 BPM. What the original's stamp says is itself within one device buffer of when it first sounded (KZ-T02a: 0.021 beats at 120 BPM). So, at speed 1, a stamped moment replays within about a sixteenth of a beat of its place.
- **Imported moments** (the first rings, KZ-T03) have no beat, only whole UTC seconds: two in one second play together, and one second is 2 beats at 120 BPM. Their order is always exact.
- **Frame-exact replay** needs KZ-W02 (not written): changes stamped with the frame they first sounded at, and an engine that takes a command "at frame F" from a timed queue. Neither exists. This lane neither builds nor fakes them.
- **The sound.** The wall after each moment is that moment exactly (its files), and seeded randomness (W01) means a module built afresh (at a cut, or born in a step) starts as it started then. But the fire begins from the moment's wall as it is now, not from the running state it had then (an LFO's phase, a sequencer's step, a delay's tail), and modules carried across moments keep their running state; so the smoke is the same music, not the same samples.

## Not this lane

`wall_fire` (T06c). The console's `F` and the fire's look (T06d). Changing speed while it burns (put it out and light it again). Burning more than one branch at once.

## Red tests first

In `daemon/wall/fire/tests.rs`, with a real `Wall` and `Engine` on a `Store::peek` state directory and a tree planted with `tree::git::plant` (as T04b's tests; `git` 2.36 or later), the engine rendered in 512-frame blocks under `assert_no_alloc` with `tick` and `tree_answers` after each (as T04b's tests collect late answers). Original moments are made by real requests with renders between them, so their stamps carry real beats. Each fails today because there is no `Request::Fire`.

1. `a_fire_plays_each_moment_at_its_beats`: turns made 2, 4, 1 and 8 beats apart; the wall put at the first (a go with `beats` 0, to keep the test short); `Fire { from: "#<first>" }`: no go in the answer; the `Fired` changes come in order, and the beats between their own stamps equal the originals' within 0.05.
2. `speed_divides_every_gap_and_every_glide`: speed 4: gaps a quarter; a turn's `FiredGlide` is a quarter of its `glide_beats`; speed 0.1 is held to 0.25, 100 to 64, NaN is `bad_request`.
3. `across_a_break_or_without_a_beat_the_utc_gap_counts_capped_at_16`: moments on two clock lines 3 s apart at 120 BPM wait 6 beats; an hour apart, 16; moments written as KZ-T03 writes them (no beat) 2 s apart wait 4 beats.
4. `after_each_moment_the_wall_is_that_moment`: after every step, `tree::to_files(&wall.patch().to_file())` equals the step's `files` but for `wall.toml`'s `last_seq` and `wall`.
5. `the_fire_starts_on_its_moments_bar_phase`: a start moment stamped at beat 33.5: the first moment's due is counted from a beat whose phase in the bar is 1.5.
6. `a_change_puts_the_fire_out_and_grows_a_bud_where_it_reached`: mid-fire, a turn: `Event::Fire` `Out` with `why` naming the seat and `turn`; no more `Fired`; the turn's commit's parent is the last moment played. A turn on an unknown module (refused) leaves it burning; a recording started, a mark and a save leave it burning; an arrange puts it out.
7. `the_fire_reaches_the_tip_and_the_wall_is_on_the_branch`: `Done`; the next turn extends the branch.
8. `lighting_from_elsewhere_goes_there_first_and_waits_for_the_crossing`: the answer carries the go (`What::Go` with `beats: 16.0`); no `Fired` is logged until `fades` is empty.
9. `a_moment_that_put_up_a_wall_is_played_as_a_cut`: a line with a load in it: that step has `replaces`, the hush is sent, `patch.wall` is the step's seq, and a seat that looked before is `wall_replaced`.
10. `the_guard_puts_out_a_seats_fire_but_not_toms`: the guard on, `set_render_load(0.7)`, a moment that adds heavy modules: a seat's fire goes out before it with the sentence; the same lit by the console plays it.
11. `put_out_and_lit_again`: `FireOut` gives `played` and `left` and the `Out` news; `FireOut` with no fire is `not_allowed` "the tree is not on fire"; `Fire` while burning puts the old one out with "lit again by" and lights the new.
12. `moments_and_branches_that_cannot_burn_are_refused`: an unknown moment (`unknown_moment`), a moment at the tip ("nothing to burn"), a branch that does not hold it, a watcher (refused by `control.rs`), and the allowance given back for each.
13. `the_fire_reads_ahead_and_never_skips`: 300 moments at speed 64: no tick plays more than 16; every moment plays once, in order; at most one `Steps` job is out at a time.
14. `a_clock_jump_is_not_time_passing`: `walk_on` with a beat going back, a NaN, and a jump of 1,000 beats adds nothing; a normal sub-block adds its beats.
15. `a_slow_tree_or_a_stopped_wall_puts_it_out`: the thread held by KZ-T02b's paused constructor past 5 s while a `Steps` job is out: `Out` with the sentence; and `close` mid-fire: `Out` "the wall stopped", then a new `Wall` on the state directory notes no `Resumed`.
16. `nothing_is_built_or_freed_on_the_audio_thread_while_it_burns`: tests 1, 9 and 13 render every block under `assert_no_alloc`.
17. In `kazoo-mcp/src/channel.rs`'s tests: `a_fire_is_news_and_its_moments_are_not_one_by_one`: `Lit` by another seat, `Out` and `Done` each make a line; a `Fired` change does not, except one that `replaces`, which says the wall is stale (urgent), as a load does.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/daemon kazoo-wall/src/protocol kazoo-wall/src/tui/app/mod.rs kazoo-mcp/src/channel.rs
```
Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 17 tests fail first (paste each under RED), then pass.
- [ ] `Request::{Fire, FireOut}`, `FireResult`, `FireOutResult`, `Event::Fire`, `FireNews`, `FireOut`, `DEFAULT_FIRE_SPEED`, `MIN_FIRE_SPEED`, `MAX_FIRE_SPEED`, `MAX_FIRE_GAP_BEATS`, `FIRE_BAR_BEATS`, `MAX_STEPS_PER_TICK`, `FIRE_AHEAD`, `FIRE_REFILL_AT`, `Wall::{fire, douse}`, `Fire`, `burn`, `burn_step`, `offset`, `walk_on`, `put_out` exist as named, each constant with its reason.
- [ ] BENCH: `burn_step`'s time on the control thread for a turn on the evening wall (mean of 1,000, release build), and the most moments a second the fire sustained in test 13's shape on a release build, with `uptime`'s load average.
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling (with T06c or T06d landed), Cassio, on Tom's wall with Tom there: light the fire from a mark of the evening at speed 1 and listen for a minute, then at speed 8 from the same mark; turn a knob mid-fire; `git -C ~/.kazoo/tree branch` shows the bud. Say what was heard and paste the news lines.

## Decisions for Tom (Cassio, please read these to him)

1. A change mid-fire puts the fire out (it does not keep burning the old line around the new change). The change grows a bud where the fire reached; the old line is untouched. Starting a recording, marking or saving does not put it out; moving a module on the rack does (the next moment would move it back).
2. Speeds from a quarter to 64 times; past that only the bunching changes.
3. The fire starts on the bar beat its first moment had, so at speed 1 every change lands where it landed in the bar.
4. Lighting a fire from somewhere else first crosses there over 16 beats; to start with a cut, go there with `beats` 0 first.
5. A seat's fire goes out rather than overload the wall; Tom's console's fire plays on, as his loads do.
6. Exactness: within about a sixteenth of a beat for changes made since KZ-T02a; within a second (2 beats at 120) for the imported first rings; frame-exact needs KZ-W02, which is not written. The sound is the same music from the moment's wall, not the same samples (running state is not in the tree).

## Code quality

As PLAN.md's STACK RULES. `fire.rs` under 1000 lines. `daemon/wall.rs` gains only the field, the routing arms, `douse`'s call and its put-back, `tick`'s call, `close`'s line and `puts_up_wall`'s arm: list its line count under FOUND. Nothing on the control thread waits for the tree thread. Nothing new runs on the audio thread.

## END

```
START KZ-T06b base=<sha> branch=lane/kz-T06b df=<GB> at <time>
END KZ-T06b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: burn_step time; moments a second sustained; load averages
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```


## Note: fire steps do not dim lanterns (Cassio, 13 Oct evening)

A lantern dims by the changes the wall has made since anyone touched it (KZ-T07a). A fire (KZ-T06) logs one `What::Fired` per step, and replaying the past is not new work. So the `revision` the wall passes to the tree thread's lantern jobs (`Write`, `Notes`, `Shape`) is the **work revision**: the wall's revision less the number of `What::Fired` changes in the log. `Wall` keeps `fired_steps: u64`, counted from the log at start (in the same pass that reads it) and raised by one for each `Fired` applied. Each lantern's own `change` field is written in the same work revision, so `since` stays consistent. Whichever of KZ-T06b and KZ-T07b lands second adds this, with a test: a lantern, then a fire of 50 steps, leaves `since` at 0.
