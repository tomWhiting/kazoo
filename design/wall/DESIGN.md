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

Every knob is also a jack by its own name (see "Wiring rule" below), so the table lists only the inputs that are not knobs. Delay and reverb come from `kazoo-fx`: every effect in its catalogue is a module kind named by its id (`tape`, `bbd`, `plate`, `hall`, `pitch`, ...), with inputs `left`/`right` (right normalled to left), outputs `left`/`right` (the mix) and `wet_left`/`wet_right` (the effect alone), and a knob per parameter.

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
- `Tempo(bpm)`
- `Hush` and `Unhush`: they bracket a swap of the whole wall (a save loaded; see Persistence). `Hush` fades the master out over 60 ms, a linear ramp per frame in the callback, applied after the limiter so the meters, the listening and a recording hear what was played. Every command after a `Hush` is held in the ring, not taken in, until the master is silent; then the swap (every `Remove`, every `Insert`, the tables, the tempo) is taken in at once, in silence, and `Unhush` fades back in over 60 ms. Nothing allocates: the gain and the flag are fields of the engine, and holding is simply not popping the ring. The engine counts the `Unhush`es it has taken in (`Shared::unhushed`), so the control side knows when a swap has landed: until then, `look` shows each knob's target rather than whatever the old wall left in that slot.

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

On start the daemon claims its socket first (a daemon already serving it is found before any state is touched), then opens the state, taking an exclusive lock on `<state>/lock` (`flock`, through rustix) held for as long as it runs: a second wall on the same state directory, whatever socket it serves, is refused with "another wall is using <dir>" (a starting wall waits up to five seconds for one that is stopping to let go: it releases the lock once it has saved). On a local disk the lock holds between processes and between two opens in one process; on a network disk `flock` may not lock at all, or may lock only between machines, and a disk that refuses to lock is refused with "the disk … cannot lock files; use a local disk". `kazoo-wall stop` returns only once the wall's socket is gone, which the wall removes after it has let go of its state, so a wall started right after finds it free. Then it removes the temporary files a crash left (`patch.json.tmp`, `saves/*.tmp`: nothing names them), saying what it removed, and loads `patch.json`. When the file is missing it builds the seed patch. When the file cannot be used (not JSON, not a wall, not readable) it moves it to `patch.broken-<unix>.json`, says so in the log as the wall's own change (`What::SetAside { reason, moved_to }`, seen in the console's log pane and by every seat), lists the file among the broken saves, and seeds. `patch.json` is never set aside for its size: the wall wrote it, and reads it whole. An engine started afresh (the audio rebuilt, even in the middle of a load's swap) is given the patch's own tempo, unless the desk keeps the tempo.

**Full flushes.** On macOS std's `sync_all` and `sync_data` are each a full flush (`F_FULLFSYNC`: the disk commits everything it holds), costly and the only true barrier; a plain `fsync` (through `rustix`, already in the tree through crossterm, no unsafe code) passes data to the disk without that, and is made durable by the next full flush. (On Linux both are one fsync.) The rule is one full flush per batch. Every batch, and its count on macOS:

