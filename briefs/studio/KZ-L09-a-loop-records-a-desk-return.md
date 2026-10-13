# KZ-L09: a loop set to record a desk strip records it on the bars Tom heard, however late its sound comes back from the desk

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: returns arrive at the wall's engine, and a loop writes them a fixed delay late at the positions they were heard. Crates: kazoo-wall only (new `src/engine/returns.rs`, `src/daemon/loops/returns.rs`; additions to `engine/mod.rs`, `dsp/mod.rs` (`Io`), `dsp/looper.rs`, `daemon/loops.rs`, `protocol/mod.rs`'s `LoopView`). Depends:
- KZ-L06 (the link asks for and receives returns);
- KZ-L03 (`from` in the patch);
- KZ-L05 (`LoopView`, where `late` and `missing` are shown);
- KZ-L02 (the loop).

Its by-hand check needs KZ-L07 and KZ-L08.

Read by Cassio, 13 Oct evening. Checked: 0.5 s is under the shortest loop (4 beats at 300 BPM, 0.8 s); a nudge of 0.02 s is 960 frames at 48 kHz. Changes: the checks are written out in the lane form, and the live check is Cassio's and Tom's. If `MAX_CHUNK_FRAMES` on the base makes the line longer than 32,768 frames, the power of two rounds up and the size in the doc follows; say so under CHANGED. Opens only after L02, L03, L05 and L06 have landed.

## Outcome

`wall_loop loop1 from mouth/dry`, then `wall_turn loop1 record 1`. Tom sings along to the wall, and at the next bar line the loop starts taking his voice from the desk. When he stops, the bar he sang comes round on the bar he sang it on, not 30 or 200 ms late. That holds although the desk plays the wall up to a quarter of a second after the wall renders it, and his voice comes back the same way. `look` says "from mouth/dry (on the desk)", or why not: "the desk has no strip called mouth/dry yet; asking again every 5 s", "the wall is not on a desk". Blocks that came back too late to place are counted ("late 3 blocks") rather than smeared.

## Why a delay

The wall renders ahead of the desk. A frame the wall renders now is played by the desk later, by up to the link's backlog (`LINK_BACKLOG_MS = 250`, `kazoo-core/src/ipc/link.rs:82`) plus the desk's own buffer. Tom sings to what he hears. The desk's return stamps each block with the wall's own stream frame that played at the same studio frame (KZ-L07), so the block tells the wall where it belongs, but it arrives after the wall has rendered far past that frame.

A loop records with read-before-write semantics: what is written at a position is heard one loop later (KZ-L02). So it can write the return a fixed `D` frames behind its read head, at the positions the head had then. Nothing heard changes: the material is still first heard on the next pass, exactly where it was sung. `D` only has to be longer than the worst lateness and shorter than the shortest loop.

## The design (decided; build exactly this)

### Constants

- `pub const RETURN_DELAY_SECONDS: f64 = 0.5;` in `dsp/looper.rs`. Reasons:
  - It is twice `LINK_BACKLOG_MS`, so a block late by the link's whole backlog, plus the desk's buffer, plus the hub's tick, is still in time.
  - It is under the shortest loop possible, 4 beats at `MAX_BPM` 300 (`engine/mod.rs:98`), which is 0.8 s. A write therefore always lands before the read head comes round to it, and read-before-write holds. A compile-time assert pins `RETURN_DELAY_SECONDS < 4.0 * 60.0 / MAX_BPM`.
- `pub const ASK_RETRY: Duration = Duration::from_secs(5);`: Tom starts the wall, then the mouth. Within five seconds of `mouth/dry` appearing, the loop is listening.

### The engine's return lines (`engine/returns.rs`)

- `pub struct ReturnLine { samples: Box<[f32]>, mask: u64, filled_to: u64, late: u64, missing: u64 }` is a stereo ring indexed by **engine frame** (`Tick::frame`, KZ-L02) modulo its length.
  - The length is `next_power_of_two(round(RETURN_DELAY_SECONDS × rate) + MAX_CHUNK_FRAMES + 2 × RETURN_BLOCK_FRAMES)`: 32,768 frames (256 KB) at 48 kHz.
  - It is allocated on the control side.
  - `filled_to` is the engine frame just after the newest frame written.
