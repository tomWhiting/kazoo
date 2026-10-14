# KZ-V08d: the mouth's looper: record a phrase, it loops, layer over it, undo the last layer, clear it, free or in bars at the desk's tempo, with its own volume, and save it as a WAV

Line numbers are from the sweep that wrote this brief, at `508b869`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Drafted 14 Oct 2026 (morning) for Cassio to read and commit, from Tom's first hum of `kazoo mouth`.

Place in the track: Track V (the voice), after V08c. It is a looper inside the mouth, not on the wall. Track L's `loop` module stays the wall's looper, and this one borrows its splice and its real-time habits, but not its seam.

Time: about 45 minutes of a builder's work. One mechanism: a layered mono loop owned by the mouth's callback, with its strip on the screen and its WAV save.

Crates: kazoo-mouth only.

Depends: KZ-V08a, KZ-V08b and KZ-V08c. This lane uses V08c's `beats` array, its tempo in use, its `Ramp`, its settings file and `atomic_write`. Opens only after all three have landed. KZ-L08 also edits `src/engine.rs` and `src/run.rs`, so it runs after this lane or before V08c, never beside either.

Files (9): `kazoo-mouth/Cargo.toml` (`hound` moves from dev-dependencies to dependencies), `src/engine.rs` (the looper in the callback, its commands, its rings), `src/engine/looper.rs` (new), `src/engine/looper/tests.rs` (new), `src/controls.rs` (the loop keys), `src/controls/settings.rs` (the loop's length mode and volume), `src/ui.rs` (the strip's two lines in the layout), `src/ui/looper.rs` (new: the strip), `src/run.rs` (the bounce hand-back and the WAV writer). `engine/looper.rs` is declared in `engine.rs` and `ui/looper.rs` in `ui.rs`.

## Why

Tom, 14 Oct 2026, about 09:2x, after his first hum of `kazoo mouth`: "a teenage engineering style little looping thing inside the mouth thing would be really cool." QUEUE.md records what he meant: "OP-1 tape / OP-Z feel: record a phrase, loop it, layer over it". The same morning: "I feel like the mouth part of this is an instrument in itself... I want it to be able to go on the wall as well, but I want it to be its own monster."

So the looper lives in kazoo-mouth, a few keys from the voice, and needs no desk and no wall. The wall's `loop` (KZ-L02, `kazoo-wall/src/dsp/looper.rs`) is a module on the wall's clock, varispeeding with its tempo, for loops that work in and out over time under modulation. This one is a hand looper: press, sing, press, and it plays.

What is reused from Track L:
- The 8 ms splice and its reason (`SEAM_SECONDS`, `looper.rs:22`): a 45° splice at 30 ips is 8.3 ms, long enough to remove a click and short enough not to smear a hit.
- The 64-entry quarter-sine table (built at `:276`, read by `quarter_sine` at `:520-527`).
- Tape memory touched on the control side so the callback never takes a page fault (`resident`, `:201-209`: `try_reserve_exact` then `resize`).
- Buffers handed back through a ring and held when the ring is full, never freed on the audio thread (`emit` and `hand_back`, `:288-320`).
- A copy spread over callbacks at a fixed budget (`COPY_FRAMES_PER_BLOCK`, `:30`).

What is **not** copied is L02's seam defect, recorded in QUEUE.md from L03's review: "recording fades linearly over the seam and playback crossfades equal-power on top, so a sustained sound dips about 3 dB for 8 ms at every seam".
- The fade is applied twice: the write envelope ramps at the seam (`advance_envelope`, `:713-737`), and `read` crossfades again (`:529-546`). That gives `sin θ · θ + cos θ · (1 − θ)`, which is 0.707 at the middle.
- Here the seam is faded exactly once.

## Outcome

