# Kazoo boards: the wall on the web

Design only. Nothing here is built and no code has changed. Written 9 Oct 2026 from the code on `main` (47b3a13) and the in-flight work in the agent worktrees, plus web research done the same day. Every external fact is cited in "Sources" at the end, as [S1], [S2] and so on.

## What Tom asked for

- **Today (9 Oct 2026):** a web interface "both as the terminal version, but like with a more sort of... more intuitive interface".
- **Kazoo boards (27 Sep 2026):** each board is its own shared wall. People join a board to listen or to play, flick between boards, or start their own, "like radio stations you can reach into".
- **Keep the rack's character,** above all the cables hanging between the modules, which he loves in the terminal rack view.
- **A GeoCities look:** a tiled, messy background and gifs he picks himself, with the live synth in the middle. He named the spinning "under construction" light and signs, and a Microsoft WordArt title.
- **Physical controls:** knobs to turn, cables to drag.
- **A portfolio piece:** the live wall, heard by visitors and maybe played by them. Not recordings.

## Goals

1. **Hear it:** anyone with the link hears the wall live in a browser, at good quality, after one click (browsers need that click before they play sound).
2. **See it:** the rack drawn as it is in the console: cream plates on a dark rack, black knobs, cables hanging between jacks in their own colours, knobs moving as they glide, the meters, the seats, the listen line and the log.
3. **Play it:** a player turns a knob and hears the change well under a second later. A player can also patch, unplug, add, remove, move modules and undo.
4. **Boards:** many walls at once, each its own daemon, with a way to flick between them and to start a new one.
5. **Terminal version:** the console itself, in a browser.
6. **The wall's rules hold on the web:** nothing automated, no seat colours, fingerprints shown as they are, the flood guard, the safety caps on knobs, and the master limiter.
7. **Safe to put on the internet:** strangers can never reach Tom's machine, his disk, his desk or his Claude seats beyond what he chooses.

**Not goals:** recordings or playback of the past, accounts and passwords for the public, chat, and any change to how the wall sounds.

## What exists today, and what it means for the web

Read from the code; file references are to `kazoo-wall/src` unless they say otherwise.

**The daemon** (`daemon/`):
- It listens on a Unix socket only (`daemon/server.rs`). Each connection gets a reader thread and a writer thread, with up to 1,024 lines queued before the connection counts as stuck. There are at most 64 connections at once (`control.rs`, `MAX_CONNECTIONS`). The web can never reach this socket directly: something in between has to be the trust boundary.
- **A seat is just a name** sent in `hello`. Any client can say it is `Tom`. On the socket that is fine, because the 0700 directory is the access control. On the web, the bridge must choose and check names.
- **Roles:** a `console` may `shutdown` and `monitor`. A `watcher` sees everything, is never listed or announced, and may change nothing. Everyone else is a seat.
- **The flood guard** is per seat name: 30 changes, refilling at 3 a second (`daemon/wall.rs`, `FLOOD_BURST`, `FLOOD_REFILL_PER_SECOND`). Changes are turn, patch, unpatch, add, remove, arrange, undo, tempo, speak and record. Each new web seat name would get its own bucket, so the wall has no limit on a crowd as a whole.
- **Some ops reach beyond the wall:**
  - `record` writes WAVs to `~/Music/kazoo-wall` (Tom's disk filled twice on 2 Oct).
  - `tempo` asks the kazoo-mix desk for a new tempo when the wall is on the desk, which moves Tom's whole studio.
  - `speak` turns text into sound with macOS `say`.
  - The designed `acceptor` module (DESIGN.md, not built yet) would poll URLs and run commands.
  
  None of these may be open to strangers by default.
- **The wall starts silent** (`heard: false`) until a console sends `monitor`. That is Tom's rule: generating always, heard only when he presses `m`.
- **Knob motion is not pushed.** Events say when a change was made, but a glide's progress is only in `look` (each knob's `value` against its `target`). The console polls `look` every 100 ms (`tui/mod.rs`, `POLL`). The live snapshot was about 36 KB at 40 modules (FEED.md). Ten of those a second to every browser would be wasteful, so the bridge should poll once and send only what moved.
- **Meters** are held with a decay on the control thread (`daemon/wall.rs` around line 1440), so any number of clients can read them without stealing each other's peaks.
- **Audio taps:** the engine already copies its master into two SPSC rings: the listen ring (mono, for analysis) and the record ring (stereo interleaved, while recording, taken *before* the monitor, so a recording hears the wall even while it is silent). `daemon/record.rs` drains the record ring on its own thread and carries a recording on into a new file when the audio is rebuilt. A third ring for streaming, built on the same pattern, is the natural audio source for the web.
- **Several walls can already run side by side:** `KAZOO_WALL_RUNTIME_DIR` and `KAZOO_WALL_STATE_DIR` place the socket and the state, and `serve --no-audio --no-desk` renders on a timer with no device and no desk (`main.rs`). A board can therefore be a daemon with its own directories, with no change to the daemon.

**The console** (`tui/`):
- `App` is already a clean state machine. Replies come in through `on_reply`, time moves through `tick(now)`, requests go out through `take_jobs()`, and drawing is `draw::draw(frame, app, now, unix_now)`. Only `tui/mod.rs` runs the loop. That is what makes a browser port feasible.
- What ties it to a terminal and to Unix:
  - crossterm's `KeyEvent` and `MouseEvent` types in `app/keys.rs` and `app/mouse.rs`;
  - `std::time::Instant` throughout;
  - two worker threads with blocking Unix-socket clients (`tui/worker.rs`);
  - the console's imports from the `kazoo_wall` library, which also holds the daemon, cpal and the engine.
- The console always says hello as seat `Tom` with the console role (`tui/mod.rs`, `SEAT`). Served to anyone else as it is, it would give them Tom's powers.
- **Cables** (`tui/rack/geometry.rs`, `cable_path`) are a quadratic Bézier from jack to jack. The control point is at the middle, hung below the lower end by `0.25 × distance + 6` dots. The web client should use the same rule in pixels so the cables hang the same way. VCV Rack draws its cables the same way: one quadratic curve whose midpoint is pushed down by the slack [S30].
- **Mouse gestures** (`tui/app/mouse.rs`): drag a knob up or down (shift for fine); scroll for fine; double-click for the default; drag jack to jack to patch; right-click a cable to unplug; drag a title bar to move a plate; click a blank panel to add; drag the rack to pan. A dragged knob sends turns at most every 100 ms with a 0.125-beat glide, then its last value on release, all within the console's own copy of the flood guard (`tui/app/flood.rs`). The web client copies all of this.
- **Knob travel** follows the catalogue's curve (`tui/knob.rs`): linear, or log with a range starting at zero being logarithmic from a thousandth of its top. The snapshot's knobs carry `min`, `max`, `unit` and `stepped`, but not the curve or the step labels; those come from `catalogue` once.

**kazoo-mcp** is a tokio client of the same socket, with one request connection and one subscribed connection per Claude seat. It turns other seats' changes and seat joins into Claude channel notifications, coalesced to one every 20 s. So **every web player who joins is announced to every Claude seat**, and a web player's seat name, 24 characters of `[A-Za-z0-9 _.-]`, reaches those seats' context. "Ignore the rules" fits in 24 characters.

**kazoo-mix** is the desk and hub. The wall on Tom's Mac joins it through `hub_link` and sends its audio there instead of to the device. A board on a server has no desk.

**In-flight work this design must fit, without waiting for it:**
- **Save and load** (named saves, load, and an auto-save before every load). A load replaces the whole patch under every client. The bridge must treat it like a reconnect: take a fresh `look` and send the browsers a whole new wall. If the save/load work adds an event for it, the bridge listens for that. If not, the bridge spots it from the revision and the module set changing at once, or from a gap in `seq`, and takes a fresh `look` (FEED.md already says to on a gap). Web players do not get save or load until phase 3, and then only a board's owner does.
- **Removing the module and cable caps** (worktree `agent-ac019f91…`, `engine/room.rs`). The engine grows by doubling, and the only guard left is compute: above 80% render load, a change that adds work is answered `full`. On the web this means:
  1. A snapshot has no upper size, so the bridge sends changes rather than whole snapshots, and the client draws only the plates in view.
  2. Because `full` is shared, one crowd could fill a board and lock everyone else out of adding. The bridge gives each guest a share of adds, and a board can have its own module ceiling.
  3. The client shows `full` in words ("the wall is working as hard as it can; take something away first"), not as an error.
