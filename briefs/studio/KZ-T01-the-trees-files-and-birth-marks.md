# KZ-T01: every module is born with a birth mark it keeps for life, and every wall turns into one text file per module and a `wall.toml` and back, exactly

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Read by Cassio, 13 Oct 20:5x, at `1786fa3`: fixture hash and "no words" checked against the live save; `fresh_wall_seed` draws a fresh `RandomState` per call, so two changes in one second get different salts. Accepted as written.

Time: about 30 minutes of a builder's work. One mechanism: a module's lasting identity and the wall's text form. Crates: kazoo-wall only. No git in this lane (KZ-T02b). Depends: nothing beyond main. This lane goes first among new wall work: birth marks must be in before anything else saves modules (PLAN.md, Track T, order of play 8).

Files (ten, Cargo.lock aside):
1. `kazoo-wall/Cargo.toml`: `toml = "0.9"`, the version kazoo-cli already locks (`Cargo.lock`: `toml 0.9.12+spec-1.1.0`). No new crate enters the lock.
2. `kazoo-wall/src/lib.rs`: `pub mod tree;`.
3. `kazoo-wall/src/tree.rs` (new): `Birth`, `stamp_births`, `Files`, `to_files`, `from_files`, `TreeError`.
4. `kazoo-wall/src/tree/tests.rs` (new).
5. `kazoo-wall/src/tree/fixtures/evening.json` (new): a real wall, copied byte for byte.
6. `kazoo-wall/src/protocol/mod.rs`: `ModuleRecord` gains `birth`.
7. `kazoo-wall/src/protocol/tests.rs`: its one `ModuleRecord` literal (`:277`) gains `birth: None`.
8. `kazoo-wall/src/change.rs`: its one `ModuleRecord` literal (`:440`, a test) gains `birth: None`.
9. `kazoo-wall/src/patch.rs`: `Module` gains `birth`; record, load, restore and the fingerprint word.
10. `kazoo-wall/src/daemon/wall.rs`: births stamped as each change is numbered.

## Outcome

Every module on the wall carries a birth mark from the moment it is born: `born 6374.0 2026-10-13T08:48:24Z 5f3c0e2a91b4d7e8 91d0c3a2b4e5f607`. That is the number and UTC instant of the change that added it, its place among that change's newborns, the wall's fingerprint when it was born, and 64 random bits. The mark never changes while the module lives. It is in every save and in `patch.json`, it comes back with a load and with the undo of a removal, and it travels in every `add`, `remove` and `restore` change the seats see. A module from a save made before birth marks gets a `found` mark the first time it loads, worked out the same way every time from its file.

The wall also has a text form, for the tree (Track T): `wall.toml` and one `modules/<id>.toml` per module, each knob on its own line, each cable in the file of the module it feeds. A save turned into files and back is the same save, byte for byte as the wall writes it, and files turned into a save and back are the same files, byte for byte.

## The design (decided; build exactly this)

### The birth mark (`tree.rs`)

```rust
pub enum Birth {
    /// Born on a wall that marks births.
    Born { seq: u64, n: u32, at: String, print: u64, salt: u64 },
    /// On a wall from a file made before births were marked.
    Found { wall: u64, seed: u64, id: String },
}
```

**Text form.** The mark is one string, the same in JSON and TOML, with fields split by single spaces:
- `born <seq>.<n> <at> <print> <salt>`. `seq` and `n` are decimal with no leading zeros (`0` alone is allowed). `at` is exactly `YYYY-MM-DDTHH:MM:SSZ`, the form `change::utc` writes (`change.rs:363`). `print` and `salt` are each 16 lower-case hex digits.
- `found <wall> <seed> <id>`. `wall` is decimal, `seed` is 16 lower-case hex digits, and `id` passes `patch::valid_id` (`patch.rs:396`).

`impl Display` writes this form. `impl FromStr` accepts exactly what `Display` writes and nothing else, so parsing a mark and writing it back gives the same string. Anything else is `TreeError::BadBirth(String)`, carrying the text through `patch::printable`.