- `Engine` gains `returns: [Option<ReturnSlot>; MAX_RETURNS]`, where `ReturnSlot { id: ReturnId, line: Box<ReturnLine> }`. There are new commands `Command::ReturnLine { index: u8, id: ReturnId, line: Box<ReturnLine> }` and `Command::NoReturnLine { index }`, and `Retired::ReturnLine(Box<ReturnLine>)`, in `drain_commands`'s pattern (`engine/mod.rs:865`).
- **Mapping stream frames to engine frames.** `Engine` gains `consumed: u64`: frames moved from the carry into a chunk (`render_frames`, `engine/mod.rs:793`, the `self.stereo[frame * 2] = …` lines). Engine frame `k` is the `k`-th frame rendered and the `k`-th consumed. At the start of each chunk, with a hub, `origin = hub.stream_frame() as i128 − consumed as i128`: the next frame to be sent is stream frame `hub.stream_frame()` and engine frame `consumed`. Then `engine_frame = stream_frame − origin`. It is recomputed every chunk, so a reconnect that restarts the link's count is followed at once.
- `drain_returns()` runs after `drain_hub()` in `render` (`engine/mod.rs:766`). For each slot, `hub.pop_return(id, &mut scratch)` (scratch: 512 floats on the engine), map the block, and write it into the line:
  - a block starting after `filled_to` zero-fills the gap and adds it to `missing`;
  - frames before `filled_to` are skipped, as a duplicate;
  - frames older than `rendered − D` are past the write head and counted in `late` (blocks).
- `Io` (`dsp/mod.rs`) gains `pub returns: &'a Returns`, a read-only view with `fn frame(&self, index: u8, engine_frame: u64) -> Option<[f32; 2]>`. It returns `None` when the slot is empty, the frame is not yet filled, or it has fallen out of the ring. `render_module` passes it, and `Bench` passes an empty one. Reason for one shared line per source: two loops recording `mouth/dry` read one ring.

### The loop (`dsp/looper.rs`)

- `LoopIn::From { line: Option<u8> }`: `Some(i)` records from return line `i`; `None` records the jacks again. Either way the write state machine goes to `Idle` and the envelope to 0, so no recording is ever half from one source and half from another.
- Allocated in `new`:
  - a beat history `[(u64 frame, f64 beat, f64 beat_step)]` with one entry per sub-block, `ceil((RETURN_DELAY_SECONDS + 0.1) × rate / 32) + 4` entries (0.1 s for `nudge`);
  - a wanted-bit ring of `round(RETURN_DELAY_SECONDS × rate)` bits.

  Both are filled every sub-block whatever the source, so switching to a return is ready at once.
- With `line: Some(i)`, at each frame `f` of the sub-block (engine frame `F = tick.frame + f`):
  - `W = F − D`.
  - `x = io.returns.frame(i, W)`, or silence, counted in the gauges' `missing` when `None`.
  - `wanted = the wanted bit at W`.
  - `beat_W = the history's beat at W − nudge_frames`, where `nudge_frames = round(io.knob(nudge) × rate)`. A positive nudge places what was heard *earlier* on the tape, for a source whose sound reaches the desk late.

  KZ-L02's write logic then runs unchanged on `(beat_W, wanted, x)`: splicing, bar lines, punch fades, keep, seam tail and snapshot copy-on-write all happen in heard time. The read head is untouched and stays at `F`. Factor the write logic into `fn write_frame(&mut self, beat: f64, bpm: f64, wanted: bool, x: [f32; 2])` so the jack path and the return path are the same code.
- Before `D` frames of history exist (just built, or just switched), nothing is written and the state shows `Armed`.
- `LoopGauges` gain `late: AtomicU64` and `missing: AtomicU64` (the frames written as silence). `LoopView` (L05) gains `late: u64`, `missing: u64` and `from_state: FromState`, one of `Listening`, `Asking`, `Refused(String)`, `NoDesk` or `None`.

### The control side (`daemon/loops/returns.rs`)

- `Returns { by_source: BTreeMap<String, Asked> }`, where `Asked` is `{ id: ReturnId, index: u8, state: ReturnState (L06), loops: BTreeSet<String>, last_ask: Instant }`.
  - One return per distinct source, shared by every loop that names it.
  - At most `MAX_RETURNS` (4) sources at once. A fifth `from` is refused when set: "the wall can take four desk strips at once; loop1 cannot also take G2".
- When a loop's `from` is set (L03's `What::LoopFrom` mirror, a load, a restore) and `self.desk` is `Some`:
  - if no `Asked` exists for the source, call `desk.ask_return(source)` and record it;
  - when `return_state` turns `Ready`, allocate the `ReturnLine` (here, control side), send `Command::ReturnLine`, and send `LoopIn::From { line: Some(index) }` to each loop;
  - a `Refused(reason)` is that loop's `from_state` and a note, and is asked again every `ASK_RETRY`;
  - a `Gone` (the desk went away) sends `From { line: None }` to each loop and is asked again when the link is back.
