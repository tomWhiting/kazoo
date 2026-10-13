# KZ-W01: the same save renders the same samples every time, because every random source behind the wall is seeded from the save

Line numbers are from the sweep that wrote this brief, at `c65de47`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md, with the `-p` list each lane names below.

**Split.** One mechanism, three lanes, run in order, each its own branch, review and END:

- **W01a** (kazoo-wall, plus `kazoo-fx/src/dsp.rs` for the shared seed functions, plus one line in kazoo-mcp): the wall seed in the save, the per-module seed derivation, `Module::set_seed`, the wall's own five random modules, and the determinism tests. About 30 minutes.
- **W01b** (kazoo-fx, plus `kazoo-wall/src/dsp/effect.rs`): `Effect::set_seed` and every random effect in kazoo-fx. Depends on W01a.
- **W01c** (kazoo-perc, kazoo-speech, plus `kazoo-wall/src/adapters/`): `Voice::set_seed`, `Rhythm::set_seed`, `Vocoder::set_seed`, every random voice and rhythm, and the vocoder. Depends on W01a.

Why three and not two: the kazoo-fx sources sit in 14 files and the kazoo-perc and kazoo-speech sources in 16 more. One lane for both would touch about 30 files, against PLAN.md's limit of ten files to a lane. W01b and W01c share no files and may run side by side once W01a has landed.

Why this lane exists: Tom wants the wall to have a timeline. A piece is its save plus a log of changes over time, kept as text, so a piece that evolves over hours or years takes kilobytes and plays back exactly. That only works if the wall's randomness is reproducible. Today it is not, and the same save sounds different each time it is loaded. KZ-W02 (frame-stamped change logs and replay) builds on this lane.

## What the sweep found (every random source a wall module can reach)

**The wall's own modules: not reproducible.** There is one generator type, `dsp::Rng` (xorshift32, `kazoo-wall/src/dsp/mod.rs:400-438`). Each module that uses it is seeded by `fn seed()` (`dsp/mod.rs:440-445`), a process-wide `AtomicU32` counter that adds `0x9E37_79B9` on every call. So a module's stream depends on how many random modules the process has built before it: every load, undo, engine rebuild, test and earlier add counts. Loading the same save twice in one daemon gives different sounds, and so does adding a module before another. The users of `seed()` are:
- `lfo` (`dsp/lfo.rs:34`): shapes 4 (random steps) and 5 (smooth random). It draws `held` at construction (`:35`) and again on reset (`:88`).
- `sh` (`dsp/sh.rs:24`): samples noise when nothing is plugged into its input (`:44`).
- `seq` (`dsp/seq.rs:54`): the gate chance (`:71`).
- `noise` (`dsp/noise.rs:100`): white noise and the colours filtered from it (`:121`).
- `branch` (`dsp/branch.rs:41`): its coin. `Branch::seeded(u32)` exists (`:45`), and its reset deliberately keeps rolling (test `:190`).

**kazoo-fx: reproducible, but every module of a kind shares one stream.** All of it uses `kazoo_fx::dsp::Noise` (xorshift32, `kazoo-fx/src/dsp.rs:328-357`) seeded from constants, so two `tape`s wow in lockstep and the wall seed cannot change them. The constants are:
- `lofi/vhs.rs`: `:203`, `:265-266`, `:349-359`; reset with constants at `:537-538` and `:548`.
- `lofi/reel.rs`: `:268`, `:345-346`.
- `lofi/genloss.rs`: `:272`, and `:361-369` (the seeds come from `0x6E11_0000 + index*16`); reset at `:381` from its stored seed.
- `lofi/cassette.rs`: `:383`, `:505-515`; reset with constants at `:728-734`.
- `lofi/radio.rs`: `:213`, `:220`, `:327-328`, `:446`, `:559`, `:644`; reset with constants at `:247-252`, `:356` and `:771`.
- `lofi/vinyl.rs`: `:207`, `:288-298`; reset at `:582`.
- `time/tape.rs:355`: drift target and hiss (its `Wobble` at `:211` is a plain phasor, not random).
- `time/bbd.rs:277`.
- `time/granular.rs:275`: scatter; reset at `:505`.
- `space/spring.rs`: `Wander` at `:224`, re-seeded with constants in reset at `:855` (`0x5EED_0000 + index`).
- `drive/crush.rs`: `:599-600`; reset at `:753-754`.
- The shared parts `Wobble` (`lofi/parts.rs:668`), `Dropouts` (`:772`) and `Hiss` (`:829`) each keep their own `seed` and reset to it (`:686`, `:786`, `:865`).
- `Drift` (`parts.rs:580`) holds no generator: its owners feed it.
- `space/line.rs:255` and `drive/fuzz.rs:553` are inside tests and are not sources.