| Batch | Full flushes | Why, where more than one |
|---|---|---|
| A change (a turn, a cable…) | 0 | Its line is written, not flushed: the periodic save flushes it, before `patch.json` can hold it. |
| The periodic save of `patch.json` (at most once a second, and at exit) | **2** | The new file is written and passed to the disk, then the log flushed (1: every line `patch.json` holds, and the file's bytes, on the disk), then the rename, then the state directory flushed (2). The rename must wait for (1), or `patch.json` could be on the disk ahead of the log; and (2) is what makes the rename durable at all. |
| A named save | **2** | The file flushed (1) before the rename that names it, then the directory (2): the bytes must be on the disk before the name, or a crash could leave a save of that name holding nothing, and saving over a save would lose the old one. |
| Deleting a save | 1 | The directory. |
| A load, with a backup | **3** | (1) the backup (the wall as it was) on the disk, bytes and name, before the log line naming it can be, or a crash could leave a load that can never be undone; (2) the log line on the disk before the answer says the load happened and before the new wall is renamed over `patch.json`, or a crash could leave a wall on disk that no change put up; (3) the rename over `patch.json`, so the wall the answer says is up is the one a restart comes up on (without it a crash after the answer leaves the load out on restart: safe, but the load is lost). The new wall's own bytes ride on (1) or (2). (Before: five.) |
| A load, the wall kept in a save already | **2** | (2) and (3) above. |
| A new wall (`new`) | **3**, or **2** | As a load: the wall as it was kept first (unless a save holds it already), then the line, then `patch.json`. |
| Undoing a load | **3**, or **2** | As a load: the wall up is kept first (unless a save holds it already), then the line, then `patch.json`; the wall brought back is read from its save. |
| Pruning the wall's own saves after a load | 0 | Passed to the disk only: a deletion a crash takes back leaves an old automatic save, which the next pruning lets go again. |
| Importing the hand-made copies | 1 | Every save written and passed to the disk, all renamed in, the saves directory flushed once. Nothing need be on the disk before anything else: the copies are never touched, so a crash part-way loses nothing (what did not land comes in again; a half-written save is listed as broken, its copy still there). The mark is written after and only passed to the disk: if a crash takes it back, the next start finds every save in and writes it again. |
| A rotation of the log (every 10,000 lines) | 1 | The old log passed to the disk, renamed, a new one made, and the state directory flushed once, which puts the old log's lines and both names on the disk together before a line goes into the new log. The lines appended since the last flush are kept, to be written again, until that flush is done; if it fails, the log is not trusted until made whole again, which flushes the directory too. |
| A note the daemon makes (a recording, a change that did not happen, a start note) | 1 | Its line. |
| Making the log whole after a failure | 1 | The lines since the last good flush written again and passed to the disk, then the directory flushed once. |
| Making the saves directory, the first time | 1 | The state directory, so its name is on the disk before anything in it is relied on. |

A rotation (and making the log whole, and making the saves directory) adds its flush to the request that brings it on: a change that rotates the log costs 1 instead of 0, and a save 3 instead of 2. A load never rotates within its own batch: if its line would rotate the log, the rotation is done first, before the batch begins, so the batch keeps its three flushes; the request as a whole then costs four, once every 10,000 changes. That is the honest worst case: four full flushes in one request.

**The exceptions, for Tom to decide.** The periodic save (2), a named save (2), and a load, `new` or undo of a load (3, or 2) break the one-flush rule, each for the reason given above: one write must be on the disk before another can be. macOS has a cheaper barrier, `F_BARRIERFSYNC`, which orders writes without flushing the disk's cache, and would let each of these batches keep its order with one full flush at the end. It is not used: rustix (the only safe way here to reach `fcntl`) exposes `F_FULLFSYNC` but not `F_BARRIERFSYNC`, and the workspace forbids unsafe code. So the exceptions stand as listed, each with its reason.

**Sizes.** No wall file, save or `patch.json`, is ever larger than 2 MB (half the longest line the daemon sends, so a wall that fits its file fits a `look`). Every part of a wall is bounded, and a compile-time check holds that the sum of the bounds fits (`store::WORST_WALL_FILE`, about 2.0 MB of the 2 MB allowed, each part at its worst, each character escaped, twice over for the indentation), given catalogue facts a test checks (at most 256 kinds; every kind, knob and port name at most 32 bytes):
- modules (96), cables (320), the rows, and the numbering of at most 256 kinds;
- every speaker's words and every retired speaker's (256), each at most `speak`'s limits (500 characters of text, 64 of voice);
- the fingerprints' dye: each module keeps the dye of its 6 strongest seats, the faintest let go as others touch it. Six is more hands than a module shows at a glance ("Tom 62%, Waffles 30%, Cassio 8%, Vesper <1%"); past the sixth a seat's share is a sliver of a sliver, and without a cap the dye grew with every seat that ever touched the wall.

Reading a file (`patch.json`, a save, a copy to import) holds it to the same bounds, dropping what is over with a note (words too long, never quoted; numbering under a name no kind could have, or past 256 kinds, while the numbering of a kind since taken out is kept so its ids never come back; dye from too many seats, or from a name longer than a seat's): no file read in can be put up, or written again, larger than the bound. Even so, every write checks: a save that would be larger is refused with `full`; a load whose wall would be is refused with `full`, and so is a load, `new` or undo of a load while the wall as it is cannot be kept; a copy that would be is left out of the import, saying why; `patch.json` that would be is not written, which is said once to everyone as a fault ("changes are not being kept"), and from then until the wall is small enough again every change that would make it larger (a module, a cable or words added, a removal or unplugging undone) is refused with `full`, while what takes something away goes through. A start whose `patch.json` cannot be written for its size starts all the same, saying so. Removing the module limits (work in progress elsewhere) must keep a byte budget at change time instead, refusing a change that would make the wall too large to save.

**The log before `patch.json`, always.** `patch.json` is never ahead of the log: each save syncs the log first, and writes nothing if that fails. A change is made only once its line is written; if the line cannot be written, the change is not made (the request fails with `internal`, and what of the patch it touched goes back exactly: a knob turn keeps only that module's knob targets to put back, a fraction of a microsecond; a cable or module added or taken away keeps the whole patch, about 64 µs for the 47-module Airports wall in a debug build, at the pace of hands), and since a write that failed may still have left the whole line on disk, the daemon says, as the seat `kazoo-wall`, `What::Abandoned { change }`: that change did not happen, cannot be undone, and neither can the note. What leaves the wall is sent only after its line is written: a tempo goes to the desk once logged (a link that refuses it then gets the note instead; a change asked of the desk records the asking, "asked the desk for 120 BPM", not the desk's answer, and the wall follows and saves whatever the desk then plays; its `from` is the desk's tempo on the desk and the wall's own off it, never the engine's, which hears a change a block later), and a seat's words reach their speaker's player only once logged (a player that cannot take them then gets the note instead, and goes on with what it was saying).

What the daemon logs of its own accord (a recording stopped or carried on, that note) has happened already: if the log cannot take it, it is *owed*. An owed change has no number and is told to nobody until it is written, before any other line; until then every change is refused (`internal`) and `patch.json` is not written. A load pays what is owed before it is numbered, so the wall it puts up is named by its own change. `revision` is the latest change made, never a number given to one that was not. `look` says so (`log: { owed, error }`, left out while all is well), the console's header shows `LOG STOPPED · n owed` (or `LOG NOT SYNCED` when nothing is owed but the last sync failed: changes still go, and only `patch.json` waits), and each says which it is in words, kazoo-mcp's `wall_look` says it in a sentence, and every subscriber is sent a `log` event when it starts, moves and clears. At shutdown the daemon tries once more; what the log still cannot take is lost and listed in the daemon's log. As it was never numbered or told, and `patch.json` stayed where the log is, no number told to anyone is ever given again.

After a write or sync of the log fails, its file handle is never used again, and before the next line the log is opened afresh and every line appended since the last good sync is written again and synced: on Linux a failed fsync may already have dropped those pages while the file still reads as if they were there. A line written twice is read once (the first), and the log reads in the order of its numbers. A log found on opening to end part-way through a line (the daemon stopped mid-write) is marked torn, so the next line starts on a line of its own. The failures are made in tests through `Store::fail_once`, which exists only in test builds (outside them no step can be made to fail).

**Starting again on another wall** (`daemon::wall::resume`). The log may hold changes `patch.json` does not: the daemon stopped within a second of a change, a load stopped between its log line and `patch.json`, or Tom put back an older copy of `patch.json`, or the patch was seeded afresh. The daemon decides by the wall `patch.json` names (`patch.wall`), not by numbers alone. When `patch.json` is behind the log, when the log has loads after that wall, when that wall is not one the log knows (no load or start note of that number, and not 0), or when the wall was seeded, it says so in the log, once, before anything plays, as `What::Resumed { from, through, left_out }`, synced, and saves `patch.json` after it; if that cannot be written, the daemon does not start, and says why. This is the only place a start writes `patch.json`: a seeded wall, or one brought up to date (whose `Migrate` note is logged first), is written after its notes, so a start stopped before then leaves the file as it found it and the next start does the same again. A wall numbered before the oldest change in the logs read (its load gone with logs rotated away, on a wall that lives long) is taken as known; and the walls of the changes kept in memory are worked out from the wall in force where memory starts, not from 0. Where the log read opens is wall 0 if it starts at change 1; else the wall patch.json names if no change in it puts up another; else the `from` of a first `Resumed` naming a wall before the log read; else a wall no number holds. A torn line stays in the log it was in when the log rotates. The wall up from there is a wall of its own, numbered by the note. When `from` is the wall `patch.json` was, the new wall is that wall as it stood after change `through`, so changes made to it until then still undo and none after; with no `from` (seeded, or a wall the log does not know) no earlier change undoes. The loads in `left_out` (those after `from`, or all of them when there is none; never one said not to have happened, nor one an earlier note left out) put up walls that did not come back: they are not said never to have happened, but they cannot be undone (`not_allowed`, saying so); their backups are saves like any other, to load by name.

### Named saves (Tom, 9 Oct 2026: "I'd love to try out some stuff, but I don't wanna lose what's there")

- **`saves/<name>.json`** (directory mode 0700): one wall per file, in `patch.json`'s format, written the same durable way (a `.tmp` beside it, fsync, rename, fsync the directory). A write that fails (a full disk) removes its temporary file and leaves any old save whole. A save put in place (or deleted) whose directory then cannot be flushed has still happened: the wall takes it as that save (or forgets it), everyone is told, and the answer's `notes` say a crash could yet take it back; such a save is not trusted to keep the wall for a later load, which saves it afresh. Whether the wall is still what a save holds (for `changed`, and for keeping it rather than saving it again before a load) leaves the tempo out while the desk keeps it, as the wall's saved tempo then follows the desk's. A save holds every module with its knobs at their *targets* (a knob mid-glide is saved where it is going), every cable under its number, the id and cable numbering, the tempo, the rack's rows, the fingerprints' dye and the speakers' words. When it was saved is the file's modification time.
- **Names:** 1 to 64 of `A-Z a-z 0-9 space _ . -`, starting with a letter or digit and not ending in a space or `.` (`protocol::valid_save_name`), so a save is always a plain, visible file in that directory: no separators, no hidden files. Names are compared ignoring case, as macOS's disks do: `Jam` and `jam` are one save, and replacing it keeps its name as stored. Names starting with `before-` are the wall's own (below) and no seat may save under one, so an automatic save is never replaced.
- **Reading** goes through the same migration as `patch.json`: a save from an older wall loads brought up to date (its file is left as it was), and parts that cannot load are left out with a note. A file that is not a wall at all is *broken*: it is listed as such and refused, never moved or rewritten. Files among the saves whose names are not save names are listed as broken too, so nothing there goes unseen. Every file is read with serde_json's `float_roundtrip`, so a save reads back to the last bit (the fingerprints' dye amounts are arbitrary floats, which the default parser can get one bit wrong). The feature unifies across any build that includes kazoo-wall; the only other crate using serde_json directly, kazoo-cs80 (its presets), passes its tests with it on. A migration's notes about a speaker's words that cannot be read say only that, never serde's error, which would quote the private words.
- **Listing** (`saves`) reads each file whole through the migration, exactly as a load would, so the tempo and counts are those of the wall a load puts up (parts it cannot load left out) and a file a load would refuse is listed as broken; it keeps what it found by the file's modification time, length, inode and change time (to the nanosecond), reading it again only when any moves. No save or copy larger than 2 MB is ever read (a full wall is about 100 KB; see **Sizes**): it is listed as broken, "too large to be a wall", and so is refused by a load and left out of the import. One listing reads at most 8 files not read since they last changed (newest first); the rest are listed as not read yet (`pending`), and the wall reads them one a tick from the start, in one pass over the files there then (each looked at once; the pass ends whatever changes meanwhile, and ends at once, said once, if the directory cannot be read), so the list is whole within moments. This keeps any one request's work on the control thread bounded (8 × 2 MB at the very worst, a few hundred KB in practice) whatever the number of saves, at the cost of a list that can be incomplete for its first moments, which the console and kazoo-mcp say and ask again for. Each seat has a listing budget of its own (10 at once, one a second after), apart from its changes; the console asks again a moment after a refusal.
- **Saving** by a seat that is not a console is held to an allowance of its own (3 at once, then 3 a minute) and to at most 200 saves made by hand (about 20 MB at most), beyond which it is refused with `full` until Tom's console deletes some. Consoles are held to neither. A save refused for the count, or for its name being taken, costs none of the allowance.
- **Replacing the wall** (`load`, `new`, `undo` of a load) by a seat that is not a console is held to an allowance of its own too (3 at once, then 3 a minute), so a seat loading in a loop cannot push the console's way back out of the wall's own saves. Consoles are not held to it. A load refused for what was asked (no such save, a name that is not one, a save changed since) costs none of it; one the disk refused does.
- **Loading** (`load`, `new`, and `undo` of a load) replaces the wall while it plays:
  1. The wall as it is is kept first. When it has not changed since it was last saved or loaded (the daemon remembers which save holds it, the hash of that save's file as written, and the wall's in-memory *fingerprint* then), and the save's file still hashes to what it did (so a file replaced from outside is never trusted), that save keeps it and nothing is written. Otherwise it is saved as `before-load-<YYYYMMDD-HHMMSS>` (`before-new-…`, `before-undo-…`; local time, `-2`, `-3`… within one second). If that save cannot be written, nothing is replaced and the request fails (`internal`): nothing is ever lost.
  2. The patch is swapped, and the engine with it: `Hush`, a `Remove` for every slot, an `Insert` for every module of the new wall (knobs at their targets, nothing gliding), fresh tables with every cable on a fresh delay line, the wall's own tempo (unless the desk is linked: then the desk keeps the tempo, the change records the desk's as what plays, and the answer says so), `Unhush`. Speakers' saved words are put back; words still rendering for the old wall go nowhere, and the seat that asked is told the wall was replaced. Faults not yet told about the old wall's modules are dropped.
  3. What carries on through it: the engine itself (no rebuild, unlike `load_engine` after an audio failure), the clock and song position, the desk link and its transport, the monitor, and a recording under way (one file, which hears the fade). The flood guard counts a load as one change.
  4. It is logged as **one change**, `What::Load { save, backup, fingerprint, modules, cables, tempo, by_console }`: the save that keeps the wall it replaced, and `fingerprint`, a 64-bit FNV-1a hash (with the length) of that save's file **as it is on disk**, its raw bytes before any migration. A later build that writes the same wall differently (a new format version, a changed migration) still recognises the file. The wall itself is stored once, in the save, never in the log: the log line stays a few hundred bytes, and the speakers' words never enter it.
  5. It is **one batch** on disk (`Store::commit_load`): the backup (when one is written) and the new `patch.json` go to temporary files, both passed to the disk; the backup is renamed into place and the saves directory flushed in full (so the log never names a save a crash could take back); the change is appended to the log and flushed in full, and from that moment the load has happened; then the new wall is renamed over `patch.json` (after the log line, so the new wall is never on disk without the change that put it up); then the state directory is flushed in full (see **Full flushes** above). Only after the log line is durable is anything changed in memory (patch, engine, log). A failure before it replaces nothing and says why (`internal`); a failure while writing or syncing the line replaces nothing, and since the line may be on disk, the daemon says at once that the load did not happen (`What::Abandoned`, above); a failure after it (renaming `patch.json`, syncing the directory) is said in the answer's notes and the wall is written again at the next save. "z undoes it" is only said of a load whose line is in the log. A torn log line (a write that failed part-way) never takes the next line with it: the next starts on a line of its own. Each step can be made to fail in tests (`Store::fail_once`), whoever runs them.
  6. Subscribers are told: the change (to the other seats, as every change), then `loaded` to everyone, the loader too.
  7. The wall's own saves are pruned (below).
- **A crash in the batch:** before the log line, the wall comes back as it was, with a spare backup that is pruned in time. Between the log line and `patch.json` (the one window left), the log has the load and `patch.json` the wall before it, whose number is not the load's: on start the daemon leaves the load out with a `What::Resumed` note (above), whose `from` is the wall before it and `through` the last change `patch.json` had. Changes made to that wall until then still undo; the load cannot be undone, and neither can the note.
- **Undoing a load** reads the save named in the change, checks its file's raw bytes still hash to the fingerprint (`not_allowed` if it is gone or has changed since, saying so), and brings that wall back **exactly**: module ids, cable numbers, numbering, dye, words and rows, even after a restart, from the log and the save alone.
- **Which wall:** every load puts up a wall of its own, numbered by its change (`look`'s `wall`, saved in `patch.json`; 0 for a wall never replaced). Ids are unique within one wall, not across saves, so a change made to a wall that has since been replaced would land on the wrong modules:
  - A request naming modules or cables may carry the wall its client last looked at (`RequestLine::wall`); when another wall is up it is refused with `wall_replaced`. The console and kazoo-mcp send it.
  - Undo works out from the log which wall each change was made to. Undoing load L puts up a *copy* of the wall L replaced as it stood just before L, under a number of its own, so the changes made to that wall before L hold on the copy, and nothing else does: undoing the same load twice never lets a change made to the first copy be undone on the second. A note, a recording or words are refused for what they are before anything about walls. Undoing a change made to a wall that is not up is refused (`not_allowed`), naming what replaced it last: the load to undo first, or the note of the wall starting again (loads said not to have happened are passed over). A load itself can be undone while its save holds its wall, unless it did not happen or its wall did not come back up when the wall started again.
- **Hand-made copies:** the first time a wall with saves starts, every `patch.<label>.json` beside `patch.json` (Tom's copies: not `patch.json`, not the wall's own `patch.before-*` and `patch.broken-*`) is *copied* in as the save `<label>`, through the migration (so an old one is saved up to date), keeping the copy's modification time. A copy that is not a wall, whose label cannot be a save's name, or whose name is taken is left out, and the daemon's log says what became of each. A mark, `saves/.imported`, records that it was done, so a save Tom deletes later stays deleted. A copy the disk will not read is named in the daemon's log and tried again on the next start, and every other copy comes in now; the mark is written only once none is left to try. If a save cannot be written, the import fails without the mark and is finished on the next start, skipping what is in. The import is one batch with one full flush (see **Full flushes** above). Copies whose labels differ only in case become one save.
- **Pruning the wall's own saves:** after every load the newest 20 `before-…` saves are kept (an evening of trying saves one after another, every wall before each still there); older ones are kept only while a load in the change log still names one, and never more than 100 in all (a full wall is about 100 KB, so at most about 10 MB), so a seat loading in a loop cannot fill the disk. The room beyond the newest 20 goes first to the saves the console's loads need (each load records `by_console`; the save holding the wall as it is counts as the console's), then to those a seat's loads need, newest first in each. A load whose save has been let go says so when asked to be undone. Saves made by hand are never pruned.

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
{"id":16,"op":"save","name":"tom fiddle 2026-10-09"}   // the wall as it is; "overwrite":true replaces one (console only)
{"id":17,"op":"saves"}                   // every save, newest first, and files that cannot be loaded
{"id":18,"op":"load","name":"tom fiddle 2026-10-09"}   // replaces the wall as it plays; saved first; one change
{"id":19,"op":"new"}                     // an empty wall, as load does
{"id":20,"op":"delete_save","name":"tom fiddle 2026-10-09"}   // console role only
{"id":14,"op":"shutdown"}                // console role only (see below)

// responses
{"id":4,"ok":true,"result":{...}}
{"id":4,"ok":false,"error":{"code":"unknown_knob","message":"vcf1 has no knob 'cutof'; knobs: cutoff, resonance, mode, drive"}}

// events, only after subscribe
{"event":"change","change":{ "seq":58, "at":"2026-09-26T12:00:01Z", "seat":"Tom", "what":{...op...}, "summary":"Tom turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats" }}
{"event":"seat","seat":"Vesper","joined":true}
{"event":"fault","summary":"vco2 produced NaN; reset"}
{"event":"loaded","seq":81,"seat":"Waffles","save":"tom fiddle 2026-10-09","backup":"before-load-20261009-143005"}  // to everyone: look again
{"event":"saved","seat":"Waffles","name":"waffles dub","replaced":false}   // to everyone; not a change
{"event":"save_deleted","seat":"Tom","name":"waffles dub"}                 // to everyone
```

**Saves' results:**
- `save` answers `{save, replaced}`, where `save` is `{name, saved_at, tempo, modules, cables, auto, version}` (`auto`: one of the wall's own `before-…` saves; `version`: the file's format, older ones loading brought up to date).
- `saves` answers `{saves: [save…], broken: [{name, saved_at, problem}…]}`, each newest first.
- `load` and `new` answer `{change, backup, notes}`: the change as logged (undo it to bring the wall back), the save the wall as it was went to, and sentences on what else there is to know (an old save brought up to date, parts left out, the desk keeping its tempo, a recording going on through it). `undo` of a load answers the same shape, which also reads as `undo`'s usual `{change, …}`.
- `delete_save` answers `{name}`, the save as it was named.
- `look` carries `save: {name, revision, changed}` once the wall has been saved or loaded since the daemon started: `changed` is whether the wall's fingerprint (everything a save holds) differs from what went to or came from the save, so the wall's own notes (which move the revision) do not set it and a move on the rack (which does not) does; and `wall`, the number of the wall up.

**Which wall a request is for:** any request line may carry `"wall": n`, the `wall` its client last saw in `look`. A request naming modules or cables (`turn`, `patch`, `unpatch`, `add`, `remove`, `arrange`, `speak`) is refused with `wall_replaced` when another wall is up, so a turn sent just as another seat loads a save never lands on the new wall's module of the same id. Left out, nothing is checked (older clients).

```jsonc
{"id":21,"wall":81,"op":"turn","module":"vcf1","knob":"cutoff","value":800.0}
```

**Who may:** `saves` is a read, open to everyone, watchers too. `save`, `load` and `new` are changes: any seat but a watcher, against the flood guard; a seat that is not a console is also held to its save allowance and the cap on saves (`slow_down`, `full`). Destroying a save is the console's alone, because the saves are Tom's safety net and a deletion is the one thing the wall cannot undo: `delete_save`, and `save` with `overwrite`, are refused to seats (`not_allowed`). A load or `new` can always be undone and the wall it replaced is saved first, so seats may do those.

**Error codes:** `bad_request`, `not_hello`, `bad_name` (also a save name that is not one, or one of the wall's own), `unknown_module`, `unknown_knob`, `unknown_port`, `unknown_kind`, `unknown_cable`, `unknown_change`, `full` (a cap was reached; also the cap on saves a seat may make), `unknown_save` (no save by that name), `save_exists` (a save by that name is there, and replacing it was not asked for), `broken_save` (the save's file is not a wall this one can load; it is left as it is), `wall_replaced` (the request named modules of a wall that has been replaced since its client looked: look again), `slow_down` (also: too many listings of the saves, or saves by a seat), `not_allowed`, `internal` (also: the disk refused a save, or a load could not be written as one batch, so nothing was replaced).

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
| `wall_save` | Save the wall as it is under a name (never over an existing save: that is the console's). |
| `wall_saves` | The saves, newest first, and any that cannot be loaded. |
| `wall_load` | Replace the wall with a save as it plays; the answer says how to undo it. |

Seats have no `wall_delete_save` and no way to save over a save: destroying a save is the console's alone. A load by another seat is told at once (the whole wall is new); saves and deletions in the usual window. Each request carries the wall the seat last saw with `wall_look`: after a load (by any seat, itself included) its changes are refused with `wall_replaced` until it looks again. `wall_saves` lists Tom's saves first and the wall's own after them.

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
| `w` | Save the wall under a name (starts as `wall-<date>-<time>`); asks before saving over a save. |
| `o` | The saves list, Tom's saves first; the wall's own (`before-…`) are counted and hidden until `a` shows them, after his. `⏎` chooses a save to load and `y` loads it (any other key, Enter too, goes back), `d` then `y` deletes it, `n` then `y` starts an empty wall, `w` saves, `r` reads the list again, `Esc` closes. While it is open it is read again when saves change, at most once a second. |
| `?` | Help. |
| `q` | Quit the TUI; the daemon keeps playing. |
| `Q` | Stop the daemon (asks to confirm). |

It is a console client on the same protocol as seat `Tom`, and it reconnects when the daemon restarts.

The header shows the save the wall last went to or came from (`save: tom fiddle`, and `· changed since` once it has). After a load, the status line says what was loaded, where the wall as it was went, and that `z` undoes it; another seat's load is told in the status line and the log, and anything held about the old wall (a turn on its way, a patch under way) is let go. The console tells its worker which wall it shows, and every request carries it, so a key pressed on the old wall never changes the new one.

## Tests (beyond unit tests on every module and knob)

- **Module DSP:** NaN/Inf inputs give silence, outputs stay in range, knob clamping holds, and the frequency is accurate for the VCO and LFO.
- **Engine:** random add/remove/patch/turn sequences under `assert_no_alloc` in the render path; feedback cycles render; removal returns the module over the retire ring.
- **Protocol:** serde round trips; every error code is reachable; oversized lines are handled; the flood guard works.
- **Daemon integration over a real socket:** two seats; events reach the other seat, not the actor; undo works; persistence survives a restart; a corrupt patch file is set aside.
- **Saves:** the store round trip and its durability (each step of writing made to fail in turn through `Store::fail_once`, which holds for any user, root included); a load failing at each step of its batch; the log reopened after a failed write or sync, a log ending mid-line on opening, and a line written twice read once; a change the log cannot take not made, a note it cannot take owed with every change refused until it is paid, and `patch.json` never written ahead of the log; a load that may be in the log said not to have happened; starting again after a crash mid-load, on a seeded wall, on a rolled-back `patch.json` with several loads after it, and on a wall the log does not know; the console's backups kept before a seat's; the seats' allowances to save and to replace the wall, and a refused load costing none; every kind of change the log refuses leaving the patch exactly as it was; a tempo the log refuses never reaching the desk, and words it refuses never reaching the player; a wall whose load rotated out of the logs, and one started again beyond what memory keeps; the log's health told when it owes and when it is paid, and a stop while it owes reusing no number told; lines since the last good sync written again after a failed one; and, through `Daemon::start`, a start that cannot log its notes leaving `patch.json` as it found it, migrated or seeded; names; refusing to replace a save; broken, misnamed and version-1 saves; importing the hand-made copies once; a load replacing the wall and its undo bringing it back exactly, ids and cable numbers included; the automatic save before a load, and no load when it cannot be written; changes to a replaced wall waiting until it is back; a load while recording; the hushed swap fading out, swapping in silence and fading in, under `assert_no_alloc`; two swaps back to back passing straight through, a change sent during the hold landing after the swap, and a swap too big for the ring waiting in order, unhush and all; the same load undone twice; a load with the desk linked; a crash right after a load; requests for a replaced wall refused; header caching, pruning and the listing budget; an import the disk refused, tried again; the console's keys and screens (Enter never loads, the wall's own saves grouped, the list read once a second in a storm); the seats' tools; and over a real socket, a load undone after a restart from the log and its save.
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

- **Interface:** the `Effect` trait, `EffectKind` and `ParamSpec` (see `kazoo-fx/src/lib.rs`). Every effect is stereo in and stereo out. `Effect::process_wet` hands over the wet signal alone beside the mix, from the same pass (a copy, never a second run).
- **Wall adapter:** each effect kind becomes a wall module kind whose id is the effect's id. It gets inputs `left` and `right` (`right` is mono-normalled to `left`), outputs `left` and `right` (the mix) then `wet_left` and `wet_right` (the wet signal alone), and a knob for every param. The wet outputs come after the mix's, so cables saved before they existed load unchanged.
- **What wet means:** for an effect with a `mix` knob, the wet outputs are exactly what `left`/`right` would be with `mix` all the way to wet, every other knob as set, whatever `mix` is at: patch `plate1.wet_left` into `pitch1.left` to shift only the tail. An effect with no mix knob (the overdrives, fuzz, amp, `trem`, `eq`) is all effect: its wet outputs carry the same signal as `left`/`right`, so a patch can always take an effect's wet side without asking which kind it has. (Processed-minus-dry was considered and refused: through an oversampled drive it is a phase-dependent difference signal, not a sound anyone patches for.)
- **Families:**
  - `drive`: circuit-modelled overdrives and fuzzes, a wavefolder, a bitcrusher, a ring modulator, a compressor.
  - `time`: tape, bucket-brigade (BBD) and digital delays, chorus, flanger, phaser, tremolo, a frequency shifter, a granular delay, and `pitch`, a clean pitch shifter: `shift` in semitones (±24), `fine` in cents (±100), `formant` (`move` or `keep`: keep holds a voice's formants in place so it never goes chipmunk) and `mix`. A jack on `shift` moves it 24 semitones per 1.0, so a cable at amount 0.5 tracks 1.0 per octave.
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
