# KZ-T03: the wall's whole change log since 26 Sep is planted as the trunk of the tree, one commit per change, exact wherever the log and the wall's own files allow, with every gap a commit that says what is not known

Line numbers are from the sweep that wrote this brief, at `816c021`, and may have moved; find each item by what it does, never rename. Items from KZ-T01, KZ-T02a and KZ-T02b are named as those briefs name them (`tree::to_files`, `tree::Birth`, `tree::git::Importer`, …); if a lane landed a name differently, use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Read by Cassio, 13 Oct evening. One change at reading: the dry run grows beside the tree, not in the system temp folder. Runs after T01, T02a and T02b land, and before T04a.

Time: about 45 minutes of a builder's work, plus the measured runs. One mechanism: replaying the log into commits. Crates: kazoo-wall (a new `tree::rings` module) and kazoo-cli (one subcommand). Depends: KZ-T01 (`tree::to_files`, `tree::Birth`, `Patch::mark_birth`, `Patch::fingerprint_word`), KZ-T02a (`Change::clock`) and KZ-T02b (`tree::git::{plant, read, Importer}`, the change commit's form, where the tree is), all landed. T04a also edits `tree/git.rs` and `tree/thread.rs`, so this lane lands first and T04a starts from it.

## Outcome

`kazoo wall tree import` plants the tree at `~/.kazoo/tree` and grows `trunk` from the wall's log since 26 Sep: first the wall as it was before the first logged change, then one commit per change in order, written exactly as KZ-T02b writes a live one (the seat as author, the change's UTC time as the date, its summary as the subject, `Kazoo-Kind: change`, `Kazoo-Seq`, `Kazoo-Change`). `git log trunk` then reads as the wall's whole history, and `wall_go` (T04b) can put the wall at any of those moments. Wherever the log alone cannot say what the wall was (the desk moved the tempo, a load whose wall was not kept, a restart that lost changes), the import writes a commit by the seat `kazoo-wall` that says exactly what is not known and which file the tree carries on from. `--dry-run` says all of it and writes nothing in the tree. Running it again does nothing. It refuses a tree that has a trunk it did not grow.

KZ-T02b's daemon never plants a tree, and writes nothing until one is planted (it shows ` NO TREE `). This import is what plants it, so the history always lies beneath every live commit.

## What the sweep found (each checked on Tom's files, read-only)

