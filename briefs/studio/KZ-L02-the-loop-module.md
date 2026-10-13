# KZ-L02: a `loop` module on the wall records what is patched into it as a loop of whole bars, plays it back in the wall's time with a spliced seam, overdubs with loss, and wears like tape

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: the loop's real-time processor and its control-side feed. Saving, takes and views are later lanes. Crates: kazoo-wall only (new `src/dsp/looper.rs`, `src/dsp/looper/feed.rs`, `src/dsp/looper/tests.rs`; `loop` is a Rust keyword, so the file is `looper`). Depends: KZ-W01a, landed at `779e316`: it changed `dsp::build` to `build(kind, rate, seed)` (`dsp/mod.rs:239`), and this lane adds a kind to it. The loop has no randomness and ignores its seed.

## Outcome

`wall_add loop` puts a loop on the wall. Patch a voice into its `left` (and `right`, or leave it unplugged for mono) and turn `record` on. At the next bar line the loop splices a fresh length of tape, `bars` × 4 + `beats` long at the tempo of that moment, and records. Turn `record` off and it records to the next bar line, then fades out over 8 ms. From then on it plays what it holds, round and round, in the wall's time:
- at the wall's beat, so it stops when the beat stops and follows a desk locate;
- faster and higher when the tempo goes up, like a tape machine slaved to the transport.

Record again and it overdubs, keeping `keep` of what was there on every pass. With `wear` up, every pass that is not recorded over is quieter and darker, as worn tape is. Two loops of 17 and 23 bars started together drift apart and meet again every 391 bars: Eno's *Music for Airports* method on the wall.

Its level, like every knob, is a jack: an LFO into `loop1.level` brings the loop in and out over minutes. The seam never clicks, and neither does a punch in or out. A NaN fault in the engine never erases what the loop holds.

## The design (decided; build exactly this)

### Constants (`dsp/looper.rs`), each with its reason in its doc comment

