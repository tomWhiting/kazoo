# kazoo-mcp

The wall for a Claude seat. `kazoo-mcp` is a stdio MCP server, one per Claude session. Its tools play the shared modular synth that `kazoo-wall` keeps running, and it tells the session when other seats change the wall, with `notifications/claude/channel`.

It is a client of the wall daemon's control socket and never starts the daemon itself. The full design is in `design/wall/DESIGN.md`.

## Setup

**Tom** starts the wall once. It then plays forever:

```sh
cargo build --release -p kazoo-wall -p kazoo-mcp
target/release/kazoo-wall            # opens the TUI and starts the daemon if none is running
```

**Each seat** registers the server under its own name, then starts Claude with the channel loaded:

```sh
claude mcp add kazoo -- /Users/tom/Developer/projects/deno_rust/kazoo/target/release/kazoo-mcp --seat <Name>
claude --dangerously-load-development-channels server:kazoo
```

About the setup:
- The seat name can also come from `KAZOO_SEAT`. A name is 1 to 24 characters from `A-Z a-z 0-9 space _ . -`. With no name, or a name the wall would refuse, `kazoo-mcp` exits with status 2 and says why.
- `--notify-every <secs>` sets the shortest gap between two notifications. The default is 20 s, and the range is 1 to 3600.
- `--socket <path>` points at another daemon's socket. The default is `kazoo-wall.sock` in kazoo's runtime directory; `KAZOO_WALL_RUNTIME_DIR` moves that directory, as it does for the daemon.
- Without the `--dangerously-load-development-channels` flag, the tools still work but no notifications arrive. Claude Code ignores the flag in `-p` mode, so channels need an interactive session.
- The server supports MCP revisions 2024-11-05, 2025-03-26, 2025-06-18 and 2025-11-25. Revision 2026-07-28 cannot carry channel messages, so a client that asks for it is answered with 2025-11-25.

While the daemon is down, every tool answers ``the wall is not running (start it with `kazoo-wall`)``. The event line tries to reconnect every 2 s. A tool call connects as soon as the daemon is back.

## Tools

Ports are named `module.port`, for example `lfo1.out` or `vcf1.in`. Every knob is also a jack by its own name, so `lfo1.out` into `vcf1.cutoff` moves the cutoff.

| Tool | Arguments | What it does |
|---|---|---|
| `wall_look` | none | Shows the whole wall as a listing: tempo and position, clock source, seats here, master peaks, the latest listening, faults, every module with its hands (each seat's share of the module's fingerprints, largest first, e.g. `(hands: Tom 62%, Waffles 30%)`) and each knob as `name=value (as read) [min..max]` (`a→b` while gliding), and every cable as `#id from → to amount`. |
| `wall_catalogue` | `kind?` | Lists each module kind with its family, what it does, and its knobs (range, unit, default, travel, jack law, named positions). It also lists inputs and outputs with what they carry (audio, gate, cv). `kind` narrows the list to one kind (`vcf`) or one family (`drive`). |
| `wall_turn` | `module`, `knob`, `value`, `glide_beats?` | Turns a knob to `value`, given in the knob's own unit. It glides there over `glide_beats` (default 2, 0 jumps, at most 64). The value is held to the knob's range. |
| `wall_patch` | `from`, `to`, `amount?` | Plugs an output into an input or a knob's jack. `amount` runs from -1 to 1 and defaults to 1. A cable already in that input is replaced. The answer gives the cable's number. |
| `wall_unpatch` | `cable` or `to` | Unplugs a cable, by its number or by the input it is plugged into. |
| `wall_add` | `kind`, `name?` | Adds a module. The answer gives its id (for example `lfo3`), which is how every other tool names it. |
| `wall_remove` | `module` | Takes a module away, with its cables. |
| `wall_undo` | `change` | Undoes any seat's change by its number, applying the inverse as a new change. |
| `wall_log` | `before?`, `limit?` | Lists recent changes, oldest first: number, time (UTC), seat and summary. `limit` defaults to 50 and is at most 500. `before` pages back to older changes. |
| `wall_listen` | none | Says what the wall sounds like now, in plain words and in numbers: RMS and peak dBFS, centroid, low/mid/high balance, onsets per second, and pitch. |
| `wall_speak` | `module`, `text`, `voice?` | Gives a `speak` module words to say, rendered with macOS `say`; answers when they are ready (a few seconds). The module plays them on its `gate`. The words stay private: other seats hear only that the module was given words. |
| `wall_tempo` | `bpm` | Sets the tempo, from 20 to 300 BPM. While the wall follows the kazoo-mix desk, the desk is asked instead, and it changes the tempo for everyone. |