- The log: `~/.kazoo/wall/log.jsonl`, 1,444,082 bytes, 6,373 lines, seq 1 (`2026-09-26T03:35:55Z`, Cassio) to seq 6373 (`2026-10-13T08:48:24Z`, Tom), no seq missing, no `log.1.jsonl` (rotation is at `LOG_ROTATE_LINES` = 10,000, `store.rs:135`). Ops: 5,055 turn, 642 patch, 364 add, 206 remove, 63 unpatch, 26 record, 11 tempo, 3 load, 2 migrate, 1 speak; one undo (of a turn). No `resumed`, `set_aside`, `abandoned` or unknown op. No line has a `clock` (KZ-T02a stamps only new changes).
- No file holds the wall before seq 1. The two `patch.before-<epoch>.json` files are the copies `migrate` keeps of `patch.json` before it rewrites it: `patch.before-1790399525.json` is `2026-09-26T05:12:05Z`, version 1, `last_seq` 277 (the migrate is seq 278 at `05:12:08Z`); `patch.before-1791869036.json` is `2026-10-13T05:23:56Z`, version 2, `last_seq` 6358 (the migrate is seq 6359, which gave the wall its seed).
- Working back is exact: taking changes 277 to 1 back from `patch.before-1790399525.json`, every change's after-value matches (a turn's `to`, a patch's cable, an add's module), and replaying 1 to 277 forward gives that file again exactly. The wall before seq 1 is 14 modules, 17 cables, 92 BPM: the version-1 seed (cables 1 to 14 are `SEED_CABLES`, `seed.rs:69`, and `SEED_TEMPO` is 92, `seed.rs:21`), with its old `delay1` and `reverb1` on cables 15 to 17.
- Every wall file is evidence of the wall after its own `last_seq`: 41 of them (`saves/*.json`, the five `patch.<name>.json` copies `.imported` brought in, both `patch.before-*`, `patch.json`). A prototype replay (Python, by the brief's writer, 1.3 s) of all 6,373 changes against all 41 files found the log consistent with every file in every module, knob and cable, and three places where only the tempo is not as the log has it, because the desk sets the tempo while the wall follows it and a desk's tempo is not logged (`What::Tempo` doc, `protocol/mod.rs:766`): before change 1214 (the log has 58 BPM since change 332; change 1214 says it was 120), before change 3107 (118 since change 3034; it was 120), and after change 5989 (the save "rain brass with flutters and buckley 2026-10-13 1250" has 120; the log has 60 since change 5759).
- The three loads are all known exactly. Seq 3217 (`2026-10-10T09:07:30Z`, Tom's console) cleared the wall: an empty wall at 135 BPM; its backup `wall-2026-10-10-2007tesr` (39,334 bytes, the fingerprint's length) is the wall after 3216. Seq 6368 loaded "grimy beat fuzzed siren octave", and seq 6373 loaded "gorgeous toms evening 2026-10-13 1913"; each is the other's backup (fingerprints `9f6f3f198e87b426-18254` and `0d88b06a25391f7b-38855`), and between them only recordings happened. `patch.json` (`last_seq` 6373) is the wall after the last change.
- Module ids are reused across walls but never on one wall (`ModuleRecord::id` doc, `protocol/mod.rs:891`), and every file says which wall it is (`PatchFile::wall`, `patch.rs:128`). So `(wall, id)` names one module for its whole life, as KZ-T01's found marks rely on.

## The design (decided; build exactly this)

### Where it lives

- `kazoo-wall/src/tree/rings.rs` (new): `pub fn grow(state: &Path, tree: &Path, dry_run: bool) -> Result<Rings, RingsError>`, the walk and the report. `tree/rings/model.rs` (new): the wall as the log makes it, and the wall's files and what each proves. `tree/rings/tests.rs` (new). `pub mod rings;` in `kazoo-wall/src/tree.rs` (KZ-T01's module root).
- The CLI: `WallCommand` (`kazoo-cli/src/cli.rs:88`) gains `Tree { #[command(subcommand)] command: TreeCommand }`, and `TreeCommand::Import(ImportArgs { #[arg(long)] dry_run: bool })`, about "Plant the tree and grow its trunk from the wall's change log (once, with the wall stopped)". `wall()` (`kazoo-cli/src/main.rs:58`) routes it to `kazoo_wall::tree::rings::grow(&state, &tree, dry_run)`, where `state` is `kazoo_wall::paths::state_dir()?` and `tree` is KZ-T02b's rule for where the tree is (`$KAZOO_TREE_DIR` when set and not empty, else `tree` beside the state directory). KZ-T02b writes that rule in `daemon/mod.rs`; move it, unchanged, into `pub fn tree_dir(state: &Path) -> Option<PathBuf>` in `tree/git.rs`, and call it from both `daemon/mod.rs` and the CLI, so there is one copy of it. `None` (a state directory with no parent) is `RingsError::NotATree` "the wall's state directory has no parent to keep the tree beside; set KAZOO_TREE_DIR". It prints the report (below) to stdout and returns `Ok` (exit 0) for a growth, a dry run, or "in already"; any `RingsError` is a `color_eyre` error with its sentence (exit 1). So the command is `kazoo wall tree import [--dry-run]`. The standalone `kazoo-wall` binary (`run.rs`) is not given it: `kazoo` is the one CLI (Track A).

### The wall must be stopped, and only a tree with no trunk is grown

- `grow` first opens the state directory with `Store::open` (`store.rs:492`), which takes the wall's exclusive lock on `<state>/lock` and holds it until `grow` returns. With the wall running this fails with `WouldBlock`, and `grow` returns `RingsError::WallRunning`: "the wall is playing: stop it first (kazoo wall stop), then import; the first rings grow only from a log nobody is writing". Reason: the log must not grow while it is read, and the daemon must not attach to the tree while the rings are written. The dry run takes the lock too, for the same reading. `Store::open` also clears temporary files a crash left, exactly as every daemon start does; nothing else in the state directory is written.
- Then the tree directory, in this order:
  1. It exists, is not empty, and is not a bare repository (`git -C <tree> rev-parse --is-bare-repository` does not print `true`): `RingsError::NotATree` "<tree> is not empty and is not a tree; nothing is changed".
  2. A bare repository with the tag `refs/tags/rings/end`, and `refs/heads/trunk` at it or descending from it (`git merge-base --is-ancestor`): report "the first rings are in already: trunk holds changes <first> to <last> from the import (rings/end at <short>), and <n> commits since" and return.
  3. `rings/end` and no `trunk`: a run whose checkpoint wrote the tag and not the branch. Write `reset refs/heads/trunk` `from <the tag's commit>` through an `Importer` and checkpoint; report "finished an import that stopped before naming trunk"; return.
  4. `rings/end`, and a `trunk` that does not descend from it: `RingsError::Diverged` "trunk does not grow from the import's last ring (rings/end at <short>); nothing is changed". Only a hand could do it; the import never moves a ref.
  5. `trunk` without `rings/end`: `RingsError::HasTrunk` "the tree already has a trunk the import did not grow (its first commit is <its subject>); the first rings grow only beneath everything, and nothing in the tree is ever rewritten, so nothing is changed".
  6. A bare repository with no `trunk` (planted and never written): grow into it.
  7. No directory, or an empty one: `tree::git::plant(tree)` (KZ-T02b: 0700, `--initial-branch=trunk`, `gc.auto 0`, so `HEAD` already names `trunk`), then grow.