**kazoo-perc: reproducible, shared per kind.** Everything uses `kazoo_fx::dsp::Noise` with a constant per kind:
- Voices: `conga.rs:130`, `cymbal.rs:140`, `hat.rs:113`, `tom.rs:129`, `rim.rs:116`, `noiseperc.rs:143`, `kick.rs:160`, `clap.rs:127`, `shaker.rs:191`, `modal.rs:228`, `snare.rs:125`. Their `reset` leaves the generator where it is (for example `hat.rs:183`).
- Rhythms: `prob.rs:175`, whose lock reloads the constant `SEED` (`:43`) at `:190`, and `grids.rs:262` (chaos).
- `euclid`, `burst`, `poly`, `metal`, `cowbell` and `membrane` have no randomness.

**kazoo-speech: reproducible, shared.** The vocoder's consonant noise (`vocoder.rs:449`) is reachable as the wall's `vocoder` (`kazoo-wall/src/adapters/mod.rs:27`). `SystemTime` in `testing.rs` and `tts/` is in tests and the speech cache, not in render.

**Construction.** The live wall builds every module in one place: `Wall::insert` (`kazoo-wall/src/daemon/wall.rs:1462`) calls `dsp::build(module.kind, rate)` at `:1486`. It holds `module.id` at that point, and the slot comes from `Rack::place` (`:1474`). `dsp::build` (`dsp/mod.rs:229`) dispatches to `Builder::Native`, `Builder::Effect` (`EffectModule::new`, `dsp/effect.rs:40`, which builds and prepares) or `Builder::Adapted` (`adapters/mod.rs:36`). Its other callers are `Bench::at` (`dsp/mod.rs:~489`), the `dsp` tests (`:~777`), `engine/tests.rs:46`, `daemon/record/tests.rs:28` and `benches/support/fixture.rs:56`.

**The save.** `PatchFile` (`patch.rs:121`) is at `FILE_VERSION = 2` (`:35`). Fields added since then (`wall`, `dye`, `rack`, `groups`) came in without a version bump: each is defaulted in `migrate::read` (`migrate.rs:41-136`), which is the only reader of patch bytes (from `store::decode`, `store.rs:1785`). `Patch::load` (`patch.rs:1316`) builds the `Patch`, and `Patch::to_file` (`:1224`) writes it.

**New walls** are made in three places: `seed::seed_from` (`seed.rs:103`), used by the daemon's first start (`daemon/mod.rs:492`); `Wall::blank` (`daemon/wall.rs:3225`); and `Patch::empty` (`patch.rs:434`, a `const fn` that tests call everywhere).

**Offline render.** `Wall::new(patch, control, None, Vec::new(), None)` with `engine(EngineConfig::new(48_000, 120.0, 0.0), None)`, then `Engine::render`. `benches/control.rs:12` and `engine/tests.rs` do exactly this, with no device.

**Other non-determinism on the render path: none found.**
- `processing_order` (`engine/tables.rs:288`) uses its `HashMap` for lookup only and orders by a `BTreeSet`.
- `send_tables` (`daemon/wall.rs:1494`) walks `patch.modules()` (a `Vec`).
- `Engine::measure` (`engine/mod.rs:764-780`) reads thread CPU time, but only feeds the load figure.

The things that are not reproducible are listed under "Not this lane" of W01a.

---