- `pub const MAX_LOOP_SECONDS: f64 = 96.0;`: 23 bars fit down to 57.5 BPM and 17 bars down to 42.5 BPM (Tom's example of unequal loops). Eno's *Airports* loops ran 17 to 32 s.
- `pub const MAX_LOOP_FRAMES: usize = 4_608_000;`, which is 96 s at 48 kHz. A loop at a higher rate gets the same number of frames (48 s at 96 kHz), so its memory never passes 36.9 MB, whatever the device.
- `pub const SEAM_SECONDS: f32 = 0.008;`: a 45° splice across quarter-inch tape at 30 ips is 8.3 ms. Long enough (over three periods of 400 Hz) to remove the click; short enough not to smear a drum hit. The punch fades use the same length.
- `pub const BEATS_PER_BAR: f64 = 4.0;`: the wall's clock divisions call 4 beats "1 bar" (`catalogue.rs`, `CLOCK_LABELS`).
- `pub const MAX_LOOP_BARS: usize = 65;`: `bars` up to 64 plus up to 3 `beats` makes a short 65th bar.
- `pub const LOOP_BINS: usize = 64;`: one level per sixty-fourth of the loop, for the views (KZ-L12, KZ-L13): enough for a reel's pack and a loom row, small enough to send in every `look`.
- `pub const COPY_FRAMES_PER_BLOCK: usize = 8_192;`: a snapshot copies 64 KB per sub-block, about 5 µs on Tom's M1 Max. A full loop is copied in 563 sub-blocks (0.38 s).
- `pub const PASS_PULSE_SECONDS: f32 = 0.005;`
- `pub const WEAR_DB_PER_PASS: f32 = 3.0;`, `pub const WEAR_PASSES_PER_OCTAVE: f32 = 4.0;`, `WEAR_TOP_HZ = 18_000.0`, `WEAR_FLOOR_HZ = 800.0`, `WEAR_SILENT_DB = -90.0`. At full `wear`, each pass loses 3 dB and a quarter-octave of top. So a loop is about half as loud after two passes, down to 2.2 kHz after 12, and silent after 30. That is the useful span for a loop fading out over a few minutes. At `wear` 0 the read is bit-exact.
- `pub const FEED_SLOTS: usize = 8;`

### The kind (`catalogue.rs`)

`Kind::LOOP = Self(22)`, appended after `READOUT`, in a new `recording_kinds()` called last in `native_kinds()`. It is named `loop`: `tape` and `reel` are already kazoo-fx effect ids. Family `synth`, and `group()` gains `"loop" => 0` (sound sources), because in the picker it is something that makes sound. The two catalogue tests that pin `READOUT` as the last own kind now pin `LOOP` (`catalogue.rs:1538`, `:1548`); say so under CHANGED. The about text: "tape loop: records what is patched in, on bar lines, and plays it round in the wall's time; overdubs keeping `keep`, wears by `wear`".

Knobs, in this order (indices are the engine's):

| # | name | range | default | unit | why |
|---|---|---|---|---|---|
| 0 | `bars` | 1–64 stepped | 4 | none | read only at a splice |
| 1 | `beats` | 0–3 stepped | 0 | none | so a loop can be 17 bars and a beat |
| 2 | `record` | 0–1 stepped, labels `off`, `on` | 0 | none | the hand's switch |
| 3 | `keep` | 0–1 linear | 1.0 | none | a looper keeps what you played; lowering it is a choice |
| 4 | `wear` | 0–1 linear | 0.0 | none | loss is optional (Tom: "optional loss per pass") |
| 5 | `level` | 0–1 linear | 0.8 | none | the output, as `vco`'s |
| 6 | `nudge` | −0.1–0.1 linear | 0.0 | Seconds | only for recording a desk return (KZ-L09); recording the jacks is never nudged |

Inputs: `left` (audio), `right` (audio; `left` when unplugged), `rec` (gate: recording is wanted while it is high, or while `record` is on). Outputs: `left`, `right` (what the tape plays, times `level`), `pass` (a 5 ms gate at each seam, so other modules can follow the loop), `phase` (a CV from 0 to 1 across the loop).

### Building (`dsp/looper.rs`)

- `pub struct LoopModule`. `LoopModule::new(rate: f32) -> Self` allocates on the control side: `capacity = min(round(MAX_LOOP_SECONDS × rate), MAX_LOOP_FRAMES)` frames, plus `tail = round(SEAM_SECONDS × rate)` frames, stereo interleaved, as one `Box<[f32]>` of `2 × (capacity + tail + 1)`.
  - Allocate it with `Vec::try_reserve_exact`, then `resize(len, 0.0)`. `resize` writes every page, so the memory is resident before the module reaches the engine, and the audio thread never takes a page fault on fresh tape. `vec![0.0; n]` maps zero pages lazily, so it is not used.
  - An allocation that fails leaves a module with no tape. It plays silence, refuses to splice, and shows state `NoTape`, never a panic.
- `dsp::native::looper(rate)` builds one for `dsp::build` (benches and tests). `pub fn build_loop(rate: f32) -> (Box<dyn Module>, LoopFeed)` builds one with its feed, for the daemon (KZ-L03). It is the `adapters::build_speaker` pattern (`adapters/mod.rs:47`).
- `Tick` gains `pub frame: u64`: the frames this engine has rendered before the sub-block. `Tick::new` leaves it 0, and a new `Tick::at_frame(self, frame) -> Self` sets it. `render_sub_block` (`engine/mod.rs:1104`) builds the tick with `.at_frame(self.rendered)`, and `Bench::step` (`dsp/mod.rs`, testing) counts its own frames. Snapshots and takes are stamped with it.

### State (all inside `LoopModule`, sized at build)

- `splice: Option<Splice>`, with `Splice { beats: f64, frames: usize, tail: usize, anchor: f64, rate: f32 }`. `None` is blank tape. `anchor` is the song beat of the loop's first frame. `beats` is the length in beats. `frames` (N) is fixed at the splice. `rate` is the rate it was spliced at.
- `rec: Rec`, one of `Idle`, `Armed`, `Rolling`, or `PostRoll { frames_left: u32, into_tail: bool }`.
- `gain: f32`, the write envelope from 0 to 1, moving `1/tail` per frame.
- `last_cell: Option<usize>`.
- `seam_carry: bool`: a recording crossed the seam while rolling, so cells below `tail` are also written into the tail.
- `bar_pass: [i64; MAX_LOOP_BARS]`: the pass in which each of the tape's bars was last written; `i64::MIN` for never.
- `wear` filter state per channel, and the bar and age it was computed for.
- `pulse_left: u32`, `peak: [f32; 2]` and `bins: [u8; LOOP_BINS]`.
- `snapshot: Option<Snap>`.
- `held: [Option<LoopOut>; 3]`: items the feed's outgoing ring had no room for, pushed again every sub-block, so nothing large is ever dropped (freed) on the audio thread.

### Where the head is (every frame `f` of the sub-block)

```
beat  = tick.beat_at(f)
rel   = beat - anchor
pass  = floor(rel / beats)                 // i64; negative after a locate before the anchor
phase = rel.rem_euclid(beats) / beats      // 0 ≤ phase < 1
pos   = phase × N                          // f64
bar   = floor(rel.rem_euclid(beats) / 4)   // the tape's own bar under the head
```

The head is a pure function of the wall's beat, so it needs no saved state. A save, a load or an engine rebuild puts it back exactly where the beat says. A tempo change moves the head at a new speed through the same N frames (varispeed: higher and faster). A device at another rate reads the same N frames over the same beats: the right pitch, sampled at the new rate.

### Reading

- Use 4-point, third-order Hermite over cells `i0 − 1 … i0 + 2`, wrapping modulo N, per channel. Reason: under a tempo change the head moves by fractions of a cell. Hermite holds images near −40 dB with four taps and no table, and reading never accumulates error.
- At the seam: for `i0 < tail`, `out = cos θ · tailread + sin θ · mainread` with `θ = (pos / tail) · π/2`. `tailread` linearly interpolates cells `N + i0` and `N + i0 + 1` of the buffer: the tail holds what was recorded just after the loop's end, the natural continuation of cell N − 1. So the end runs smoothly into the tail, and the tail crossfades into the start over 8 ms: a diagonal splice.
- Wear: whenever `bar` or `pass` changes, `age = max(0, pass − bar_pass[bar])` (a never-written bar has age 0 and is silent anyway). Compute `gain_w = 10^(−3 · wear · age / 20)` (0 when below −90 dB) and `fc = max(800, 18 000 · 2^(−age · wear / 4))`, with the one-pole coefficient `1 − exp(−2π fc / rate)`.
  - The `exp` runs only on these changes, at most a few times a second. A `wear` knob that is moving or patched recomputes once per sub-block, not per frame.
  - When `wear == 0` or `age == 0`, the filter and gain are skipped entirely, so the read is bit-exact.
- `out[left/right][f] = level(f) × wearread`. `level` is `io.knob_at(5, f)`, so a cable on `loop1.level` moves it sample by sample.
- `pass` output: 1.0 for `PASS_PULSE_SECONDS` after every frame where `pass` changed (including the splice), else 0. `phase` output: `phase` as f32.
- A blank loop outputs silence on `left`/`right`, 0 on `phase` and nothing on `pass`.

### Recording

- **Wanted** = `io.inputs[rec][f] > 0.5 || io.knob(record) >= 0.5`.
- **Blank tape**: wanted moves `Idle` to `Armed`. On the first frame of the next song bar line (`floor(beat/4)` steps up), splice:
  1. `beats = bars × 4 + beats_knob`, read at that frame with `io.knob`.
  2. `N = round(beats × 60 / bpm × rate)`.
  3. If `N > capacity`: `beats = max(4, floor(capacity × bpm / (60 × rate) / 4) × 4)` (whole bars that fit), `N` is recomputed, and the gauge flag `shortened` is set (the console and `look` say "17 bars do not fit 96 s at 40 BPM; spliced 16").
  4. `anchor` = that bar line's beat. `bar_pass` is all `i64::MIN`, the bins are 0, and the cells from 0 to `N + tail` are already zero (fresh tape, or zeroed on erase by KZ-L03 handing in a zeroed buffer).
  5. The state becomes `Rolling`.
- **Content**: wanted moves `Idle` to `Armed`. Recording starts on the first frame of the next *tape* bar line (`bar` changes, or `pass` changes). So punches land on the loop's own bars, and the last, short bar of a 17-bar-and-1-beat loop is a bar too. Not wanted while `Rolling` keeps rolling to the next tape bar line, then gives `PostRoll { frames_left: tail, into_tail: <that bar line is the seam> }`. `Armed` and not wanted goes back to `Idle`.
- **The envelope**: `gain` ramps up while `Rolling` (from 0 at the punch-in frame) and down in `PostRoll`. `PostRoll` ends when `frames_left` reaches 0. It then pushes `LoopOut::Took` (below) and goes `Idle`.
- **Writing a frame** (while `gain > 0`), with input `x` = each channel's input sanitised (`finite`, `dsp/mod.rs`) and clamped to ±16, as `out.rs` does:
  - Let `w = round(pos) mod N`.
  - If `last_cell` is `Some(c)` and `w` is 1 to 8 cells ahead of `c` (modulo N), write every cell from `c + 1` to `w` with `x`. When the head moves faster than one cell a frame (tempo above the splice tempo), the skipped cells hold the same sample: lo-fi and deterministic.
  - If `w == c`, write nothing: the head is slower than a cell a frame, and this frame's input is dropped (decimation).
  - Any other jump (a locate) writes `w` alone.
  - A cell `k` is written as `buf[k] = buf[k] · (1 − gain · (1 − keep)) + gain · x`. In `PostRoll` with `into_tail`, cell `k` goes to the tail (`N + k`) instead of the start.
  - While `seam_carry` is set and `k < tail`, the tail cell `N + k` is written the same way as well. `seam_carry` is set when cell N − 1 is written while `Rolling`, and cleared when the head passes `tail`.
  - `keep` is read once per frame with `io.knob_at`.
  - Each write updates `bar_pass[bar] = pass`, the written-bars mask, and the bin peak (`max` of `|value|`, as a u8 on a 60 dB scale: 0 at −60 dBFS and below, 255 at 0 dBFS).
- **Read before write**: the read at frame `f` happens before the write. Material recorded at a position is heard one loop later, as on a real tape loop with record and play heads.

### The feed (`dsp/looper/feed.rs`): the control side's handle, never used on the audio thread

- `LoopIn` (into the module, `HeapRb<LoopIn>` of `FEED_SLOTS`):
  - `Install { tape: Box<[f32]>, splice: Option<Splice>, bar_pass: [i64; MAX_LOOP_BARS], bins: [u8; LOOP_BINS], only_if_blank: bool }` replaces what the loop holds. With `only_if_blank`, a module that has spliced tape of its own since it was built refuses it (`Refused`), so a reel put back late never overwrites a fresh recording (KZ-L03). The old buffer goes back as `LoopOut::Retired`. A tape shorter than `2 × (N + tail + 1)` for the splice comes back as `LoopOut::Refused`. The module pops an `Install` only when one `held` slot is free, so it always has somewhere to put the old buffer.
  - `Snapshot { into: Box<[f32]> }` starts a snapshot.
- `LoopOut` (out of the module, `HeapRb<LoopOut>` of `FEED_SLOTS`):
  - `Retired(Box<[f32]>)`.
  - `Refused(Box<[f32]>)`.
  - `Took(TakeMark)`, where `TakeMark { frame: u64, beat: f64, pass: i64, bars: [bool; MAX_LOOP_BARS] }` gives the bars written since the last mark.
  - `Snapped { tape: Box<[f32]>, at: TakeMark, splice: Option<Splice>, bar_pass: [i64; MAX_LOOP_BARS] }`.
- **Snapshot, copy-on-write.** On `Snapshot { into }`, if `into` is too short or a snapshot is running, it comes back `Refused`. Otherwise record `at` (the sub-block's first frame), `copied = 0` and `len = N + tail + 1`, both counted in stereo frames (the buffer holds `2 × len` floats).
  - Every sub-block before rendering, copy `COPY_FRAMES_PER_BLOCK` frames from `copied` onward and advance.
  - Every write to cell `k` while a snapshot runs first copies the old value of `k` (and of tail cell `N + k` when that is written) into `into`, if `k ≥ copied`, then writes. That is one compare per write.
  - When `copied == len` (frames), push `Snapped` (or hold it).

  The snapshot is therefore exactly the tape at the frame it began, whatever is recorded meanwhile. That is what makes a take (KZ-L04) and a save (KZ-L03) exact without a second copy of every loop living on the control side.
- `LoopGauges` (an `Arc`, all atomics, `Relaxed`), written once per sub-block and read by the control side for `look` and the views:
  - `state`: `Blank`, `Armed`, `Rolling`, `PostRoll`, `Holding` or `NoTape`, as a u8;
  - `frames`, `beats` bits, `anchor` bits, `pass` (i64), `rate` bits;
  - `written: [AtomicU64; 2]` (bars mask);
  - `peak_left` and `peak_right` (f32 bits, max-hold; the reader swaps in 0);
  - `bins: [AtomicU8; LOOP_BINS]`;
  - `shortened`;
  - `refused` (count).
- `pub struct LoopFeed { inbox: HeapProd<LoopIn>, outbox: HeapCons<LoopOut>, gauges: Arc<LoopGauges> }`, with:
  - `install(..) -> Result<(), LoopIn>`;
  - `snapshot(into) -> Result<(), Box<[f32]>>`;
  - `collect(&mut self) -> Vec<LoopOut>` (it allocates; control side);
  - `gauges(&self) -> &LoopGauges`.

### Reset and faults

`Module::reset` (the engine calls it after a non-finite output, `engine/mod.rs:1211`):
- clears the wear filters, the pass pulse, the envelope and `last_cell`;
- moves a `Rolling` or `Armed` loop to `Idle`, and a `PostRoll` loop to `Idle`, pushing `Took` for what was written.

It never clears the tape, the splice or `bar_pass`: a recording is the musician's work, and a fault elsewhere must not erase it. Every value written to tape is finite by construction (sanitised input, finite gains), so the tape itself never holds a NaN. `rewired` is the default no-op.

## Not this lane

- KZ-L03: the loop in the patch, saved and loaded through reels, erased, `from`, the eight-loop cap, and loops as D03 sinks.
- KZ-L04: takes and the change log.
- KZ-L05: `look` and the MCP.
- KZ-L09: recording a desk return (`nudge` is declared here and used there).
- KZ-L12 and KZ-L13: the views.

## Red tests first

In `dsp/looper/tests.rs`, with `Bench::seeded(Kind::LOOP, RATE, 0)` (W01a) at 120 BPM, 48 kHz. Write a helper that renders N frames feeding `left` from a closure. Each test fails today because there is no `loop` kind.

1. `nothing_is_recorded_until_the_bar_line`. Start at beat 1.0 with `record` on and 1 bar. A ramp fed from beat 1 to beat 4 is not on the tape. Recording begins at beat 4.0 exactly: the frame where `floor(beat/4)` steps. The loop plays the ramp's values from beat 4 onward, one bar later.
2. `a_loop_plays_back_bit_exact_at_its_own_tempo`. Record one bar of a deterministic signal and stop. On the next pass, outside the seam (frames `tail..N`), every output sample equals `level × recorded` within 1e-6. The test documents why: Hermite at a fractional offset of about 1e-9 is not bit-identical. Then render the same thing twice on two benches and assert bit-identical (`to_bits`).
3. `the_seam_does_not_click`. Record a 220 Hz sine for one bar, starting mid-cycle. Across every seam over 4 passes, the largest sample-to-sample step is at most 1.5 × the sine's own largest step. Without the tail crossfade, a phase jump at the seam breaks this.
4. `punch_out_runs_to_the_bar_line_and_fades`. With 4 bars, record and turn `record` off at beat 6.5. Writing continues to beat 8.0, then fades over 384 frames. A `Took` arrives after the post-roll, with bars 0 and 1 marked.
5. `overdub_keeps_keep_of_what_was_there`. Record a constant 0.5 for a pass, then overdub 0.25 at `keep` 0.5: the next pass plays 0.5 × 0.5 + 0.25 = 0.5 (× level) in the middle of the bar.
6. `wear_darkens_and_quietens_each_pass_and_zero_wear_is_exact`. With `wear` 1, the RMS of passes 1, 2 and 3 after recording falls by 3 dB ± 0.2 each. With `wear` 0, passes are identical bit for bit.
7. `the_loop_follows_the_beat_not_the_clock`. Set the bench's beat back by 4 beats mid-loop (a desk locate): the output equals what played 4 beats ago. At 240 BPM after recording at 120, a recorded 1 kHz sine plays at 2 kHz ± 1%. Measure zero crossings.
8. `unequal_loops_drift_and_meet_again`. Two loops at 120 BPM, 5 and 7 bars, both armed before beat 0, so both splice at beat 0 and record their first bar. Render 36 bars (3.5 M frames, about a second of test time). Count `pass` pulses: loop A fires at bars 5, 10, …, loop B at 7, 14, …, and both fire on the same frame at bar 35 and on no frame before. Beside it, a pure check of the head function `head(beat, anchor, beats)` (make it a `pub(crate) fn` so it can be tested alone): for 17 and 23 bars, the first beat after 0 where both passes change together is 391 × 4. Rendering 391 bars would take 37.5 M frames, too long for a unit test, so it is checked as arithmetic.
9. `too_long_a_loop_is_shortened_to_whole_bars_that_fit`. At 40 BPM with 64 bars: the splice is `floor(96 × 40 / 60 / 4) × 4 = 64` beats (16 bars), and `shortened` is set.
10. `a_snapshot_is_the_tape_at_the_frame_it_began`. Record continuously across 3 passes at `keep` 0.7. Send a snapshot mid-pass. The `Snapped` tape equals a copy taken by a reference implementation that stops the world at that frame. Both run the same input on a second bench. Recording continues meanwhile.
11. `install_hands_the_old_tape_back_and_never_drops_it`. Fill the outgoing ring so it is full, then install: the old buffer is held, comes back once the ring has room, and no buffer is freed inside `process`. The crate's tests run under `assert_no_alloc` (`lib.rs`), which also catches frees.
12. `nan_in_never_reaches_the_tape_and_reset_keeps_it`. Feed NaN and Inf while recording: the outputs are finite and the tape holds 0 where they fell. Then `reset()`: the next pass still plays the earlier material.
13. `no_memory_is_a_silent_loop_that_says_so`. `LoopModule::with_capacity(0)` (a test constructor) refuses to splice and shows `NoTape`.
14. `every_kind_built_twice_with_one_seed_renders_the_same` (W01a's test) passes with `loop` in it unchanged.

## Bench

Add rows `wall/loop/play` (one loop holding 8 bars, playing) and `wall/loop/record` (one loop overdubbing) to `kazoo-wall/benches/wall_render.rs`, as D01's rows are built. Target, stated in BENCH.md with its reason: each row at most 1% of real time (106,667 ns per 512-frame block), so eight loops cost under a tenth of one core and leave the circuit models their room. Report the numbers, and say whether the machine was quiet (D01's rule).

## Checks

As PLAN.md, with `-p kazoo-wall`, and `ast-grep scan kazoo-wall/src/dsp kazoo-wall/src/catalogue.rs`. Run doc tests for kazoo-wall.

## Definition of done

- [ ] The 14 tests fail first (paste each under RED), then pass.
- [ ] `Kind::LOOP`, `LoopModule`, `LoopFeed`, `LoopIn`, `LoopOut`, `LoopGauges`, `TakeMark`, `Splice`, `build_loop` and `Tick::frame` exist as named.
- [ ] Nothing in `process` allocates, frees, locks or does I/O (`assert_no_alloc` green). Every `exp`, `powf` and `cos` on the render path is per bar change or per sub-block, never per frame, except the seam crossfade's `cos`/`sin` over the first 8 ms. Make that a 64-entry quarter-sine table built in `new`.
- [ ] BENCH rows written.

## Code quality

As PLAN.md's STACK RULES. `looper.rs` stays under 1000 lines: the feed and the tests are their own files, and the snapshot logic may be a `looper/snapshot.rs`. No `as` casts that clippy pedantic flags: positions go through `f64` `floor` and checked conversion (`usize::try_from` on a value already known to lie in `0..N`, the error arm being unreachable by construction and written as `return` silence, never a panic).

## END

```
START KZ-L02 base=<sha> branch=lane/kz-L02 df=<GB> at <time>
END KZ-L02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