- The dry run never plants or writes `<tree>`. It grows into a tree planted for the purpose beside the real one, at `<tree dir>.dry-run-<pid>-<UTC now as YYYYMMDDTHHMMSSZ>` (never the system temp folder: Tom keeps nothing there), reads its `trunk`, and removes it (`fs::remove_dir_all`; a failure to remove is said in the report with the path). Reason: the report's commit id and counts are then those of a real growth of the same log, checked by git itself.

### The model (`rings/model.rs`): the wall in its save form, changed as the log says, checked as it goes

- The model holds a `PatchFile` (`patch.rs:121`), its version (1 or 2), the wall's number (`PatchFile::wall`), `doubt: Option<String>`, `agreed: u64` (the latest change whose checks all passed), and the births `BTreeMap<(u64, String), tree::Birth>` keyed by `(wall, module id)`.
- Reason it is the save form and not a `Patch`: the log records modules as `ModuleRecord`s with the knobs of the build that made them. A `Patch` built by today's catalogue drops knobs and kinds it no longer has (`Patch::load`, `patch.rs:1319`), and a later change's before-value on a dropped knob would then read as a gap that never happened. A `Patch` is made from the model only to write each commit (below).
- `apply(&mut self, change: &Change) -> Applied` applies one change at the save form's level and checks every before-value the log carries. Values are compared as `f32` bit patterns (`(a as f32).to_bits()`, the narrowing the crate already makes with a comment at `change.rs:249`): every knob target and amount is an `f32` widened (`widen`, `protocol/mod.rs:131`), whether written as `0.55` or `0.550000011920929`.
  - `turn`: the module and knob exist, and the knob is `from`; set it to `to`.
  - `patch`: both ends' modules exist; the input `to` holds `replaced` (by id) or nothing when `replaced` is absent; no cable has `cable.id`. Take out what is in `to`, put `cable` in, `next_cable = max(next_cable, id + 1)`.
  - `unpatch`: `cable.id` is in, from `cable.from` to `cable.to`. Take it out; put `replugged` back if present.
  - `add`: no module has the id. Push the record as logged (with `birth: None` on every line written before KZ-T01, and its birth on every line after), and count its id into `next_ids` as `Patch::count_id` does (`patch.rs:1286`).
  - `remove`: the module is there with the logged knobs, and its cables (either end) are exactly `cables` by id. Take them out, put `replugged` in. Words move to `retired_speech` as `Patch::remove` does (`patch.rs:822`).
  - `restore`: the id is free; put the module and `cables` in, take `replaced` out.
  - `chain`: each inner change in order, as above.
  - `tempo`: the tempo is `from`; set it to `to`, whether on the wall's own clock or asked of the desk (the desk answers what it is asked; when it later moves on its own, the next evidence says so).
  - `record`, `speak`: no change to the save form. (A speaker's words are private and never logged, `Request::Speak` doc, `protocol/mod.rs:281–286`; the words a file holds come in with the next file, below.)
  - `abandoned`: none. Before the walk, collect every `abandoned.change`; those changes are skipped entirely and get no commit, as `Wall::undo` treats them (`daemon/wall.rs:2834`).
  - `load` and `migrate`: below.
  - One final arm for every other op (`resumed`, `set_aside`, `What::Unknown`, and any variant added later: T04a, beside this lane, adds `What::Go`) makes the model unknown, with the change's own summary as the reason. So a variant added later compiles without touching this file.
- A failed check does not stop the walk. The change is applied as far as its own after-values go (a turn sets `to`; a patch puts its cable in whatever was in that input; a remove takes the module and every cable at it; a tempo sets `to`), `doubt` is set to one sentence naming what disagreed and since when ("the tempo was 120 BPM, not 58 BPM as the log has it since change 332"), and the walk writes a gap commit just before this change's commit.
- A change that cannot be applied at all (a turn on a module the model does not have, a patch whose end is missing) makes the model *unknown*: no commit is written for it or for the changes after it until the next file that holds a whole wall.

### The evidence (`rings/model.rs`, beside the model): files that hold the whole wall at a known change

- Read, bounded at `MAX_WALL_FILE` (`store.rs:67`) as `read_wall_file` does (`store.rs:1766`; write the same bounded read here rather than exposing it), every one of: `<state>/patch.json`; `<state>/patch.before-*.json`; `<state>/patch.<name>.json`; `<state>/saves/<name>.json` for names that pass `valid_save_name` (`protocol/mod.rs:102`). Not the set-aside files (`Store::set_aside_files`, `store.rs:765`: a wall that could not be used at a start). A file that `migrate::read` (`migrate.rs:41`) refuses is skipped and named in the report.
- Each file is `Evidence { path, bytes, file: PatchFile (as read, unmigrated), after: u64 }` with `after = file.last_seq`. Reason: every wall file is written by the daemon from the wall as it is, and `last_seq` is the latest change it had (`patch.rs:126`); a save made by hand is the wall at the moment of saving.
- A load's backup is also evidence of the wall just before the load: for each logged `load` at seq `k`, the save `backup` whose bytes hash (`hash_bytes`, `patch.rs:346`) to the logged `fingerprint` is evidence with `after = k - 1`, whatever its own `last_seq` says. (A backup can be an older save the wall had not changed since: "grimy beat fuzzed siren octave" has `last_seq` 5717 and is the wall after 6372.) A backup whose bytes do not match is named in the report and is not evidence for `k - 1`.
- Several files with the same `after`: the first in this order decides, and the others are compared with it, any difference going into the report only: a verified load backup; a `patch.before-*`; `patch.json`; `patch.<name>.json`; saves by name, ascending. Reason: the backups and the pre-migration copies are the wall's own continuity, written in the same batch as the change after them.

### The walk (`rings.rs`)

The log is `Store::load_log` (`store.rs:1702`): both files, in seq order, unreadable and repeated lines counted. A seq missing from the run `first..=last` makes the model unknown there ("change <n> is not in the log").

1. **The start.** If evidence exists with `after = first - 1`, the start is that file's wall. Else take the evidence with the smallest `after = s ≥ first` and work back: undo changes `s` down to `first` in the save form, checking each (`turn` sets `from` after checking `to`; `patch` takes its cable out after checking it is in, and puts `replaced` back; `unpatch` puts the cable back after checking it is out, and takes `replugged` out; `add` takes the module out after checking it is there with no cables; `remove` puts the module and its cables back after checking they are out; `restore` takes them out and puts `replaced` back; `tempo` sets `from` after checking `to`; `record`, `speak`, `abandoned` nothing). If every check passes and no `load`, `migrate` or other op lies in `first..=s`, the start is that wall, "worked back". Otherwise the start is unknown and the first commit is the gap commit for evidence `s` (step 6). The numbering (`next_ids`, `next_cable`) of a worked-back wall is the file's: it only grows, so it is never too low.
   - On Tom's files: the version-1 seed of 14 modules at 92 BPM, from `patch.before-1790399525.json` through changes 277 to 1.
2. **Births at the start.** Each module of the start wall gets KZ-T01's found mark, `Birth::Found { wall, seed, id }` with the model's wall and seed: exactly what `Patch::load` gives a module of a file made before births. So a later load of an old save of the same wall finds the same marks.
3. **Each change `k`, in order.** If the model is unknown, skip to step 6 at the next evidence with `after ≥ k`, counting the skipped changes by seat; else apply. Births:
   - A module whose logged record carries a birth (a line written after KZ-T01) keeps it, exactly. Each other module a change adds gets KZ-T01's `Birth::Born { seq: k, n, at: change.at, print, salt }`, stamped as `tree::stamp_births` stamps a live change (in `What` order, `n` from 0): `print` is the commit view's `fingerprint_word()` with the newborns on it and not yet marked, and `salt` is `chance::fnv1a64` of the change's line exactly as it is in the log. Reason: KZ-T01's live salt is random (`fresh_wall_seed`), but the import must grow the same trunk every time, and a hash of the logged line is fixed and differs for every change.
   - A `restore` keeps the birth its `(wall, id)` already has. Any module whose `(wall, id)` has a birth keeps it, always.
4. **Loads.** A `load` at `k` puts up a new wall; the model's wall number becomes `k`. What it put up:
   - Evidence with `after` in `k ..` up to the next change after `k` that changes the wall (anything but `record`, `speak`, `abandoned`): that file's wall (Tom's 6368: the backup of 6373; 6373: `patch.json`).
   - Else, a load with no `save` and no `undoes` (a new wall): `Patch::empty` at the load's `tempo` (`patch.rs:435`), seed 0 unless evidence with `wall == k` has one.
   - Else, a load that `undoes` load `j`: the wall `j`'s verified backup holds (its evidence at `j - 1`).
   - Else, a save in `saves/` with that name whose file was last modified before the load's `at`, and whose module count, cable count and tempo agree with the load's `modules`, `cables` and `tempo`: that file (it was not saved over since).
   - Else unknown: "the wall change <k> put up (the save "<name>") was not kept exactly".
   - Births across a load: a module keeps the birth of `(the incoming file's own wall, id)` when the import has given that pair one (Tom's 6368 and 6373: every module keeps the birth it had on wall 3217); otherwise it has the found mark `Patch::load` gives it.
5. **Migrations.** A `migrate` at `k`: if evidence with `after = k - 1` and version `from_version` exists (the `patch.before-*` copy), the model after `k` is exactly what the daemon made: `migrate::migrate(&mut migrate::read(&that file's bytes)?.0)` (`migrate.rs:245`). The model takes the migrated seed only when one of the change's `notes` starts with "the patch had no seed" (`migrate.rs:122`); otherwise it keeps its own. Reason: today's `migrate::read` gives every seedless file a seed from its bytes (`migrate.rs:116`), but at change 278 (26 Sep) the wall had no seeds; only change 6359 gave one. Without the copy, the model migrates its own save form, and the commit's message adds "patch.json before this migration was not kept, so anything the migration took from the file's own bytes may differ".
6. **Evidence at `after = s`**, checked after change `s` (or for the start). Compare the model with the file in what makes the sound: modules (id, kind, name, every knob of both, as `f32` bits), cables (id, from, to, amount as `f32` bits), the tempo, and the seed when the file has a `seed` key (a file without one was written before seeds). Then:
   - The model unknown: a gap commit "the wall from here is <file> as it stood after change <s>; changes <a> to <b> (<n> changes: <seat> <count>, …) are not in the tree one by one: <the reason the model became unknown>", with `Kazoo-Gap: <a>-<b>`. The model becomes the file.
   - The sound differs: a gap commit "after change <s> the wall was as <file> has it, not as the log makes it: <the differences, at most 8 named, then "and <n> more">; something the log does not have changed it (the desk's tempo is not logged)". The model becomes the file.
   - The sound agrees and `tree::to_files` of the file's wall differs from that of the model's (rows, groups, fingerprints, words, numbering): a settle commit "the rack's rows, groups, fingerprints and words as <file> kept them after change <s>; they are not logged". The model takes them from the file. Reason: moves and groups are not logged (`Request::Arrange` doc, `protocol/mod.rs:235–240`), nor are words.
   - Otherwise nothing is written, and the model takes the file's unlogged parts silently (they write the same files).
   - `doubt` is cleared by any evidence that agrees or is adopted.
7. **The end.** The last commit is the wall after `last`. `patch.json` is evidence with `after = last` when the wall stopped cleanly, and step 6 checks it. If its `last_seq` is below `last`, the daemon stopped before saving; its next start logs `resumed` (`daemon/wall.rs:979`) and KZ-T02b's start commit says so. The report says it and the import does nothing about it.

### The commits

- **A change commit is KZ-T02b's.** Build it with the very function KZ-T02b's thread uses to turn a change item into a `tree::git::Commit` (if it is written inline in `tree/thread.rs`, factor it into `pub(crate) fn change_commit(change: &Change, files: …) -> Commit` without changing a byte it writes, and call it from both). So the author, the dates, the subject and the trailers are exactly KZ-T02b's: `Kazoo-Kind: change`, `Kazoo-Seq`, `Kazoo-Change` (the logged line as compact JSON). `Kazoo-Beat`, `Kazoo-Tempo` and `Kazoo-Clock` are left out, as KZ-T02b leaves them out of any change with no `clock`; none of the old lines has one (no beat was ever stamped before KZ-T02a). The fire (T06) reads such moments by their UTC times.
- **The files of a commit.** `Patch::load(&view)` (`patch.rs:1319`) of the model's save form (while the model is version 1, a clone migrated with `migrate::migrate` and given back the model's seed), each module given its birth from the model with KZ-T01's `Patch::mark_birth`, then `tree::to_files(&patch.to_file())`. Rows the model does not have are hung by `hang_rows` as every load hangs them (`patch.rs:1403`). The fingerprints follow every change as the daemon's `apply` moves them (`daemon/wall.rs:2094`): `view.dye.touch(seat, what, &view)` (`fingerprints.rs:180`); the model then keeps the view's dye, rows, groups and retired words (never its modules or cables). The notes `Patch::load` gives (a knob or kind this build no longer has) are counted and listed in the report, never in a commit. Each commit carries only what differs from the previous commit's files, as KZ-T02b's change items do (`M` for new or changed paths, `D` for gone ones); the first commit, and the commit after a load or an adopted file, carry `deleteall` and every file.
- Reason the commits before change 278 hold the version-2 wall the migration makes of each version-1 state: every moment in the tree must be one today's wall can put up (`wall_go`, T04b; the crossing, T05; the fire, T06), and the old `delay` and `reverb` kinds are gone (`migrate.rs:9`). The migration keeps ids and names, and numbers the right-hand cables it adds from the model's `next_cable`, which through the version-1 stretch is the file's (71), so they are cables 71 to 74, as at change 278.
- **Start, gap and settle commits** are authored and committed `kazoo-wall <kazoo-wall@kazoo.invalid>` (KZ-T02b's form for the wall's own commits), dated the `at` of change `first` (start), of the change a doubt's gap precedes, or of change `s` (evidence). The subject is the sentence above (the start's: "the wall before change <first>, worked back from <file>"); then a blank line, the detail (the file, the seqs, the counts by seat), a blank line, and the trailers `Kazoo-Kind: start | gap | settle`, `Kazoo-Seq: <the change the wall is after>` (0 for the start; KZ-T02b's start and gap commits likewise carry the seq they follow), and `Kazoo-Gap: <a>-<b>` when changes are not in the tree one by one. No `Kazoo-Change`: they are not changes.
- **Determinism.** Nothing in a commit depends on when or where the import runs: no clock, no host, no random draw; every date comes from the log. Two imports of one state directory into two trees give the same `trunk` id.

### Writing: one importer, one checkpoint

- One `tree::git::Importer` on `trunk` (the planted tree has no tip, so the first commit has no `from`). Every commit goes through `Importer::commit`. After the last, the tag: `reset refs/tags/rings/end` / `from :<last mark>` through a new `Importer::tag(&mut self, name: &str, from: &str) -> Result<(), TreeError>` in `tree/git.rs` (`from` is `:<mark>` or a 40-digit commit id; T04a uses it for marks) that writes `reset <name>`, `from <from>` and a blank line, after checking with `git rev-parse -q --verify <name>` that the ref does not exist and refusing with `TreeError::Git("<name> exists; a tag is never moved")` if it does. Then one `checkpoint` (KZ-T02b's: `core.fsync=committed`, then the sha check of `trunk`), then `finish`.
- Reason: a batch is one durable operation with one full flush (Tom, 3 Oct). Git writes the pack, flushes it, then the refs: a power cut leaves either no `trunk` or the whole trunk. KZ-T02b measured 0.35 ms a commit for `fast-import`, so the commits of Tom's log take about 2 s.
- Memory is bounded by the model and the previous commit's files: nothing else is held. The log is at most `2 × LOG_ROTATE_LINES` changes.

### What it says (stdout; the same lines with `--dry-run`)

```
the first rings: <state dir> → <tree dir> (dry run: grown in <temp dir>, then removed | planted and grown)
log: changes <first> to <last> (<lines> lines, <MB> MB); <n> unreadable, <n> repeated, <n> abandoned
evidence: <n> wall files; <n> load backups hold their fingerprints, <n> do not (<names>); <n> unreadable (<names>)
start: <the start commit's subject> (<modules> modules, <cables> cables, <bpm> BPM)
commits: <total> (1 start, <n> changes, <n> batch parts, <n> gaps, <n> settles)   (batch parts since KZ-T10c)
gap before change <k>: <sentence>            (one line for every gap)
gap after change <s>: <sentence>
births: <n> found at the start, <n> at adds, <n> kept across loads, <n> found at loads
this build left out: <n> knobs, <n> modules (<first 8 as id.knob or id>)
trunk: <40-hex> (change <last>, <at>)<", as patch.json has it" | "; patch.json is after change <n>: the wall's next start says so">
took <seconds> s
```

## Not this lane

- Live commits, the tree thread, `HEAD` beyond what `plant` sets: KZ-T02b and T04a. The files and births: KZ-T01.
- Beats for imported commits: none exist. `wall_go` and marks: T04b.

## Red tests first

In `tree/rings/tests.rs`. Each state directory is a `tempfile::tempdir()`, each tree another (planted by `grow` itself unless the test says otherwise). Logs are made by a real `Wall` on a `Store::peek` store with no tree attached (as `the_patch_is_saved_when_due_and_the_log_keeps_every_change`, `daemon/wall/tests.rs:677`), driven through `Wall::request` and ended with `Wall::close`, so the log and `patch.json` are the daemon's own. Lines a real wall cannot write (an old tempo, a change it never logs) are written into the log file as `Change` values serialised as the store writes them. The tests need `git` 2.36 or later on `PATH`, as KZ-T02b's do. Every test fails today because there is no `tree::rings`.

1. `a_real_walls_log_grows_to_the_wall_it_kept`. Drive a seeded wall through 120 scripted changes: turns with glides, patches replacing cables, unpatches, adds, removes, an undo of a remove (a restore), an undo of a turn, own-clock tempo, a recording started and stopped, a save, a load of that save, an undo of the load, and a new wall. Import. `trunk`'s files equal `tree::to_files` of `patch.json`'s wall, births included (this wall logs them); the change commits carry `Kazoo-Seq` 1 to 120 in order; no gap commit.
2. `the_wall_before_the_first_change_is_worked_back_from_the_earliest_file`. As 1 with 40 changes, and only `patch.json` (after change 40) in the state directory: the start commit is "worked back", and its files equal `tree::to_files` of the seeded wall the test began with, every module with its found mark.
3. `a_tempo_the_desk_moved_is_a_gap_and_the_walk_goes_on`. A log whose `tempo` change 9 says `from` 120 while change 4 set 60: one gap commit, dated as change 9, subject "the tempo was 120 BPM, not 60 BPM as the log has it since change 4", directly before change 9's commit; changes 10 onward committed normally.
4. `a_load_whose_wall_was_not_kept_is_one_gap_until_the_next_file`. A load of a save whose file is gone, and no evidence until a save at change 30: changes after the load to 30 have no commits; one gap commit after 30 with `Kazoo-Gap` naming them, the seats' counts in its body, and that save's wall in its files.
5. `a_file_that_disagrees_with_the_log_is_adopted_by_a_gap_commit`. A save whose `vcf1.cutoff` differs from the log's: the gap commit names `vcf1.cutoff`, and the next commit's files have the file's value.
6. `rows_moved_without_a_log_are_settled_by_their_own_commit`. Arrange a module (not logged), save, import: one settle commit at the save's `last_seq`; none when nothing moved.
7. `a_version_1_stretch_is_the_version_2_wall_its_migration_makes`. A version-1 `patch.json` written by hand (a `vco`, a `delay`, a `reverb`, an `out`, with `vco1.out → delay1.in`, `delay1.out → reverb1.in`, `reverb1.out → out1.left`; the old kinds' knobs `time`, `sync`, `feedback`, `tone`, `mix` and `size`, `damping`, `mix`), five turns on it written into the log including `delay1.tone`, then a `Wall` started on that state directory the way the daemon starts (so it migrates, logs `migrate` and keeps `patch.before-<epoch>.json`), then five more changes. Every commit before the migrate holds `delay1` as a `digital` with `highcut` from `tone`; the right-hand cables have the same numbers in every one of them and at the migrate commit; the seed is 0 through the version-1 stretch.
8. `births_follow_the_log_and_cross_loads_of_the_same_wall`. A log written by hand without births, as every line before KZ-T01: add `vco3` at change 5, save, add `lfo2`, load the save at change 12: `vco3` keeps its `born 5.0` mark; a module of a save of another wall has its found mark; two runs give the same marks, salts included.
9. `abandoned_changes_are_not_grown`. Change 7 and a later `abandoned { change: 7 }`: no commit carries `Kazoo-Seq: 7` with `Kazoo-Kind: change`.
10. `a_dry_run_writes_nothing_in_the_tree`. With no tree directory: none exists afterwards and the temporary one is gone; with an empty planted tree: every file and byte of it is unchanged. The report's counts and `trunk` id equal those of the real run that follows.
11. `running_twice_writes_nothing_the_second_time`. The second `grow` reports "in already"; every file and byte under the tree is unchanged.
12. `a_trunk_the_import_did_not_grow_is_refused`. A tree planted with `plant` and given one commit by an `Importer`: `RingsError::HasTrunk`, the tree unchanged.
13. `an_import_that_stopped_before_trunk_finishes`. A tree with `rings/end` and no `trunk`: `trunk` is made at the tag, nothing else is written.
14. `the_import_refuses_while_the_wall_holds_its_lock`. With `Store::open` holding the state directory: `RingsError::WallRunning`, nothing planted.
15. `the_same_log_grows_the_same_trunk_anywhere`. Two imports of one state directory into two trees, a second apart: the same `trunk` id.
16. In `kazoo-cli/src/cli/tests.rs`: `wall_tree_import_parses` (`kazoo wall tree import --dry-run` is `WallCommand::Tree { command: TreeCommand::Import(ImportArgs { dry_run: true }) }`; an unknown flag is refused).

## Measured on Tom's files (copies, never the originals)

`cp -R ~/.kazoo/wall <scratch>/wall` (read-only use of the original), then with `KAZOO_WALL_STATE_DIR=<scratch>/wall` and `KAZOO_TREE_DIR=<scratch>/tree`:
- `kazoo wall tree import --dry-run`, then `kazoo wall tree import`, then `kazoo wall tree import` again. Paste all three reports. They must show: the worked-back start of 14 modules, 17 cables, 92 BPM from `patch.before-1790399525.json`; the three tempo gaps (before 1214, before 3107, after 5989) and no other gap; the three loads known; `trunk` ending as `patch.json` has it; the third run "in already".
- Time: each run within 60 s on Tom's M1 Max (the wall is stopped for the run, once; a minute is the most that is reasonable). Say whether the machine was quiet. Paste `git -C <scratch>/tree log --oneline trunk | wc -l`, `git -C <scratch>/tree count-objects -v`, and `jj git clone <scratch>/tree <scratch>/jj` then `jj -R <scratch>/jj log -r 'trunk' -n 5`, to show jj reads it.
- Never run the import on `~/.kazoo/tree` or with the real state directory: that is the landing's step, by the lead, with the wall stopped.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src/tree.rs kazoo-wall/src/tree kazoo-cli/src
```
(`kazoo` is kazoo-cli's package name, `kazoo-cli/Cargo.toml:2`.) Doc tests only if doc comments changed: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-wall -j 4`.

## Definition of done

- [ ] The 16 tests fail first (paste each under RED), then pass.
- [ ] `tree::rings::grow`, `Rings`, `RingsError` (`WallRunning`, `NotATree`, `HasTrunk`, `Diverged`, and `Tree(TreeError)` carrying KZ-T02b's sentence), `Importer::tag` and `kazoo wall tree import [--dry-run]` exist as named.
- [ ] KZ-T02b's allowed-git-commands test still passes: the import's own `git` calls (`rev-parse`, `merge-base --is-ancestor`) are in `rings.rs`, which that test does not read; `Importer::tag` writes only the fast-import stream.
- [ ] The measured runs above, pasted, with the times and the counts.
- [ ] Files: `tree/rings.rs`, `tree/rings/model.rs`, `tree/rings/tests.rs`, `tree.rs` (one line), `tree/git.rs` (`Importer::tag`, `tree_dir`), `tree/thread.rs` (only if `change_commit` must be factored out), `daemon/mod.rs` (the call to `tree_dir`), `kazoo-cli/src/cli.rs`, `kazoo-cli/src/main.rs`, `kazoo-cli/src/cli/tests.rs`: at most ten. `rings/model.rs` holds the model and the evidence and stays under 1000 lines; the walk, the commits and the report are `rings.rs`'s.

## Landing (the lead's step, written here so the order is not lost)

Stop the wall (`kazoo wall stop`), run `kazoo wall tree import --dry-run`, read it, run `kazoo wall tree import`, then start the wall with the new build. KZ-T02b's daemon then attaches to the planted tree, and its first commit is a start commit on `trunk` after change 6373. Until the import runs, the console shows ` NO TREE ` and nothing is lost: every change is still in the log, and this import takes all of them.

## Code quality

As PLAN.md's STACK RULES. Each new file under 1000 lines. No `as` casts clippy pedantic flags except the `f64 as f32` narrowing for comparison, written as `change.rs:249` writes it, with its comment. Every refusal and every gap sentence is a whole plain sentence, as `change.rs`'s are. No `unwrap` or `expect` outside tests. Every child process (`git`) is waited for.

## END

```
START KZ-T03 base=<sha> branch=lane/kz-T03 df=<GB> at <time>
END KZ-T03 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the three measured runs: the reports, the seconds, the commit and object counts
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