- When no loop names a source any more, `drop_return`, `Command::NoReturnLine`, and free the line when it comes back retired.
- With no desk (`--no-desk`, `DeskMode::Off`), `from_state` is `NoDesk` and nothing is asked.

## Not this lane

The desk's taps (KZ-L07). The mouth's stream (KZ-L08). Hearing a return live on the wall as a module (a `desk in` module): not in this track. A return heard live arrives late by the desk's lead and would flam against the wall. Only a loop, which hears it one loop later, can put it back on the beat. Write this sentence in `engine/returns.rs`'s module doc.

## Red tests first

Engine tests use the stand-in hub pattern B04 uses (`kazoo-arp/src/audio.rs:509–552`, a `UnixListener` answering the register), extended to announce `FEATURE_RETURNS`, answer `0x15` with READY, and send `0x17` blocks. Daemon tests use L03's offline wall plus that stand-in. Each fails today because there are no return lines.

1. `engine::returns::tests::a_stream_frame_maps_to_the_engine_frame_that_played_it`. Render chunks of 512 with the hub connected. A block stamped `stream_frame = S` lands in the line at engine frame `S − origin`. Check it against the engine frame whose rendered output was sent as stream frame `S` (the test records which engine frame's sample went out at each stream frame through the stand-in's received `0x10` frames).
2. `a_click_sung_on_the_bar_lands_on_the_bar`:
   - the wall is at 120 BPM, with a 1-bar loop, `from` line 0 and `record` on;
   - the stand-in sends silence, plus one click at the stream frame the wall sent for beat 4.0 exactly (found from the received `0x10` frames as in test 1);
   - it sends it 300 ms late: held back by the test, still within `D`.

   After the take, the tape's cell 0 holds the click, ±1 frame. Without the delay path, the click is 300 ms late on the tape, or lost.
3. `nudge_moves_the_take_earlier_by_its_time`. As test 2 with `nudge` 0.02: the click lands 960 frames earlier, modulo the loop.
4. `a_block_later_than_the_delay_is_counted_not_smeared`. Send a block 0.6 s late: `late` counts it, and the tape there is silent.
5. `a_gap_is_silence_counted_as_missing`.
6. `switching_source_mid_recording_never_mixes_two_sources`. Set `From` while `Rolling`: the state is `Idle` and the envelope 0. The tape holds the jacks' material up to the switch and nothing from the return until a new punch-in.
7. `daemon` `a_strip_the_desk_does_not_have_is_asked_for_again`. The stand-in refuses twice, then accepts: asks arrive about 5 s apart (use a test clock passed to `tick`), and then `from_state` is `Listening`.
8. `two_loops_on_one_source_share_one_return`. Exactly one `0x15` is sent.
9. `the_fifth_source_is_refused_by_name`.
10. `with_no_desk_from_is_kept_and_said`.
11. `the_render_path_with_returns_allocates_nothing`. The wall's `assert_no_alloc` around `render` with two lines filled.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4
cargo fmt --all --check
ast-grep scan kazoo-wall/src
```

## Definition of done

- [ ] The 11 tests fail first (paste each under RED), then pass.
- [ ] `RETURN_DELAY_SECONDS` with its compile-time assert, `ASK_RETRY`, `ReturnLine`, `Command::ReturnLine`, `Io::returns`, `LoopIn::From`, `write_frame` and `FromState` exist as named.
- [ ] The jack path's tests from KZ-L02 pass unchanged, which proves one write path serves both.
- [ ] Not the builder's: after L07 and L08 have also landed, Cassio runs the desk, `kazoo mouth` and the wall on the desk, sets `loop1` to record `mouth/dry` on a 2-bar loop, and asks Tom to sing "one, two, three, four" on the beats of two bars and listen for the words coming round on the beats; then adds a `clock`-driven `env` into a `vca` on the loop's output to fade it in and out every 16 bars. The builder never opens Tom's mic or speakers or touches the live wall, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `engine/mod.rs` is about 1510 lines: the return code goes in `engine/returns.rs`, and `mod.rs` gains only the fields, the two commands, the retired arm, the `drain_returns` call and the `consumed` counter. List it under FOUND with its line count. `dsp/looper.rs` must stay under 1000 lines: the return path goes in `dsp/looper/heard.rs`.

## END

```
START KZ-L09 base=<sha> branch=lane/kz-L09 df=<GB> at <time>
END KZ-L09 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
