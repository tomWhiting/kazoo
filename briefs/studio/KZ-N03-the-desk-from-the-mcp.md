# KZ-N03: the desk from the MCP: everything that plays gets a strip, comes up from silent, and a seat sets the levels

**Withdrawn, 18 Oct 2026 at 10:02.** This brief rests on Tom's words as Waffles and then Buckley relayed them. Buckley has withdrawn that note: it was a sentence Tom had not finished, nothing in it is settled, and Tom wants it to reach Cassio from him directly. Nothing here is built or briefed further until Tom says it himself. The text below is kept as it was written.

Written by Cassio, 18 Oct 2026, about 09:55 (first stamped 11:30 by mistake). Decided by Cassio; the reasons are given with each decision.

Place: Track N, after KZ-N02 (it uses N02's one MCP), and before the network lanes. KZ-N01's order moves accordingly (see the end). Crates:

- **kazoo-mix:** a control socket, and new strips joining silent.
- **kazoo-mcp:** the desk tools.
- **kazoo-cli:** nothing new.

## Why

Tom, 18 Oct 2026, 09:46 to 09:49, by voice to Waffles, relayed by Waffles and then Buckley:

- **Kazoo is many instruments.** It is not just the wall: it is many instruments that make up the wall, each playable on its own, joined over Unix sockets.
- **The desk first.** The MCP server should let a seat control the other parts, the mixing desk first.
  - Each thing (the wall, the next thing) is registered as a channel on the desk automatically.
  - The desk is where a thing is brought up from silent, and where relative levels are set.
  - The wall's internal levels stay the wall's own.
- **No seat file.** He and Cassio agreed that needing a seat file just to play music together is unnecessary (KZ-N02).
- **One general instrument-playing MCP:** "what can you play? electronic, heaps of instruments".

Today the desk has no way in but its own screen. A strip joins at unity, so a thing plugged in is at full level at once.

## Outcome

### 1. Every source has a strip, and a new one comes up from silent

**Joining.** Every instrument and the wall already take the next free strip when they register with the hub. That stays, and it is said in the desk's words for each one ("strip 3: the wall").

**The change.** A strip that a newly registered source takes now starts with its fader at `FADER_OFF_DB`, and not at unity. Everything else on it stays the default: trim, EQ, pan, sends, mute and solo.

**What the desk shows:** the strip reads "silent: bring it up" until its fader first moves.

**`b` on the desk** brings the selected strip up to unity over 4 seconds, along the fader law, as a hand would.

**A source that re-registers** (the same name, back within 30 seconds, as after a restart) keeps its strip and that strip's controls.

**Decision:** "the desk is where a thing is brought up from silent". Nothing new ever arrives loud.

### 2. The desk's control socket

**The socket.** kazoo-mix opens a control socket beside the hub's: `kazoo-mix-control.sock` in the same runtime directory, mode 0600.

**The protocol.** It speaks the wall's line protocol: one JSON object per line, `id` and `op`, answered `{id, ok, result | error}`, at most 64 KiB a line. It takes at most 16 connections. The requests are:

| Request | Answers |
|---|---|
| `look` | Every strip: its number, source name, kind (wall, instrument, input), connected or not, fader dB, mute, solo, pan, and peak and RMS over the last second. The master: fader, peaks, and whether the desk is playing. |
| `fader {strip, db, over_seconds}` | Moves the fader to `db`, held to `FADER_OFF_DB` to `FADER_MAX_DB`. It glides along the fader law over `over_seconds` (0 to 30, default 2). |
| `up {strip, over_seconds}` | `fader` to 0 dB. The default glide is 4 seconds. |
| `down {strip, over_seconds}` | `fader` to `FADER_OFF_DB`. The default glide is 4 seconds. |
| `mute {strip, on}` | Sets mute. |
| `solo {strip, on}` | Sets solo. |
| `pan {strip, value}` | Sets pan, -1 to 1. |
| `master {db, over_seconds}` | Moves the master fader. |

**Naming a strip.** `strip` is the strip's number, or its source's name. A name that matches more than one strip is refused, listing the strips.

**How a change is applied.** Each change goes through the same `ChannelControls` setters the desk's keys use. A glide is stepped on the desk's worker at 50 Hz and never on the audio callback. The desk's screen shows the moving fader.

**Decision:** one protocol shape for the wall and the desk, so the MCP's link code is shared.

**What it does not do.** It does not reach into the wall. "The wall's internal levels stay the wall's own": the desk sets the wall's strip, and the wall's own knobs are `wall_turn`'s.

**Who.** In this lane, anyone who can open the socket may use it. That means this Mac's user only, through the file mode. When the network lanes put the desk on the network, they put this socket behind the same keys as the wall (KZ-N01 §3).

### 3. The MCP's desk tools

The one MCP (KZ-N02) gains:

- **`desk_look`:** the strips and the master, in words. For example: "strip 2, the wall: −6 dB, peaks −14 dBFS, playing".
- **`desk_fader {strip, db, over_seconds}`**, **`desk_up {strip}`** and **`desk_down {strip}`.**
- **`desk_mute {strip, on}`**, **`desk_solo {strip, on}`** and **`desk_pan {strip, value}`.**

**Finding the desk.** The desk is found as this Mac's desk, through the control socket. When it is not running, every desk tool says so: "the desk is not running: kzoo mix starts it". No joining is needed: the desk has no seats.

**What the instructions add:**

> the desk (desk_look) is where every instrument and the wall come up from silent and where their levels are set; a new one is silent until someone brings it up.

### 4. What can you play: the instruments

**`kazoo_instruments`** lists what kazoo can play. For each instrument it gives the name, what it is in a line, whether it is running, and which strip it is on. The list is the one in `kzoo --help` (808, 303, cs80, mini, juno, prophet, dx, string, glass, arp, mouth and the wall).

**Decision:** this answers "what can you play?" now. Playing an instrument from the MCP has a lane of its own, KZ-N04. That lane covers starting one with a looped phrase (`kzoo dx --phrase`, `kzoo play`), sending it notes, and stopping it, and it comes straight after this one. This lane gives the list and the levels.

### 5. Tests (red first)

**kazoo-mix:**

- A newly registered source's strip starts at `FADER_OFF_DB`.
- `b` brings a strip up to unity over 4 seconds.
- A source that re-registers within 30 seconds keeps its controls.
- The control socket:
  - each request, the refusals included (a strip that is not there, an ambiguous name, a value out of range, an over-long line, the seventeenth connection);
  - a glide reaches its value, and is stepped off the audio thread (the audio-thread no-allocation guard holds);
  - mode 0600.

**kazoo-mcp:**

- Each desk tool against a stand-in control socket.
- The desk not running is said.
- `kazoo_instruments` lists every `kzoo` instrument.

## Checks

- nextest, clippy, fmt and ast-grep on `-p kazoo-mix -p kazoo-mcp`
- `cargo test --doc` on both
- Opus review until READY, then land, publish and install.

## KZ-N01's order, as it now stands

1. N02, the one MCP.
2. N03, the desk from the MCP (this lane).
3. N04, instruments played from the MCP.
4. N05, keys, pairing and Noise on the local network, for the wall and the desk.
5. N06, the internet: UDP, `kzoo meet`, the hole punch, the relay and invitations.
6. N07, spins.