# KZ-W01a: the wall seed, kept in the save, seeds the wall's own random modules by module id

Crates: kazoo-wall, kazoo-fx (`src/dsp.rs` only), kazoo-mcp (`src/render.rs` only). Time: about 30 minutes. Depends: nothing beyond main at `c65de47`.

## Outcome

`wall_look` opens with the wall's seed (`seed 5eed00000000a1b2`). Loading the same save twice, in one daemon or in two, gives sample-for-sample the same `lfo` shapes 4 and 5, `sh` noise, `seq` chance, `noise` and `branch` coins. Adding, removing or moving any other module does not change `lfo6`'s random stream. A save written before this lane gets a seed when it is read: the same old file always gets the same seed, and the wall keeps that seed in every save it writes from then on. A new wall (first start, or `:new`) gets a fresh seed from the OS once, and keeps it.

## The design (decided; build exactly this)

1. **Seed functions, in `kazoo-fx/src/dsp.rs`** (beside `Noise`, so W01b and W01c share them). They are `const fn`, have no allocation and are documented, each with a pinned-value test:
   - `pub const fn splitmix64(x: u64) -> u64`, the standard SplitMix64 finaliser, all arithmetic wrapping: `z = x + 0x9E37_79B9_7F4A_7C15; z = (z ^ (z >> 30)) * 0xBF58_476D_1CE4_E5B9; z = (z ^ (z >> 27)) * 0x94D0_49BB_1331_11EB; z ^ (z >> 31)`. Pinned: `splitmix64(0) == 0xE220_A839_7B1D_CDAF`.
   - `pub const fn fold32(x: u64) -> u32`: the low 32 bits of `x ^ (x >> 32)`. Take them through `to_le_bytes` and `u32::from_le_bytes`, so there is no `as` cast for clippy's `cast_possible_truncation`.
   - `pub const fn stream_seed(seed: u64, salt: u32) -> u32 = fold32(splitmix64(seed ^ widen(salt)))`. Here `widen` is a private `const fn` that puts `salt.to_le_bytes()` into the low four bytes of `u64::from_le_bytes`, because `u64::from` is not `const` and `as` trips `cast_lossless`. It must be `const`, because `Dropouts::new` and `Wander::reset` are `const fn` and will call it in W01b. This is the one way any generator inside a module gets its 32-bit xorshift seed from the module's 64-bit seed: each generator has its own `salt`, and `Noise::new` and `Rng::new` already replace 0.
   - `pub const fn seeded(seed: Option<u64>, salt: u32) -> u32`: `salt` itself when `None`, so a module nobody seeds sounds exactly as it does today; `stream_seed(s, salt)` when `Some(s)`. W01b and W01c use it. W01a only adds it and tests it.
2. **Per-module seed, in a new file `kazoo-wall/src/chance.rs`** (`pub mod chance;` in `lib.rs`), with its tests in `kazoo-wall/src/chance/tests.rs` (the `migrate.rs` + `migrate/tests.rs` pattern):
   - `pub const fn fnv1a64(bytes: &[u8]) -> u64`: 64-bit FNV-1a, offset `0xcbf2_9ce4_8422_2325`, prime `0x0000_0100_0000_01b3`, over every byte, in a `while` loop since it is a const fn. Never std's hasher, which is randomised per process. Pinned: `fnv1a64(b"lfo6") == 0x1DB7_19AD_9E8B_B9DA`. Make `patch::hash_bytes` (`patch.rs:341`) call it in place of its inline loop. The constants are the same, so every kept save hash is unchanged, and its existing tests prove that.
   - `pub const fn module_seed(wall_seed: u64, id: &str) -> u64 = kazoo_fx::dsp::splitmix64(wall_seed ^ fnv1a64(id.as_bytes()))`. Pinned: `module_seed(0, "lfo6") == 0x9532_2FC4_F4B2_B9B8`, `module_seed(0x5EED, "lfo6") == 0xF7ED_B033_1822_2FF1` and `module_seed(0x5EED, "noise1") == 0x4B85_3CA8_7D1E_9ABD`. Also `stream_seed(module_seed(0x5EED, "lfo6"), 0) == 0xDEE0_7630`. The id (`lfo6`) is the identity, never the slot, the render position or the order of adding.
   - `pub fn fresh_wall_seed() -> u64`: build a `std::collections::hash_map::RandomState` (keyed from the OS) and hash into it the `SystemTime` nanoseconds since the epoch (`map_or(0, |d| d.as_nanos())`) and `std::process::id()`; return `splitmix64(finish())`. It runs only when a new wall is made, on the control side.