In `kazoo mouth`:
- `r` records. A second `r` closes the loop and it plays from the top.
- From then on, `r` punches an overdub layer in and out, `u` takes the newest layer away, `C` clears the loop, and `o` stops and starts it.
- `l` chooses the length before recording: free (as long as the phrase), or 1, 2, 4, 8 or 16 bars at the desk's tempo. In bars mode the take starts on the desk's next bar line and closes itself.
- `-` and `=` set the loop's volume.
- `w` saves the loop as a 32-bit float WAV that loops seamlessly in any player.

The loop is recorded from the mouth's synth before the effects, and plays back through them with the live voice. A two-line strip on the screen shows its state, length, place, layers and volume. It never clicks at the seam, at a punch, at an undo, at a stop, or at a clear.

## The design (decided; build exactly this)

### Where it records: after the synth, before the effects

The looper takes V08a's mono synth block (after V08a's soft clip) and records it. Its playback is added to that block, and the sum is what V08c's chain receives:

`chain input = synth + loop × volume`

Why before the effects:
- **The effects are the master, as on the OP-1.** Turning the megaphone, the drive or the delay changes the loop and the live voice together, so Tom plays the whole performance through one set of knobs.
- **No doubled tails.** A reverb or delay tail recorded into the loop would ring again through the live reverb and delay. A tail cut at the loop's end would also be spliced across the seam.
- **Half the memory.** The synth is mono, and the chain makes stereo afterwards.
- **A clean seam.** The splice crossfades dry material, not a smeared reverb.

What this costs: an effect's texture cannot be frozen into the loop. When Tom wants that, he records the desk strip into a wall `loop` (KZ-L09).

The looper records only the live synth block, never its own playback, so a layer never contains the loop.

`--silent` (V08a) still records and plays into the chain; V08a's rule silences the output.

### Memory (`src/engine/looper.rs`, constants each with its reason in its doc comment)

