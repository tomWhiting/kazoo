# The Wall — a shared modular synth that never stops

Tom's brief (26 Sep 2026, voice room with the seats): a big modular synth "along the wall of the office". It keeps playing forever. Nobody is responsible for playing it. Anybody can reach up at any time to:

- twiddle a knob;
- move a plug;
- add a new module;
- take a module away.

"Anybody" means Tom, and every Claude seat through an MCP server that also pushes Claude channel notifications. Tom gets a TUI to see and control it.

Explicit non-goals, from Tom:
- Nothing is automated.
- No event streams or embeddings drive knobs.
- Nothing is decided in advance about how it should sound.
- Knobs do **not** drift back home.

Hard limits exist only for dangerous levels.

**Done means:**
1. `kazoo-wall` plays forever from a persistent patch.
2. The `kazoo-mcp` server works in Claude Code. It exposes the tools and delivers `notifications/claude/channel` when other seats change the wall.
3. Several seats can take part at once.
4. Tom's TUI shows the wall live and can change everything.

Every crate passes the usual gates: fmt, clippy pedantic+nursery with `-D warnings`, the tests, and `ast-grep scan` with zero findings. The work also gets an Opus review with every finding fixed.

## Crates

| Crate | Kind | What it is |
|---|---|---|
| `kazoo-wall` | lib + bin | The lib holds the patch model, the module DSP, the realtime engine and the control protocol types. The bin has three commands. `kazoo-wall serve` runs the headless daemon that plays forever. `kazoo-wall` with no argument opens the TUI, starting a detached daemon if none is running. `kazoo-wall stop` asks the daemon to exit. |
| `kazoo-mcp` | bin | A stdio MCP server, one per Claude session, using rmcp `=3.2.0` (the version dot-seat uses) and tokio. It is a client of the daemon's control socket. |

## Signal conventions

All ports carry `f32` samples at audio rate:

- **Audio:** nominally ±1.
- **CV:** nominally ±1. A knob's CV input adds `cv × knob range / 2`, scaled by the cable's `amount`.
- **Pitch:** 1.0 per octave, with 0.0 = C4 (261.626 Hz). V/oct, as in Eurorack.
- **Gate/trigger:** high when > 0.5.

## Module catalogue

A knob has a name, a range, a unit, a default and a curve (linear or log). Values are always clamped to the knob's range. The ranges marked ⚠ are the safety caps.