3. **The save field.** `PatchFile` gains `#[serde(default)] pub seed: u64` after `wall`, and `Patch` gains `pub seed: u64`.
   - `Patch::empty` stays `const` with `seed: 0`.
   - `Patch::to_file` writes `seed`, and `Patch::load` copies it.
   - `fingerprint` and `fingerprint_but_tempo` include it with no further change: a different seed is a different-sounding wall.
   - `FILE_VERSION` stays 2, following the precedent of `wall`, `dye`, `rack` and `groups`. An older build reading a seeded file skips the unknown field.
4. **Old saves.** In `migrate::read`, read `seed` like `wall`, as `as_u64`. If it is missing or unreadable, set it to `fnv1a64(bytes)` of the raw file bytes and push the note "the patch had no seed; it is <16 hex digits>, from the file's bytes". This is the "assigned on load" rule. It is deterministic, so a named save that only Tom's console may rewrite still sounds the same on every load, from any build, and once the wall writes `patch.json` or a save the seed is in the file and kept. Every `PatchFile { .. }` literal (`migrate.rs:119`, `patch.rs:1225`, `:1257`, `:1273`, and the tests in `daemon/wall/tests.rs`) gains the field.
5. **New walls.** `seed::seed_from` sets `patch.seed = chance::fresh_wall_seed()` after `Patch::empty`. `Wall::blank` does the same to its empty patch. A load uses the loaded patch's seed, and the undo of a load brings back the old wall's seed with its save.
6. **The trait method.** `dsp::Module` gains:
   ```rust
   /// Restart every random source from `seed`, as if the module had been
   /// built with it. Called once by [`build`], before the module reaches
   /// the engine; never on the audio thread. A module with no randomness
   /// ignores it.
   fn set_seed(&mut self, _seed: u64) {}
   ```
   `dsp::build` becomes `build(kind: Kind, sample_rate: f32, seed: u64) -> Box<dyn Module>` and calls `module.set_seed(seed)` on what it built, for all three builder kinds. `Builder::Native` keeps its `fn(f32) -> Box<dyn Module>` signature.
7. **Seeding a module.** `Wall::insert` passes `chance::module_seed(self.patch.seed, &module.id)`. The `speak` path through `adapters::build_speaker` has no randomness and is left as it is. Every rebuild (load, undo of a removal, chains, `attach` after an audio backend rebuild) goes through `insert`, so a module with the same id on the same wall always starts the same stream. The other callers pass:
   - `Bench::at`: `0`. Add `Bench::seeded(kind, rate, seed)` for the tests below.
   - `engine/tests.rs` and `daemon/record/tests.rs`: the slot number as `u64`.
   - `benches/support/fixture.rs`: the slot number.
   - the `dsp` NaN-rate test: `0`.
8. **The five modules.** Each implements `set_seed` as `self.rng = Rng::new(kazoo_fx::dsp::stream_seed(seed, 0))`. The `lfo` also redraws `held` from the new generator, exactly as `new` does. Delete `fn seed()` (`dsp/mod.rs:440`) and its imports. Each `new()` uses `Rng::new(stream_seed(0, 0))`, so a module that is never seeded (benches, tests) is still deterministic. `Branch::seeded` stays for its test.
9. **Reset.** Reset keeps each module's existing contract: it rolls on and does not re-seed (`branch`'s documented behaviour, and what `lfo`, `sh`, `seq` and `noise` do now). The engine resets a module only after a non-finite sample, and that happens on the same frame in both renders of the same save, so this is deterministic. Reload re-seeds identically, because every reload rebuilds through `insert` from the same id and wall seed.
10. **Look.** `protocol::Snapshot` (`protocol/mod.rs:1071`) gains `#[serde(default)] pub seed: u64`, filled from `self.patch.seed` in `Wall::snapshot` (`daemon/wall.rs:3879`). Every `Snapshot { .. }` literal in `tui/tests/*` gains `seed: 0`. kazoo-mcp's `render::look` (`render.rs:22`) adds `, seed {:016x}` after the tempo in its first line.