- **Real-time priority** for threads that carry audio (worktree `agent-ae651b3c…`, `kazoo-core/src/realtime.rs`, using `audio_thread_priority`). The stream tap's draining thread and the bridge's encoder must stay *ordinary* threads, so they can never steal time from the render. On Linux the promotion is `SCHED_FIFO`, which needs `RLIMIT_RTPRIO` or `CAP_SYS_NICE`. A VPS service unit has to grant that (`LimitRTPRIO=`), or the render runs at normal priority. Several boards with promoted render threads on a small shared-vCPU server can starve the bridge, so boards per core are capped.

## Architecture

```
                         browsers (listeners, players, Tom)
                 ┌───────────────────────────────────────────────┐
                 │ page: GeoCities shell + rack (SVG, TypeScript)│
                 │   ├─ WebSocket /b/<board>/ws  (JSON + Opus)   │
                 │   ├─ AudioWorklet player (jitter buffer)      │
                 │   └─ optional: terminal tab (ttyd, Tom only)  │
                 └──────────────────────┬────────────────────────┘
                                        │ HTTPS / WSS
                         (Cloudflare in front for pages, optional)
                                        │
┌───────────────────────────────────────┴─────────────────────────────────────┐
│ kazoo-boards  (new crate, one process; tokio + axum)                        │
│                                                                             │
│  auth: invite tokens, roles ── rate limits ── op allowlist ── name rules    │
│                                                                             │
│  per board:                                                                 │
│   ├─ watcher link ── subscribe + look every 100 ms ── diff ──► fan-out      │
│   ├─ player links (one daemon connection per active player, seat "x.web")  │
│   ├─ tap reader ── f32 stereo ─► resample to 48k ─► Opus 10 ms ─► fan-out  │
│   └─ supervisor: starts/stops the board's daemon, restarts it if it dies    │
└───────┬──────────────────────────────┬──────────────────────────────┬───────┘
        │ Unix socket (NDJSON)         │ Unix socket (binary PCM)     │ spawn
        ▼                              ▼                              ▼
┌───────────────────────────────────────────────────────┐   ┌──────────────────┐
│ kazoo-wall serve --no-audio --no-desk  (one per board) │   │ board B, C, ...  │
│  KAZOO_WALL_RUNTIME_DIR=…/boards/<slug>/run            │   │ same shape       │
│  KAZOO_WALL_STATE_DIR=…/boards/<slug>/state            │   └──────────────────┘
│  control socket: kazoo-wall.sock     (unchanged)       │
│  stream tap:     kazoo-wall-tap.sock (new, one reader) │
│  engine ─► master ─► [listen ring] [record ring] [stream ring: new]          │
└───────────────────────────────────────────────────────┘

Tom's own wall on his Mac (desk-linked, `say` works) is "board zero" in phase 1,
with the bridge running beside it on the Mac. From phase 3 it can be shown on the
public site through an outbound link from the Mac (see Hosting).
```

## Components

### 1. The gateway: `kazoo-boards`

A new binary crate in the workspace. It runs on tokio, as kazoo-mcp does, and uses axum 0.8 (0.8.9, 14 Apr 2026) with its `ws` feature for HTTP and WebSockets [S1]. It depends on `kazoo-wall` for the protocol types, as kazoo-mcp does, and speaks to daemons only through their sockets.

**What it does:**
- **Serves the pages:** the static client, the boards directory, and the per-board WebSocket at `/b/<board>/ws`.
- **Watches each board on one connection:** `hello` as watcher `kazoo-boards`, then `catalogue`, `look` and `subscribe`. Every event is passed on to the browsers. That includes a player's own changes, which a seat's own connection never hears.
- **Shows motion cheaply:** it takes a `look` every 100 ms on its own request connection, compares it with the last one, and sends a small `motion` message holding only what moved: each knob whose `value` changed, the beat, the meters, and the listen line when it changes. A browser joining gets one full `wall` message (the snapshot) and then motion and events. One `look` per board every 100 ms costs the daemon what one console costs today, whatever the number of browsers.
- **Plays for players:** each player who acts gets their own daemon connection, saying `hello` as their seat. That gives each player the wall's own per-seat flood guard and fingerprints, and their own name in the log. The connection opens on the player's first change and closes after 5 minutes without one, so the seat list shows people actually playing. The daemon's 64-connection limit and its two threads per connection bound this, so phase 1 to 3 allow at most 24 active players per board. A protocol addition (the relay, below) lifts the limit in phase 4.
- **Streams audio:** it reads the board's stream tap, resamples to 48 kHz if the board renders at another rate (rubato is already in the workspace), and encodes Opus once per board. It then sends the same packets to every listener.
- **Supervises boards:** it starts and stops each board's daemon with its own directories, and restarts it if it dies, with a backoff. It takes a `look` after every reconnect.
- **Enforces everything in "Security and abuse"** before a request reaches a daemon.

**Messages to the browser** are JSON text frames, with Opus packets in binary frames on the same socket. One connection is easier to authenticate and rate-limit, and survives proxies. Each audio frame is a 1-byte kind, an 8-byte sample counter and the packet. The counter lets the client spot gaps and line events up with sound. The JSON messages are:
- `hello`: the role, the seat name, the board, the server time;
- `wall`: a full snapshot with the catalogue;
- `motion`;
- `event`: the daemon's `change`, `seat`, `fault`, `fingerprints` and `rack` events passed on as they are;
- `answer`: the reply to the browser's own request, with the daemon's error code and message;
- `board`: the listener count and the board's settings.

**Messages from the browser** are the wall's own request shapes (`turn`, `patch`, ...) with a browser-chosen `id`. The bridge checks each one against the role's allowlist, the size cap and the rate limits, then forwards it on that player's own connection and passes back the answer. There is no new vocabulary for the browser to learn.

**One process, many boards, one daemon per board.**

| | One daemon per board, one bridge (**recommended**) | Many boards inside one daemon |
|---|---|---|
| Daemon changes | none: the env vars and `--no-audio --no-desk` already exist | large: the control thread, store, socket, seats, flood guard and log are all one-wall |
| Isolation | a board that faults or is overloaded affects only itself; its compute guard is its own | one heavy board starves the rest |
| Real-time | one render thread per board, each promotable | one thread rendering many walls |
| Cost | a process and some memory per board | a little less memory |
| Ops | the supervisor starts, restarts and stops boards | one thing to run |

Several bridges (one per board) would duplicate the HTTP, auth and rate-limit state for no gain.

### 2. The audio stream

**The daemon side (new): a stream tap.**
- **A third ring**, built like the record ring, the stream ring, holds the master stereo interleaved. It is taken *before* the monitor gain, as the record ring is, and is filled only while a reader is connected (a `Shared::streaming` flag, as `recording` is). The audio thread only ever pushes into it. When the ring is full it counts dropped samples, never blocks and never allocates (the same code path as `push_record`, `engine/mod.rs` around line 908).
- **A tap socket,** `kazoo-wall-tap.sock`, sits in the same 0700 runtime directory and takes one reader at a time. On connect, the daemon sends one JSON header line: `{"tap":1,"sample_rate":48000,"channels":2,"format":"f32le","frame":<counter>}`. After that it sends raw little-endian f32 frames, drained by an ordinary thread every 10 ms.
- **When the engine is rebuilt** (device lost, rate changed), a new header line marks the new segment, as the recorder carries a recording on into a new file. A second reader is refused with a line saying the tap is in use.
- **Why a separate socket rather than an op:** binary audio does not belong in the newline-delimited JSON stream. It would also hold up event delivery, and the control socket's reader and writer threads were never meant for 3 Mbit/s. On a local socket, 48 kHz stereo f32 is 384 KB/s, which costs nothing.
- **Why the daemon does not encode:** that keeps libopus, and a codec's CPU spikes, out of the process that renders, and keeps the daemon's job "make the wall". The bridge can change codec or bitrate without touching the daemon.

