# KZ-T11a: a moment sounds the same every time the wall goes to it: a go lands on the next bar, and the wall it puts up starts from its own beginning there, every counter, phase and random stream from zero at that downbeat

Line numbers are from main at `eab16dd` and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 00:4x, from an Opus agent's diagnosis of Tom's report. Decided by Cassio; reasons beside each decision.

Place: Track T, T11. Time: about 60 minutes. Crates: kazoo-wall (`engine/` for the origin and the timed swap, `dsp/mod.rs` `Tick`, `daemon/wall.rs` `swap_engine`, and the tests; the modules that read the song position, `dsp/clock.rs`, `dsp/lfo.rs`, read it through `Tick` and should need no change). Runs after KZ-T04d (it touches `wall.rs`), before KZ-T05b; T05b and T05f build on it (see Outcome 5).

## Why

Tom, 15 Oct 2026, about 00:40, walking the tree with the arrows: "sometimes moving around between changes, it doesn't come back quite sounding the same." Stopping and starting the wall did not change it.

The seeds are right: every moment's `wall.toml` holds the wall seed (`tree.rs:551`), `from_files` reads it back (`tree.rs:697-706`), and each module's seed comes from the wall seed and its id only (`chance.rs:21-23`, used at `wall.rs:1685`). What differs is where each module starts relative to the beat:

- `swap_engine` (`wall.rs:1583-1617`) builds every module afresh, but the engine's song position carries on through the swap (`wall.rs:1576-1581`).
- Clocks and synced LFOs read the song position (`clock.rs:60-66`, `lfo.rs:68-72`), while step counters start at the go: `seq` from `step: None` (`seq.rs:49-60`), the rhythms by counting clock edges (`perc.rs:108-147`), free LFOs at phase 0 (`lfo.rs:31-41`), random streams at draw 0.
- So the beat the go lands on decides where each pattern's step 1 falls against the clocks. Moment 6904 has 8 clocks, 8 sequencers of 12 and 16 steps, 5 rhythms and 7 synced LFOs: the same notes, a different groove, on every visit.

## Outcome

1. **A moment is its files, started from its own beginning.** Decision: of the two ways to make a go repeatable, (a) putting every module where it would be had the wall played those files since the song began, or (b) starting the moment from its beginning on a downbeat, (b) is total and exact for every module kind (a counter clocked by an LFO through a logic chain cannot be placed by (a) without rendering hours of song), and it is what a player expects of launching a scene. A go never resumes the old wall's running state: delay lines, the hall's tail, envelopes and slews start empty, as they do now.
2. **The wall keeps an origin.** The engine holds `origin`, the song position (in beats) at which the wall now playing started. Every module sees the song position as beats since the origin: `Tick::beat` and `Tick::beat_at` are measured from it (the engine's own song position, the desk link, recordings and the console's bar display keep the absolute position; only the modules see the origin's). At the wall's first start the origin is 0, so nothing changes for a wall that is never replaced.
3. **A go lands on the next bar.** `swap_engine`'s hush, removes, inserts and unhush are applied by the engine at the first frame of the next bar of the song position (4 beats, as the console counts bars), and the origin becomes that frame's beat. Decision: on a bar so that the moment's patterns start on a downbeat every time; the wait is at most one bar (2 seconds at 120 BPM). The swap commands are queued in the engine's command channel as today and held until that frame; nothing is allocated in the callback (pre-allocate what the hold needs at engine start). The go's answer says when it lands (`lands on bar 2931`), and a second go before the first has landed replaces it (the first never sounds). A load (`wall_load`) and an undo of a go land the same way.
4. **Random streams restart with the moment.** Every module's random stream is re-seeded at its build (as now) and draws from zero at the origin, so with (2) and (3) the same moment gives the same samples from its downbeat on, every visit, for every kind: sequencer chances, the branch coin, random LFO shapes, sample and hold, noise.
5. **For the crossings (T05b to T05f).** A crossing replaces the swap: its arriving modules start at the crossing's start, and the arrival is set on a bar so the arriving wall's origin is a downbeat. T05b's brief is amended to say so; this lane only adds the origin and the timed swap.
6. **The tree's gap wording.** The start commit after a restart says "the tree holds changes {first} to {last} as one commit" (`git.rs:331`, range `parent seq + 1 .. seq`), which counted changes that already had their own commits and go requests, which by design have none (tonight: 6905 to 7145, where 6905 to 7073 had commits). The range is computed from the log: only changes that have no commit and are not goes are named, and with none, the subject says `the wall started again after change {seq}`.

## Tests (write them first; each fails before the change)

- A wall with a clock, a 12-step `seq` with chance 0.5 clocked by it, a `euclid` rhythm, a free LFO at 0.013 Hz and a random-shape LFO into a VCO: render 8 bars after a go to it from song beat 37.3, then go to another wall and back to it from song beat 1,201.9; the 8 bars after each landing are sample-identical.
- A go requested at beat 37.3 lands at beat 40 (the next bar), not before; the modules' `Tick::beat` reads 0 at that frame; the console's bar display still reads the song position.
- Two goes within one bar: only the second lands, at that bar.
- No allocation in the callback while a go is held and lands (the crate's `assert_no_alloc` guard).
- A wall that is never replaced renders exactly as before this lane (origin 0).
- The gap commit after a restart names only changes with no commit of their own and no goes; with none, its subject is `the wall started again after change {seq}`.