## Not this lane

- **W01b and W01c**: the kazoo-fx, kazoo-perc and kazoo-speech sources. Until they land, those stay on their constants. They are still reproducible, but two of a kind share a stream and the wall seed does not reach them.
- **KZ-W02**: frame-stamped change logs and replay. Rendering live changes back on the same frames is W02, including the frame at which a module added live starts its stream.
- A console or MCP command to reseed or show the seed anywhere but `wall_look`.
- Things that are not reproducible and are out of reach here; each is named for W02:
  - `speak` phrases, rendered by the OS speech engine (`daemon/speech.rs`).
  - Sound and clock from the desk when the wall is on the desk (`Engine::drain_hub`).
  - The load guard (`engine/mod.rs:764`, CPU time), which can refuse a live add on a busy machine.
  - The engine rebuilt by `Wall::attach` after the audio device changes: every stream restarts from its seed.

## Red tests first

Each test fails today for the reason given and passes after.

- `chance::tests::seeds_are_pinned`. It checks `splitmix64(0)`, `fnv1a64(b"lfo6")`, the three `module_seed` values and the `stream_seed` value above. Today it fails to compile, because none of these functions exist.
- `chance::tests::same_save_renders_the_same_twice`. Build a `Patch` in code at seed `0x5EED`:
  - `clock1` drives `seq1` (`chance` 0.5) into `vco1.pitch`.
  - `lfo1` (shape 5) goes to `vco1.width`, and `lfo2` (shape 4) to `vcf1.cutoff`.
  - `sh1`, with nothing in its input, is clocked by `clock1` into `vcf1.resonance`.
  - `noise1` goes to `mix1`.
  - `branch1`, fed by `clock1`, goes to `env1.gate`.
  - `vco1` and `noise1` go through `mix1`, `vcf1`, `vca1` and then `tape1` (wow up) to `out1`.

  Encode it with `store::encode`, then read it back twice through `migrate::read`, `migrate::migrate` and `Patch::load`. Put each copy up on its own fresh engine with `Wall::new(.., None, Vec::new(), None)`, render 48 000 frames, and assert every sample is bit-identical (`to_bits`). Today the second wall's modules take later values of the global counter, so `lfo`, `sh`, `seq`, `noise` and `branch` all differ.
- `chance::tests::another_module_added_does_not_move_this_ones_randomness`. Wall A is `noise1` into `out1`. Wall B, at the same seed, first adds `lfo1` (shape 5) and `sh1`, both into a `vca1` with its level at 0 that feeds `out1` (so they are rendered, add exactly zero, and push `noise1` to another slot and render position), and only then `noise1` into `out1`. Assert both masters are bit-identical over 48 000 frames. Today `noise1`'s seed depends on the build count, so it fails.
- `chance::tests::a_different_wall_seed_moves_the_randomness`. Render the first patch at seeds `0x5EED` and `0x5EEE` and assert the two differ. This guards against a seed that is plumbed but never used. Set the seed through `Patch::seed` and read it back from `to_file().seed`. Today it fails to compile, because neither field exists.
- `migrate::tests::old_save_without_seed_gets_one_and_keeps_it`. Take a version-2 file's bytes with no `seed`:
  - `migrate::read` gives `fnv1a64(bytes)` and the note.
  - Reading the same bytes again gives the same seed.
  - `Patch::load`, then `store::encode`, then `read` again gives the same seed, now from the file, with no note.

  Today it fails to compile, because there is no `seed` field.