**The wire to the browser: Opus over the WebSocket, into an AudioWorklet.**

| | Latency | Robustness | Fit |
|---|---|---|---|
| Raw PCM (int16) over WebSocket into an AudioWorklet | ~40 ms on a LAN [S10]; plus the jitter buffer over the internet | 1.536 Mbit/s per listener (arithmetic). One lost TCP packet stalls everything behind it for a round trip or more, then a burst arrives [S11] | Fine for a first local test; too heavy for mobile, and costly per listener (about 40 always-on listeners per 20 TB, against ~480 for Opus at 128 kbps) |
| **Opus over WebSocket, decoded by WebCodecs `AudioDecoder` (WASM fallback), into an AudioWorklet** | Opus adds 26.5 ms at 20 ms frames and less with shorter ones [S13]. With a 60–100 ms jitter buffer, about 150–300 ms mouth to ear on a decent connection | Same TCP stalls, but at ~1/10 the data they are rarer and shorter. The client can drop old audio to catch up | **Recommended for both players and listeners.** One encode serves everyone; one socket carries control and sound |
| WebRTC | ~150–300 ms typical; adaptive jitter buffer (NetEq) [S16][S17] | Best on lossy networks: UDP, packet-loss concealment | Needs ICE/STUN/TURN, SDP editing for stereo music (`stereo=1`, `maxaveragebitrate`) [S19], and an SFU to fan out [S22]. The Rust stacks are moving fast: webrtc-rs 0.21 is a new sans-IO rewrite from early 2026 [S20]; str0m is 0.24.1 [S21]. Too much machinery for phase 1 |
| HLS / LL-HLS | LL-HLS ~2–5 s; classic HLS 20–30 s [S23] | Very robust, CDN-friendly | Listeners at scale only; "turn a knob and hear it" is impossible |
| Icecast (Ogg/Opus or MP3) | ~12 s even when tuned; browsers buffer 15–30 s [S25] | Robust; plain `<audio>` element | Simplest embed; listeners only. Icecast 2.5.0 is current (31 Dec 2025) [S24] |

**Codec settings:** Opus at 48 kHz stereo, 10 ms frames, constrained VBR around 160 kbps (music, CELT mode). Short frames keep both the codec delay and the time a lost packet blocks small. Opus accepts 2.5–60 ms frames and 6–510 kb/s [S12]; at under 10 ms it drops SILK, which music does not miss [S14]. That is 20 KB/s per listener.

**The encoder crate:** `opus` 0.4.0 (23 Aug 2026), bindings to the C libopus [S26]. Its unsafe code stays inside the dependency, so the workspace's `unsafe_code = "forbid"` still holds. Pure-Rust encoders exist but are young:
- `ruopus` 0.1.2: its decoder passes the conformance vectors [S28];
- `opus-rs` 0.1.37: recent fuzzing fixes [S27];
- `mousiki` 0.2.1: called "unstable" [S29].

Revisit them in phase 4. libopus needs a C toolchain when the bridge is built, which is a new build dependency for the workspace.

**The browser side:**
- **Decode:** use WebCodecs `AudioDecoder` where `AudioDecoder.isConfigSupported({codec:"opus", sampleRate:48000, numberOfChannels:2})` says yes. Chrome has it from 94, Firefox from 130 and Safari from 26.0 [S7][S8]. No Apple source says Safari's decoder takes Opus, and WebKit has open Opus decoder bugs [S9], so detect it at run time and fall back to `opus-decoder` from wasm-audio-decoders (85.5 KiB minified, MIT) [S15].
- **Play:** an AudioWorklet holds a ring of decoded audio and plays it out. Decoded chunks reach it over the worklet node's `MessagePort` [S36], not a SharedArrayBuffer. SharedArrayBuffer would need the page to be cross-origin isolated (COOP/COEP headers) [S34][S35], and that makes embedding in a portfolio page awkward.
- **Jitter buffer:**
  - Players: a target of 80 ms, at most 250 ms. Past the ceiling it drops to the target, cutting at a zero-crossing with a short crossfade. When the buffer runs dry it plays silence and counts an underrun.
  - Listeners: a target of 400 ms, which rides out Wi-Fi hiccups.
  - The target grows by 20 ms after each underrun and shrinks slowly after a quiet minute.
  - The page shows the current delay ("you are hearing the wall 0.3 s late"), because Tom wants the UI to say what is happening.
- **Start button:** browsers keep an `AudioContext` suspended until a user gesture [S31][S32], so the page has a big Start button. It is GeoCities-style: "CLICK HERE TO ENTER THE WALL".
- **iPhones:** set `navigator.audioSession.type = "playback"` where it exists (Safari 16.4+), or the ringer switch silences Web Audio [S37][S38]. Take the `AudioContext`'s own rate (44.1 or 48 kHz) and let the browser resample, and rebuild the context when the audio route changes [S39].

**The knob-to-ear budget for a player** (the target is well under 1 s):

| Step | Typical |
|---|---|
| Drag gathered before sending (same as the console: at most every 100 ms) | 0–100 ms |
| Browser to bridge to daemon | 10–80 ms |
| The turn's glide (0.125 beat; at 96 BPM) | 78 ms |
| Render block, master limiter lookahead, tap drain | ~15 ms |
| Opus frame and codec delay | ~15 ms |
| Daemon to bridge to browser | 10–80 ms |
| Jitter buffer (players) | 80 ms |
| Browser output | 10–30 ms |
| **Total** | **about 220–480 ms** |

The knob on screen moves at once, because the client draws the value it is sending, as the console does. Only the sound waits. Phase 1 measures this end to end rather than trusting the table.

**Plain listeners and the portfolio:** the same page and socket, with a bigger buffer. Phase 3 adds a plain HTTP Ogg/Opus stream from the bridge (`/b/<board>/live.opus`), which an `<audio>` element, a podcast app or a link in a portfolio can play with no script. That is the station's "listening" design from DESIGN.md, served by the bridge rather than the daemon. FLAC for Tom's rig can come the same way later. Icecast is not needed until listener numbers outgrow one bridge.

**Upgrades:**
- **WebTransport datagrams** remove TCP's stalls without WebRTC's setup. Chrome, Firefox and Safari 26.4 have them, so the feature has been in every major browser since March 2026 [S40][S41], and the Rust crate is `wtransport` 0.7.2 [S42]. They are worth adding in phase 4 if the measurements show stalls on real connections.
- **WebRTC** stays in reserve, if players on bad mobile networks matter more than simplicity.

### 3. The terminal-style client: ttyd now, Ratzilla later

**ttyd** serves a real terminal program to xterm.js over a WebSocket.
- **The release:** the latest is 1.7.7 (30 Mar 2024), with commits on main into Aug 2026. Since 1.7.4 it is read-only unless started with `-W`. It has basic auth (`-c`), a client limit (`-m`) and an origin check (`-O`) [S43][S44].
- **One process per connection:** ttyd starts a new copy of the command on its own terminal for each browser connection [S45]. Each visitor would therefore run their own `kazoo-wall` console, which makes two daemon connections and polls `look` 10 times a second, as `Tom`, with the console role.
- **Mouse:** xterm.js speaks SGR mouse, so the rack view's mouse works. Mouse input is still input, though, so it needs `-W` [S45][S46].
- **Verdict:** this is right for Tom alone, behind authentication, and wrong for the public.

**Ratzilla** runs Ratatui in the browser on WebAssembly.
- **Versions:** it is now `ratatui/ratzilla`, at 0.3.1 (8 Jun 2026). It depends on ratatui `^0.30.1` with default features off, so no crossterm [S47][S48].
- **Backends** [S49]:
  - **WebGL2** (through beamterm): the fast one, under 1 ms a frame, with full Unicode through the dynamic font atlas, so braille cables and dials work. Its mouse support is basic: moved, button down and button up.
  - **Canvas:** full mouse, but not 60 fps on large grids.
  - **DOM:** full mouse, the slowest.
- **Input** arrives through `on_key_event` and `on_mouse_event` as Ratzilla's own types, which are not crossterm's. **There is no scroll-wheel event** [S50].
- **The loop:** `draw_web` redraws on `requestAnimationFrame` with no threads [S51].
- **Ratatui itself:** 0.30 split out `ratatui-core`, made crossterm optional, and gave `Backend` an associated error type, so draw code generic over the backend runs unchanged [S52].
- **Known problems:** wide glyphs, the first frame's size, and font size [S53].

