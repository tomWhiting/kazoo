# KZ-A08: a Justfile for the gate, and the docs tell the truth

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks: `just gate` must pass once at the end (this lane is the one allowed to run it, since it creates it). END block as PLAN.md. Depends: KZ-A06.

## Outcome

`just gate` runs the whole gate the way Tom's rules say; `just lane <crates>` runs a lane's narrow checks. CLAUDE.md, README.md and the old briefs no longer describe a kazoo that does not exist.

## The design (decided; build exactly this)

- `Justfile` at the root: `gate`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `ast-grep scan`, `cargo nextest run --workspace`, `cargo test --doc --workspace`, each under `nice -n 10 env CARGO_BUILD_JOBS=4`, stopping at the first failure. `lane crates`: the same five on `-p` for the named crates (`cargo test --doc -p`). `bench name`: `cargo bench -p kazoo-wall --bench {{name}}`. `wall`: builds and restarts the release wall (`kazoo wall stop`, build, `kazoo wall serve`), used by Cassio after landings.
- CLAUDE.md rewritten: the crate list from `Cargo.toml` with one line each; kazoo-mix is the hub, kazoo-tui is the mouth synth; the thread rules unchanged; build commands are `just gate` and `just lane`; tests are nextest only; remove the two `@.claude/plans/*.md` lines that point at files that do not exist; remove the "Current Task List" (all done per memory) and the Code Review section's model name is Opus 5.5; keep the Vision section.
- README.md rewritten around `kazoo` subcommands (KZ-A06) and `kazoo studio` (KZ-A07): what each instrument is in one line; how to start a session; where files live under `~/.kazoo`.
- `git mv briefs/0*.md briefs/1*.md briefs/old/` with a one-line `briefs/old/README.md` saying they predate the crate names.

## Not this lane

Code changes of any kind.

## Red tests first

None (docs and a Justfile); the proof is `just gate` exit 0 and `just lane kazoo-core` exit 0.

## Definition of done

- [ ] `just gate` 0; `just lane kazoo-core` 0.
- [ ] `grep -n "cargo test " CLAUDE.md README.md` returns only `cargo test --doc`.
- [ ] No `@` include in CLAUDE.md points at a missing file.