- `dsp::tests::every_kind_built_twice_with_one_seed_renders_the_same`. For every `Kind` except `speak`, make two `Bench::seeded(kind, RATE, 7)`. Feed each input jack the same deterministic signal: a 2 Hz square gate on gate inputs, and `Rng::new(3)` audio on the others. Render 16 sub-blocks and compare every output bit for bit. Today the five random kinds differ. This test also catches any process-wide or time-based source in a kind added later.
- `seed::tests::a_new_wall_gets_its_own_seed`. Two `seed()` patches have different seeds. Today it fails to compile.
- `render::tests::look_shows_the_seed` (kazoo-mcp). A snapshot with `seed: 0x5EED` renders `seed 0000000000005eed` in its first line.

## Checks

As PLAN.md's STACK RULES, with `-p kazoo-wall -p kazoo-fx -p kazoo-mcp` for nextest and clippy, `cargo fmt --all --check`, and `ast-grep scan kazoo-wall kazoo-fx/src/dsp.rs kazoo-mcp`. Run the doc tests (`cargo test --doc -p kazoo-wall -p kazoo-fx`), because doc comments change.

## Definition of done

- [ ] The eight red tests fail first (paste each failure under RED), then pass.
- [ ] `fn seed()` in `dsp/mod.rs` is gone. `rg 'fn seed\(\)' kazoo-wall/src/dsp` finds nothing, and `rg 'AtomicU32' kazoo-wall/src/dsp` finds nothing.
- [ ] Every caller of `dsp::build` passes a seed, and `Wall::insert` passes `module_seed(patch.seed, id)`.
- [ ] `patch::hash_bytes` uses `chance::fnv1a64`, and every existing save-hash test still passes unchanged.
- [ ] On the live wall (after the landing reinstalls it): `wall_look` shows a seed; `wall_save` then `wall_load` of the same save twice gives the same seed both times; `~/.kazoo/wall/patch.json` holds `"seed"`. Say what you saw.

---

# KZ-W01b: every kazoo-fx effect on the wall draws its wow, flutter, hiss, crackle, dropouts and scatter from its module's seed

Crates: kazoo-fx, kazoo-wall (`src/dsp/effect.rs` only). Time: about 30 minutes. Depends: KZ-W01a.

## Outcome

Two `tape`s, `cassette`s, `vinyl`s, `vhs`s, `reel`s, `genloss`es, `radio`s, `granular`s, `spring`s, `bbd`s or `crush`es on one wall no longer wobble, crackle or drop out in lockstep. Each one's randomness follows the wall seed and its own id, and the same save still renders bit-identically. An effect used anywhere else (kazoo-mix inserts, the fx bench) is never seeded and sounds exactly as it does today.

## The design (decided; build exactly this)

1. **The trait method.** `kazoo_fx::Effect` (`kazoo-fx/src/lib.rs:125`) gains:
   ```rust
   /// Restart every random source from `seed`. Called off the audio thread,
   /// after `prepare`; `prepare` and `reset` keep it. An effect with no
   /// randomness ignores it.
   fn set_seed(&mut self, _seed: u64) {}
   ```
   `EffectModule` (`kazoo-wall/src/dsp/effect.rs`) implements `Module::set_seed` by forwarding to `self.effect.set_seed(seed)`.
2. **The shared parts.** `Wobble`, `Dropouts` and `Hiss` (`lofi/parts.rs`) each gain `pub fn reseed(&mut self, seed: u32)`. It sets their stored `seed` and calls their existing reset of the generator, so their own reset already returns to the new seed. `spring.rs`'s `Wander::reset(seed)` is used as it is.
3. **Each random effect** gains a field `seed: Option<u64>`, `None` from `new`. Every place it now writes a constant seed (each listed under "What the sweep found": the constructor and the reset) uses `kazoo_fx::dsp::seeded(self.seed, CONSTANT)`, with the constant it has today as the salt. `genloss` uses its computed `0x6E11_0000 + index*16 + k`, and `spring` uses `0x5EED_0000 + index`. `set_seed(s)` stores `Some(s)` and re-seeds every generator at once, the same way as its reset (without silencing tails; it is called before the first block). Files: `lofi/vhs.rs`, `reel.rs`, `genloss.rs`, `cassette.rs`, `radio.rs`, `vinyl.rs`, `time/tape.rs`, `bbd.rs`, `granular.rs`, `space/spring.rs` and `drive/crush.rs`. Each effect's own constructor and reset logic stays where it is.
4. **Reset** of an effect that re-seeds in reset today now re-seeds from `seeded(self.seed, ..)`, never from the bare constant. An effect that rolls on in reset today keeps rolling on.

