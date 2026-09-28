# wall-engine handoff (26 Sep 2026)

## State: fixing the Opus review of this seat's work (25 findings)

The fixes are all written; a full test run and the gates are next, then the reviewer verifies. The last green gates are recorded below.

**Last full run** (under `lockf`, clippy with `--no-deps`):
- **kazoo-wall:**
  - fmt 0, clippy 0, ast-grep 0;
  - tests: lib 196, TUI 44, daemon 11, doc 2, all passing.
- **kazoo-mcp:**
  - fmt 0, clippy 0, ast-grep 0;
  - tests: unit 24, seat 5, all passing.

## Done (all reported to team-lead)

- **The kazoo-wall library and daemon.**
  - Parts: catalogue registry, DSP, engine, protocol and client, patch, store, listen, daemon.
  - Also: fingerprints, the watcher role, and `seq` on seat and fault events.
  - Also: the runtime dir is kept 0700 (M3), and the tempo is persisted.
- **Patch migration v1 → v2** (`src/migrate.rs`).
  - Changes: `delay` becomes `digital`, `reverb` becomes `plate`, ports are remapped, and stereo partner cables are added.
  - Loading is lenient, with `patch.before-<ts>.json` kept as a backup.
  - Kind ids may contain digits.
  - It is tested against Tom's patch, saved as `tests/fixtures/airports-v1.json`.
- **Adapters** (`src/adapters/{mod,perc,speech}.rs`):
  - every kazoo-perc voice and rhythm generator;
  - `vocoder` and `speak`;
  - `Builder::Adapted`, with one arm in `dsp::build`;
  - `MAX_KNOBS` raised to 24.
- **The `speak` op** (`src/daemon/speech.rs`, the wall and control plumbing).
  - The answer is deferred until the words are rendered.
  - The words are kept privately in `patch.json`.
- **`wall_speak` in kazoo-mcp**, with a 75 s answer window (`SPEECH_WITHIN` in `link.rs`), on a connection of its own so no other tool waits behind it.
- **Latency compensation** (`daemon/timing.rs`, `engine/lines.rs`, wall `send_tables`/`settle`/`latencies`/`unsteady`).
  - Each cable has its own delay line.
  - A delay change moves a read head that glides at no more than 2% (50 frames per frame moved, at least 5 ms), read on a cubic curve.
  - A gate switches between pulses: the output is held low until the new reading point is low.
  - Effect latency comes from a probe of each kind, with the module's *settled* knobs applied. Each knob settles on its own deadline: 100 ms after its glide ends, or, for a stepped knob, after it lands on its new step.
  - `look` shows `timing`: latency, arrival, delayed cables, `uncompensated` cables and `unsteady` modules.
- **Review fixes:**
  - speech:
    - a `Renderer` seam with a scripted test renderer;
    - the daemon's own tickets;
    - renders that lose their worker are failed;
    - a put-back backlog, fed one render at a time;
    - superseded renders are refused with `not_allowed`;
    - a phrase is kept through a rate change;
    - the render rate is clamped;
    - each cache warning is told once;
    - seats are refused while 2 renders are in flight;
  - `retired_speech`, so undoing a speaker's removal brings back its words;
  - `What::Unknown` and `Event::Unknown`;
  - perc velocity is how far the gate rises past 0.5;
  - `admit` refuses duplicate port names;
  - `wall_look` renders the timing.
- **Docs:** DESIGN.md is updated, including latency compensation and the speech changes.

## In flight elsewhere, which may affect this code

- **fx-time changes the flanger's `latency()`.**
  - It has been `manual`-driven, then a fixed hold in through-zero mode, now 7 frames in classic.
  - The wall tests compare against the effect's own `latency()` at each mode and rate, never a fixed number.
- **kazoo-fx has its own dead-code warnings**, so clippy must run with `--no-deps` until they are fixed.

## Left

Nothing assigned is outstanding. Possible follow-ups nobody has asked for:
- A `speech` status in `look` for renders still in flight.
- An `Event` for speech that fails after the request was answered. Today saved-words re-renders only log to stderr.

## Next command

```
cd /Users/tom/Developer/projects/deno_rust/kazoo && nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo test -p kazoo-wall -j 4
```

## Gotchas

- **Build rule:** every cargo command is `nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo … -j 4`, one at a time. No release builds unless asked.
- **Tom's live wall:**
  - Never run `serve` or `stop`, or tests, against the default dirs.
  - Always set `KAZOO_WALL_RUNTIME_DIR` and `KAZOO_WALL_STATE_DIR` to temp dirs.
  - Runtime dirs go under `/tmp`, because socket paths are limited to 104 bytes.
- **Formatting:** don't `cargo fmt -p kazoo-wall` over `src/tui` (wall-tui's) or audio-audit's files. Run `rustfmt --edition 2024 <file>` on your own files only.
- **audio-audit owns:** `src/dsp/*`, `engine/master.rs`, the knob-glide code, and output conversion in `daemon/audio.rs`.