**What each field is, and why.**
- `seq` and `at` are those of the change that added the module (Tom: "every module, every change all has a timestamp ... helps us just get the order of things right").
- `n` counts that change's newborns from 0, in the order they appear in its `What`. A chain add (`What::Chain`, `daemon/wall/chains.rs:125`) gives birth to several modules in one change.
- `print` is `Patch::fingerprint_word()` (below): the wall the module was born into. That is PLAN.md's "plus the wall's fingerprint". It ties the mark to that wall, so T03's importer can check a birth against the wall it rebuilds.
- `salt` is `crate::chance::fresh_wall_seed()` (`chance.rs:27`, an OS-keyed hasher of the time and the process id), drawn once per change. **Why it is there, beyond PLAN.md's form.** Within one tree, `seq` alone is unique: one daemon numbers every change once (`daemon/wall.rs:2076`). Across clones it is not. Two people at the same commit, each with a log at the same number, who each add a module within the same second, would mint the same `seq`, `at` and `print`. The salt makes that a 2⁻⁶⁴ chance. The salt is not derived from the wall's seed, because every clone has the same seed and would draw the same "random" number. Nothing that sounds depends on a birth, so the randomness costs no exactness. Replay (T06) reads births from the commits; it never mints them again.

**Order.** `impl Ord for Birth`: every `Found` comes before every `Born`, because found modules are older than births were marked. `Found` is ordered by `(wall, seed, id)`. `Born` is ordered by `(at, seq, n, print, salt)`: `at` first, so marks from different trees order by time. The `at` strings sort chronologically because their form is fixed. Derive `PartialEq`, `Eq` and `Hash` to match.

**Found marks for files made before births.** A module whose record has no birth gets `Found { wall: file.wall, seed: file.seed, id }`, where `file` is the `PatchFile` it loads from. This happens in `Patch::load` (`patch.rs:1319`), which every load path goes through: `patch.json` at start (`store.rs:617`), every save (`store.rs:1785`, `decode`), and the backup an undo of a load reads (`daemon/wall.rs:3268`). A record restored by `Patch::restore` without a birth (a removal logged before this lane) gets `Found { wall: self.wall, seed: self.seed, id }`. The rule is deterministic, and here is why it is right:
- The module numbering never gives an id out twice on one wall (`next_ids`, `patch.rs:786`; `count_id`, `:1286`), and `wall` changes every time the wall is replaced or resumed (`protocol/mod.rs`, `What::Load` and `What::Resumed`). So one `(wall, id)` names one module across every save made of that wall. The saves of 13 October (`wall` 3217) share it, so a crossing (T05) between two of them glides the modules they share.
- `seed` separates lineages. A save from before seeds gets its seed from its own bytes (`migrate.rs:116`), so two old saves never share a found mark, and the same old save gives the same marks every time it loads.
- The known cost: a wall loaded before this lane has a new `wall` number (the wall up at `816c021` is `wall` 6373, loaded from a save of `wall` 3217). So its modules' found marks differ from those in the save it came from. After this lane a load carries the marks with it, so this happens only to walls loaded before it.

No note is made for a found mark: nothing is lost, and a note would make `Patch::from_file` (`patch.rs:1300`) refuse every old file. A mark that does not parse, or one already on the wall, is replaced by the found mark, with a note: `"module 'vco3' had a birth mark that does not read ('…'); it is found afresh"`, or `"… had the birth mark of vco2; it is found afresh"`. Births on one wall are unique, always.

### Where births are kept (`protocol/mod.rs`, `patch.rs`)