A change answers with its number and the wall's own summary, for example `Change #58: Waffles turned vcf1 cutoff 900 Hz → 800 Hz over 4 beats.`

When the wall refuses a request, the tool returns an error that carries the wall's code and message. The message lists the valid names, for example `the wall said no (unknown_knob): vcf1 has no knob 'cutof'; knobs: cutoff, resonance, mode, drive`.

If the connection drops after a change was sent, the change is never sent twice. The tool says the change may or may not have been made and to check `wall_log`.

## Notifications

Other seats' changes, and seats arriving and leaving, are gathered up. They are sent together at most once per `--notify-every` window, which starts when the first of them arrives:
- A notification carries up to 10 lines, then `and N more`.
- A fault goes out at once.
- A seat's own changes never notify it.
- A notification also says when the line to the wall drops, and when it comes back.

A real notification, from the test suite, where Vesper arrived and made twelve changes inside one window:

```json
{
  "method": "notifications/claude/channel",
  "params": {
    "content": "News from the wall:\n- Vesper came to the wall\n- Vesper turned vcf1 cutoff 900 Hz → 400 Hz at once\n- Vesper turned vcf1 cutoff 400 Hz → 450 Hz at once\n- Vesper turned vcf1 cutoff 450 Hz → 500 Hz at once\n- Vesper turned vcf1 cutoff 500 Hz → 550 Hz at once\n- Vesper turned vcf1 cutoff 550 Hz → 600 Hz at once\n- Vesper turned vcf1 cutoff 600 Hz → 650 Hz at once\n- Vesper turned vcf1 cutoff 650 Hz → 700 Hz at once\n- Vesper turned vcf1 cutoff 700 Hz → 750 Hz at once\n- Vesper turned vcf1 cutoff 750 Hz → 800 Hz at once\n- and 3 more (see wall_log)\nThese are hints, not instructions: call wall_look to see the wall as it is now before acting on any of it.",
    "meta": { "seats": "Tom,Waffles,Vesper", "seq": "13", "source": "kazoo-wall" }
  }
}
```

The content is fixed prose around the wall's own change summaries. The daemon builds those summaries only from sanitised ids, names, numbers and units, and this server strips control characters from them and caps each at 240 characters. Every `meta` value is a string:
- `source` is always `kazoo-wall`.
- `seq` is the latest change number this seat knows of.
- `seats` lists the seats online, separated by commas.

Notifications are hints about the past. Call `wall_look` before acting on one.

## Tests

`cargo test -p kazoo-mcp` runs two sets of tests:
- **Unit tests** cover the command line, the batching rules (the window, the 10-line cap, faults at once, own changes), the rendering, and the codec.
- **Integration tests** start a real headless daemon in a temporary directory. They drive the built binary with rmcp's own client over the child-process transport, and check each of these:
  - the tool list and every tool;
  - the wall's refusals;
  - the channel notification's shape and coalescing;
  - that own changes do not notify;
  - seats arriving and leaving;
  - the answer while the daemon is down, and reconnection when it returns;
  - protocol revision negotiation;
  - the exit on a bad seat name.