## Not this lane

kazoo-perc and kazoo-speech (KZ-W01c). The wall seed plumbing (KZ-W01a). Any change to an effect's sound when unseeded.

## Red tests first

- `kazoo-fx`, `lib` tests: `every_effect_seeded_alike_renders_alike_and_unseeded_is_unchanged`. For every `EffectKind`, build two, `prepare(48_000.0)`, `set_seed(9)` on both, and feed both the same seeded noise: the outputs are bit-identical. A third, never seeded, renders bit-identically to a fourth never seeded. This is the regression guard for kazoo-mix. Today it fails to compile, because there is no `set_seed`.
- `kazoo-fx`: `random_effects_follow_their_seed`. For each of `tape`, `cassette`, `vinyl`, `vhs`, `reel`, `genloss`, `radio`, `granular`, `spring`, `bbd` and `crush`, with the knob that brings out its randomness turned up (wow, crackle, hiss, scatter, dropouts or dither), seeds 1 and 2 render different output over one second. Today `set_seed` does not exist, and once a default no-op exists it fails for each effect until that effect is done.
- `kazoo-fx`: `a_reset_returns_to_the_seeded_stream`. Do `cassette`, `radio` and `spring`, each seeded with 5: render, reset, render. The second render equals a fresh effect seeded with 5. Today their resets return to the constants.
- `kazoo-wall` `chance::tests::two_tapes_on_one_wall_do_not_wobble_together`. `tape1` and `tape2`, fed the same sine with wow and flutter up, each into its own `scope`. The two traces differ. Today they are identical.

## Checks

As PLAN.md, with `-p kazoo-fx -p kazoo-wall`, and `ast-grep scan kazoo-fx kazoo-wall/src/dsp/effect.rs`. Run the doc tests for kazoo-fx, because a trait doc changes.

## Definition of done

- [ ] Four red tests fail first, then pass.
- [ ] `rg 'Noise::new\(0x|Hiss::new\(0x|Wobble::new\(&[A-Z_]+, 0x|Dropouts::new\(0x' kazoo-fx/src` lists only lines inside `seeded(..)` or tests. Paste the output.
- [ ] Every existing kazoo-fx test passes unchanged, which proves unseeded effects are bit-identical.
- [ ] By ear on the live wall: two `cassette`s on one sine, panned apart, wobble apart. Say what you heard.

---

# KZ-W01c: every drum voice, rhythm generator and the vocoder on the wall draws from its module's seed

Crates: kazoo-perc, kazoo-speech, kazoo-wall (`src/adapters/perc.rs` and `src/adapters/speech.rs` only). Time: about 30 minutes. Depends: KZ-W01a.

## Outcome

Two `hat`s or two `prob`s on one wall no longer hiss or roll alike. A locked `prob` repeats a pattern of its own, not every `prob`'s pattern. The same save renders bit-identically, and the voices played by the instruments outside the wall are unchanged.

## The design (decided; build exactly this)