**What porting the console to Ratzilla takes:**
1. **A client-only build of `kazoo-wall`.** A `daemon` feature (on by default) gates `daemon`, `engine`, `dsp`, `adapters`, `listen`, `store`, `paths` and the socket client, with cpal, kazoo-core's IPC, kazoo-speech and signal-hook behind it. With it off, what is left is the protocol types, `catalogue`'s plain types (`Curve`, `JackLaw`, `Signal`, `GROUPS`, `group`), `format`, `patch::{arrange_rows, place_for}`, and the flood constants. `catalogue.rs` imports `dsp::Module` for its builders, so the kind table's data must be split from its builders. A separate `kazoo-wall-protocol` crate is the tidier end state, but features are the smaller first step.
2. **The console's own input types.** A small `Input` enum for keys and mouse replaces crossterm's types in `app/keys.rs` and `app/mouse.rs`, with one conversion from crossterm and one from Ratzilla. It is unproven whether crossterm's event types even compile for `wasm32-unknown-unknown`, and owning the types removes the question.
3. **Time:** the `App` already takes `now` as a parameter almost everywhere. Its `Instant`s become a small clock type, which is `std::time::Instant` natively and `web-time` 1.1.0 (`performance.now()`) in the browser [S54].
4. **A link that is not threads:** `worker::Connection` becomes a trait with two implementations: today's threads with Unix sockets, and a browser WebSocket to the bridge whose callbacks push `Reply`s into a queue that `draw_web` drains each frame.
5. **The scroll wheel:** add a DOM `wheel` listener beside Ratzilla (or contribute the event upstream). Every wheel gesture already has a key equivalent.
6. **Seat and role** come from the bridge's `hello`, not the `SEAT` constant.

That is about two weeks of careful work, and the console's own tests keep passing on the native build throughout.

**Recommendation:**
- **Phase 1:** ttyd, for Tom only. It is one day of work and gives him the real console in a browser tab:
  - Add `--seat <name>`, `--watch` and `--no-launch` to the console (today it is always `Tom`, a console, and starts a daemon).
  - Run ttyd bound to `127.0.0.1` with `-W -m 2 -O`, reached through the bridge's authenticated reverse proxy or a Cloudflare Access-protected hostname.
- **Phase 4:** Ratzilla, as the public "terminal" view of any board. It goes through the bridge with the visitor's role, beside the graphical rack, using the WebGL2 backend plus a wheel listener. It is the same `App`, so the two terminals never drift apart.

### 4. The graphical client

**The stack: plain TypeScript, SVG and a handful of custom elements, bundled with esbuild. No framework.**

| Option | For | Against |
|---|---|---|
| **Plain TS + web components (recommended)** | The rack is mostly pointer handling on SVG plus "apply this event to this element"; that needs no framework. Tiny pages suit a GeoCities site Tom will edit by hand (drop a gif in, add a `<marquee>`). Nothing to upgrade. Custom elements (`<kazoo-knob>`, `<kazoo-jack>`) give reuse without a runtime. TypeScript is already in the estate's toolchain | Hand-written DOM updates; discipline needed to keep state in one place |
| Svelte 5 (5.57.2) / Solid (1.9.17; 2.0 at RC) [S58] | Pleasant reactivity, small output | Another build and runtime to learn and keep current for little gain on an SVG rack; Solid 2.0 is about to change things. patchcab, a Svelte modular synth, has been dormant since 2023 [S57] |
| Leptos 0.8 / Dioxus 0.7 / Yew 0.23 (Rust/WASM) [S59] | Shares the protocol types with the daemon; Rust only | Larger downloads before the first knob shows; pointer and SVG drag code is clumsier through signals; slower to iterate on looks. The Rust-in-browser need is already met by the Ratzilla terminal, which reuses real console code rather than rewriting it |

**Keeping the types honest:** a test in `kazoo-wall` writes example JSON for every request, response and event. The TypeScript client's tests read those same files, so a protocol change that breaks the browser fails a test. Generated TypeScript types (a derive such as ts-rs) are an option for later; they are not needed to start.

**Drawing:**
- **SVG,** one `<g>` per plate. The layout comes from `snapshot.rack` (the shared rows). Plate sizes come from the console's rules: knob columns three knobs high, and jack rows. Only plates near the view are in the DOM, so an unlimited wall stays fast. An overview density draws small blocks with their jacks, as the console's overview does.
- **Knobs** are SVG: a black cap with a white pointer sweeping from 7 o'clock to 5 o'clock as the console's dial does, a ring of detents for stepped knobs, and a small brass mark where a glide is heading. Values show as `display` text.
- **Cables** are an SVG layer *above* the plates:
  - Each cable is one quadratic path using the console's sag rule (midpoint, hung below the lower end by a quarter of the length plus a little).
  - It is drawn twice: a dark shadow offset downward, then the coloured cable with round caps. Plugs are small circles at the jacks.
  - The colour is the fingerprint dye where there is one, or the cable's colour by number (the console's ten cable colours) where there is not. That follows the rule of no seat colours: a view decides how to show dye.
  - Cables reroute live as plates move.
  - A gentle swing (a few points of verlet rope settling over a second when a plate moves or a cable is dropped) is a phase 4 delight, not a need. VCV Rack gets believable cables with the plain curve alone [S30].
- **The header:** tempo, beat and bar, the clock source, the seats, the master meter (an SVG bar with peak hold), and the listen words.
- **The live log** at the bottom shows each change's `summary`.

**Controls,** copying the console's mouse gestures:
- **Knob:**
  - `pointerdown` takes pointer capture [S60], and `touch-action: none` stops touch devices from scrolling instead.
  - Vertical drag turns the knob across its travel, following the catalogue's curve. That is a port of `tui/knob.rs` to TypeScript, checked against the same test values. Shift gives fine control.
  - The wheel gives fine turns, and double-click sends the knob back to its default.
  - Turns are sent at most every 100 ms with a 0.125-beat glide, plus the final value on release. They go through a client copy of the flood guard, as the console's `flood.rs` does, so a long sweep never hits `slow_down`.
- **Cable:**
  - Press on a jack, and a cable hangs from it and follows the pointer with live sag.
  - Release over a compatible jack, or a knob (its jack), to `patch`. Release anywhere else to drop it.
  - Hovering shows which jacks would take the cable.
  - Right-click, or a long press on touch, unplugs a cable at the input or knob it is plugged into.
  - `amount` is a small knob on the cable's plug, shown when the plug is selected.
- **Plates:** drag the title bar to move a plate (`arrange`). Drag the rack to pan, and pinch or ctrl-wheel to zoom. Click the blank panel at the end of a row to add a module there, from a picker sorted by `GROUPS` and filtered as you type.
- **Undo:** an undo button on each log line the player may undo (see the open questions).
- Every gesture has a keyboard path, and focus rings are visible.

