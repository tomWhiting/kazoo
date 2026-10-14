# KZ-D17: the desk remembers its tempo, and the wall's tree hears the desk's tempo

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-mix -p kazoo-wall`.

Place: Track D. Written by Cassio, 14 Oct 2026, 20:5x, from Tom: "i just restarted the mixer and it reset the temp again, can we fix that?" Decided by Cassio.

Time: about 35 minutes. Crates: kazoo-mix (`shared.rs`, `hub.rs`, `engine.rs`, `run.rs`, a new `tempo_store.rs`), kazoo-wall (`daemon/wall.rs`, `change.rs`).

## Why

The desk keeps the studio's tempo, and the wall follows it while it is on the desk (`kazoo-wall/src/daemon/wall.rs`, `playing_after`). The desk holds nothing on disk: `SharedState` starts at `DEFAULT_BPM` (120, `kazoo-mix/src/shared.rs:283`), as do the hub's first `SongAnchor` (`hub.rs:552`) and the engine's `SongClock` (`engine.rs:269`). So every desk restart drags the wall to 120 BPM. The wall also never logs a tempo the desk sets, so the tree misses it (the T03 import found five such gaps today).

## The design (decided; build exactly this)

1. **The desk keeps its tempo on disk.** File: `$KAZOO_MIX_DIR/tempo` when that is set and not empty, else `~/.kazoo/mix/tempo`. Contents: one line, the BPM as a decimal (`100` or `96.5`), then a newline. It is written by a temp file in the same folder, fsynced, then renamed over the old one, so a crash never leaves half a file.
2. **On start** the desk reads the file before `SharedState`, the hub's first anchor and the engine's `SongClock` are built, and all three start at that tempo. A missing file means `DEFAULT_BPM`, silently. An unreadable, unparsable, non-finite or out-of-range value (outside the desk's `clamp_bpm` range) means `DEFAULT_BPM` plus one line on stderr and in the desk's status line saying why; the bad file is left as it is until the next write replaces it.
3. **On change** (a key on the desk, a tap, or a `TransportRequest` with a tempo from the wall or an instrument) the new tempo is written off the audio thread: the desk's UI/worker loop notices `transport_revision` move, and after the tempo has held still for 500 ms it writes the file. Nothing on the output callback touches the file. A failed write shows in the status line and on stderr, and is tried again on the next change. It is never silent.
4. **The wall logs the desk's tempo.** When the wall is on the desk and the desk's tempo changes by more than 0.01 BPM from what the wall last saw (including the first sync after a desk restart, if it differs), the daemon logs a change `What::Tempo { from, to, desk: true }` under the seat name `desk`, with the text `the desk set the tempo {from} → {to}`. That makes it a commit in the tree like any other change. This change is logged only and never asked back of the desk, so it cannot loop. A glide the wall itself asked of the desk is logged once, as today, and not again when the desk reports it.

## Red tests first

1. `a_restarted_desk_keeps_its_tempo`: build the desk's start path with a temp `KAZOO_MIX_DIR` holding `96`; `SharedState`, the first anchor and the `SongClock` all read 96.
2. `no_file_is_the_default` and `a_bad_file_is_the_default_and_said`: missing gives 120 with no message; `abc`, `NaN`, `0` and `9999` each give 120 and a message naming the file and the reason.
3. `the_tempo_is_written_once_it_settles`: three tempo changes 100 ms apart, then 500 ms still, give one write holding the last value; the file is never seen half-written (read it in a loop during the writes).
4. `the_callback_never_writes`: the engine's existing `assert_no_alloc` render path with a tempo change is untouched (no file I/O from the callback; a grep test on `callback.rs` for `std::fs` if needed).
5. `the_wall_logs_the_desks_tempo`: a wall on a fake desk whose tempo goes 120 → 100 logs one change from seat `desk` with `desk: true`, and with a planted tree the tree gains one commit with that message; a wall-asked tempo change logs exactly one change.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `uptime` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix kazoo-wall
```

## Definition of done

- [ ] Each red test fails first (paste it), then passes; every kazoo-mix and kazoo-wall test passes; clippy, fmt and ast-grep exit 0.
- [ ] Tom restarts the desk after landing and the tempo holds (NOT CONFIRMED until then).

## END

```
START KZ-D17 base=<sha> branch=lane/kz-D17 df=<GB> load=<1m> at <time>
END KZ-D17 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
