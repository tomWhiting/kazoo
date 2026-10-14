# KZ-T08c: every moment keeps its sound: each commit carries what the wall sounded like when it was made

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Place: Track T, before T08a. Written by Cassio, 14 Oct 2026, 21:2x, from Tom on the tree view: "the graphics protocol would allow us to show the branches with different colours based on the sound". Decided by Cassio.

Time: about 25 minutes. Crates: kazoo-wall (`tree/git.rs`, the tree thread's commit path, `daemon/wall.rs` where a change is handed to the tree). Depends on nothing beyond main.

## The design (decided; build exactly this)

1. Every commit the wall writes from now on (a change, a start, a gap, a settle, a load) gains one trailer, after `Kazoo-Clock`: `Kazoo-Sound: rms=-27.2 peak=-9.9 centroid=599 low=0.284 mid=0.712 high=0.004 onsets=3.0`, the wall's latest `Listen` (`protocol/mod.rs`, the same numbers `wall_listen` gives) at the moment the change is made. Decibels and onsets to one decimal place, centroid in whole Hz, the shares to three places. `pitch` and `words` are not written; the numbers are the record.
2. When the wall has no listening yet (the first quarter second after start) or the latest one is older than 2 seconds, the trailer is left out. A missing trailer means unknown, never silence. A wall that is silent (nobody hearing it) still listens: the trailer is written whether or not anyone is hearing it.
3. The listening is read from where `Listen` is already kept on the wall's side; the commit path gets a copy taken when the change is made, so a change's sound is what was playing as it was made, not when the tree thread wrote it.
4. One parse function in `tree/git.rs`, `pub fn sound_of(message: &str) -> Result<Option<Sound>, TreeError>`, with `pub struct Sound { rms_db: f32, peak_db: f32, centroid_hz: f32, low: f32, mid: f32, high: f32, onsets: f32 }` (public fields, `Copy`, serde). A trailer that is present but malformed is an error naming the commit message's `Kazoo-Seq`, never a silent `None`. T08a reads it.
5. Commits written before this lane have no trailer; nothing rewrites them (the tree is append-only).

## Red tests first

1. `a_change_carries_its_sound`: a wall with a stub listening (rms -20, centroid 800, shares 0.3/0.6/0.1, onsets 2) makes a turn; the commit's message holds the exact trailer, and `sound_of` reads it back equal.
2. `no_listening_is_no_trailer`: before the first listening, and with one 3 s old: no `Kazoo-Sound` line; `sound_of` gives `None`.
3. `a_bad_trailer_is_an_error`: `Kazoo-Sound: rms=loud` gives an error naming the seq.
4. `the_sound_is_when_the_change_was_made`: the listening changes between the turn and the thread's write; the commit holds the first.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `uptime` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall
```

## Definition of done

- [ ] Each red test fails first (paste it), then passes; every kazoo-wall test passes; clippy, fmt and ast-grep exit 0.
- [ ] After landing and a restart, `git --git-dir ~/.kazoo/tree log -1` on Tom's Mac shows a `Kazoo-Sound` line (Cassio checks; NOT CONFIRMED until then).

## END

```
START KZ-T08c base=<sha> branch=lane/kz-T08c df=<GB> load=<1m> at <time>
END KZ-T08c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