**The GeoCities shell:**
- **The page:** the rack sits in the middle, framed like a `<table border=3>`. Around it are a tiled background and Tom's gifs, which go in `assets/tom/` and are listed in one HTML file he can edit. Above it, a WordArt title.
- **The WordArt** is made with CSS: gradient-filled text, a skew or arc, and a hard drop shadow, in a font he chooses. `css3wordart` shows the idea but has no licence, so it is a reference to look at, not code to copy [S64].
- **98.css** (MIT) for dialogs and buttons, if he wants that era's chrome [S63].
- **The extras:** the spinning "under construction" light by the board's name, a `<marquee>` of the live log, and a hit counter that is the real listener count. The boards directory is a **webring**: "« previous board | random | next board »".
- **The rack stays legible:** the chaos lives around it, never on top of it. The rack keeps the console's cream-and-black palette and the dark rack behind it.
- **The gifs:** GifCities (the Internet Archive's GeoCities gif search, relaunched with semantic search in June 2025) is the place to find them [S61][S62]. Its about page states no licence, and the original authors probably still own the gifs, so the choice is Tom's (see the open questions).
- **Animation:** `prefers-reduced-motion` turns the marquees and spinners off. Gifs are swapped for a still frame where a still is provided.

**The player's view:** a listener sees the same page with the controls dimmed, plus a "pick a name and play" button if the board lets people play.

### 5. Boards

- **A board** is a slug, a title, an owner, a policy (who may play) and a daemon with its own directories under the bridge's home, for example `~/.kazoo/boards/<slug>/{run,state,saves}`. The bridge keeps the board list in a small JSON file, written by temp file, fsync and rename, as `store.rs` writes the patch.
- **Starting a board:** from the seed patch, or, once save/load lands, from a copy of another board's current patch: save there, then load the save into the new daemon. Starting a board is open to whoever the open questions decide.
- **Flicking between boards** keeps one WebSocket per tab and moves it to the new board. The audio crossfades over half a second from the old board's buffer to the new one's, like turning a radio dial. A short burst of tuning noise between them is optional.
- **When nobody is there:** the wall "plays forever", but on a paid server an empty board is CPU spent on no one. The bridge can stop a board's daemon after N hours with no listeners and start it again on the next visit. Its patch is saved, and its clock carries on from the saved tempo; the beat restarts. Whether boards may sleep is Tom's call.
- **Board zero** is Tom's own wall on his Mac. On the Mac, the bridge simply points at the existing socket and tap and does not supervise that daemon: Tom's wall keeps its own life, desk link and `say` voice.
- **Limits:**
  - A board count per server, set by measured CPU.
  - A module ceiling per board, set by the owner and checked before the compute guard.
  - Active players per board (24 before the relay).
  - Listeners per board (bandwidth).

## Protocol additions

Each is optional to old clients, in the style the protocol already uses: `#[serde(default)]` fields, new events read as `Unknown` by old builds. kazoo-mcp and the console keep working unchanged.

| Phase | Addition | Why |
|---|---|---|
| 1 | **Stream tap:** `kazoo-wall-tap.sock`, one reader, JSON header then f32 frames; `Shared::streaming`; a stream ring taken before the monitor. Not part of the NDJSON protocol | Audio for the web without touching the control socket |
| 2 | **`hello` with `"guest": true`**: the daemon refuses `record`, `monitor`, `shutdown`, `speak` and `tempo` to a guest, whatever the bridge does, and lists guests apart: `seats` stays as it is, and a new `guests` count is added to `look` and `HelloResult`. The `seat` event gains `guest: true` | Defence in depth: the daemon itself refuses strangers the ops that reach the disk, the desk or a speaker. kazoo-mcp can then tell Claude seats "3 web guests are playing" instead of passing on strangers' names |
| 2 | **`look` with `"only": ["knobs","levels","beat","listen"]`**: a light snapshot without modules' ports, cables, fingerprints or timing | Cuts the cost of the bridge's 10 Hz poll on large walls. Until then, a full `look` every 100 ms is what the console already costs |
| 3 | **A load event** (if the save/load work does not add one): `{"event":"loaded","revision":N,"name":"…"}` | Tells every client to take a fresh `look`. Without it, the bridge relies on the revision jump |
| 4 | **A relay role:** `hello` with `"relay": true` (allowed only on the local socket, as `console` is), after which each request may carry `"as": "<seat>"`, `"guest": true`; the daemon keeps a flood guard and seat entry per `as` | Many players over one connection: lifts the 64-connection ceiling and the two threads per player |

Nothing else is needed: the browser speaks the wall's own requests, and the bridge passes the daemon's events on as they are.

## Security and abuse

**The trust boundary is the bridge.** No daemon socket is ever reachable from a network. The tap and control sockets stay in 0700 directories owned by the bridge's user.

**Roles:**

| Role | How | May |
|---|---|---|
| Listener | anyone with the board's link | hear, see, read the log |
| Player | an invite link, or "pick a name" on an open board (Tom decides which) | the listener's rights, plus `turn`, `patch`, `unpatch`, `add`, `remove`, `arrange`, and `undo` as decided below |
| Owner | a long-lived owner token for each board (Tom's for his boards) | the player's rights, plus `tempo` on boards with no desk, save and load (phase 3), the board's policy, and freezing the board to watch-only |
| Tom's console | ttyd, behind basic auth or Cloudflare Access, bound to localhost | everything the console does today |

**Invitations:**
- **The token:** an HMAC-SHA256 over the board, the role, an expiry and a nonce, with a key kept in the bridge's state directory (0600). It travels in the URL fragment, so it never reaches server logs or `Referer` headers. The page sends it in the first WebSocket message.
- **Revoking:** a revoked nonce goes on a deny list, and a new key revokes every token at once.
- **No accounts:** a player's name is chosen per visit.

**Seat names:**
- **The form:** the bridge builds a guest's seat name as `<handle>.web`, where the handle is 1–20 characters of the wall's own charset. That keeps it inside `valid_name`, and every log line shows where a change came from.
- **Reserved names:** a guest may never claim `Tom`, `kazoo-wall`, `kazoo-boards`, any name a Claude seat uses (read from the board's `seats` and a configured list), or a name already in use on the board.
- **A word filter** on handles and on module display names, with the list in a file Tom can edit, if he wants one (see the open questions).
- **Claude seats:** guest names reach them only as a count, through the phase 2 guest flag. Until then, guests' join and leave events are dropped by the bridge from what it forwards (it does not control kazoo-mcp), and kazoo-mcp is changed to coalesce seats ending in `.web` into a count.

**What is never open to a guest:**
- `record`, `monitor`, `shutdown` and `speak`. Speak means a stranger's text spoken aloud, and it uses `say` on Tom's Mac. It can be opened per board later if Tom wants it.
- `tempo` on a desk-linked wall.
- Any module kind that reaches the host: the planned `acceptor`, `world` and sampler `rec`/mic sources. The bridge holds a deny list of kinds, and phase 2's guest flag repeats it in the daemon.
- save and load.

**Rate limits,** on top of the wall's own per-seat flood guard:
- **Connections:** at most 4 WebSockets per IP, and new connections from one IP limited with `governor` 0.10 (via `tower_governor` 0.8 for the HTTP side) [S5]. Behind Cloudflare, the client IP is `CF-Connecting-IP`, trusted only because the origin accepts nothing except the tunnel.
- **Changes:**
  - Per player, the bridge mirrors the wall's bucket (30, then 3 a second) and answers `slow_down` itself, without bothering the daemon.
  - Per board, all guests together share a second bucket (for example 60, then 6 a second), so a crowd cannot churn the wall faster than people can hear it.
  - Adds: at most 8 modules per guest per 10 minutes, and guests stop adding at the owner's module ceiling.
- **Messages:** text frames are capped at 4 KiB. That is far below axum's 64 MB default, which must be lowered [S2].
- **Origin:** axum does not check `Origin`, so the bridge checks it against the site's own origins and the portfolio's [S2].
- **Slow readers:** each browser has a bounded queue. Audio frames are dropped first, then the connection is closed, as the daemon closes a stuck connection. A ping every 30 s also keeps Cloudflare's idle timeout of about 100 s from cutting the socket [S6].
- **Idle players:** a player's seat connection closes after 5 minutes without a change.

**What is already safe:**
- Knob ranges carry the wall's safety caps, so no input can push a knob past them.
- The master chain's −1 dBTP limiter and NaN guard protect every listener's ears and speakers.
- `full` from the compute guard keeps a board from being added to death.
- Change summaries carry only sanitised ids, names and numbers.

**Undo storms and griefing:**
- The owner can freeze a board (everyone becomes a listener) and use the existing undo on any change.
- With save/load, the owner can load the auto-save from before the trouble.
- The bridge can drop a guest's player rights for the rest of the visit.

**Privacy:** no cookies. IPs are kept only in memory for rate limiting. The bridge logs board, role and seat name, never IPs or tokens.

**Embedding:**
- No SharedArrayBuffer, so no COOP/COEP is needed.
- `Content-Security-Policy: frame-ancestors` names the portfolio's origin, and the default `script-src 'self'` allows only the site's own scripts.
- WASM decoding needs `'wasm-unsafe-eval'` in `script-src` on the fallback path.

## Hosting

**Tom's Mac:**
- **For it:** his real wall is right there, desk-linked, with `say`. There is no new cost.
- **Against it:**
  - It is his working machine, and a public service there widens what strangers can poke at.
  - It sleeps, travels, and has a home uplink, and each listener costs about 160 kbps up.
  - Exposing it means a Cloudflare named tunnel (no open ports). Quick tunnels have no SLA and a random name [S7a].
  - Cloudflare's terms restrict video and "disproportionate" large files on Free, Pro and Business plans. Audio is not named, so continuous public audio through a free tunnel is a grey area [S7b].
  - The bridge's encoder and the boards' renders take CPU from a machine that has had load problems before.
- **Good for:** phase 1, with Tom and invited friends, bound to localhost or the LAN, or through a tunnel behind Cloudflare Access.

**A small VPS:**
- **The price:** Hetzner's CX23 is €5.49 a month (and the ARM CAX11 €5.99), after a price rise on 15 Jun 2026. Both include 20 TB of traffic in the EU, and overage is €1/TB [S65][S66].
- **What that carries:** about 480 always-on listeners at 128 kbps, or a great many more casual ones. The bandwidth arithmetic is in [S68]. Fly.io instead charges $0.02/GB egress in North America and Europe [S67].
- **Speech:** a Linux server has no `say`, so `speak` modules answer `internal` there. The bridge hides the speech kinds on such boards.
- **The build:** today `kazoo-wall` always links cpal, which on Linux needs ALSA's headers to build even when it renders headless. A `device` feature that gates cpal, and leaves only the headless loop, makes the server build clean and smaller.
- **Real-time:** for the audio threads, the service needs `LimitRTPRIO`. Boards per core are capped.
- **Good for:** public boards from phase 3, and the portfolio.

**Recommendation:**
- **Phase 1 and 2:** on Tom's Mac, private, the bridge on `127.0.0.1` (Tom's browser) or behind Cloudflare Access for invited players.
- **Phase 3:** public boards on a VPS, with the pages behind Cloudflare and the audio straight from the VPS to keep clear of the terms question.
- **Board zero on the public site:** Tom's own wall can appear there through a small **board agent** on the Mac. It connects *out* to the VPS bridge over an authenticated WebSocket and carries the watcher stream, player requests and Opus packets, so nothing on the Mac listens on a network (phase 4).

## The private wall in a browser (later)

**"Your own private wall"** does not need the engine in the browser at first. A private board on the server (an unlisted slug, with the owner as the only player) gives it now. Running the engine in the visitor's own browser is a later mode, worth a spike first.

**What blocks it in the code today:**

| Blocker | Where | How to gate it |
|---|---|---|
| cpal device output | `daemon/audio.rs`, `daemon/control.rs` | `device` feature. The engine is already driven by whoever calls it (the headless loop does), so in the browser an AudioWorklet calls it for each 128-frame block. cpal 0.17 and later also has an `audioworklet` backend (nightly, `+atomics`, build-std, COOP/COEP) [S54a] |
| kazoo-core IPC (`HubLinkAudio`, `HubMessage`, `TransportFollower`) | `engine/mod.rs`, `daemon/control.rs`, `dsp/mod.rs` (`beats_in`) | `desk` feature. The engine takes `Option<HubLinkAudio>` already; make the type and the desk branch conditional. The pure pieces it uses (`sanitize_sample`, `beats_in`, the transport follower's maths) move to a kazoo-core feature with no cpal, midir or symphonia |
| `std::time::Instant` (panics on `wasm32-unknown-unknown`) | the control side throughout (`daemon/wall.rs` flood and settling, `listen.rs`, the link) | The control side already takes `now` as a parameter in most places; finish that, and supply the time from `web-time` [S54] on the main thread. Inside the worklet, use its frame counter, never a clock |
| Threads | listen thread, recorder writer, TTS worker, headless loop, socket threads | No threads on `wasm32-unknown-unknown`. The engine and the control side must share memory for the existing SPSC rings, because commands carry boxed modules. That means the atomics build (nightly, build-std, COOP/COEP) [S55], with the control side in a Worker and the engine in the worklet. Running the control side inside `process()` would break the realtime rule. The listen analysis and recorder run on the Worker's timers |
| macOS `say` | `kazoo-speech/src/tts/mod.rs`, `daemon/speech.rs` | `tts` feature; speech kinds left out of the browser's catalogue. The browser's `speechSynthesis` cannot render into a buffer the wall can patch |
| Files and sockets | `store.rs`, `paths.rs`, `daemon/server.rs`, `protocol/client.rs` | `daemon` feature; the browser keeps the patch in IndexedDB or OPFS |
| `unsafe_code = "forbid"` | workspace | A hand-exported `#[no_mangle]` function trips the lint [S56]. Exports must come from wasm-bindgen 0.2.105 or later, which also fixed `TextDecoder` inside worklets [S55], and a spike must show that its macros pass `forbid`, or Tom grants a narrow, written exception |
| CPU | the render | The budget is 2.67 ms per 128 frames at 48 kHz [S57a]. Measure a 40-module wall in a browser before promising anything |

**Precedents:**
- **Glicol** runs a Rust engine in an AudioWorklet with SharedArrayBuffer and COOP/COEP; it calls itself experimental [S58a].
- **Cardinal** runs VCV Rack in the browser through Emscripten [S56a].

## Phased plan

Sizes are focused working days for one agent, including tests and an Opus review with every finding fixed. Each phase passes the full chain the estate requires: fmt, clippy pedantic with `-D warnings`, ast-grep, `cargo nextest run`, `cargo test --doc`, and the TypeScript tests.

### Phase 1: hear it, see it, turn knobs (Tom, on his Mac)

What Tom gets: he opens `http://127.0.0.1:<port>/` (or a Cloudflare Access hostname), clicks Start, hears his wall, sees the rack with its hanging cables and moving knobs, and turns knobs with the mouse. A terminal tab shows the real console.

| # | Work | Size |
|---|---|---|
| 1.1 | **Stream tap in the daemon:** the stream ring (before the monitor), `Shared::streaming`, the tap socket with its header, a drain thread, segment headers across engine rebuilds and rate changes, and a dropped-sample count. Tests: no allocation in the render path with the tap on (`assert_no_alloc`); a reader sees a header and continuous frames; a rebuild gives a new header; a second reader is refused; a reader that stalls drops samples and never blocks | 2 |
| 1.2 | **`kazoo-boards` skeleton:** axum server; one board pointing at an existing socket and tap (board zero); the watcher link with reconnect; `look` every 100 ms and the motion diff; fan-out with bounded queues; static files; token from a 0600 file in the state directory, checked on the first message; `Origin` check; 4 KiB frame cap; bound to `127.0.0.1` by default. Tests against a real headless daemon, as kazoo-mcp's tests do | 4 |
| 1.3 | **Audio path in the bridge:** tap reader, resampling to 48 kHz when needed, Opus encode (10 ms, ~160 kbps), binary frames with a sample counter, drop-oldest for slow browsers. Tests: an encoded sine decodes back at the right pitch and level; a rate change mid-stream; a stalled browser does not hold up the others | 2 |
| 1.4 | **Browser audio:** Start button; AudioWorklet with the jitter buffer (player and listener targets); WebCodecs decode with the `opus-decoder` fallback; `audioSession` on iOS; an on-page delay readout and underrun count. Tests: buffer logic in TypeScript unit tests | 3 |
| 1.5 | **Rack v1 in SVG:** plates from `rack` rows, knobs, jacks, hanging cables (read-only in this phase), header, meter, listen words, log; knob drag, wheel and double-click with the curve port of `knob.rs`; held turns at most every 100 ms with a 0.125-beat glide and the client flood guard; the console's palette. Players act as seat `Tom web` (a console-free seat) | 5 |
| 1.6 | **GeoCities shell v1:** tiled background slot, `assets/tom/` for his gifs with an editable list, CSS WordArt title, under-construction spinner, marquee log, reduced-motion handling | 1 |
| 1.7 | **Terminal tab:** console flags `--seat`, `--watch`, `--no-launch`; ttyd on `127.0.0.1` with `-W -m 2 -O`, proxied by the bridge behind the same token | 1 |
| 1.8 | **Measure:** knob-to-ear round trip (a test page turns a VCA's gain and times the onset in the decoded audio), bridge CPU, browser CPU on a laptop and a phone. Write the numbers into this document | 1 |

**About 19 days.** Risks:
- libopus adds a C build dependency; check how the `opus` crate finds or builds it on macOS and Linux first.
- Safari's Opus decoding is unconfirmed, so the WASM fallback may be what Safari users get.
- The monitor question (open question 2) decides whether the web hears the wall while Tom's speakers are silent. The tap follows the recorder (before the monitor) unless he says otherwise.
- Tom's Mac's load: the encoder is light (one encode a board), but the bridge is one more process.

### Phase 2: play it properly (invited players)

| # | Work | Size |
|---|---|---|
| 2.1 | Cable drag with live sag, compatible-jack hints, patch on drop, unplug by right-click or long press, `amount` on the plug | 3 |
| 2.2 | Add from blank panels with the grouped, filtered picker; remove with confirmation; move plates by title (`arrange`) with the held optimistic move; pan and zoom; overview density | 4 |
| 2.3 | Fingerprint dye on plates and cables, with no seat colours; undo buttons on the log (as decided) | 2 |
| 2.4 | Roles and invites: HMAC tokens in the fragment, deny list, owner token; guest seat names (`<handle>.web`), reserved names, optional word filter; one daemon connection per active player, opened on first change, closed after 5 idle minutes | 3 |
| 2.5 | Abuse controls: op allowlist and kind deny list, per-IP connection cap and `governor`, per-player and per-board buckets, add quotas, freeze | 2 |
| 2.6 | Protocol: `hello` `guest`, the daemon's guest refusals, `guests` count; `look` `only`. kazoo-mcp reports guests as a count. Tests in `protocol/tests.rs`, `daemon/wall/tests.rs`, `tests/daemon.rs`, `kazoo-mcp/tests/seat.rs` | 3 |
| 2.7 | Touch: knobs, cables and panning on a phone and a tablet | 2 |

**About 19 days.** Risks:
- Touch gestures conflicting with page scroll.
- Strangers' names and notices reaching Claude seats before 2.6 lands, so 2.6 ships with or before 2.4.
- A crowd hitting the compute guard.

### Phase 3: boards and the public site

| # | Work | Size |
|---|---|---|
| 3.1 | Board supervisor: per-board directories, spawn `kazoo-wall serve --no-audio --no-desk`, restart with backoff, stop, the board list file, optional sleep when empty | 3 |
| 3.2 | Boards directory as a webring; flicking between boards with an audio crossfade; listener counts as the hit counter | 2 |
| 3.3 | Start a board (seed, or a copy through save/load); owner controls (policy, freeze, ceiling, save and load, tempo) | 3 |
| 3.4 | `device` feature gating cpal; Linux build; systemd unit with `LimitRTPRIO`; deploy to a VPS; Cloudflare in front of the pages | 3 |
| 3.5 | Plain HTTP Ogg/Opus stream per board for `<audio>` and the portfolio; an embeddable listen-only widget page with `frame-ancestors` | 2 |
| 3.6 | Measure CPU per board on the VPS and set the board cap | 1 |

**About 14 days.** Risks:
- CPU per board on a shared vCPU.
- `SCHED_FIFO` limits in a VPS.
- Cost if boards never sleep.

### Phase 4: the terminal on the web, more players, better transport

| # | Work | Size |
|---|---|---|
| 4.1 | `kazoo-wall` client-only build (features, catalogue data split from builders); console `Input` types; clock type; link trait | 4 |
| 4.2 | Ratzilla build of the console (WebGL2, wheel listener), served per board with the visitor's role | 4 |
| 4.3 | Relay role in the protocol; the bridge multiplexes players over one connection per board | 3 |
| 4.4 | Board agent on Tom's Mac: outbound authenticated link carrying board zero to the VPS | 3 |
| 4.5 | WebTransport datagram audio, if phase 1's measurements showed stalls; cable swing | 3 |

**About 17 days.** Risks:
- Ratzilla's young edges (wide glyphs, first-frame size).
- Upstream changes in Ratzilla and ratatui.

### Phase 5: the private wall in the browser (spike first)

- **A spike (5 days):** the engine alone, with the seed patch, rendered in an AudioWorklet through cpal's `audioworklet` backend or wasm-bindgen. Measure the CPU, and prove that `forbid(unsafe_code)` holds.
- **Then decide.** If it is worth it, the feature split for `desk`, `tts`, `daemon` and `device`, a Worker-hosted control side, and IndexedDB storage come to about three weeks.
- **Risks:** nightly-only builds; COOP/COEP make the page hard to embed; DSP cost in WASM.

## Open questions only Tom can answer

1. **Who may play?** Anyone who picks a name, only people with an invite link, or each board's owner chooses?
2. **The monitor and the web:** should the web stream play while his speakers are silent (`m` off)? Is "silent by default" about his room only, or about the wall being heard anywhere?
3. **The first public board:** should it be his own studio wall (desk-linked, on his Mac), or a fresh board on a server, with his own wall kept private?
4. **Undo:** may web players undo anyone's change (his and the Claude seats' included), or only their own?
5. **Speech:** may web players give a `speak` module words, spoken aloud to every listener? Never, on some boards, or with a word filter?
6. **Tempo:** may web players change a board's tempo? On his desk-linked wall it moves the whole studio.
7. **Claude seats and guests:** should Claude seats hear web guests by name, as a count, or not at all?
8. **Empty boards:** keep playing forever with nobody listening (CPU and money), or sleep when empty and wake on the next visit?
9. **Starting boards:** who may start one: anyone, invited people, or only him? How many at most?
10. **The portfolio:** which site and domain? Embed the wall in a page or link out to it? Listen-only from the portfolio, or playable?
11. **The gifs:** is he happy using found GeoCities gifs with credit and a takedown contact, or should they be drawn new in the same style? Which WordArt style and font?
12. **Money and names:** is a server at about €6 a month fine, and what domain should kazoo boards live on?
13. **Names on show:** should guests' handles and module display names pass a word filter, and whose list?

## Sources

All fetched or checked on 9 Oct 2026; the date given is the source's own where it has one.

- [S1] axum 0.8.9, 14 Apr 2026: https://crates.io/crates/axum
- [S2] axum `WebSocketUpgrade` defaults (64 MB messages; no Origin check): https://docs.rs/axum/latest/axum/extract/ws/struct.WebSocketUpgrade.html
- [S5] governor 0.10.4 (16 Dec 2025), tower_governor 0.8.0 (14 Aug 2025): https://github.com/benwis/tower-governor
- [S6] Cloudflare WebSockets (docs updated 14 Aug 2026): https://developers.cloudflare.com/network/websockets/ ; ~100 s idle timeout: https://websocket.org/guides/infrastructure/cloudflare/
- [S7a] Cloudflare quick tunnels' limits: https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/
- [S7b] Cloudflare video and large-file policy (updated 25 Aug 2026): https://developers.cloudflare.com/fundamentals/reference/policies-compliances/delivering-videos-with-cloudflare/
- [S7] `AudioDecoder` support (Chrome 94, Firefox 130, Safari 26.0): https://caniuse.com/mdn-api_audiodecoder
- [S8] WebKit features in Safari 26.0, 15 Sep 2025: https://webkit.org/blog/17333/webkit-features-in-safari-26-0/
- [S9] WebKit Opus decoder bugs (Dec 2024, Jan 2025): https://bugs.webkit.org/show_bug.cgi?id=284075 , https://bugs.webkit.org/show_bug.cgi?id=286526
- [S10] JackTrip over WebRTC with AudioWorklets, ~40 ms on a LAN: https://iris.polito.it/bitstream/11583/2974022/1/JackTrip_WebRTC_Journal.post-print.pdf
- [S11] TCP head-of-line blocking for live audio: https://livekit.com/blog/why-webrtc-beats-websockets-for-voice-ai-agents
- [S12] Opus frame sizes and bitrates; libopus 1.6.1, 14 Jan 2026: https://opus-codec.org/
- [S13] Opus algorithmic delay 26.5 ms at 20 ms frames: https://en.wikipedia.org/wiki/Opus_(audio_format)
- [S14] Opus frames under 10 ms are CELT only: https://ffmpeg.org/pipermail/ffmpeg-trac/2023-April/065709.html
- [S15] wasm-audio-decoders `opus-decoder`: https://github.com/eshaz/wasm-audio-decoders
- [S16] WebRTC jitter buffer and typical latency: https://omr.it.com/blog/jitter-buffer-tuning-webrtc-voice-20-200ms/
- [S17] NetEq: https://webrtchacks.com/how-webrtcs-neteq-jitter-buffer-provides-smooth-audio/
- [S19] Opus SDP parameters (`stereo`, `maxaveragebitrate`): https://www.iana.org/assignments/media-types/audio/opus
- [S20] webrtc-rs 0.21.0, 19 Sep 2026, on the sans-IO `rtc` crate: https://github.com/webrtc-rs/webrtc , https://github.com/webrtc-rs/rtc
- [S21] str0m 0.24.1, 3 Oct 2026: https://github.com/algesten/str0m
- [S22] LiveKit server: https://github.com/livekit/livekit
- [S23] LL-HLS latency: https://bitmovin.com/live-low-latency-hls/
- [S24] Icecast 2.5.0, 31 Dec 2025: https://icecast.org/news/icecast-release-2_5_0/
- [S25] Icecast latency reports (Aug 2013, Feb 2015): https://lists.xiph.org/pipermail/icecast/2013-August/012608.html , https://lists.xiph.org/pipermail/icecast/2015-February/013138.html
- [S26] `opus` crate 0.4.0, 23 Aug 2026: https://crates.io/crates/opus
- [S27] `opus-rs` 0.1.37, 7 Oct 2026: https://docs.rs/crate/opus-rs/latest
- [S28] `ruopus` 0.1.2, 1 Jul 2026: https://docs.rs/crate/ruopus/latest
- [S29] `mousiki` 0.2.1, 26 Mar 2026: https://lib.rs/crates/mousiki
- [S30] VCV Rack cable drawing (one quadratic curve, slack-pushed midpoint): https://github.com/VCVRack/Rack/blob/v2/src/app/CableWidget.cpp
- [S31] Chrome autoplay policy and Web Audio: https://developer.chrome.com/blog/autoplay
- [S32] MDN autoplay guide (modified 10 Sep 2026): https://developer.mozilla.org/en-US/docs/Web/Media/Guides/Autoplay
- [S34] SharedArrayBuffer needs cross-origin isolation (MDN, 10 Feb 2026): https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer
- [S35] COOP/COEP and embedding (web.dev, 21 Jun 2022): https://web.dev/articles/coop-coep
- [S36] `AudioWorkletNode.port` (MDN, 26 Jul 2024): https://developer.mozilla.org/en-US/docs/Web/API/AudioWorkletNode/port
- [S37] `navigator.audioSession`, Safari 16.4+: https://caniuse.com/mdn-api_navigator_audiosession ; MDN (27 Aug 2026): https://developer.mozilla.org/en-US/docs/Web/API/AudioSession
- [S38] The silent switch and Web Audio: https://bugs.webkit.org/show_bug.cgi?id=237322 , https://adactio.com/journal/19929
- [S39] Safari sample-rate and route-change bugs: https://bugs.webkit.org/show_bug.cgi?id=258864 , https://bugs.webkit.org/show_bug.cgi?id=274507
- [S40] WebTransport support: https://caniuse.com/webtransport ; Safari 26.4, 24 Mar 2026: https://webkit.org/blog/17862/webkit-features-for-safari-26-4/
- [S41] WebTransport Baseline, 23 Apr 2026: https://webrtc.ventures/2026/04/webtransport-is-now-baseline-what-it-means-for-real-time-media/
- [S42] `wtransport` 0.7.2, 11 Aug 2026: https://crates.io/crates/wtransport
- [S43] ttyd releases (1.7.7, 30 Mar 2024; main active to Aug 2026): https://github.com/tsl0922/ttyd/releases
- [S44] ttyd 1.7.4 made read-only the default: https://github.com/tsl0922/ttyd/releases/tag/1.7.4
- [S45] ttyd spawns one process per connection; input dropped unless writable: https://github.com/tsl0922/ttyd/blob/main/src/protocol.c
- [S46] xterm.js 6.0.0, 22 Dec 2025 (mouse modes incl. SGR): https://github.com/xtermjs/xterm.js/releases/tag/6.0.0
- [S47] Ratzilla 0.3.1, 8 Jun 2026: https://github.com/ratatui/ratzilla/releases , https://crates.io/crates/ratzilla
- [S48] Ratzilla's ratatui `^0.30.1`, default features off: https://github.com/ratatui/ratzilla/blob/main/Cargo.toml
- [S49] Ratzilla backends compared: https://github.com/ratatui/ratzilla/blob/main/src/backend/mod.rs
- [S50] Ratzilla input events (no wheel): https://github.com/ratatui/ratzilla/blob/main/src/event.rs
- [S51] Ratzilla render loop: https://github.com/ratatui/ratzilla/blob/main/src/render.rs
- [S52] Ratatui 0.30 highlights (0.30.0, 26 Dec 2025): https://ratatui.rs/highlights/v030/
- [S53] Ratzilla open issues: https://github.com/ratatui/ratzilla/issues
- [S54] `web-time` 1.1.0: https://docs.rs/crate/web-time
- [S54a] cpal `audioworklet` backend (PR merged 14 Nov 2025; cpal 0.17.0, 20 Dec 2025; latest 0.18.2, 16 Aug 2026): https://github.com/RustAudio/cpal/pull/958 , https://github.com/RustAudio/cpal/blob/master/CHANGELOG.md
- [S55] wasm-bindgen's AudioWorklet example and the `TextDecoder` fix (0.2.105, 27 Oct 2025): https://wasm-bindgen.github.io/wasm-bindgen/examples/wasm-audio-worklet.html , https://github.com/wasm-bindgen/wasm-bindgen/pull/4703
- [S56] The `unsafe_code` lint covers `no_mangle` and `export_name`: https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html
- [S56a] Cardinal (VCV Rack in the browser): https://github.com/DISTRHO/Cardinal
- [S57] patchcab (Svelte, last push Apr 2023): https://github.com/spectrome/patchcab
- [S57a] AudioWorklet render size and measured latency (Chrome 153, 10 Sep 2026): https://dev.classmethod.jp/en/articles/chrome-153-webaudio-render-size-hint-latency/
- [S58] Svelte 5.57.2 (6 Oct 2026), SolidJS 1.9.17 (7 Oct 2026; 2.0.0-rc.14, 8 Oct 2026), Lit 3.3.3 (14 May 2026): npm registry
- [S58a] Glicol: https://github.com/chaosprint/glicol
- [S59] Leptos 0.8.22 (5 Oct 2026), Dioxus 0.7.10 (30 Jul 2026), Yew 0.23.0 (10 Mar 2026): crates.io
- [S60] `setPointerCapture` (MDN, 1 Oct 2025): https://developer.mozilla.org/en-US/docs/Web/API/Element/setPointerCapture
- [S61] GifCities launch, 1 Nov 2016: https://blog.archive.org/2016/11/01/gifcities-the-geocities-animated-gif-search-engine/
- [S62] GifCities relaunch, 9 Jun 2025: https://blog.archive.org/2025/06/09/keep-on-gifin-a-new-version-of-gifcities-internet-archives-geocities-animated-gif-search-engine/
- [S63] 98.css 0.1.21 (28 Apr 2025, MIT): https://github.com/jdan/98.css
- [S64] css3wordart (no licence; last push 29 Apr 2019): https://github.com/arizzitano/css3wordart
- [S65] Hetzner price adjustment, 15 Jun 2026: https://docs.hetzner.com/general/infrastructure-and-availability/price-adjustment/
- [S66] Hetzner traffic allowances: https://docs.hetzner.com/robot/general/traffic/
- [S67] Fly.io pricing: https://docs.fly.io/about/pricing
- [S68] Bandwidth per listener (arithmetic): Opus at 128 kbps is 57.6 MB an hour, about 347,000 listener-hours in 20 TB; 16-bit 48 kHz stereo PCM is 691 MB an hour, about 28,900.