| kind | knobs | inputs | outputs |
|---|---|---|---|
| `vco` | `octave` (-4..4, stepped), `tune` (-12..12 st), `shape` (0 sine → 1 triangle → 2 saw → 3 square, continuous morph), `width` (0.05..0.95), `level` (0..1), `fm_depth` (0..1) | `pitch`, `fm` (linear, depth = `fm_depth`) | `out` |
| `lfo` | `rate` (0.01..30 Hz, log), `shape` (sine/tri/saw/square/random-step/smooth-random, continuous 0..5), `depth` (0..1), `offset` (-1..1), `sync` (0 = free, else beat division 1/16..16 bars) | `reset` | `out` |
| `noise` | `colour` (0 white .. 1 pink .. 2 brown), `level` | — | `out` |
| `vcf` | `cutoff` (20..18000 Hz, log), `resonance` (0..0.95 ⚠), `mode` (0 LP → 1 BP → 2 HP morph), `drive` (0..1) | `in` (the `cutoff` knob's jack moves it in octaves: ±1 = ±5 oct × amount) | `out` |
| `vca` | `gain` (0..1), `bias` (0..1) | `in`, `cv` | `out` |
| `env` | `attack` (1 ms..10 s, log), `decay`, `sustain` (0..1), `release` (1 ms..20 s, log) | `gate` | `out` (0..1) |
| `clock` | `division` (1/16 .. 4 bars, stepped), `swing` (0..0.75), `width` (0.05..0.95) | `reset` | `out` (gate), `beat` (ramp 0..1 over the division) |
| `seq` | `steps` (1..16), `step1`..`step16` (-24..24 st), `gate` (0.05..1 length), `chance` (0..1, per-step gate probability) | `clock`, `reset` | `pitch`, `gate` |
| `sh` | `slew` (0..1 s) | `in`, `trig` | `out` |
| `quant` | `scale` (chromatic, major, minor, dorian, phrygian, lydian, mixolydian, pentatonic major/minor, whole tone, stepped 0..9), `root` (0..11) | `in` | `out` |
| `slew` | `rise`, `fall` (0..5 s, log) | `in` | `out` |
| `mix` | `level_a`, `level_b`, `level_c`, `level_d` (0..1) | `a`, `b`, `c`, `d` | `out` |
| `out` | `level` (0..1 ⚠, see the master chain), `pan` (-1..1) | `left`, `right` (mono when `right` is unplugged) | — |

Several `out` modules sum into the master bus.

Every knob is also a jack by its own name (see "Wiring rule" below), so the table lists only the inputs that are not knobs. Delay and reverb come from `kazoo-fx`: every effect in its catalogue is a module kind named by its id (`tape`, `bbd`, `plate`, `hall`, ...), with inputs `left`/`right` (right normalled to left), outputs `left`/`right`, and a knob per parameter.

Module ids are `kind` + a number (`vco1`, `lfo3`), assigned by the daemon and never reused within a patch's lifetime. An optional display `name` is limited to `[A-Za-z0-9 _.-]{1,24}`.

**Caps:**
- 96 modules.
- 320 cables.
- One cable per input port: patching into a used input replaces the old cable, and that shows in the log.
- An output may fan out to any number of inputs.
- Each cable has an `amount` from -1 to 1 (an attenuverter, default 1).

## Engine (realtime rules from CLAUDE.md apply)

The audio callback owns a fixed slot table of `MAX_MODULES` boxed modules (`Option<Box<dyn Module>>`). It also owns a fixed cable table and pre-sized port buffers (`MAX_MODULES × 4 outputs × SUB_BLOCK`). It renders in sub-blocks of 32 frames, visiting the modules in a processing order.

The control side is the only place that allocates. It builds modules and computes the order: a topological order, where each cycle is broken with a one-sub-block delay on its back-edge, so feedback patches are legal. It sends commands over a lock-free SPSC ring:

- `Insert { slot, module: Box<dyn Module> }`
- `Remove { slot }`: the module is returned over a retire ring and freed off-thread.
- `Cables(Box<CableTable>)` and `Order(Box<Order>)`: the tables are swapped, and the old boxes are returned over the retire ring.
- `Knob { slot, knob, target, glide_frames }`

A knob moves toward its target linearly over the glide ("weather, not a crash"). The default glide is 2 beats; a glide can be 0 to 64 beats.

**Latency compensation:** an effect that oversamples, or holds a dry path back (a through-zero flanger), delays its sound. Where its output meets a path that went round it, the two would comb-filter, so the wall keeps every path in step:
- The control side asks each effect module its latency (`Effect::latency()`), from a prepared probe of its kind with the module's knobs applied. It asks again when the engine's rate changes, and when an effect's knob has settled: 100 ms after its glide ends, or for a stepped knob, after it lands on its new step.
- Walking the processing order, a module's *arrival* is the latest of its sources' arrival plus latency. Each cable into it is delayed by the difference, and the `out` modules are brought level so all of them meet the master together. Cables closing a feedback loop are left alone.
- Every cable has a delay line of its own (2 048 frames, allocated with the engine), written on every pass even at delay 0, so its history is always there. A delay is at most 1 024 frames (21 ms at 48 kHz); a cable that needs more gets 1 024 and is listed as *uncompensated*.
- When a cable's delay changes, audio and CV are read from a point that glides from the old delay to the new one like a tape head, over at least 5 ms and 50 frames per frame moved (at most 2% fast or slow, about a third of a semitone), read between samples on a cubic curve, so the level holds at every pitch. A gate switches only between pulses: once the old reading point is low it is held low until the new one is too, so every pulse is whole.
- An effect whose latency follows a knob with a cable in it is listed as *unsteady*: the cable moves its latency faster than any re-timing, so it is kept in step only for the knob as set.
- `look` shows all of it under `timing`: each module's `latency_frames`, `latency_ms` and `arrival_frames`, each delayed cable's frames, and the `uncompensated` cables and `unsteady` modules.

**Master chain** (transparent by design; colour belongs in modules people patch in): sum of the outs → an f64 10 Hz high-pass (rate-aware; it removes DC too) → a lookahead true-peak limiter at −1 dBTP (4× oversampled detection, stereo-linked) → a NaN guard that never engages in normal use. There is no saturation stage; the audio audit of 26 Sep 2026 replaced the old DC blocker, the 20 Hz biquad, soft saturation and the −3 dBFS limiter. Any NaN/Inf from a module zeroes that module's outputs and resets its state. That event is counted and reported in the log as a fault, with no panic.

**Clock:**
- The wall is always running and keeps its own beat.
- When the kazoo-mix desk is up, the wall joins it as an instrument named `kazoo-wall`, using `hub_link`. It follows the desk's tempo and beat with `TransportFollower`, which is the same code as the jam engine.
- When the desk stops, or disappears, the wall keeps playing at the last tempo, with its beat continuing from where it was.
- The `tempo` op asks the desk for a tempo when linked (`request_tempo`), and sets the wall's own tempo otherwise.

**Output:**
- A cpal output stream is always open. On a stream error it is rebuilt after 2 s, forever, and each attempt is logged.
- While the wall is plugged into the desk, the rendered stereo goes to the desk through `HubLinkAudio::send_audio` and the device gets silence. Otherwise the device plays it.
- The wall never plays twice.

**Listen:** the callback copies the master output into an SPSC ring. An analysis thread computes a description every 250 ms, using `kazoo_core::analysis` where it fits:
- RMS and peak dBFS;
- spectral centroid, with the balance of low (<250 Hz), mid and high (>4 kHz) energy;
- onset rate per second;
- the dominant pitch, if there is one.

It also renders that description as a short plain-words line, for example "dark, sparse, slow pulse around A2, quiet". This is how seats *hear* the result.

## Persistence

The state lives in `~/.kazoo/wall/`, a directory with mode 0700:

- **`patch.json`:** the full patch, every module with its knobs and every cable. Writes go to a temp file, are fsynced, then renamed, debounced to at most once a second. They are also written on exit.
- **`log.jsonl`:** append-only, one change per line. At 10,000 lines it rotates to `log.1.jsonl`.

On start the daemon loads `patch.json`. When the file is missing it builds the seed patch. When the file is unreadable it moves it to `patch.broken-<unix>.json`, logs why, and seeds.

**The seed patch** is a starting point only, not a destination:
- two VCOs through a `quant`ised `seq`;
- a slow LFO on the filter;
- an envelope on a VCA;
- delay and reverb into `out`.

## Control protocol

The daemon listens on a Unix socket, `kazoo-wall.sock`. It lives in kazoo's runtime dir, the same dir and ownership rules as the desk socket in `kazoo_core::ipc::discovery`. Stale-socket handling works the same way.

- Messages are newline-delimited JSON, with a maximum line length of 64 KiB. A longer line closes the connection with an error.
- Every client must send `hello` first.
- Each request carries a client-chosen `id: u64`.
- Every request gets exactly one response:

```jsonc
// requests
{"id":1,"op":"hello","seat":"Waffles","client":"kazoo-mcp 0.1.0"}   // seat: [A-Za-z0-9 _.-]{1,24}
{"id":2,"op":"look"}                     // full snapshot
{"id":3,"op":"catalogue"}                // every kind with knobs (range, unit, default) and ports
{"id":4,"op":"turn","module":"vcf1","knob":"cutoff","value":800.0,"glide_beats":4.0}
{"id":5,"op":"patch","from":"lfo1.out","to":"vcf1.cutoff","amount":0.4}
{"id":6,"op":"unpatch","cable":12}       // or {"to":"vcf1.cutoff"}
{"id":7,"op":"add","kind":"lfo","name":"slow wobble"}
{"id":8,"op":"remove","module":"lfo3"}   // its cables go too, logged
{"id":9,"op":"undo","change":57}         // applies the inverse of change 57 as a new change
{"id":10,"op":"log","before":null,"limit":50}
{"id":11,"op":"listen"}
{"id":12,"op":"tempo","bpm":96.0}
{"id":15,"op":"speak","module":"speak1","text":"hello from the wall","voice":"Samantha"}  // voice optional; answered once rendered
{"id":13,"op":"subscribe"}               // then events are pushed on this connection
{"id":14,"op":"shutdown"}                // console role only (see below)

// responses
{"id":4,"ok":true,"result":{...}}
{"id":4,"ok":false,"error":{"code":"unknown_knob","message":"vcf1 has no knob 'cutof'; knobs: cutoff, resonance, mode, drive"}}

// events, only after subscribe
{"event":"change","change":{ "seq":58, "at":"2026-09-26T12:00:01Z", "seat":"Tom", "what":{...op...}, "summary":"Tom turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats" }}
{"event":"seat","seat":"Vesper","joined":true}
{"event":"fault","summary":"vco2 produced NaN; reset"}
```

**Error codes:** `bad_request`, `not_hello`, `bad_name`, `unknown_module`, `unknown_knob`, `unknown_port`, `unknown_kind`, `unknown_cable`, `unknown_change`, `full` (a cap was reached), `slow_down`, `not_allowed`, `internal`.

**Look snapshot:**
- `revision`, `tempo`, `beat`, `clock` (`"desk"` or `"own"`), and the `seats` online;
- `modules`: each with `id`, `kind`, `name`, knobs as `{name, value, target, min, max, unit}`, `inputs`, and `outputs`;
- `cables`: each with `id`, `from`, `to`, and `amount`;
- `levels` (`peak_l`, `peak_r` in dBFS), `listen` (the latest description), and `faults`.

**Flood guard:** each seat gets a token bucket of 30 changes per 10 s. Beyond that, requests are answered `slow_down`. It is a guard against runaway loops, not a taste rule.

**Roles:**
- `hello` from the TUI carries `"console":true` and may `shutdown`.
- Seats may not shut the daemon down.
- Every connection is a local user on this machine; the socket's 0700 directory is the access control.

Change summaries are built only from sanitised ids, names, numbers and units. A seat's free text never reaches another seat, except a module's display name, which is charset-limited.

## kazoo-mcp

- Run as `kazoo-mcp --seat <Name>`, or with `KAZOO_SEAT` set. With no seat it exits with an error saying how to set one.
- It connects to the daemon's socket and reconnects every 2 s while the daemon is down; tools answer "the wall is not running (start it with `kazoo-wall`)".
- It never starts the daemon itself.
- It supports MCP protocol versions 2024-11-05, 2025-03-26, 2025-06-18 and 2025-11-25, following dot-seat, because 2026-07-28 cannot carry channel notifications.

**Tools:**

| tool | what |
|---|---|
| `wall_look` | The whole wall as readable text: modules, knob values, cables, tempo, who's here, and a listen line. |
| `wall_catalogue` | The module kinds, their knobs with ranges and units, and their ports. |
| `wall_turn` | Turn a knob, with an optional `glide_beats`. |
| `wall_patch` / `wall_unpatch` | Plug or unplug a cable. |
| `wall_add` / `wall_remove` | Add a module (returning its id) or take one away. |
| `wall_undo` | Undo a change by its number. |
| `wall_log` | The recent changes: who, what, when. |
| `wall_listen` | What the wall sounds like right now, in numbers and words. |
| `wall_tempo` | Set the tempo. |
| `wall_speak` | Give a `speak` module words to say (`module`, `text`, optional `voice`). |

**Channel:**
- It declares `capabilities.experimental["claude/channel"] = {}`.
- It subscribes to events and sends `notifications/claude/channel` with `content` (fixed prose built from the sanitised summaries) and `meta` (string values: `source=kazoo-wall`, `seq`, `seats`).
- Other seats' changes are coalesced: at most one notification per 20 s, or `--notify-every <secs>`, carrying up to 10 summaries plus "and N more".
- The seat's own changes never notify it.
- Seat joins and leaves are included. Faults notify at once.
- Notifications are hints: the content tells Claude to `wall_look` before acting on them.

**Setup, documented in the crate README:**

```
claude mcp add kazoo -- /path/to/kazoo-mcp --seat <Name>
claude --dangerously-load-development-channels server:kazoo
```

## TUI (kazoo-wall with no argument)

It uses the analogue look of kazoo-mix: cream panel, black knobs, patch-cable colours.

**Layout:**
- **Header:** tempo, beat and bar, the clock source (desk or own), the seats online, a master meter, and the listen line.
- **Main area:** the wall, drawn as a grid of module panels. Each panel shows the id or name, and its knobs as dials with values and units. Glide in progress is shown with an arrow toward the target. Each port has a jack glyph coloured by its cable.
- **Right side:** the cable list.
- **Bottom:** a live log feed showing who did what.

**Keys:**

| Key | Action |
|---|---|
| arrows or `hjkl` | Move between modules and knobs. |
| `+` / `-` | Turn a knob (shift for a fine turn). |
| `[` / `]` | Choose the glide. |
| `a` | Add a module (opens a picker). |
| `x` | Remove the module (asks to confirm). |
| `p` | Patch: pick an output, then an input, then set the amount. |
| `u` | Unplug the selected cable. |
| `z` | Undo the last change. |
| `t` | Set the tempo. |
| `?` | Help. |
| `q` | Quit the TUI; the daemon keeps playing. |
| `Q` | Stop the daemon (asks to confirm). |

It is a console client on the same protocol as seat `Tom`, and it reconnects when the daemon restarts.

## Tests (beyond unit tests on every module and knob)

- **Module DSP:** NaN/Inf inputs give silence, outputs stay in range, knob clamping holds, and the frequency is accurate for the VCO and LFO.
- **Engine:** random add/remove/patch/turn sequences under `assert_no_alloc` in the render path; feedback cycles render; removal returns the module over the retire ring.
- **Protocol:** serde round trips; every error code is reachable; oversized lines are handled; the flood guard works.
- **Daemon integration over a real socket:** two seats; events reach the other seat, not the actor; undo works; persistence survives a restart; a corrupt patch file is set aside.
- **kazoo-mcp,** driven by rmcp's client over the child-process transport against a real daemon:
  - the tool list;
  - each tool;
  - the channel notification's shape;
  - coalescing;
  - own changes don't notify;
  - a clear error when the daemon is down, and reconnection when it comes back.
- **TUI:** render tests on ratatui's `TestBackend`, and key handling that produces protocol requests.

## Additions (Tom, 26 Sep 2026, midday)

Tom asked for four things:
- serious effects, written from scratch ("think Klon, not Zoom");
- a lot more percussion for rhythm;
- a sampler for recording your own sounds ("beyond essential");
- a vocoder that works with text-to-speech.

He also wants good control surfaces and a clean way to wire everything to everything. Each addition is its own crate with no dependency on the wall. The wall adapts each one into modules.

### Wiring rule: every knob is a jack

Every knob on every module is also a CV input port with the same name: `vcf1.cutoff`, `plate1.decay`, `kick1.tune`. A cable into a knob adds `cv × (max − min) / 2 × amount` to the knob's gliding value. The sum is clamped to the knob's range, so modulation can never cross a safety cap.

Audio and trigger inputs are listed separately. A module's knob names and input names share one namespace. The catalogue marks each input as `audio`, `gate` or `cv`, and each knob as a knob.

### Effects: `kazoo-fx`

- **Interface:** the `Effect` trait, `EffectKind` and `ParamSpec` (see `kazoo-fx/src/lib.rs`). Every effect is stereo in and stereo out.
- **Wall adapter:** each effect kind becomes a wall module kind whose id is the effect's id. It gets inputs `left` and `right` (`right` is mono-normalled to `left`), outputs `left` and `right`, and a knob for every param.
- **Families:**
  - `drive`: circuit-modelled overdrives and fuzzes, a wavefolder, a bitcrusher, a ring modulator, a compressor.
  - `time`: tape, bucket-brigade (BBD) and digital delays, chorus, flanger, phaser, tremolo, a frequency shifter, a granular delay.
  - `space`: hall, plate, spring and shimmer reverbs, a resonator bank, an EQ.
  - `lofi`: worn media: VHS, cassette and reel tape machines, vinyl, generation loss.
- **The bar:** boutique, component-level modelling where a real circuit exists, anti-aliased (oversampling or ADAA), with character first. The wall's own `delay` and `reverb` rows in the catalogue above are replaced by these.

### Wall modules from the other crates (built)

- **Every `kazoo-perc` voice** is a module named by its id (`kick`, `snare`, `modal`, …): inputs `trigger` (a rising gate strikes it on that exact sample; how far the gate rises past its 0.5 threshold is the velocity: 1 or above is full, 0.75 half, just over 0.5 barely a touch), `accent` (high as it is struck) and `choke` (a rising gate damps it), a mono `out`, and a knob per parameter.
- **Every rhythm generator** (`euclid`, `prob`, `grids`, `poly`, `burst`) is a module with `clock` and `reset` gate inputs, its lanes as outputs (gates, or 0–1 CV), and a knob per parameter.
- **`vocoder`:** inputs `carrier_left`, `carrier_right` (normalled to the left) and `modulator`, outputs `left` and `right`, a knob per parameter.
- **`speak`:** a `gate` input, a mono `out`, and the player's knobs (`mode` once/loop/gate, `rate`, `timing`, `start`, `level`).
- Catalogue families: `synth`, `fx`, `perc`, `rhythm`, `speech`.

### Percussion: `kazoo-perc`

- **Voices:** built from scratch, analogue-modelled and physically modelled. Each is mono out with `trigger` and `accent` inputs, plus params.
- **Rhythm generators:** Euclidean, probability/ratchet, and a topographic drum map. They become wall modules with a `clock` input and gate outputs.

### Sampler: `kazoo-sampler`

- **Sample store:** WAV files in `~/.kazoo/wall/samples/`.
- **Recorder:** lock-free from the audio thread to a disk writer. It can record the mic (the daemon opens a cpal input), the wall's master, or any wall signal through a `rec` module.
- **Playback voice:** one-shot, loop, gate, slice and reverse modes, V/oct pitch, start/end/loop points with crossfades, a granular mode, and good interpolation.
- **Protocol ops:** `record` (start and stop, with source and name), `samples` (the list), and `load` (a sample into a sampler module).

### Speech: `kazoo-speech`

- **Vocoder:** carrier and modulator inputs; 8–40 bands; formant shift; unvoiced/sibilance detection that injects noise; attack and release per band; emphasis.
- **Text-to-speech:** a renderer running off the audio thread, using macOS `say`, rendered straight to f32 at the wall's rate. It supports a choice of voices and caches its renders.
- **`speak` module:** plays the rendered phrase on a gate, once or looped. It feeds a vocoder's modulator or goes straight out.
- **Protocol op:** `speak` (text, voice, into module), so any seat can make the wall sing words.
  - `{"op":"speak","module":"speak1","text":"…","voice":"Samantha"}` checks the module, the text (at most 500 characters) and the voice at once, renders through `TtsWorker` off the audio thread, and hands the phrase to the module through its `PhraseFeed`; the old phrase comes back the same way and is freed off the audio thread.
  - It is answered once the words are ready (a `ChangeResult`; `say` can take a few seconds), or with `bad_request` (not a speak module, bad text or voice), `slow_down` (two renders are already ahead of it, so the answer could not come within kazoo-mcp's 75 s), `not_allowed` (newer words for the same speaker were asked for before these were ready: the latest asked always wins) or `internal` (no `say` on this machine, or the speech thread stopped).
  - Rates `say` cannot render (above 192 kHz, below 8 kHz) are rendered at the nearest it can; the player resamples, as it does a phrase kept through a change of engine rate.
  - It is a change: flood-guarded, refused to watchers, fingerprinted (a whole touch on the speaker), logged as `{"op":"speak","module":"speak1","words":4,"seconds":1.2}`. The text itself never reaches another seat: it is kept privately in `patch.json` (`"speech"`) and rendered again when the daemon starts (saved words waiting for room in the render queue are put back as it frees, and never replace words asked for since). It cannot be undone. A removed speaker's words are kept as privately (`"retired_speech"`, the latest 256), so undoing the removal brings it back saying them.

### Control surfaces

- **The TUI:** the primary surface, see above.
- **MIDI controllers:** the daemon opens the MIDI inputs (midir is already in the workspace). Knob CCs and notes can be *learned* onto any wall knob or gate: in the TUI, press `L` on a knob and turn the hardware control.
- **Mappings:** stored in `~/.kazoo/wall/midi.json`. Each move is a change in the log under the seat `Tom (midi)`, coalesced to one entry per control per second.

### Fingerprints (Tom, 26 Sep 2026, 14:00–14:03)

Tom: "Every single time somebody touched it, they left a fingerprint on everything in your touch… like adding dye to water, with the electricity being the signal flowing through it… you can see who's responsible for which part." And: "It doesn't give you the answers, and it shouldn't try to. It should make you want to ask questions."

**What it is:** pure causality, shown. It never judges, never labels and never changes the sound.

**Dye**
- A fingerprint is the seat's identity (its name). No colours are assigned in advance (Tom, 14:11). A view decides how to show it.
- A touch deposits the seat's dye into that module: a turn, a patch into the module, or an add. The amount follows the size of the change relative to the knob's range.
- The dye flows downstream along the cables, to everything the touched module feeds. The share each cable carries follows the cable's |amount|.
- Where hands meet, their shares mix.

**The fingerprints change only when something changes.** A touch changes it, and so does a cable plugged or unplugged, because that re-routes where the dye flows. Nothing fades with time.

**What is excluded:** no feelings, no "tension" or "turbulence", no good/bad, no words about what it means. The view shows who touched what, and where the signal carried that touch.

**Where it shows**
- The snapshot carries `fingerprints`: each module's and each cable's share per seat, 0 to 1.
- The TUI shows the hands on each module and cable. How it shows them is a view choice, not a pre-assigned seat colour.
- `wall_look` lists the hands on each module, e.g. `vcf1 (Tom 62%, Waffles 30%, Cassio 8%)`.
- Changes of this kind are computed on the control side when a change lands, never in the audio thread.

### The world plays the wall (Tom, 26 Sep 2026, 14:10)

Tom: "Their audio data coming out into our visualiser — I was more thinking the audio would go the other way around… we've got all of these gorgeous scenes… being able to explore that with music that was generated from it would be extraordinary."

**The idea:** Waffles' meridian world (a Bevy app: every message is a light in a helix, with bloom and a real lens) becomes a place you explore by ear. Whoever moves the lens is the player. The data is the landscape; the wall's patch decides how that landscape sounds.

**Keeping to "nothing automated, nothing decided in advance":**
- The world never turns knobs itself.
- It appears on the wall as a `world` module, a source like an LFO. Its outputs are jacks:
  - lens position `x`, `y`, `z`;
  - `speed`;
  - `density` (lights near the lens);
  - `brightness`;
  - `hue`;
  - `nearest` (the distance to the closest light);
  - a `pass` gate that fires as the lens passes a light;
  - a `pitch` CV derived from the passed light's position on the helix.
- People patch those jacks into anything, so how the world sounds is itself a patch that seats and Tom shape and change. The fingerprints show who wired it.

**Protocol:**
- A new `feed` op streams the world's values at control rate, 30–120 Hz. Feed messages are not changes and are not logged.
- The daemon smooths the values into the module's CV outputs sample-accurately, with a per-sample ramp so nothing steps.
- If the feed stops, the outputs hold their last values.
- `add world` creates the module. A module is bound to one named feed source, e.g. `meridian`.

### Acceptors: data sources become signal (Tom, 26 Sep 2026, 14:13)

Tom: "one of the principles of the visualisation… everything will be attributable. You have all these attributes and properties in your data source that you can link them to: x and y value, size, distance… I'd love the different attributes driving out like a module on the wall… modules that are just acceptors, that convert data sources into signal. GitHub, git changes, file changes."

**What an acceptor is:** a source module family. Each acceptor watches one data source and turns its records into signal. It never touches other modules; people patch it.

**Outputs:**
- An `event` gate fires for each record, sample-accurately at the time it lands.
- Four CV outputs, `a`, `b`, `c` and `d`, carry attributes of the latest record, held and ramped per sample.

**Attributable:** which attribute drives which output is itself a set of knobs: `attr_a`…`attr_d`. These are stepped knobs whose labels are that source's attribute names. Binding is therefore turned, logged, undoable and fingerprinted like any other knob. Nothing about the mapping is decided in advance.

**Scaling:** each attribute has a declared scale: a count, bytes, time of day, a hash, or 0/1. It is mapped onto −1..1 with a per-output `scale` knob and a `curve` (linear/log). Categorical attributes use a stable hash of the value to a point in −1..1, with no meaning attached.

**Sources:**

| Source | Configured by | Records | Attributes |
|---|---|---|---|
| `git` | a local repository path | each new commit on any branch; working-tree edits as a separate record kind | lines added, lines removed, files touched, message length, author (hashed), hour, weekday, depth of paths touched, record kind |
| `files` | a directory path (recursive, polling with a bounded file count) | create, modify, delete | size change, size, extension (hashed), path depth, kind |
| `github` | `owner/repo`, polled read-only via the `gh` CLI with Tom's existing login at ≥ 30 s intervals | the repository's public events (pushes, PRs, issues, reviews, comments) | event type (hashed), actor (hashed), additions, deletions, commits in the push, comment length |
| `world` | a feed name | Waffles' meridian world, through the `feed` op | lens x, y, z, speed, density, brightness, hue, nearest, pitch; a pass as the event |

**Security and limits:**
- An acceptor is created with a `config` on `add`, e.g. `{"kind":"git","config":{"path":"~/Developer/projects/…"}}`.
- Paths must be under the user's home and must exist. Symlinks are resolved and checked.
- Nothing is written.
- Commit messages and file contents never leave the daemon. Only the numeric attributes do, so no text from a data source can reach a seat.
- Polling intervals and record rates are bounded.
- Each source runs on its own thread, feeding the engine through the same smoothed path as `feed`.

**One general acceptor (Tom, 14:17):** "just make it a general one… here's this module, plugged into this, poll this every whatever, or receive".
- Acceptors are **one** module kind, `acceptor`, configured on `add`.
- **Modes:**
  - `receive`: values are pushed through the `feed` op, e.g. Waffles' world.
  - `poll`: a source is read every N seconds (N ≥ 1).
- **Poll sources:**
  - a JSON file;
  - a local git repository;
  - a directory (file changes);
  - a GitHub repository (via `gh`, read-only);
  - an http(s) GET returning JSON;
  - a local command's JSON stdout.
- **Fields:** the record's numeric fields (JSON paths) become the attribute list that `attr_a`..`attr_d` choose from. A new record, or a changed value, fires `event`.
- **Safety (Tom, 14:19):** any seat may configure any source, because the seats are AIs that already have a shell on Tom's machine. A console-only rule would only obstruct Waffles. The guardrails that matter:
  - HTTP is GET only.
  - Commands run with argv and no shell, under a timeout, with bounded output and no inherited secrets beyond the user's environment.
  - Paths are resolved.
  - Everything is read-only.
  - A source's full configuration is recorded in the change log under the seat that set it up.

### The station (Tom, 26 Sep 2026, 14:21)

Tom: "a radio station… a little Cambium bot, so anybody can listen to it, and whenever you submit a thing, a little song… it starts playing: 'this is Waffles playing his Great Despair. Shout out to Waffles!'"

**Pieces**
- A piece is a patch snapshot with a title and an author seat, saved in `~/.kazoo/wall/pieces/`.
- Protocol ops: `submit` (the current wall or a supplied patch, plus title), `pieces`, `queue`, `skip`.

**Station**
- When the station mode is on, the wall plays the queue. Each piece runs for its chosen length (or until skipped).
- The wall crossfades into the next piece, with both patches briefly alive. The module cap is budgeted for that.
- Each piece is announced by the DJ voice: a `speak` module from kazoo-speech, optionally through `radio` or other effects.
- **Announcement:** "This is Waffles, playing Great Despair." The words come only from the author's seat name and the charset-limited title.
- The Cambium bot is the front door: it takes submissions and calls `submit`/`queue`.

**Listening:** the daemon streams its master output over HTTP.
- FLAC is lossless, for Tom's rig.
- Ogg/Opus is for everyone else.
- The stream is a tap after the master chain. It never affects the device output.

**Dedications and notifications (Tom, 14:23):** "you get to put in a message, and it goes out to everybody… whenever it changes, everybody gets a notification… a good way of building culture."

- **Messages:** `submit` carries an optional message of at most 280 characters. Control characters are stripped and it is otherwise free text.
- **Delivery:** when the station changes piece, every seat running kazoo-mcp gets a channel notification: "Now playing: <title>, from <seat>", then the message.
- **Framing:** the message is quoted and marked as the sender's own words, not an instruction ("Waffles wrote: …"). It is the only free text allowed through, so it is framed so that no seat's AI treats a dedication as a command.
- **Reach:** Tom intends every seat to have kazoo-mcp, so everyone hears it.