- `LOOP_SECONDS: f64 = 32.0`: 16 bars at 120 BPM, 8 at 60. That is a sung phrase or two, which is what a hand looper holds.
- `MAX_LOOP_FRAMES: usize = 3_072_000`: 32 s at 96 kHz (Tom's interface runs at about 96 kHz, QUEUE.md). `capacity = min(round(LOOP_SECONDS × rate), MAX_LOOP_FRAMES)`, so a 192 kHz device holds 16 s and the memory is bounded whatever the device.
- `SEAM_SECONDS: f32 = 0.008`, with `tail = round(SEAM_SECONDS × rate)` cells. The reason is L02's.
- `BUFFERS: usize = 4`. This is the OP-1's four tracks: at most three layers are kept apart for undo, and one buffer is always free to record into.
  - Each buffer is mono f32 of `capacity + tail` cells, allocated at start on the control side by `resident` (as L02).
  - That is 24.6 MB at 48 kHz and 49.2 MB at 96 kHz, all of it pre-allocated: nothing is allocated when recording starts.
- `MERGE_CELLS_PER_FRAME: usize = 64` and `BOUNCE_CELLS_PER_FRAME: usize = 64`. Per output frame rendered, a merge or a bounce moves 64 cells. The longest loop (32 s) is therefore merged or copied in `32 / 64 = 0.5 s`, whatever the callback size. At 128 frames that is 8,192 cells a callback, L02's own figure.
- `MIN_TAKE_SECONDS: f64 = 0.25`: a shorter close is a slip of the finger, and it is ignored.
- `VOLUME_DEFAULT: f32 = 0.8` (as the wall loop's `level`).
- `BAR_CHOICES: [u8; 5] = [1, 2, 4, 8, 16]`, and `BEATS_PER_BAR = 4.0` (as `kazoo-wall`'s clock and L02).

If an allocation fails, the looper has no tape. It shows `no tape: the loop could not get its memory`, every loop key says so, and the mouth plays on. It never panics.

### State

`Looper` holds:
- the four buffers (`[Box<[f32]>; 4]`);
- `n: Option<usize>`, the loop's length in cells, `None` while blank;
- `stack`: up to four `Layer { buffer: u8, start: usize, written: usize }`, oldest first. `stack[0]` is the bed (the first take);
- `head: usize`;
- `state`: one of `Blank`, `Armed`, `Taking`, `Playing`, `Dubbing`, `Waiting`, `Stopped` or `NoTape`;
- `length: LoopLength` (`Free` or `Bars(u8)`);
- `anchor: Option<f64>`: the desk beat of cell 0, when the take was started on a desk bar line;
- `loop_bpm: f64`: the tempo of a bars take;
- `volume: Ramp` (V08c's `Ramp`, 10 ms);
- `play: Ramp` (V08c's `Ramp`, made with `tail` frames, for stop and start);
- the write envelope `env` (0 to 1, stepped `1 / tail` a frame);
- `fresh_seam: usize`;
- `merge: Option<usize>` (cells merged so far);
- `bounce: Option<Bounce>`;
- `held: [Option<LoopOut>; 2]`;
- the quarter-sine table.

**Reading a layer at cell `c`**: the layer sounds there if `written ≥ n`, or if `(c − start) mod n < written`. Otherwise it reads 0. So a buffer reused after an undo never plays stale cells, and nothing has to be zeroed. A buffer is free when it is not in `stack`.

### The first take, and the seam faded once

**Free mode**:
1. `r` on blank tape moves to `Taking`. The free buffer becomes `stack[0]` with `start` 0. Each frame writes the live sample at **full gain** into cell `written` and counts it.
   - There is no fade-in. The only fade at the seam is the one in step 4, so nothing is faded twice.
2. A second `r` with `written < MIN_TAKE_SECONDS × rate` is ignored (note `too short to loop`). Otherwise it closes the loop. Reaching `capacity` closes it by itself.
3. On closing, `n = written`, and the head goes to cell 0 at that frame: the loop restarts where Tom pressed. The next `tail` frames of live input are still written, at full gain, into cells `n .. n + tail`. These cells hold what he sang just after the loop's end, its natural continuation.
4. **The first pass** (`fresh_seam = tail`): for those first `tail` frames the head reads cells `0 .. tail` as `qsin(c / tail) · bed[c]`.
   - The tail term is left out because the tail is being written at that very frame, and the live voice, still sounding, is the continuation.
   - So on its first pass the loop fades in under Tom's own voice. A read of the tail there would double the voice for 8 ms.
5. **The bake**: on the frame the tail is complete, cells `0 .. tail` are rewritten once as `qsin(c / tail) · bed[c] + qsin(1 − c / tail) · bed[n + c]`. That is the splice from the end's continuation into the start, equal-power, from the quarter-sine table. It is at most 768 cells at 96 kHz, done in one frame.
   - From then on every read is a straight read of cells, and the seam is in the tape, faded once.
   - Why equal-power: the two sides of the splice are different moments of a voice (the phrase's start and what came after its end), so on average they are uncorrelated, and equal-power keeps uncorrelated sound at constant loudness through the splice.
   - The worst case is a perfectly correlated seam (the same waveform in phase on both sides). It bulges at most 3 dB for 8 ms, which a sustained note masks, where a dip punches a hole.
   - Why bake rather than crossfade at read time as L02 does: the bake makes every read, merge and bounce a plain sum of cells. Overdub layers, which wrap the seam continuously in time, are then never faded by the bed's splice.

**Bars mode** (`length = Bars(b)`):
1. `r` on blank tape computes `n = round(b × 4 × 60 / bpm × rate)` with V08c's tempo in use.
   - If `n > capacity`, it refuses with the note `<b> bars at <bpm> BPM need <s> s; the loop holds <capacity s> s` and stays blank.
   - Otherwise it moves to `Armed`.
2. If the desk is playing (V08c's `beats[i]` is not NaN), the take starts on the first frame whose `floor(beat / 4)` is above the previous frame's, the next bar line, and `anchor` is that bar line's beat (`4 × floor(beat / 4)`). If the desk is not playing, the take starts on the press, and `anchor` is `None`.
3. It records exactly `n` cells, as free mode does, then closes itself. The head goes to 0 on the frame after cell `n − 1`, which is the next bar line within a frame, and the tail and the bake follow as in free mode.
4. `loop_bpm` is the tempo used.

### Overdubs, undo, clear, merge

**Punch in** (`r` while `Playing` or `Stopped`; from `Stopped` it starts the loop first, as `o` would):
- If a buffer is free, a new layer starts now with `start = head`, `written = 0`, and `env` ramping 0 → 1 over `tail` frames. The state is `Dubbing`.
- If no buffer is free (a merge is running), the state is `Waiting`. The punch-in happens on the frame the merge frees a buffer, which is at most 0.5 s later, and the strip says `waiting: merging`.
- Punches are not quantised in either mode. Why: the loop is already playing, and holding a punch to the next bar would lose the start of what Tom sings.

**Writing a dub frame** at cell `c = head`:
- While `written < n`, the cell is replaced: `buf[c] = env · x`.
- After a full pass, the cell is added to: `buf[c] += env · x`, so a dub held round the loop layers over itself.
- `written` counts to `n` and stops there.
- `x` is the live sample, sanitised (`kazoo_core::sanitize_sample`) and clamped to ±4. A sum that comes out non-finite is stored as 0.
- **Read before write**: the layer is heard from the next pass, as on tape.

**Punch out** (`r` while `Dubbing`): `env` ramps 1 → 0 over `tail` frames, still writing as above. Then the layer is closed and stays on the stack. Punch fades are mid-loop, never at the bed's seam, so they never fade the splice.

**Merge**:
- When a punch-in takes the last free buffer (four layers on the stack), `stack[1]` starts merging into `stack[0]`. Each frame, cells `[m, m + 64)` of the bed get `+=` the layer's cells, but only where the layer sounds (the read rule above). The bed's tail cells are not touched (they are baked).
- While it runs, a read at `c` takes `stack[1]`'s cell only where `c ≥ m`, so the sum heard never changes. A test proves it bit for bit, within float addition order (1e-6).
- At `m = n`, `stack[1]` leaves the stack and its buffer is free.
- A merged layer can no longer be undone.

**Undo** (`u`):
- `Taking`: the take is cancelled and the tape is blank again.
- `Dubbing` or `Waiting`: the layer being recorded fades out over `tail` frames (its read multiplied by a falling ramp), then leaves the stack.
- Otherwise the newest layer fades out the same way and leaves the stack.
- If the newest layer is the bed alone, undo is a clear.
- An undo that would reach the layer a merge is folding in is refused with the note `that layer is being merged into the one below and can no longer be undone`.
- The fade is linear over `tail` frames. Why linear: what fades is one layer of the sum, and fading a signal's own level linearly to zero never changes anything else.

**Clear** (`C`):
- The whole loop fades out over `tail` frames, then everything goes: the stack, `n`, `anchor` and `loop_bpm`.
- A running merge is abandoned (the bed is gone anyway). A running bounce is refused back (below).
- The state is `Blank`. `length` and `volume` are kept.

**Stop and start** (`o`):
- Stop fades the loop's output out over `tail` frames, then holds the head. A dub in progress punches out first.
- Start fades it in over `tail` frames from cell 0, or from the desk-aligned cell (below) when `anchor` is `Some` and the desk is playing.

### The desk

- **A desk locate or start moves an anchored loop.** A jump is a frame whose beat is not NaN and differs by more than 0.001 beat from the previous frame's beat plus one frame's worth, or whose previous beat was NaN.
  - On a jump, the head moves to `round(((beat − anchor) mod (b × 4)) / (b × 4) × n)`.
  - It moves by crossfading, over `tail` frames, a read at the old position (still advancing) out and a read at the new one in, both from the quarter-sine table (equal-power: two places in the loop are uncorrelated).
  - A free or un-anchored loop never moves with the desk.
- **When the desk stops, the loop plays on.** It is its own monster.
- **No varispeed.** When the tempo in use differs from `loop_bpm` by more than 0.1 BPM, the loop keeps its cells and its speed, and the strip shows the recorded tempo beside the desk's in the warning style. Why: varispeed would put a sung loop in another key, and the quantiser has just put it in tune. Track L's loop varispeeds because it is tape on the wall's clock; this one is a phrase.

### Commands and rings

V08a's command enum gains `LoopRecord`, `LoopUndo`, `LoopClear`, `LoopLength(LoopLength)`, `LoopPlay(bool)` and `LoopVolume(f32)`, all `Copy`, so V08c's promise holds: no heap value goes through the command channel.
- `LoopLength` is accepted only on blank tape. Otherwise it gets the note `clear the loop first (C) to change its length`.
- `LoopVolume` is held to 0 to 1, NaN ignored, and ramped over 10 ms.

The bounce has rings of its own, built with the looper, in the L02 pattern:
- `bounce_in: HeapRb<Box<[f32]>>` of 2 (UI to engine);
- `out: HeapRb<LoopOut>` of 4 (engine to UI), where `LoopOut` is `Bounced { samples: Box<[f32]>, rate: u32, layers: u8 }` or `Refused(Box<[f32]>)`.

The engine pops `bounce_in` only when a `held` slot is free, and an item the `out` ring has no room for is held and pushed again every callback. So a buffer is never dropped on the audio thread.

`LoopFeed { bounce_in: HeapProd<..>, out: HeapCons<..>, gauges: Arc<LoopGauges> }` goes to `run.rs`.

### Saving a loop as a WAV (`w`)

1. **The UI** reads `n` from the gauges. On blank tape the note is `nothing to save`; while a bounce runs, `already saving`.
2. **The UI** allocates `vec![0.0; n].into_boxed_slice()` and pushes it on `bounce_in`. A full ring gives the note `busy: save not sent`.
3. **The engine** refuses the box when its length is not `n` or the tape is blank. Otherwise it copies 64 cells a frame:
   - `samples[c] = Σ layers sounding at c` (merge-aware, as the read is), at unity: not × volume, and not through the effects (the loop as recorded).
   - Because the seam is baked, the file loops seamlessly in any player.
   - Each cell is copied as it is when the copy reaches it. A dub writing ahead of the copy is in the file, and one behind it is not; the strip says `saving` for the half second.
   - A clear, an undo, or a new take during a bounce refuses it back, and the note is `save cancelled: the loop changed`.
4. **`run.rs`**, each redraw, pops `out`.
   - On `Bounced`, it spawns a thread named `mouth-loop-wav`. The thread writes `$HOME/.kazoo/mouth/loops/mouth-loop-<unix seconds>.wav` (directory created `0700`; on a name clash `-2`, `-3` and so on) through V08c's `atomic_write`:
     - a `hound::WavWriter` over a `BufWriter<&mut File>`;
     - mono, the stream rate, 32-bit float;
     - finalised, then `atomic_write`'s `sync_all`, `rename` and directory sync: one durable operation.
   - Why not kazoo-core's `DiskRecorder` (`kazoo-core/src/io/recorder.rs:27`): it streams blocks as they arrive, and a loop is finished all at once.
   - Why not the reel store (KZ-L01): it sweeps unpinned reels a day after nothing names them, and a loop Tom saves must stay.
   - The thread reports through a `crossbeam_channel::bounded(8)`, and the footer shows `saved ~/.kazoo/mouth/loops/mouth-loop-<n>.wav (4.00 s)` or `loop not saved: <error>`.
   - On `Refused`, the box is dropped on the UI thread and the note is shown.
   - The writer threads' handles are joined when the mouth quits, before the terminal is restored, so a save in flight finishes.
   - `run.rs` takes the directory from `HOME` and passes it down. No environment variable overrides it, and tests use `target/kz-logs/`.

### Gauges (`Arc<LoopGauges>`, atomics, `Relaxed`, written once per callback)

`state` (u8), `n`, `capacity`, `head`, `taken` (cells of a take so far), `layers`, `merging`, `saving`, `bars` (0 = free), `loop_bpm` bits, `off_tempo`, `arcs: [[AtomicU64; 2]; 4]` (each stacked layer's `start` and `written`, oldest first), `beat_in_bar` bits (for the armed count) and `note` (u8, the last refusal's code, which the UI turns into its words) with `note_seq` (u32, bumped on every note so the UI shows each once).

### The screen (`src/ui/looper.rs`, `src/ui.rs`)

**The strip** is two lines between the trail and the meters. The trail gives them up: 7 lines at 80×24.
- **Line 1**: `◐◑  ▶ 2.10 / 4.00 s  4 bars ♩120.0  layers 3  vol 80%`.
  - The reels are two glyphs stepping through `◐◓◑◒`, one step per eighth of the loop while it plays, still otherwise. Tom's rule from Track L, "the dressing should never overpower function", is why there is nothing more than that.
  - The state words are: `blank: r records`, `armed: next bar 3.2` (the desk's bar.beat), `● rec 2.10 s`, `▶ 2.10 / 4.00 s`, `● dub 2.10 / 4.00 s`, `waiting: merging`, `■ stopped` and `no tape: the loop could not get its memory`.
  - The mode is `free`, or `4 bars ♩120.0`, followed by ` · desk 128.0` in the warning style when off tempo. `merging` and `saving` show when true.
- **Line 2**, the loop across the width: each column shows how many layers sound in its middle cell (` ` none, `░` 1, `▒` 2, `▓` 3, `█` 4), computed by the UI from the arcs. The head's column is reversed.
  - While a take runs, the bar is `capacity` wide in free mode and fills as it grows. In bars mode it is `n` wide.

**Keys** are global, in either focus:

| Key | Does |
|---|---|
| `r` | record; close the loop; punch an overdub in or out |
| `u` | undo the newest layer |
| `C` | clear the loop (capital, so a stray key cannot wipe it) |
| `l` | length: free → 1 → 2 → 4 → 8 → 16 bars → free, blank tape only, skipping any that does not fit at the tempo in use (the note says which and why) |
| `o` | stop or start the loop |
| `-` / `=` | loop volume down or up by 5% |
| `_` / `+` | by 25% |
| `w` | save the loop as a WAV |

None of these is bound by V08b or V08c. Every key sends with `try_send`, under V08b's busy rule. `?` help gains a "loop" section, one sentence per key, and the loops folder's path. The notes show in the footer for 3 s.

### Settings

V08c's file gains `loop: { length: "free" | <bars>, volume }`. A missing section loads the defaults (free, 0.8) with no note, so the file stays `version: 1`. Loop audio is never in the settings, only in saved WAVs.

## Not this lane

- Putting a mouth loop on the wall or the desk as a reel (L10's road).
- Recording the effects into the loop (record the desk strip into a wall `loop`, KZ-L09).
- Varispeed.
- More than one loop.
- Reversing, halving or doubling a loop.
- Loading a WAV into the looper.
- An audible count-in: the desk's click is the click; without a desk, a bars take starts on the press.

## Red tests first

Red for all: the looper does not exist.

Engine tests are in `src/engine/looper/tests.rs`. They drive `Looper` directly at 48 kHz in 128-frame chunks with a hand-built `beats` slice, plus `MouthEngine` with V08a's fake `DeskOut` where the desk or the chain matters. "Loop output" means `Looper::process`'s own output, before the engine adds the live block.

1. `a_free_take_closes_on_the_second_press_and_plays_from_the_top`: a 1.0 s ramp in, `r` at 0 and `r` at 1.0 s. `n` is 48,000. At volume 1, on every pass, the loop output at cells `tail .. n` equals the recorded ramp bit for bit.
2. `the_seam_is_faded_once_and_baked` (the L02 defect's guard):
   - Before the bake, cells `0 .. n` and `n .. n + tail` hold the input bit for bit: no write fade at the seam.
   - After the bake, cells `0 .. tail` equal `qsin(c/tail)·start + qsin(1−c/tail)·continuation`, computed the same way in the test, and every other cell is unchanged.
3. `a_sustained_sound_does_not_dip_at_the_seam`: 1.5 s of seeded white noise (xorshift32, constant seed) at −12 dBFS, with a 1.0 s take closed while the noise goes on. Over four passes, the RMS of each 8 ms seam window is within ±1 dB of the RMS of the 100 ms after it. With L02's double fade it would read about −3 dB.
4. `the_seam_does_not_click`: a 220 Hz sine starting mid-cycle and a 1.0 s take. Across every seam over four passes, the largest sample-to-sample step is at most 1.5 × the sine's own largest step.
5. `the_first_pass_fades_in_under_the_live_voice`: a constant 0.5 in, and a take closed at frame F. For `i < tail`, the loop output at F + i is `0.5 · qsin(i / tail)` exactly, with no tail term.
6. `an_overdub_is_a_layer_and_undo_takes_it_away`: a 1.0 s take of constant 0.25, then a full-pass dub of 0.125. The next pass plays 0.375 (outside the punch fades). `u` brings it to 0.25 within `tail` frames, and from then on it is exact.
7. `a_short_overdub_is_heard_only_where_it_was_sung`:
   - A full-pass dub of 0.5 is taken and undone.
   - Then a 0.2 s dub of 0.125 lands in the freed buffer.
   - Outside its arc, the loop plays the bed alone. None of the undone layer's stale cells is heard.
8. `punches_fade_in_and_out`: the dub layer's cells ramp linearly 0 → 1 over `tail` cells from the punch-in and 1 → 0 over `tail` cells after the punch-out press. With a 220 Hz sine dubbed over a sine bed, test 4's step bound holds at both punches.
9. `a_fourth_layer_merges_the_oldest_two_and_nothing_is_heard_to_change`:
   - Bed A, then dubs B, C and D. Starting D starts the merge.
   - Throughout, the loop output equals the sum of every layer read separately within 1e-6.
   - Afterwards the stack is [A+B, C, D], and a buffer is free.
   - Three undos leave [A+B], then [], each step heard as stated. An undo reaching B during the merge is refused with its note.
10. `a_merge_takes_half_a_second_at_any_callback_size`: with a 32 s loop at 48 kHz, the merge finishes within 24,000 frames (+1 chunk) at chunk sizes 64, 128, 1024 and 4096.
11. `a_punch_waits_for_a_merge_and_never_writes_into_it`: `r` during a merge with no free buffer gives `Waiting`. The punch lands on the frame the merge frees a buffer, and no merging buffer's cell changes from the dub.
12. `bars_mode_takes_its_bars_from_the_desks_bar_line`:
   - At 120 BPM, with `beats` running from 2.5, `Bars(1)`, and `r` at beat 2.6.
   - Nothing is written before the frame where beat crosses 4.0.
   - `n` is 96,000. It closes by itself 96,000 frames later and plays. `anchor` is 4.0.
13. `bars_mode_follows_a_desk_locate_without_a_click`: a 1-bar loop of a sine, anchored at beat 4.0. `beats` jumps from 9.3 to 6.0. The head lands on cell `round(2/4 · n)` after the crossfade, and test 4's step bound holds across it.
14. `bars_mode_without_a_desk_uses_the_mouths_tempo`: `beats` all NaN and `own_bpm` 100. `Bars(2)` gives `n = 230,400` from the press, and `anchor` is `None`.
15. `a_tempo_change_keeps_the_loops_pitch_and_says_off_tempo`: a 1-bar loop at 120, then the tempo goes to 128. A sine's zero-crossing rate is unchanged within 0.1%, and `off_tempo` is set.
16. `a_length_that_does_not_fit_is_refused`: `Bars(16)` at 40 BPM (96 s) refuses with its note and stays blank. In the UI, `l` at 40 BPM skips 8 and 16.
17. `the_loop_is_recorded_before_the_effects_and_played_through_them`: `MouthEngine` with V08c's reverb on.
   - The bed's cells equal the synth's mono block, not the reverb's output.
   - With the bed playing and the voice silent, the output equals the chain run on the loop output alone.
18. `the_loop_never_records_itself`: a dub over a playing bed with silent live input leaves the dub's cells at 0.
19. `volume_stop_and_clear_ramp_without_clicks`: volume 1 → 0 at once ramps over 10 ms. `o` fades over `tail` frames and restarts from cell 0. `C` fades over `tail` frames and leaves blank tape. Test 4's step bound holds at each.
20. `a_bounce_is_the_loop_with_its_seam_and_comes_back_without_a_free`:
   - The bounced samples equal the sum of the layers cell by cell, at unity.
   - With the `out` ring pre-filled, the box is held and delivered once there is room.
   - The whole run is under `assert_no_alloc`, which also catches frees.
21. `a_change_during_a_bounce_refuses_it_back`: an undo mid-bounce comes back as `Refused`. A box of the wrong length is `Refused` at once.
22. `nonsense_in_never_reaches_the_tape`: NaN, ±Inf and 1e30 recorded in a take and a dub. Every cell is finite and within ±4, and the output is finite.
23. `render_with_the_looper_never_allocates`: V08a's `render_never_allocates` with a take, two dubs, an undo, a merge, a bounce and a clear in its 10 s.
24. `no_memory_is_a_looper_that_says_so`: `Looper::with_capacity(0)` (a test constructor) gives `NoTape`. Every command is a no-op, and nothing panics.

UI and run tests:
25. `a_saved_loop_is_a_float_wav_that_loops`: `run`'s writer, given a bounced buffer and `target/kz-logs/mouth-loop-wav-test/`, writes `mouth-loop-<n>.wav`. Read back with `kazoo_core::io::read_wav`, it is 1 channel, 48 kHz and equal to the buffer. No `.tmp-` file is left, and a second save in the same second gets `-2`.
26. `the_loop_strip_shows_state_length_layers_and_volume`: a `TestBackend` at 80×24 from hand-built gauges shows `▶ 2.10 / 4.00 s`, `4 bars ♩120.0`, `layers 3`, `vol 80%`, a coverage line with `▓`, and the reversed head. Off tempo shows `· desk 128.0`.
27. `loop_keys_send_their_commands`: `r u C l o - = _ + w` each send their one command (`w` pushes a box of `n` onto a fake ring).
28. V08c's `the_footer_and_help_name_every_key` and `settings_round_trip` extended with this lane's keys and the `loop` section.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mouth --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mouth --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mouth
```

Doc tests if doc comments were written: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-mouth -j 4`.

## Definition of done

- [ ] All 28 red tests red first (paste each under RED), then green. Every V08a, V08b and V08c test is still green.
- [ ] `LOOP_SECONDS`, `MAX_LOOP_FRAMES`, `SEAM_SECONDS`, `BUFFERS`, `MERGE_CELLS_PER_FRAME`, `BOUNCE_CELLS_PER_FRAME` and `MIN_TAKE_SECONDS` exist as named, each with its reason in its doc comment. The memory figure at 48 and 96 kHz is in `BUFFERS`'s.
- [ ] Not the builder's: after landing, Cassio installs and asks Tom to:
  - loop a hummed phrase free, layer two harmonies over it, undo one and save it;
  - then, with the desk playing, take 2 bars and listen at the seam for a click or a dip.

  The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `Looper::process` and everything it calls never allocate, free, lock, format or do I/O:
- the four buffers are made resident on the control side;
- a bounce box arrives and leaves through rings and is never dropped on the callback;
- merges and bounces are bounded at 64 cells a frame;
- the bake is at most `tail` cells, once per take.

Cell positions are `usize` throughout (the head moves one cell a frame, so there is no fractional read and no rounding at the seam: L02's "write position rounds to the nearest cell" defect cannot arise). No `as` cast that clippy pedantic flags. The WAV is written off the callback and the UI thread, as one durable operation. Files under 1000 lines.

## END

```
START KZ-V08d base=<sha> branch=lane/kz-V08d df=<GB> at <time>
END KZ-V08d head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
