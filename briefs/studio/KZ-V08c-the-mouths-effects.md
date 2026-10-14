# KZ-V08c: the mouth's effects: megaphone, distortion, delay, echo and reverb on the mouth's own sound, every knob on its screen, the delays on the desk's tempo, all of it kept between runs

Line numbers are from the sweep that wrote this brief, at `508b869`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Drafted 14 Oct 2026 (morning) for Cassio to read and commit, from Tom's first hum of `kazoo mouth`.

Place in the track: Track V (the voice), after V08b. It adds to the mouth the chain Tom asked for, built from kazoo-fx kinds the wall already plays. No new effect is written.

Time: about 45 minutes of a builder's work. One mechanism, an effect chain owned by the mouth's callback, with its panel and its settings file.

Crates: kazoo-mouth only. kazoo-fx is read, never changed.

Depends: KZ-V08a (the crate, `MouthEngine::render`, the `DeskOut` trait, the command channel) and KZ-V08b (the screen, `App`, `controls.rs`, `ui::draw`). Opens only after both have landed. KZ-V08d and KZ-L08 also edit `src/engine.rs` and `src/run.rs`, so they run after this lane, never beside it.

Files (9): `kazoo-mouth/Cargo.toml` (adds `kazoo-fx`, `serde`, `serde_json`), `src/engine.rs` (the chain in the callback, the desk's transport, the new commands), `src/engine/fx.rs` (new: `FxChain`, the slots, the ramps), `src/engine/fx/tests.rs` (new), `src/controls.rs` (focus, the fx keys, the tempo keys), `src/controls/settings.rs` (new: the settings file), `src/ui.rs` (the layout), `src/ui/fx.rs` (new: the fx panel), `src/run.rs` (loads the settings at start, saves them). `engine/fx.rs` is declared in `engine.rs`, `controls/settings.rs` in `controls.rs` and `ui/fx.rs` in `ui.rs`, so `lib.rs` is not touched.

## Why

Tom, 14 Oct 2026, about 09:05 to 09:25, after his first hum of `kazoo mouth` (QUEUE.md, his asks (2) and (b)):
- "I feel like the mouth part of this is an instrument in itself. It's not just another component of the wall."
- "what I really want to see in front of me is all the controls... I want to be able to chuck in like reverb and delay."
- "I can't tell you how great... some megaphone and distortion... delay and echo just... would sound stunning."
- "I just want knobs and dials to fuck with the delay and the things like that... the synth sound and tone."
- "I want it to be able to go on the wall as well, but I want it to be its own monster."

V08a gives the mouth its own synth and V08b gives it a screen with every quantiser and synth control. Nothing touches the sound after the synth. V08a's engine writes the synth's mono block straight to the outputs and the desk (`KZ-V08a-the-mouth.md`, engine step 5). An effect chain inside kazoo-mouth makes the mouth its own monster: its sound is finished before it reaches the desk, as a guitarist's pedalboard is, and the desk still gets one ordinary strip.

Every effect Tom named already exists in kazoo-fx, and the wall already plays them all:
- The wall's own `delay` and `reverb` modules were retired into kazoo-fx kinds: the old delay became `digital` (or `tape`), and the old reverb became `plate` (or `hall`) (`kazoo-wall/src/migrate.rs:313-347`).
- The wall plays any kazoo-fx kind through one adapter (`kazoo-wall/src/dsp/effect.rs:1-12`).
- So the wall's delay, echo, reverb and spring are `digital` (`kazoo-fx/src/time/digital.rs:163`), `tape` (`time/tape.rs:196`), `plate`, `hall` and `spring` (`space/mod.rs:31-38`).
- Distortion is the drive family (`drive/mod.rs:35-44`).
- **The megaphone is already a real model**: `radio`, model 3 (`lofi/radio.rs:31-34`, `:538-600`). It is a voice mic high-passed at 300 Hz, a small amplifier clipped hard (`drive`, oversampled twice), then a horn's 500 Hz to 4.5 kHz band with resonant peaks at 1.1 kHz (+6 dB) and 2.6 kHz (+5 dB). `tune` shifts the band and `drift` sways it as if the horn were waved about.

So the megaphone is a preset over an existing kind, not a new kazoo-fx kind, and this lane writes no DSP of its own beyond gains and ramps.

Tom's (c), "the tracking stuff... needs to come out of there and just go into the tracking part", is V08a's move of kazoo-tui to `kazoo tui`. This lane adds nothing of the DAW to the mouth.

## Outcome

`kazoo mouth` has five effect slots after its synth, in this order: megaphone, distortion, delay, echo, reverb. Each one:
- is switched on and off with `b` without a click;
- can be moved earlier or later in the chain with `{` and `}`;
- shows every knob of its kazoo-fx kind on the screen, each turned with the arrow keys.

Distortion offers fuzz, screamer, valve amp or bit crusher. Delay offers the clean digital delay or the bucket-brigade. Reverb offers plate, hall or spring. The megaphone slot is the radio, so telephone, walkie-talkie and AM radio are a knob away.

When a desk is running, the delays' synced times follow the desk's tempo from the frame it changes. Otherwise they follow the mouth's own tempo, which `,` and `.` set. Every setting, the synth's and the quantiser's included, is saved a second after it changes and is there at the next start.

## The design (decided; build exactly this)

### The slots (`src/engine/fx.rs`)

`pub(crate) enum Slot { Megaphone, Distortion, Delay, Echo, Reverb }` (`Copy`, `Eq`, with `ALL: [Slot; 5]` in that order and `name()`: `megaphone`, `distortion`, `delay`, `echo`, `reverb`). Each slot holds a fixed list of kazoo-fx kinds, found with `kazoo_fx::find(id)` at build. The first kind in each list is its default. Each slot also has a ramp style.

| Slot | Kinds (ids, default first) | Ramp style | Mouth defaults that differ from kazoo-fx's |
|---|---|---|---|
| megaphone | `radio` | colour | `model` 3 (megaphone); `drive` 50; `noise` 0 |
| distortion | `fuzz`, `screamer`, `amp`, `crush` | colour | none |
| delay | `digital`, `bbd` | trail | `digital`: `lsync` 7 (`1/8d`), `rsync` 9 (`1/4`), `routing` 1 (ping-pong) |
| echo | `tape` | trail | none |
| reverb | `plate`, `hall`, `spring` | trail | none |

Why these choices:
- **The defaults that differ.**
  - `noise` 0: the megaphone's hiss (`radio.rs:594`) would otherwise hiss on between notes while V08a's gate holds the synth silent.
  - `drive` 50: Tom asked for megaphone *and* distortion, and 30 is a polite PA.
  - Ping-pong at a dotted eighth against a quarter: a mono voice then spreads across the stereo field.
- **The default order.** The megaphone is first because it is the voice's own colour, as if the mouth sang through a horn. Driving that band-limited signal keeps the distortion from turning the low end into mud. The time effects come after the drive, so each repeat is a clean copy of the driven sound and is not driven again into mush. The reverb comes last because it is the room.
- **The four distortions.** They span the family's characters: fuzz (square, with octave), screamer (mid-hump overdrive), amp (valve with cabinet) and crush (digital). Klon, fold, ringmod and comp stay on the wall.
- **Echo is tape only.** `tape` is the Space Echo (`tape.rs:1-37`) and is the "echo" Tom named. `bbd` sits under delay as its darker alternative.

Every kind of every slot is built, prepared at the stream rate and seeded at start, on the control side (`FxChain::new`). That is ten effects in all. Each one's seed is `0x6D6F_7574_6800 + n`, where n is its place in the table read top to bottom and left to right, so a test can build the same effect alone. Switching a slot's kind therefore never allocates.

Each kind instance keeps its own parameter values, so going from fuzz to amp and back finds the fuzz as it was left. `pub(crate) const MAX_PARAMS: usize = 16` is the per-kind value array. A test asserts that every listed kind has at most 16 parameters (digital has 12 today; KZ-P03 adds one to the drives).

### The chain and its place in the callback

In V08a's `MouthEngine::render`, per chunk (V08a renders in chunks of at most 4096 frames):
1. V08a's step 5 makes the synth's mono block (`synth.render(mic, &mut mono)`).
2. `fx.process(&context, &mono[..frames], [&mut left[..frames], &mut right[..frames]])`. `left` and `right` are pre-allocated, sized to V08a's largest chunk.
3. The safety ceiling (below) is applied to both sides.
4. The result goes where V08a sent the mono block:
   - Silent: the outputs are zeroed, as V08a's silent rule says.
   - Plugged into the desk: the stereo block, interleaved L/R, goes by `send_audio` and the local output is silent.
   - Playing direct: channel 0 gets `left` and channel 1 gets `right`. A one-channel device gets `0.5 · (left + right)`. Channels past 2 are zeroed, as `kazoo-dx/src/audio.rs:332-337` does.

The chain runs even when silent, so the tails are continuous when Tom turns silent off. The voice frames, V08a's step 4, are untouched.

`FxChain` holds:
- `order: [Slot; 5]`;
- per slot: the built kinds (`Box<dyn kazoo_fx::Effect>` each), `active: usize`, `on: bool`, `gain: Ramp` (0 to 1), `presence: Ramp` (0 to 1, used by reorders), `fading_from: Option<usize>` (the kind being faded out after a switch) with its own `Ramp`, and `asleep: bool`;
- three pre-allocated stereo scratch pairs (`a`, `b` and `spare`), each sized to the largest chunk.

The slots run in `order`, ping-ponging between `a` and `b`. The mono input is copied to both sides of `a` first.

`Ramp` (`pub(crate)`, so KZ-V08d can use it) is linear. It crosses from one end to the other in a number of frames given when it is made, is stepped once per sample, and lands exactly on its target. Every ramp in this lane is `RAMP_SECONDS: f32 = 0.010`. Why linear and 10 ms: the dry and wet sides of a slot are the same signal, correlated, so a linear (equal-gain) crossfade holds the level where an equal-power one would bulge by 3 dB. 10 ms is past the point where a fade is heard as a click and short enough to feel instant under a key.

**Colour slots** (megaphone, distortion): `out = (1 − g) · in + g · fx(in)`, per sample.
- When `g` reaches 0 with the slot off, the effect is no longer processed (`asleep`).
- When the slot is turned on, the effect is `reset()` (real-time safe) and processed from the first sample of the ramp.
- Why skip it while off: the oversampled drives are the costliest kinds in kazoo-fx (PLAN.md, "Where we are": 8× to 16×). An off pedal should cost nothing.

**Trail slots** (delay, echo, reverb): the effect is fed `g · in` and `out = (1 − g) · in + fx(g · in)`.
- So turning one off fades its input out while its repeats or tail ring on and die naturally: a pedal with trails.
- An off trail slot keeps being processed until its output has stayed below 1e-6 (−120 dBFS) on both sides for one whole second of frames. It then sleeps. Turned on again, it resumes without a reset, because its lines are already silent.
- Why trails: a delay switched off mid-phrase that cut its repeats would be the click and the hole Tom would hear. A drive has no tail to keep.

**Kind switch** (a slot's kind changes):
- The old kind's output fades out over one `Ramp` while the new kind's output fades in, both fed the same input.
- When the fade ends, the old kind is `reset()` and stops being processed.
- Delay and reverb tails of the old kind are cut by that 10 ms fade. It is a fade, not a click, and switching the kind is a deliberate act.
- A second switch while one is fading first snaps the running fade to its end, then starts the new one.

**Reorder**:
- A new order is accepted only if it is a permutation of the five slots. Anything else is ignored and counted in `fx_refused`.
- Every slot whose position changes ramps its `presence` to 0 over one `Ramp`. `out = (1 − p) · in + p · slot_out`, so the slot passes its input straight through.
- When every moving slot is at 0, `order` is swapped, and they ramp back to 1.
- The effects keep their state, so a delay's repeats return after the 20 ms. Only their contribution was muted.

**Off slots cost nothing**: all slots off, every one asleep, makes the chain's output equal to its input bit for bit.

**NaN and Inf**: every slot's output is passed through `kazoo_core::sanitize_sample`. A slot that produces a non-finite sample is `reset()`, and `fx_faults` is counted. `fx_faults` and `fx_refused` are `AtomicU64`s in V08a's shared stats (`Relaxed`). The header shows each beside V08a's counts whenever it is above zero. kazoo-fx promises this never happens (`kazoo-fx/src/lib.rs:19-25`); the mouth checks anyway.

**The safety ceiling** (after the chain, both sides): `|x| ≤ 0.8` passes unchanged; above that, `sign(x) · (0.8 + 0.2 · tanh((|x| − 0.8) / 0.2))`. It is continuous with slope 1 at 0.8, never reaches 1.0, and `tanh` runs only above 0.8.
- Why: delay feedback is capped at 0.95, but `freeze` holds the loop at unity, and the drives' `level` adds gain. Playing direct, there is no desk limiter between this and Tom's ears.
- V08a's own soft clip, before the chain, stays.

### The desk's tempo

kazoo-fx gives every effect `Context { bpm }` on every call (`kazoo-fx/src/lib.rs:109-122`). `digital` and `tape` resolve a synced time through `synced_seconds` (`time/parts.rs:47`; the note values are `SYNC_LABELS`, `:12`). The mouth only has to pass the right tempo.

- **`DeskOut`** (V08a's test trait) gains two methods, each implemented for `HubLinkAudio` and for the fake:
  - `fn drain_events(&mut self, on: &mut dyn FnMut(HubEvent))`. For `HubLinkAudio` this is `kazoo_core::host::link::drain_events(&mut self.events, |e| on(e))` (`kazoo-core/src/host/link.rs:234`; `events` is public, `ipc/link.rs:426`).
  - `fn stream_frame(&self) -> u64`, which is `HubLinkAudio::stream_frame` (`ipc/link.rs:732`).
  - The fake holds a queue of events the test pushes, and counts frames as `send_audio` does.
- **`MouthEngine`** gains:
  - `follower: kazoo_core::ipc::follow::TransportFollower` (`ipc/follow.rs:82`), built at the stream rate;
  - `desk_bpm: Option<f64>`;
  - `own_bpm: f64`, default 120, held to 30 to 300;
  - `beat: Option<f64>`;
  - `beats: Box<[f64]>`, sized to the largest chunk. V08d reads it.
- **Each callback** first drains the events:
  - `TransportSync(sync)` goes to `follower.schedule(&sync)`. An error is counted in `desk_lost`, as `kazoo-dx/src/audio.rs:272-295` does.
  - Every other `HubEvent` variant is ignored, listed by name in the match (no wildcard), so a new variant is a compile error here and gets a decision.
- **Each frame `i` of a chunk** is handled as follows (first = `hub.stream_frame()` before this chunk's `send_audio`):
  - If `follower.due(first + i)` gives a change, then `desk_bpm = Some(change.bpm)` and `beat = change.beat`.
  - Then `beats[i] = beat.unwrap_or(f64::NAN)`.
  - Then, while `beat` is `Some`, it is advanced by `kazoo_core::ipc::follow::beats_in(1.0, bpm, rate)`.
- **The chunk's tempo** is `desk_bpm` while `hub.is_connected()`, else `own_bpm`. The effects get the tempo at the chunk's first frame: a change lands at most one chunk late (≤ 2.7 ms at 128 frames), and `digital` glides its time change over 40 ms anyway (`digital.rs:10-13`).
- **When the desk goes away** (connected becomes false), `own_bpm` takes the last `desk_bpm`, rounded to 0.1, so the repeats do not jump. `desk_bpm` is then cleared. `own_bpm` is published for the screen and the settings file as an `AtomicU64` of f64 bits, beside `desk_bpm` (0 bits meaning none).

### Commands (added to V08a's command enum)

- `FxOn { slot: Slot, on: bool }`.
- `FxKind { slot: Slot, kind: u8 }`. An index past the slot's list is ignored and counted in `fx_refused`.
- `FxParam { slot: Slot, kind: u8, index: u8, value: f32 }`. It goes to that kind's `set_param`, which clamps and ignores NaN. It is also stored in the kind's values, so a later switch back finds it.
- `FxOrder([Slot; 5])`.
- `OwnTempo(f64)`, held to 30 to 300 (NaN ignored).

All are `Copy`. Nothing in this lane sends a heap value through the channel.

### Settings: saved, restored (`src/controls/settings.rs`)

This supersedes V08a's "Not this lane" decision on saving. Once a file exists for the effects, leaving the synth and quantiser out of it would have Tom re-turn half his knobs at every start.

- **Where**: `$HOME/.kazoo/mouth/settings.json`. The directory is created `0700` if missing.
  - `run.rs` computes the path from `HOME` and passes it down. No environment variable overrides it: QUEUE.md records KZ-T02b's finding that an inherited override leaks into tests.
  - Every function in `settings.rs` takes the path or directory explicitly, and tests use `target/kz-logs/mouth-settings-<test name>/`, never the system temp directory.
- **What** (serde, JSON, pretty, `version: 1`):
  - `quantise`: `{ on, scale (SCALE_NAMES name), root (NOTE_NAMES name), snap_ms, keep_vibrato, hysteresis_cents, blend }`. `kazoo_core::voice::quantise` has no serde, so this mirrors `QuantiseSettings` (`kazoo-core/src/voice/quantise.rs:86`), plus V08b's on/off and remembered blend.
  - `synth`: `{ shape (name), cutoff_hz, air, volume }`.
  - `tempo`: `own_bpm`.
  - `fx`: `{ order: [slot names], slots: { <slot name>: { on, kind (id), kinds: { <kind id>: { <param name>: value } } } } }`.
  - Parameters are saved **by name**, and kinds by id, so a change to kazoo-fx's parameter order never moves a saved knob onto another.
  - `silent` is not saved. It is a choice for one run (`--silent`).
- **Load** (`pub(crate) fn load(path) -> Loaded { settings, notes: Vec<String>, writable: bool }`), at start, before the engine is built:
  - The values go into `FxChain::new` and V08a's synth and quantiser construction, so a start never pushes a hundred commands through the 64-slot channel.
  - Each value is passed through its own clamp: `ParamSpec::clamp` (`kazoo-fx/src/lib.rs:80`), `QuantiseSettings::held` (`quantise.rs:117`), and V08a's `SynthSettings` ranges. A non-finite number becomes the default.
  - An unknown slot, kind, parameter, scale or note name is skipped and named in `notes`.
  - An `order` that is not a permutation falls back to the default order, with a note.
  - **No file**: defaults, no note.
  - **A file that is not valid JSON, or has the wrong shape**: it is renamed to `settings.json.bad-<unix seconds>`, the defaults are used, and the note says so with the new name.
  - **A `version` above 1** (a newer kazoo wrote it): defaults are used, `writable` is false, the file is left untouched, and the note says "settings are from a newer kazoo: not saved this run".
- **Save** (`pub(crate) fn save(path, &Settings) -> io::Result<()>`): one durable operation, as Tom's storage rule asks and as `kazoo-core/src/reels.rs:270-300` does.
  - Create `<path>.tmp-<pid>` with `create_new` and mode `0600`, write, `sync_all`, `rename` over `path`, then `sync_all` on the directory.
  - `pub(crate) fn atomic_write(path, impl FnOnce(&mut File) -> io::Result<()>)` is the shared helper. KZ-V08d writes its WAV files through it.
- **When**: on the UI thread, never the callback.
  - A save happens one second after the last change the engine accepted (each accepted change restarts the second).
  - Another save happens when the mouth quits (Esc, Ctrl-C), before the terminal is restored.
  - A failed save shows `settings not saved: <error>` in the footer until the next save succeeds. It never blocks or quits.
  - The UI's own model (V08b's "last value the engine accepted") is the source, so the file holds what Tom hears.

### The screen (`src/ui/fx.rs`, `src/ui.rs`, `src/controls.rs`)

**Focus**: `Tab` moves the focus between V08b's synth controls panel and the fx panel. The header shows `synth` or `fx` beside the link state.

**Layout**: V08b's controls panel is 6 lines. The fx panel is 6 lines too.
- At 30 rows or more, both are shown, synth above fx.
- Under 30 rows (V08b's minimum is 80×24), the focused one is shown in the same 6 lines.
- The trail keeps "every line the other panels leave", which is still 9 lines at 80×24.

**The fx panel**:
- Line 1, the chain: one chip per slot in chain order, e.g. ` 1 megaphone ●  2 fuzz ○  3 delay ● 1/8d·1/4  4 echo ○  5 plate ○ `, followed by the tempo, `♩120.0 own` or `♩128.0 desk`.
  - Each chip is the slot's position, then the active kind's short name. The megaphone chip shows the radio's model label when it is not the megaphone: `telephone`, `walkie-talkie`, `am radio`.
  - `●` means on, `○` off, and `◐` while any of its ramps is moving.
  - The delay chip adds its synced note values, or its times in ms when free.
  - The selected slot is drawn reversed.
- Lines 2 to 6: the selected slot's cells in 3 columns of 26 characters × 5 rows (15 cells; digital needs 13). The cells fill column by column.
  - The first cell is `kind  fuzz ◂▸`.
  - Every other cell is one parameter: `name  value unit  ▕████░░░░▏`.
  - The bar is the knob's travel: linear by value, log by `ln(v/min)/ln(max/min)`, and none for a stepped knob, which shows its label instead.
  - Values: Hz as `850 Hz` or `1.20 kHz`; s as `375 ms` below 1 s, else `1.25 s`; dB to one decimal; `%` as a whole number; unitless to two decimals.
  - The selected cell is reversed.

**Keys** with the fx panel focused (V08b's global keys still work: Space, `v`, `s`, `?`, Esc, Ctrl-C):

| Key | Does |
|---|---|
| `[` / `]` | select the previous or next slot in chain order (wrapping) |
| `↑` / `↓` | select the previous or next cell (wrapping) |
| `←` / `→` | turn the cell: on `kind`, the previous or next kind (wrapping); otherwise one fine step |
| `Shift+←` / `Shift+→` | one coarse step |
| `b` | the selected slot on or off |
| `{` / `}` | move the selected slot one place earlier or later (sends `FxOrder`; the selection follows the slot) |
| `d` | the selected knob back to the mouth's default (the table above, else kazoo-fx's) |

Steps: linear, fine `(max − min) / 100` and coarse `/ 10`. Log, fine `× (max/min)^(1/100)` and coarse `^(1/10)`. Stepped, ±1 for both. Every step lands within the range (`ParamSpec::clamp`). Why: a hundred presses cross any knob finely, and ten cross it fast, the same feel on every effect whatever its unit.

**Tempo keys** (global): `,` and `.` set the own tempo down or up by 1 BPM, and `<` and `>` by 10. While a desk tempo is in use, they do not change it: the footer says `the tempo is the desk's (128.0): change it on the desk` for 2 s.

**Every key** sends its command with `try_send`, and follows V08b's rule: a full channel shows `busy: change not sent`, and the screen goes back to what the engine has.

**Footer**: per focus.
- Synth: `↑↓ choose  ←→ change  space quantise  v vibrato  s silent  tab fx  ? help`.
- Fx: `[ ] slot  ↑↓ knob  ←→ turn  b on/off  { } move  d default  tab synth  ? help`.
- Both fit 80 columns. `Esc quit` and the shift steps move into `?` help, which lists every key of V08b and of this lane with one sentence each, every kazoo-fx kind's own `description`, and the settings file's path.

**Settings notes**: `notes` from loading show in the footer for 5 s each at start, one after another.

## Not this lane

- The looper (KZ-V08d).
- `mouth/dry` (KZ-L08). The dry voice is the mic and never goes through this chain.
- New kazoo-fx kinds, and klon, fold, ringmod, comp, shimmer, resonator or the lofi media in the mouth: they are on the wall.
- Saving named presets (one settings file is what was asked).
- The desk's inserts (C02, C03).
- Sending the mouth's own tempo to the desk (the desk's tap tempo sets the studio's).
- The input-rate defect Tom hit (QUEUE.md, 14 Oct 09:0x): that is a fix to V08a's input path, briefed as KZ-V11.

## Red tests first

Red for all: the chain, the commands, the settings file and the panel do not exist.

Engine tests are in `src/engine/fx/tests.rs` and drive `FxChain` directly at 48 kHz in 128-frame chunks, plus `MouthEngine` with V08a's fake `DeskOut` where the desk matters.
1. `the_chain_starts_in_order_and_all_off_is_the_synth_bit_for_bit`: the default order is the table's, and every slot is off. 2 s of a 220 Hz saw at −12 dBFS comes out equal to the input, bit for bit (`to_bits`), on both sides.
2. `each_slot_is_its_kazoo_fx_kind`: for every slot and every kind in its list, a chain built with only that slot on from the start (no ramp) and that kind active is fed 1 s of the saw. Its output equals that kind built alone with `kazoo_fx::find`, prepared at 48 kHz, seeded with the same seed, set to the mouth's defaults and fed the same input, sample for sample.
3. `the_megaphone_is_the_radios_horn`: the megaphone slot's default settings read back as `model` 3, `drive` 50, `noise` 0. With white noise in, the output's energy below 250 Hz is at least 20 dB under its energy from 1 to 3 kHz (FFT band sums). Between notes (input silence after a note), its output falls below −90 dBFS within 50 ms: no hiss.
4. `switching_on_and_off_never_clicks`: for each slot, a 220 Hz sine at −12 dBFS goes in and the slot is switched on and off five times, 37 ms apart. The largest sample-to-sample step across every switch is at most 1.5 × the larger of the largest steps of the steady off output and the steady on output.
5. `a_delay_rings_out_after_it_is_switched_off`: delay on at 120 BPM (`lsync` 1/8d, 0.375 s). An impulse at 0 is followed by off at 50 ms. The first left repeat appears at 375 ms ± 1 ms. A second impulse at 200 ms has no repeat at 575 ms. The slot is asleep within 1 s after its output stays under −120 dBFS.
6. `the_delay_follows_the_desks_tempo`: `MouthEngine` with the fake desk connected. The delay is on, with `lsync` set to 9 (`1/4`), and the first left repeat is measured. (Ping-pong feeds the left line only, so the right's first repeat comes later.)
   - A pushed `TransportSync` (playing, 100 BPM, `at_frame` 0) puts that repeat at 0.600 s ± 1 ms.
   - A second sync at 150 BPM, after 1 s, puts it at 0.400 s ± 1 ms once 100 ms have passed.
   - Unplugged, it is at `own_bpm` 120, 0.500 s.
   - Disconnecting at 150 leaves `own_bpm` at 150.0.
7. `kinds_switch_without_allocating_or_clicking`: inside `assert_no_alloc`, distortion goes fuzz → screamer → amp → crush → fuzz and reverb goes plate → hall → spring → plate while rendering. The output is finite, and the step bound of test 4 holds against both kinds' steady outputs.
8. `a_reorder_swaps_without_a_click`: every slot is on and delay is moved before distortion. The order reads back as sent after 20 ms, and test 4's step bound holds. A non-permutation (`[Delay; 5]`) is ignored and `fx_refused` counts 1.
9. `nonsense_in_is_survived`: NaN, ±Inf and 1e30 in, NaN and ±Inf through `FxParam`. Output is finite and never above 1.0 in magnitude. A clean sine afterwards is processed as test 2's.
10. `the_ceiling_holds_full_scale`: distortion `level` at its maximum, delay `feedback` 0.95 and `freeze` on, a full-scale saw for 10 s. Every output sample's magnitude is under 1.0, and samples under 0.8 in magnitude before the ceiling are unchanged by it.
11. `render_with_every_slot_on_never_allocates`: V08a's `render_never_allocates`, with every slot on and one switch of each kind during the run.
12. `every_kind_fits_max_params`.

Settings tests are in `src/controls/settings.rs`, with files under `target/kz-logs/`.
13. `settings_round_trip`: non-default values in every section are saved then loaded and come back equal. No `.tmp-` file is left behind.
14. `a_corrupt_file_is_kept_aside_and_defaults_are_used`: the file is renamed `settings.json.bad-<n>`, the defaults load, and a note names the file.
15. `unknown_and_out_of_range_values_are_skipped_or_held`: an unknown kind, an unknown parameter, `cutoff_hz` 1e9, `blend` NaN and an `order` with a repeat give the defaults where needed, held values otherwise, and one note for each.
16. `a_newer_version_is_never_overwritten`: `version` 2 loads defaults with `writable` false, and the file's bytes are unchanged after a save is asked for.

UI tests use `TestBackend` and the pure `App` key functions with V08b's fake sender.
17. `the_fx_panel_shows_the_chain_and_the_selected_slots_knobs`: at 80×24, focus fx, delay selected. The chips appear in order with `●`/`○`, `1/8d·1/4` is shown, and the `ltime`, `feedback` and `mix` cells are there.
18. `both_panels_show_at_thirty_rows`: at 80×30, both panels. At 80×24, only the focused one.
19. `fx_keys_select_turn_bypass_and_move`: `]` selects the next slot; `b` sends one `FxOn`; `→` on `kind` sends `FxKind`; `Shift+→` on `feedback` sends one `FxParam` with the coarse step; at the maximum it stays at the maximum; `}` sends `FxOrder` with the two slots swapped, and the selection follows; `d` sends the default.
20. `the_tempo_keys_belong_to_the_desk_when_it_has_one`: unplugged, `.` sends `OwnTempo(121)`. With a desk tempo shown, nothing is sent and the footer says so.
21. `the_footer_and_help_name_every_key`: V08b's test, extended to every key of this lane.

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

- [ ] All 21 red tests red first (paste each under RED), then green. Every V08a and V08b test is still green.
- [ ] Every mouth default that differs from kazoo-fx's carries its reason in a doc comment where it is set.
- [ ] Smoke without the mic or speakers: `kazoo mouth --help` exits 0 (paste).
- [ ] Not the builder's: after landing, Cassio installs and asks Tom to hum into `kazoo mouth` and turn on the megaphone, then the fuzz, then the delay with the desk running at two tempos, then the reverb. Tom also moves one slot and restarts the mouth to see his settings come back. The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.
- [ ] Not the builder's: the callback's cost with every slot on, at 128 frames, measured by Cassio on a quiet Mac after landing.

## Code quality

As PLAN.md's STACK RULES and kazoo-fx's real-time contract (`kazoo-fx/src/lib.rs:19-25`). `MouthEngine::render` and everything it calls never allocate, free, lock, format or do I/O:
- every effect is built and prepared in `FxChain::new` on the control side;
- every buffer is sized there to V08a's largest chunk;
- a kind switch, a bypass and a reorder move indices and ramps only.

Settings files are written on the UI thread only, as one durable operation. Files under 1000 lines: `engine/fx.rs` splits its ramps into `engine/fx/ramp.rs` if it nears that, and that split is the only allowed extra file.

## END

```
START KZ-V08c base=<sha> branch=lane/kz-V08c df=<GB> at <time>
END KZ-V08c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
