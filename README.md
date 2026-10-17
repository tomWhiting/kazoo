# kazoo

A studio of synthesizers that lives in the terminal.

- **The wall:** one shared modular synth that never stops. People and Claude seats turn its knobs, move its cables, and add or take away modules while it plays. Every change is logged and can be undone.
- **The tree:** every moment the wall has been, grown as a tree you can walk and go back to.
- **The desk:** a mixer that every instrument plugs into, all on one tempo and one play/stop.
- **The instruments:** 808, 303, CS-80, Minimoog, Juno, Prophet-5, a four-operator FM synth, a plucked string, glass and bells, and an arpeggiator.
- **The mouth:** sing, hum or whistle into a synth, and play the other instruments with your voice.

Everything is real synthesis, not samples.

## Install

kazoo is one command, `kzoo`, also installed as `kazoo`. It runs on macOS (Apple silicon or Intel).

```sh
npm install -g @tomwhiting/kzoo    # or try it once: npx @tomwhiting/kzoo wall
cargo install kzoo                 # from source, with Rust 1.89 or later
```

Install it (rather than run it through `npx`) for anything that keeps running: `npx` runs from npm's cache, and the wall and the studio start programs from where `kzoo` is.

Only `@tomwhiting/kzoo` (and its `kzoo-darwin-*` builds) on npm and `kzoo` on crates.io are this project, together with the `kazoo-*` crates it is built from.

## Commands

```sh
kzoo wall               # the wall's console; it starts the wall if none is playing
kzoo tree               # the wall's tree
kzoo mix                # the desk
kzoo mouth              # the mouth synth
kzoo 808                # an instrument: 808, 303, cs80, mini, juno, prophet, dx, string, glass, arp
kzoo play               # text notation to notes or sound
kzoo studio up <name>   # start the desk and instruments of ~/.kazoo/studio/<name>.toml
kzoo mcp --seat <name>  # the wall for a Claude seat: an MCP server on stdio
```

To give a Claude seat the wall:

```sh
claude mcp add kazoo -- kzoo mcp --seat <Name>
```

## Building

```sh
cargo build --release -p kzoo   # target/release/kzoo and target/release/kazoo
cargo nextest run --workspace
```

## The workspace

| Crate | What it is |
|---|---|
| `kazoo-cli` (published as `kzoo`) | the one command |
| `kazoo-core` | the audio library: synthesis, effects, analysis, the studio link |
| `kazoo-wall` | the wall: its daemon, console, log and tree |
| `kazoo-tree` | the tree you walk |
| `kazoo-mcp` | the wall for Claude seats |
| `kazoo-mix` | the desk |
| `kazoo-mouth` | the mouth synth |
| `kazoo-808`, `kazoo-303`, `kazoo-cs80`, `kazoo-mini`, `kazoo-juno`, `kazoo-prophet`, `kazoo-dx`, `kazoo-string`, `kazoo-glass`, `kazoo-arp` | the instruments |
| `kazoo-fx`, `kazoo-perc`, `kazoo-speech`, `kazoo-play`, `kazoo-host`, `kazoo-tui` | effects, percussion voices, speech, notation, shared terminal plumbing, and the voice tracker |

The wall's design is in [design/wall/DESIGN.md](https://github.com/tomWhiting/kazoo/blob/main/design/wall/DESIGN.md), and the instruments' in [studio/](https://github.com/tomWhiting/kazoo/tree/main/studio).

## Licence

kazoo is free software under the GNU Affero General Public License, version 3 or later (see `LICENSE`).