- `ModuleRecord` (`protocol/mod.rs:889`) gains, last, `#[serde(default, skip_serializing_if = "Option::is_none")] pub birth: Option<String>`. It holds the text form, not a `Birth`. That way `migrate::read` (`migrate.rs:70`, which reads modules one by one from JSON values) never drops a whole module over a damaged mark: the mark is checked in `module_from_record`, which can say what it did. Old saves, old log lines and old seats' records read with `birth: None`.
- `patch::Module` (`patch.rs:43`) gains `pub birth: Option<Birth>`. `Module::record` (`:58`) writes `birth.map(|b| b.to_string())`. `module_from_record` (`:1608`) parses it. On failure it returns the module with `birth: None` and the reason, and the caller decides the found mark and the note. Change its return to `(Module, Vec<String>, Option<String>)`; both callers (`load_module` `:1469` and `restore` `:890`) are in this file.
- `Patch::add` (`:786`) makes the module with `birth: None`. The change's number is not known until the daemon numbers it (`daemon/wall.rs:1931`, `next_change`, which first lets the wall's own owed changes take theirs). So the birth is stamped there, below. `None` exists only between `Patch::add` and that stamp, inside one call of `Wall::commit`.
- `pub fn mark_birth(&mut self, id: &str, birth: Birth) -> bool` sets a module's birth and returns whether it was there.
- `pub fn fingerprint_word(&self) -> u64` is `chance::fnv1a64` over the exact bytes `fingerprint` (`:1259`) hashes. Factor those bytes into one private `fn print_bytes(&self) -> Vec<u8>` that both use, so they can never disagree.
- `Patch::load` (`:1319`): after every module is loaded, give each module with no birth its found mark, keeping a `HashSet<Birth>` of those seen to apply the duplicate rule. Load order is file order, so the first holder of a duplicate keeps it.

### Stamping (`tree.rs`, `daemon/wall.rs`)

