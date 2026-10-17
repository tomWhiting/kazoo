# KZ-D23: an instrument on one Mac plays into a desk on another, over the local network

Written by Cassio, 17 Oct 2026, about 18:20. Decided by Cassio; the reasons are given with each decision.

Place: Track D, after KZ-D22. Two lanes, in order:

- **D23a**, in kazoo-core and kazoo-cli: the instrument side.
- **D23b**, in kazoo-mix: the desk side. It is cut from D23a's branch.

## Why

Tom, 17 Oct 2026, about 18:05: "my regular laptop the screen is broken... Could we make kazoo work over like local network sockets very quickly and so I could potentially run it here and do the singing into the mouth synthesiser while my old laptop it's plugged into the speaker is playing everything else."

So the old laptop runs the desk (kazoo-mix) and the wall, and plays to the speaker. This laptop runs `kzoo mouth`, takes Tom's voice, and sends the mouth's audio to that desk over the home network. It joins the desk as any instrument does: a strip, the tempo, play and stop.

## Outcome

### D23a: an instrument can reach a desk by host and port

1. **The address.** `kazoo_core::ipc::link::HubAddress` gains `Tcp(String)`, holding `host:port`. The host may be a name (such as `tom-old.local`), IPv4 or IPv6 in brackets.
   - **Parsing.** `HubAddress::parse(&str)` reads `host:port` as `Tcp`. Anything containing a `/` is a `Socket` path. Anything else is refused, with the reason.
   - **Decision:** one text form that a person can type covers both kinds.

2. **The stream.** A new module, `kazoo_core::ipc::stream`, holds `HubStream`: either `Unix(UnixStream)` or `Tcp(TcpStream)`.
   - **What it implements:** `Read`, `Write` and `AsFd`, plus `set_nonblocking`, `set_read_timeout`, `set_write_timeout`, `shutdown` and `try_clone` if the client uses it.
   - **Who uses it:** `HubIpcClient` holds a `HubStream` in place of its `UnixStream`. `register_with_timeout` takes the address (Unix path or TCP `host:port`) in place of a `&Path`.
   - **Both link paths:** `ipc/link/streams.rs::try_connect` and `host/link.rs` connect either way.
   - **Connecting over TCP:**
     - It resolves the host with `ToSocketAddrs` on the link's worker thread, never an audio thread. It tries each resolved address with `TcpStream::connect_timeout` at the registration timeout already used.
     - It sets `set_nodelay(true)` before registering. A refusal names the address it tried.
   - **Decision:** the frames, the registration and the pacing stay exactly as they are; only the pipe under them changes. A TCP stream is ordered and reliable, as a Unix one is, so nothing above it can tell.

3. **Choosing the desk from outside.** `HubAddress::Discover` first reads the environment variable `KAZOO_DESK`.
   - **If it is set,** it is parsed as above and used. This applies to every instrument, the mouth and the wall included, without a flag of its own.
   - **If it does not parse,** the link's refusal says so. It never falls back to the local desk.
   - **The flag.** `kzoo` gains a global option `--desk <host:port | socket path>`. It sets `KAZOO_DESK` for the program before anything starts, in `kazoo-cli/src/entry.rs` or wherever the arguments are first read.
   - **Decision:** an environment variable reaches every program that `kzoo` starts, including the instruments `kzoo studio` launches in tmux, and a flag would have to be threaded through each.

4. **Tests (red first).**
   - `HubAddress::parse` covers each form, the refusals included.
   - A link to a `TcpListener` on `127.0.0.1:0` registers, sends audio frames and receives a transport sync. This mirrors the Unix tests in `ipc/link/tests` and the client tests.
   - `KAZOO_DESK` is read by `Discover`. Use a helper that takes the variable's value as an argument, so no test sets process environment.
   - A bad `KAZOO_DESK` is refused, and not replaced by the local desk.

### D23b: the desk listens on the network when asked

1. **The flag.** `kzoo mix` (kazoo-mix) gains `--listen <ip:port>`, such as `--listen 0.0.0.0:7700`.
   - **What it opens:** a `TcpListener`, non-blocking, alongside the Unix socket it always has. The hub polls it with the others (raise `MAX_POLLED` by one).
   - **Accepted streams:** each one is set non-blocking with `set_nodelay(true)`, then goes through the same path as a Unix connection: registration, its strip, the instrument's clock. `Instrument.stream` and `refuse` take a `kazoo_core::ipc::stream::HubStream`.
   - **Without the flag,** nothing listens on the network.
   - **Decision:** a desk on the network is something Tom asks for, never a default.

2. **Who may join.** A connection from outside the local network is refused before it registers, and the desk's notices say who it was. The local network is:
   - IPv4: loopback, 10/8, 172.16/12, 192.168/16, 169.254/16 and 100.64/10 (Tailscale's range, which this house already uses);
   - IPv6: loopback, fc00::/7 and fe80::/10.

   **Decision:** a desk listening on 0.0.0.0 in a café must not take a stranger's audio, and the house's own machines (including over Tailscale) must never be refused.

3. **Saying where it is.** When it listens, the desk's header (or its status line, wherever the socket path is shown now) shows the listening address and this machine's local IPv4 addresses. Tom can then read off the `host:port` to give `--desk` on the other Mac. Getting the addresses must not need a new crate: use `getifaddrs` through `rustix`, if it has it, or the existing dependencies. If neither has it, show the listening address only and say so under FOUND.

4. **The mouth's latency over the network.** Wi-Fi adds jitter of a few milliseconds that a Unix socket never has. Find the margin the desk already keeps before a late block is re-placed on the studio clock (`hub/instrument.rs`, `studio_clock.rs`). Then report under FOUND what a TCP stream on loopback measures, using the existing jitter and wake tests run over TCP. Do not change that margin in this lane.

   **Decision:** first measure what the network costs, then decide.

5. **Tests (red first).**
   - The hub with `--listen 127.0.0.1:0` takes a registration over TCP, seats it on a strip, and carries audio and a transport anchor, as `hub/tests` does for Unix.
   - A connection whose peer address is outside the local network is refused, through a function that takes the peer address. A test cannot dial from a public address.
   - Without `--listen`, no TCP port is open.

## Checks

- **D23a:** nextest, clippy, fmt and ast-grep on `-p kazoo-core -p kzoo`.
- **D23b:** the same on `-p kazoo-mix`.
- **Review:** Opus, until READY.
- **Land:** with the pooled gate, then publish.
- **Then:** Tom runs `kzoo mix --listen 0.0.0.0:7700` (plus the wall) on the old laptop, and `kzoo --desk <old laptop>:7700 mouth` here.