1. **The trait methods.** `kazoo_perc::Voice` and `kazoo_perc::Rhythm` (`kazoo-perc/src/lib.rs:47`, `:136`) gain `fn set_seed(&mut self, _seed: u64) {}`, documented as in W01b. `kazoo_speech::Vocoder` gains an inherent `pub fn set_seed(&mut self, seed: u64)`. `VoiceModule`, `RhythmModule` and `VocoderModule` (`kazoo-wall/src/adapters/perc.rs` and `speech.rs`) implement `Module::set_seed` by forwarding.
2. **The voices.** Each random voice (`conga`, `cymbal`, `hat`, `tom`, `rim`, `noiseperc`, `kick`, `clap`, `shaker`, `modal`, `snare`) implements `set_seed(s)` as `self.noise = Noise::new(kazoo_fx::dsp::seeded(Some(s), CONSTANT))`, with the constant it has today. Reset keeps rolling on, as it does today.
3. **`prob`** gains `lock_seed: u32`, equal to `SEED` from `new`. `set_seed(s)` sets `lock_seed = seeded(Some(s), SEED)` and `dice = Noise::new(lock_seed)`, and the lock reloads `Noise::new(self.lock_seed)`.
4. **`grids`**: `set_seed(s)` sets `dice = Noise::new(seeded(Some(s), 0x4752_4944))`.
5. **The vocoder**: `noise[0]` and `noise[1]` are re-seeded with salts `0x5EED_0001` and `0x5EED_0002`. If its `reset` re-seeds from constants, it now re-seeds from a stored `seed: Option<u64>` through `seeded`.

## Not this lane

kazoo-fx (KZ-W01b). The voices' own instrument binaries, which never call `set_seed`.

## Red tests first

- `kazoo-perc`: `random_voices_and_rhythms_follow_their_seed`. Each listed voice is struck at velocity 1, and `prob` and `grids` are clocked (chaos up for `grids`, density 0.5 for `prob`). Seeds 1 and 2 differ, and the same seed twice is identical. Today there is no `set_seed`.
- `kazoo-perc`: `a_locked_prob_repeats_its_own_seeds_pattern`. Locked `prob`s seeded 1 and 2 each repeat cycle after cycle, and their patterns differ. Today every locked `prob` plays `SEED`'s pattern.
- `kazoo-perc`: `unseeded_voices_are_unchanged`. A voice never seeded renders bit-identically to another never seeded, the same as today.
- `kazoo-speech`: `vocoder_noise_follows_its_seed`.
- `kazoo-wall` `chance::tests::two_hats_on_one_wall_do_not_hiss_together`. `hat1` and `hat2` on one clock, each into its own `scope`. Their traces differ. Today they are identical.

## Checks

As PLAN.md, with `-p kazoo-perc -p kazoo-speech -p kazoo-wall`, and `ast-grep scan kazoo-perc kazoo-speech/src/vocoder.rs kazoo-wall/src/adapters`. Run the doc tests for kazoo-perc and kazoo-speech.

## Definition of done

- [ ] Five red tests fail first, then pass.
- [ ] `rg 'Noise::new\(0x' kazoo-perc/src kazoo-speech/src/vocoder.rs` lists only `new()` defaults and lines inside `seeded(..)`. Paste the output.
- [ ] By ear on the live wall: two `hat`s on one clock sound like two hats, not one twice as loud. Say what you heard.

---

## Code quality (all three lanes)

These are as PLAN.md's STACK RULES:
- Nothing allocates, locks, formats or does I/O on the render thread. `set_seed` is called only on the control side, and the seed functions are `const fn` on integers.
- No `unsafe`, no `#[allow]` or `#[expect]`, no `let _ =`, no `.ok()` discards, no empty `Err` arms, and no `if let Ok` without an `else`.
- No `as` casts that clippy pedantic flags: fold through bytes.
- Red test first, one behaviour per commit.

These files are already over 1000 lines at `c65de47`: `patch.rs` (1702), `store.rs` (3369), `catalogue.rs` (1813), `daemon/wall.rs` (4656), `protocol/mod.rs` (1854), `kazoo-mcp/src/render.rs` (1916), `kazoo-fx/src/lofi/parts.rs` (1409), `space/spring.rs` (1236) and `drive/crush.rs` (1112). Never split an existing file. Keep your additions to them to the lines this brief names, put new logic in `chance.rs`, and list each such file under FOUND with its line count.

## END

Each lane finishes with PLAN.md's END block, every field, as its last message (`KZ-W01a`, `KZ-W01b`, `KZ-W01c`).
