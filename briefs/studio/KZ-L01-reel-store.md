# KZ-L01: every recorded loop lives once, by the hash of its samples, in one reel store the wall and the desk share, kept while anything names it and swept when nothing does

Line numbers are from the sweep that wrote this brief, at `247ca07`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Time: about 30 minutes of a builder's work. One mechanism: a content-addressed store of stereo f32 WAV files with pin lists and a sweep. Crates: kazoo-core only (new `src/reels.rs`, its tests in `src/reels/tests.rs`). Depends: nothing beyond main.

## Outcome

A loop the wall records (KZ-L04) or the desk keeps as a clip (KZ-L10) is one file, `~/.kazoo/reels/<64 hex>.wav`, named by the SHA-256 of its samples. The wall's save and the desk's session are text and name the reel by that hash; they never hold audio. The same loop saved in ten saves is one file. A reel read back is checked against its name, so a save that says `9f3c…` either plays exactly those samples or says the reel is damaged; it never plays something else. Each program that names reels writes its own pin list, and a sweep deletes only reels that no pin list names and that are more than a day old. A sweep that cannot read every pin list deletes nothing.

## Why a store and not the save

The wall's save is text with a hard size bound (`MAX_WALL_FILE`, `kazoo-wall/src/store.rs:67`), checked before every change (`WORST_MODULE_BYTES`, `:97`). A 23-bar loop at 120 BPM is 46 s, or 17.7 MB of f32 stereo, which cannot go in it. Tom's pieces are meant to be "music to fork to": text in git. Audio by hash keeps the text small and diffable. A fork of a piece names the same reels, and two programs that hold the same loop share one file.

## The design (decided; build exactly this)

### Place and names

- `pub const REELS_DIR_ENV: &str = "KAZOO_REELS_DIR";`. `pub fn reels_dir() -> io::Result<PathBuf>` returns `$KAZOO_REELS_DIR` when it is set, else `$HOME/.kazoo/reels`. It follows the shape of `kazoo-wall/src/paths.rs:44` (`state_dir`). The reason for a directory beside `~/.kazoo/wall` and `~/.kazoo/sessions` (C04) is that neither program owns the reels.
- Layout: `<dir>/<hex>.wav` for each reel, `<dir>/pins/<holder>.txt` for each pin list, and temporary files `<dir>/<hex>.wav.tmp-<pid>-<n>`. Create `dir` and `pins/` with mode 0700 (`DirBuilderExt::mode`, as `store.rs:23` does).
- `pub struct ReelId([u8; 32])`. `Display` and `FromStr` use 64 lower-case hex digits; anything else is `ReelError::BadId`. `serde` support goes through the string form with `serialize_with`/`deserialize_with` helpers, `reel_id_serde`. kazoo-core already has serde. `pub fn short(&self) -> String` gives the first 8 hex digits, for people.

### What is hashed, and the file

- A reel is always stereo, interleaved f32, at one sample rate: `pub struct Reel { pub rate: u32, pub samples: Box<[f32]> }`. `samples.len()` is even, and `frames()` is `len / 2`.
- `pub fn reel_id(rate: u32, samples: &[f32]) -> ReelId` is SHA-256 over `b"KZREEL1\0"`, then `rate` (u32 LE), then `2u16` LE (the channel count), then the frame count as u64 LE, then each sample's `to_bits()` as u32 LE. Hashing the samples rather than the file's bytes keeps a reel's name the same whatever header a WAV writer chooses. `sha2 = "0.10"` is added to kazoo-core's `[dependencies]`; it is already in `Cargo.lock` at 0.10.9.
- The file is a WAV, IEEE float, 32 bits, 2 channels, at `rate`, written with `hound` exactly as `kazoo-core/src/io/recorder.rs:81` writes. Every tool Tom owns opens it. TAPE.md names 32-bit float WAV as the recording format (`design/terminal-daw/TAPE.md`, "File Format"). It round-trips sample-exact, which seeded replay needs: a reel loaded must render bit for bit like the loop that was recorded.
- `pub const MAX_REEL_FRAMES: u64 = 4_612_096;`. This is the largest loop KZ-L02 holds (96 s at 48 kHz, `MAX_LOOP_FRAMES = 4_608_000`) plus 4,096 frames for its seam tail (8 ms is 3,072 frames at the highest rate the store takes, 384 kHz). A file that claims more frames is refused before any sample is read, so a damaged header can never make the reader allocate gigabytes.

### The store

`pub struct ReelStore { dir: PathBuf }`, with these methods:

- `pub fn open(dir: &Path) -> io::Result<Self>`. It creates both directories.
- `pub fn put(&self, rate: u32, samples: &[f32]) -> Result<ReelId, ReelError>`. It refuses an odd length (`ReelError::NotStereo`), more than `MAX_REEL_FRAMES` frames (`TooLong`), a rate outside 8,000 to 384,000 (`BadRate`), and any non-finite sample (`NotFinite { at }`). A loop never holds one: the loop module sanitises every write. Refusing beats rewriting, because a reel must be what was played. Next it computes the id.
  - If `<hex>.wav` already exists with the expected size (44-byte canonical header written by hound for float + `frames * 8`; compute it once from a written test file and pin it as `wav_size(frames)`), it returns the id without writing (dedupe), but first sets the file's modified time to now (`File::set_modified`). Reason: the sweep deletes unpinned reels by age, and a writer pins only after `put` returns; an old unpinned reel that a new take dedupes onto must get the same day of grace a fresh write gets, or a sweep in between deletes the take.
  - Otherwise it writes the temporary file, calls `sync_all`, renames it over `<hex>.wav`, and syncs the directory (`File::open(dir)?.sync_all()`), the wall's sequence (`kazoo-wall/src/store.rs:4`).
  - The temporary file's `<n>` is a process-wide `AtomicU64` counter, so two threads never share one.
