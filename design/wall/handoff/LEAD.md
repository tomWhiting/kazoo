# Lead handoff (Cassio, 26 Sep 2026 ~15:40)

Tom restarted the lead session. Nothing is committed (do not commit unless Tom asks).
Each agent wrote its own handoff next to this file; the Grand Final work is in ~/Desktop/grand-final-oracle/.

## Goal (Tom)
Finished when: kazoo-mcp works and delivers Claude channel notifications so every seat can play the wall;
the generative wall plays forever; Tom has a TUI to see and control it. Truly generative: no scored timelines.
Full spec: design/wall/DESIGN.md; feed guide for Waffles: design/wall/FEED.md.

## State
- Live wall daemon: STOPPED at Tom's request (15:25); patch saved in ~/.kazoo/wall.
  Restart: `( nohup target/tom/release/kazoo-wall serve >> ~/.kazoo/wall/daemon.log 2>&1 & )`
  (the kazoo-wall source is mid-edit by wall-engine; rebuild only once it compiles:
  `CARGO_TARGET_DIR=target/tom nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo build --release -p kazoo-wall -p kazoo-mcp -j 4`).
- wall.py here: a small CLI for the wall socket /tmp/kazoo/kazoo-wall.sock.
- kazoo-mcp: done and green. Half-time target still open: one seat playing the wall through it with live
  channel notifications: `claude mcp add kazoo -- <repo>/target/tom/release/kazoo-mcp --seat <Name>`, then
  `claude --dangerously-load-development-channels server:kazoo`.
- Build rule: `nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo … -j 4`, one at a time, single crate (Tom's Mac
  hit load 230). Heavy gates belong on Dean's laptop via the chain, which needs Tom's OK for a branch + card.

## Agents at restart (see their handoff files)
wall-engine (perc/vocoder/speak into the wall), audio-audit (core sound fixes), fx-drive (done; Opus review next),
fx-space (done; spring loudness calibration + Opus review next), fx-time (finishing; its clippy lints block the
kazoo-fx crate), oracle-stats (Grand Final live model + hyperbolic tree page).
Idle and done: mcp, wall-tui, perc, sampler, speech, fx-lofi.

## Later (from DESIGN.md)
Sampler adapter + record/samples/load ops; general acceptor module; world module + feed op (Waffles builds the
meridian side); MIDI learn; the station (queue, dedications, stream, DJ TTS); per-module analogue personality;
Opus review of everything and fix all findings; the ~80 earlier review findings; register the Cambium seat.

## Tom's standing preferences (this session)
No artifacts, ever (plain local HTML, open it). No scored music. Fingerprints: no fading, no labels, no preset
seat colours. They/them for Cassio. Be creative with the footy maths (hyperbolic tree = the game's future tree).
