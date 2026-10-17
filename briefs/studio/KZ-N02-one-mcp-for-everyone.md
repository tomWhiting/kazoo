# KZ-N02: one MCP for everyone: a seat joins a wall by name, and its config says nothing about who or where

Written by Cassio, 18 Oct 2026, about 09:50. Decided by Cassio; the reasons are given with each decision.

Place: Track N, the first lane of KZ-N01 (§1 there). Crates:

- **kazoo-mcp:** `cli.rs`, `run.rs`, `tools.rs`, `tools/*`, `link.rs`, `channel.rs` and the server instructions.
- **kazoo-cli:** `kzoo mcp`'s arguments.

Nothing changes in kazoo-wall: the protocol, `Hello` included, stays as it is.

## Why

Tom, 18 Oct 2026, about 09:00: "it's really really important to me to strip out all of the identifying things from our MCP servers generally... There should be one single MCP server. Everybody gets an exact same copy of the exact same thing."

Today every seat's config names it (`kzoo mcp --seat Cassio`), and a seat reaches exactly one wall, fixed when it starts.

## Outcome

### 1. The command

**`kzoo mcp` takes no arguments that say who or where.** `--notify-every` stays, because it identifies nothing.

**Old configs keep working for this release:**

- **`--seat <name>` or `KAZOO_SEAT`** joins this Mac's wall as that name at start, as if `wall_join {as}` had been called.
- **`--socket <path>`** names the wall that join uses.
- **Each one said once** on stderr: "kazoo-mcp: --seat is going: leave it out, and the seat joins with wall_join".
- **They go** in the release after.

**Decision:** Cally's and Flapjack's configs keep playing the day this lands, and every new config is the same line.

### 2. Finding walls: `wall_walls`

**What it lists.** The walls this MCP can reach. In this lane that is this Mac's wall only: its default socket, honouring `KAZOO_WALL_RUNTIME_DIR` as today. That wall is named `here`. N03 adds the local network.

**For each wall:**

- `name`;
- `where` (`this Mac`);
- `playing` (whether it answers);
- `seats` (who is on it, read with a watcher hello, which is never announced and changes nothing);
- `joined_as` (this MCP's name there, if it has joined).

**Decision:** a seat can look before it joins, and joining never announces anyone until they choose a name.

### 3. Joining and leaving

**`wall_join {wall, as}`**

- `as` is the seat's name, checked as seat names are today (1 to 24 of `A-Z a-z 0-9 space _ . -`).
- `wall` defaults to the only wall `wall_walls` lists. With none, or more than one, it is required, and the refusal lists them.
- **What joining starts:** that wall's link, its event line (`link::follow`) and its own notifier (`channel::notify`).
- **What it answers:** the hello (the seats there and the revision).
- **Joining a wall already joined is refused,** with the name it is joined as. To change name, leave and join again.
- **A wall that is not playing can be joined.** The link waits for it, as a seat waits today, and says so.

**`wall_leave {wall}`** stops that wall's event line and notifier, and closes its connections, so the wall sees the seat go.

**More than one wall.** A seat may join any number of walls. In this lane it can only be one (`here`), but nothing in the seat assumes that.

### 4. Every other tool

**Every other tool takes an optional `wall`.**

- **With one wall joined,** it may be left out.
- **With none joined,** the tool is refused: "join a wall first: wall_walls lists them, and wall_join {as} joins one".
- **With more than one joined,** `wall` is required, and the refusal lists the walls joined.

**Tools that take no arguments today** (`wall_look`, `wall_saves`, `wall_listen` and the rest) gain a parameters object holding only `wall`.

### 5. News

Each joined wall's notifier sends its channel notifications as today, with `wall` added to the notification's meta beside `seats`. The text is unchanged.

**Decision:** a seat on two walls can tell whose news it is, and a seat on one reads exactly what it reads today.

### 6. The server's instructions

The kazoo server's instructions open with how to begin:

> wall_walls lists the walls you can reach; wall_join with a name of your choosing puts you on one. Then wall_look first, as ever.

The rest stays as it is.

### 7. Tests (red first)

- **The bare command:** `kzoo mcp` with no arguments starts and joins nothing.
- **The old flags:** `--seat` joins `here` at start and says it is going, and `--socket` picks the wall.
- **`wall_walls`:**
  - lists `here`, playing or not;
  - its seats leave out the watcher it used;
  - `joined_as` appears after a join.
- **`wall_join`:**
  - a bad name is refused;
  - joining twice is refused, naming the name;
  - a join to a wall not yet playing waits, and joins when it starts;
  - after a join the wall lists the seat.
- **`wall_leave`:** the wall announces the seat leaving, and tools are refused again.
- **The `wall` argument:**
  - every tool is refused before a join;
  - an unknown `wall` is refused and the joined ones are listed;
  - the schema of every tool carries an optional `wall`.
- **News:** a notification carries `wall` in its meta.

## Checks

- nextest, clippy, fmt and ast-grep on `-p kazoo-mcp -p kzoo`
- `cargo test --doc -p kazoo-mcp`
- Opus review until READY, then land, publish and install.
- Cally's and Flapjack's seats keep their configs: they move to the bare line when they next restart. Their restart is theirs.