- `pub fn stamp_births(what: &mut What, seq: u64, at: &str, print: u64, salt: u64) -> Vec<(String, Birth)>`. It walks `what` depth-first in order (a `What::Chain`'s `changes` in sequence, nested chains included). For each `What::Add { module }` whose `module.birth` is `None` it sets `Born { seq, n, at, print, salt }` with `n` counting from 0, and returns each `(module id, birth)`. Every other variant is left alone, `Restore` included, because a restored module keeps its old birth. Write the match out in full, with no `_` arm, so a new `What` variant is a compile error here.
- In `Wall::commit` (`daemon/wall.rs:1913`), between `next_change` and `record`: if the change holds any add, compute `print = self.patch.fingerprint_word()` and `salt = chance::fresh_wall_seed()` once, call `stamp_births(&mut change.what, change.seq, &change.at, print, salt)`, and `self.patch.mark_birth(&id, birth)` for each. So the line logged, the result returned and the event told all carry the births. When `record` fails, `request_as` puts the patch back as it does now (`:2236`), and the newborn goes with it.
- **Why the fingerprint is taken at this point.** `print` is taken with the newborns on the wall and still unmarked: the one moment the wall is defined and the same for every newborn of the change. It changes no cost that matters: a fingerprint encodes the wall once (`:1259`), and adds are rare (364 in 6,373 logged changes, `~/.kazoo/wall/log.jsonl`).
- `daemon/wall/chains.rs:188` rebuilds each chained `What::Add` from `self.patch.find(id)?.record()`, which has `birth: None` then. Nothing changes there, because the stamp happens at the chain's single commit.

### The wall as files (`tree.rs`)

`pub type Files = BTreeMap<String, Vec<u8>>`, keyed by path relative to the tree's root: `wall.toml` and `modules/<id>.toml`. These functions are pure, use no catalogue lookup to decide what goes where, and do no I/O:

- `pub fn to_files(file: &PatchFile) -> Result<Files, TreeError>`
- `pub fn from_files(files: &Files) -> Result<PatchFile, TreeError>`

The catalogue is used for one thing only: the order knob lines are written in.

**`modules/<id>.toml`**, sections in this order, one blank line between sections, ending with a newline:

```toml
kind = "vcf"
name = "low brass"
birth = "born 6374.0 2026-10-13T08:48:24Z 5f3c0e2a91b4d7e8 91d0c3a2b4e5f607"

[knobs]
cutoff = 0.35
res = 0.2

[inputs]
in = { cable = 141, from = "mix5.out", amount = 1.0 }
cutoff = { cable = 150, from = "lfo6.out", amount = -0.4 }

[dye]
Cassio = 3.21
"Tom W" = 1.0

[words]
text = "…"
voice = "af_heart"
```

- `name` and `birth` lines are present only when `Some`.
- `[knobs]` is always written, even when empty. Lines go in the kind's catalogue order (`Kind::from_name(kind)` then `spec().knobs`) for the names the record has. Any other names follow in name order. Reading gives a `BTreeMap`, so the order is for people (`step1 … step16`, not `step1, step10, …`), and writing is still a pure function of the record.
- `[inputs]` holds every cable whose `to` is `<this id>.<jack>`, keyed by `<jack>` (the text after the first `.`), in ascending cable number. **A cable lives in the file of the module it feeds**, because an input takes one cable (`patch.rs:639`, a new plug replaces the old). Omit the section when there are none.
- `[dye]` is written when `file.dye` has an entry for this id, even an empty one, so an empty entry round-trips. Seats go in name order.
- `[words]` is written when `file.speech` has an entry for this id, with `text`, then `voice` when `Some`.

**`wall.toml`**, in this order:

```toml
version = 2
tempo = 120.0
last_seq = 6373
wall = 6373
seed = "d4ae3c0ffee11223"
next_cable = 281
modules = [
    "clock5",
    "seq3",
]
rack = [
    ["clock5", "seq3"],
    ["vco9"],
]

[next_ids]
bbd = 2

[groups]
brass = ["clock5", "seq3"]

[[retired_speech]]
module = "speak2"
text = "…"
voice = "…"
```

- `modules` is the wall's module order, one id per line (`PatchFile::modules` is "in the order added", `patch.rs:140`). A file per module cannot hold that order, and render order and `look` depend on it. One id per line keeps a git diff to one line per add or remove.
- `rack` is one row per line. Empty lists are written `modules = []` and `rack = []`.
- `[next_ids]` and `[groups]` are always written, even when empty. Each `[[retired_speech]]` is preceded by a blank line, in file order, with `voice` only when `Some`.
- **Why `seed` is a hex string.** TOML integers are signed 64-bit, and a seed uses all 64 bits: the wall up now has seed 15325402214781944465, which is past `i64::MAX`. So the seed is 16 lower-case hex digits.
- `last_seq` and `wall` are TOML integers. Above `i64::MAX` is `TreeError::TooLarge { what }`, which a wall never reaches.

**Writing scalars.** It is our own code, for the same bytes every time:
- **Strings** are basic strings. `\` becomes `\\`, `"` becomes `\"`, U+0008 `\b`, U+0009 `\t`, U+000A `\n`, U+000C `\f` and U+000D `\r`. Every other code point below U+0020, and U+007F, becomes `\uXXXX` (upper-case hex). Everything else is written as it is.
- **Keys** are bare when they match `[A-Za-z0-9_-]+`, else written as a basic string (seat names may hold spaces and dots, `protocol/mod.rs:74`).
- **Floats** must be finite, else `TreeError::NotANumber { path, key }`. A save's JSON can hold no other kind, and `Module::record` passes values through `widen`. Write `-0.0` and `0.0` for zeros. Write `format!("{x}.0")` when `x % 1.0 == 0.0`, else `format!("{x}")`. That is what `toml_writer` does. Rust's `Display` gives the shortest decimal that reads back to the same `f64`, never an exponent, so every value reads back bit for bit.

**Reading** uses `toml::from_str` into private serde structs, each `#[serde(deny_unknown_fields)]`:
- `WallToml`, `ModuleToml`, and `InputToml { cable: u32, from: String, amount: f64 }`.
- `knobs: BTreeMap<String, f64>` and `dye: Option<BTreeMap<String, f64>>`.
- `words: Option<Words>` (`patch.rs:181`) and `retired_speech: Vec<RetiredToml>` (`module`, `text`, `voice`).

Then:
- The modules come in `modules` order.
- Each cable is `CableRecord { id: cable, from, to: format!("{id}.{jack}"), amount }`, and all of them are sorted by number.
- Dye and speech are gathered by id, and the seed is parsed from its 16 hex digits.

**What `to_files` refuses** (`TreeError::NotOneWall(String)`, saying what): an id that fails `valid_id`; the same id twice; a cable whose `to` is not `<id on the wall>.<non-empty jack>`; two cables into one jack; cable numbers not strictly ascending; dye or words for an id not on the wall. These are exactly what `Patch::to_file` (`patch.rs:1226`) never makes. `valid_id` also keeps every id a plain file name: 1 to 40 of `a-z 0-9 _`, so never `..` and never `/`. So `Ok` means the round trip holds.

**What `from_files` refuses**:
- A path other than `wall.toml` or `modules/<valid id>.toml` (`UnknownFile`).
- A missing `wall.toml`, or an id in `modules` without its file, or the reverse (`Missing` / `UnknownFile`).
- TOML that does not parse into the structs (`Parse { path, reason }`).
- A seed that is not 16 hex digits, the same cable number twice, or a birth that does not parse (`BadBirth`). A file is checked strictly; forgiveness is `Patch::load`'s job.

**The round trip, stated as the tests check it.** `ser(f)` is `serde_json::to_vec_pretty(f)`, as `store::encode` writes (`store.rs:1799`). For every `f` where `to_files(&f)` is `Ok`: `ser(&from_files(&to_files(&f)?)?) == ser(&f)` (byte equality of JSON, not `PartialEq`, which would take `-0.0` for `0.0`). And for every `files = to_files(&f)?`: `to_files(&from_files(&files)?)? == files`, byte for byte.

### Errors

`pub enum TreeError { BadBirth(String), NotOneWall(String), NotANumber { path: String, key: String }, TooLarge { what: &'static str }, UnknownFile(String), Missing(String), Parse { path: String, reason: String } }`, with `Display` sentences for people and `std::error::Error`. Nothing panics on any input.

## Not this lane

- Git, commits, the tree thread, the queue, and stamping beats (KZ-T02a, KZ-T02b). Writing files to a directory: nothing needs it yet. T02b streams them into git.
- Showing birth marks in `look`, the console or the MCP (T04 or later). They travel in change records already.
- Matching modules by birth (T05). Importing the old log (T03).
- Chains (`patch/chains.rs`) carry no births: a chain is a template, and each add from it is a new birth.

## Red tests first

All in `kazoo-wall/src/tree/tests.rs`. Each fails today because `tree` does not exist, or, for the patch and daemon ones, because no record has a birth.

1. `a_birth_reads_back_exactly`. Both forms parse and print to the same string. Each of these is refused: two spaces, a leading zero in `seq`, upper-case hex, 15 hex digits, `at` without `Z`, a bad id, an unknown first word, and the empty string.
2. `births_order_found_first_then_by_time`. `Found` < `Born`. Two `Born` with `at` a second apart order by `at` even when `seq` runs the other way. With the same `at` they order by `seq` then `n`.
3. `stamping_numbers_each_newborn_of_a_change`. `stamp_births` on a `What::Chain` holding add, turn, add, and a nested chain with one add gives `n` 0, 1, 2 in order. It leaves a `Restore` and an add that already has a birth untouched.
4. `an_added_module_is_born_with_its_change`. Build a `Wall` as `daemon/wall/tests.rs:16` does (`engine(EngineConfig::new(48_000, 120.0, 0.0), None)`, `Wall::new(seed().unwrap(), control, None, Vec::new(), None)`) and request `Add { kind: "lfo" }`. The change's `What::Add` record has `birth == "born {seq}.0 {at} {print} {salt}"` with `seq` and `at` equal to the change's own. A second add has a different salt and `n` 0.
5. `a_birth_survives_save_load_and_the_undo_of_a_remove`. On a `Wall`: add, remove (the `Remove` record carries the birth), undo the remove (the `Restore` record carries the same birth). Then `Patch::load(&patch.to_file())` keeps every birth.
6. `an_old_save_is_found_the_same_way_every_time`. Take `fixtures/evening.json` with every `birth` field stripped (it has none; assert so). Read it through `migrate::read` and `Patch::load` twice: identical marks, each `found 3217 <its seed> <id>`. A copy with its `seed` field removed gets a seed from its bytes (`migrate.rs:116`), and the same marks on both reads.
7. `a_damaged_or_doubled_birth_is_found_afresh_and_said`. A record with `birth: "born x"` and two records sharing one good birth: the first keeps it, and the others get found marks, each with its note.
8. `old_records_and_lines_read_without_a_birth`. A `ModuleRecord` JSON with no `birth` key deserialises with `None`, and serialises with no `birth` key.
9. `save_to_files_to_save_is_identical`. Over (a) `fixtures/evening.json` (85 modules, 121 cables, dye from four seats); (b) a wall built in the test with every field in use: a name with a quote and a backslash, words with a newline and a voice, retired words, a seat named `Tom W.`, groups, three rack rows, numbering for a kind the catalogue lacks, a seed of `u64::MAX`, a tempo of 97.3, a knob of `-0.0`, a dye amount of `2.166666671357773`; (c) `Patch::to_file()` after each of 50 random requests (add, remove, turn, patch, unpatch, undo) on a `Wall`, for 64 seeds drawn from `kazoo_fx::dsp::splitmix64`. JSON bytes are equal each time.
10. `files_to_save_to_files_is_byte_identical`. The same three sources.
11. `the_files_are_written_as_shown`. For one wall of an `lfo` feeding a `vcf`'s cutoff, the bytes of `wall.toml` and `modules/vcf1.toml` equal literals in the test, as this brief lays them out.
12. `every_cable_lives_in_the_module_it_feeds`. For the fixture, each cable appears in exactly one file, the one named by its `to`.
13. `what_is_not_one_wall_is_refused`. One case for each `to_files` and `from_files` refusal above, each giving the named error. None panics.

`fixtures/evening.json` is a byte copy of `~/.kazoo/wall/saves/gorgeous toms evening 2026-10-13 1913.json`. Read it, never write it. Its SHA-256 is `ec84d61828301e4a461c7cf31291b947fd1b00fc7db428238a844425898ebfe3`; check it with `shasum -a 256` and paste the result under DONE. It holds no words (checked), so nothing private enters the repository. Read it with `include_bytes!`.

## Checks

As PLAN.md's STACK RULES, run on Tom's Mac under the shared lock:
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4`
- `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings`
- `cargo fmt --all --check`
- `ast-grep scan kazoo-wall/src/tree.rs kazoo-wall/src/tree kazoo-wall/src/patch.rs kazoo-wall/src/protocol kazoo-wall/src/daemon/wall.rs kazoo-wall/src/change.rs`
- Doc tests, because `tree` is documented: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

Nothing else: no workspace build.

## Definition of done

- [ ] The 13 tests fail first (paste each under RED), then pass.
- [ ] `Birth`, `stamp_births`, `Files`, `to_files`, `from_files`, `TreeError`, `Patch::mark_birth`, `Patch::fingerprint_word`, `Module::birth` and `ModuleRecord::birth` exist as named.
- [ ] Every add logged by the daemon carries a birth. Under DONE, paste one `add` line from a test wall's log.
- [ ] `Cargo.lock` gains only `"toml"` in kazoo-wall's dependency list. Paste the diff.
- [ ] Every crate test that was green before is green.

## Code quality

As PLAN.md's STACK RULES:
- `tree.rs` stays under 1000 lines (about 700 expected). If it nears the limit, the birth mark goes to `tree/birth.rs`, and you say so under CHANGED; that would be file 11, so do it only if needed.
- `patch.rs` and `daemon/wall.rs` are over 1000 lines already. Add to them only what is named here (about 60 and 15 lines), and put nothing there that could live in `tree.rs`.
- No `as` casts clippy pedantic flags. No `unwrap` outside tests.
- No allocation on any audio path. Nothing here runs on one.

## END

```
START KZ-T01 base=<sha> branch=lane/kz-T01 df=<GB> at <time>
END KZ-T01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