- `pub fn get(&self, id: &ReelId) -> Result<Reel, ReelError>`:
  - A missing file is `Missing`.
  - Open it with `hound::WavReader`. The spec must be float, 32 bits and 2 channels, else `WrongFormat`. More than `MAX_REEL_FRAMES` frames is `TooLong`. A bad rate is `BadRate`.
  - Read the samples into a `Vec` reserved with `try_reserve_exact`; failure is `NoMemory`.
  - Recompute the id. A mismatch renames the file to `<hex>.corrupt-<unix>.wav` (the wall's moved-aside rule, `store.rs:9`) and returns `Corrupt`.
- `pub fn contains(&self, id: &ReelId) -> bool`.
- `pub fn path(&self, id: &ReelId) -> PathBuf`, for the desk's peak builder (KZ-L11).
- `pub fn pin(&self, holder: &str, ids: &BTreeSet<ReelId>) -> io::Result<()>`.
  - `holder` must match `[a-z0-9-]{1,48}`, else `InvalidInput`. The wall uses `wall`, and a desk session uses `session-<name>`.
  - It writes one hex id per line, sorted, then a last line `end`. It writes atomically, as `put` does.
  - The `end` line lets a reader tell a whole list from one cut short by a crash: a list without it is unreadable, never "empty".
- `pub fn unpin(&self, holder: &str) -> io::Result<()>` removes the holder's list. A missing list is not an error.
- `pub fn usage(&self) -> io::Result<Usage>` returns `Usage { reels: u64, bytes: u64 }`, counting `<hex>.wav` files only.
- `pub fn remove(&self, id: &ReelId) -> io::Result<bool>` removes one reel and syncs the directory. Only the wall's budget pruning (KZ-L04) calls it, and only for a reel it has checked nobody else pins.
- `pub fn pinned_elsewhere(&self, holder: &str) -> Result<BTreeSet<ReelId>, ReelError>` returns the union of every pin list except `holder`'s. An unreadable list is `PinUnreadable { holder }`.
- `pub fn sweep(&self, grace: Duration, now: SystemTime) -> Result<Swept, ReelError>`:
  1. Read every pin list. If any is unreadable, return `PinUnreadable` having deleted nothing.
  2. For each `<hex>.wav` that no list names and whose modified time is older than `now - grace`, delete it, and any sidecar beside it with the same stem (`<hex>.peaks`, which the desk's timeline writes, KZ-L11). `remove` deletes sidecars too.
  3. Delete `*.tmp-*` files older than `grace`.
  4. Sync the directory once, at the end.

  It returns `Swept { deleted: u64, bytes: u64, kept: u64 }`. `pub const SWEEP_GRACE: Duration = Duration::from_secs(24 * 60 * 60);`. The reason for a day: a reel is written before anything pins it (a take is written, then logged, then pinned; a desk takes a reel the wall made and pins it at its next save). A day covers a crash and a restart in between. Only the wall sweeps (KZ-L04 calls it at daemon start and after auto-save pruning). The desk never deletes.
- `pub const REELS_BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024;` is declared here for KZ-L04 to enforce. The reason: Tom's disk filled twice in October, and the lanes keep 60 GB free. 4 GiB holds 116 full-length loops (36.9 MB each), or about 680 eight-bar loops at 120 BPM (6.1 MB each).

### Errors

`pub enum ReelError { BadId, NotStereo, TooLong { frames: u64 }, BadRate(u32), NotFinite { at: usize }, Missing(ReelId), WrongFormat(String), NoMemory, Corrupt(ReelId), PinUnreadable { holder: String }, Io(io::Error) }`, with `Display` sentences for people ("reel 9f3c01ab is damaged: its samples do not match its name; moved aside as …") and `From<io::Error>`. Nothing is `.ok()`-discarded: a failed directory sync after a rename is returned as `Io`, because the reel may not survive a power cut, and the caller must say so.

## Not this lane

- Who writes reels and when: KZ-L04 (the wall's takes) and KZ-L10 (the desk). Who pins what: KZ-L04 and KZ-L10. Peak files for the timeline: KZ-L11, through C07a's `peaks` writer.
- No compression. TAPE.md: "No compressed format in the recording path."
- Carrying a piece's reels with its text when it is forked to another machine (Tom's "music to fork to"): a later lane exports and imports a piece with its reels; here the store only has to make that possible, which naming by content does.

## Red tests first

All in `kazoo-core/src/reels/tests.rs`, each with a `tempfile::tempdir()`. `tempfile` is already a kazoo-core dev-dependency. Each test fails today because `reels` does not exist.

- `a_reel_round_trips_bit_for_bit`. Put 1,000 frames of a ramp with a NaN-free `-0.0`, a subnormal and `f32::MAX`. `get` gives the same `to_bits()` for every sample and the same rate.
- `the_id_is_pinned`. `reel_id(48_000, &[0.0, 0.0, 1.0, -1.0])` equals a hex string computed once by hand from the byte layout above (compute it with `sha2` in the test's first run, then paste it as a literal so the layout can never drift).
- `the_same_samples_are_stored_once`. Put twice: one file, the same id, and the file's bytes unchanged by the second put.
- `a_deduped_reel_gets_fresh_grace`. Put a reel, set its mtime two days back, put the same samples again: the mtime is now within a minute of now, and `sweep(SWEEP_GRACE, now)` with no pins keeps it.
- `a_damaged_reel_is_refused_and_moved_aside`. Flip one byte of a sample in the file. `get` is `Corrupt`, the file is renamed `.corrupt-…`, and a second `get` is `Missing`.
- `a_header_claiming_too_many_frames_is_refused_before_reading`. Hand-write a WAV header claiming `MAX_REEL_FRAMES + 1` frames: `TooLong`. The allocator is not asked for the samples, which the test proves with `try_reserve_exact` never reached (no `NoMemory`, and the test runs in well under a second).
- `non_finite_samples_and_mono_and_bad_rates_are_refused`.
- `a_pin_list_cut_short_stops_the_sweep`. Pin A under `wall`, write `pins/session-jam.txt` without `end`, and make reel B old (set its mtime with `File::set_modified`). `sweep` returns `PinUnreadable` and B still exists.
- `the_sweep_keeps_what_is_pinned_and_what_is_young`. There are three reels: pinned and old, unpinned and old, and unpinned and new. Only the second is deleted, its `.peaks` sidecar with it, and `Swept` counts it.
- `stale_temporary_files_are_swept`.
- `holders_are_checked`. `pin("../x", ..)` is `InvalidInput`.

## Checks

As PLAN.md, with `-p kazoo-core`, and `ast-grep scan kazoo-core/src/reels.rs kazoo-core/src/reels`. Run doc tests for kazoo-core, because the new module is documented.

## Definition of done

- [ ] The eleven red tests fail first (paste each under RED), then pass.
- [ ] `ReelStore`, `ReelId`, `Reel`, `ReelError`, `reel_id`, `reels_dir`, `MAX_REEL_FRAMES`, `SWEEP_GRACE` and `REELS_BUDGET_BYTES` exist with the signatures above, and `pub mod reels;` is in `kazoo-core/src/lib.rs`.
- [ ] Every write is temporary file, `sync_all`, rename, then directory sync. Name each place in DONE.
- [ ] `sha2` is the only new dependency. Paste the `Cargo.lock` diff.

## Code quality

As PLAN.md's STACK RULES. Nothing here runs on an audio thread: the store is called from the wall's tape thread (KZ-L04) and the desk's deck thread (KZ-L10). It still never panics on a bad file: every `hound` error is `WrongFormat` or `Io`. New files are under 1000 lines.

## END

```
START KZ-L01 base=<sha> branch=lane/kz-L01 df=<GB> at <time>
END KZ-L01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
