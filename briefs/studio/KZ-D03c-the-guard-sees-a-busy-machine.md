# KZ-D03c: the load guard costs a wall it has not measured by how the wall up now measures against its bench costs, so a busy machine is seen before a load or a wake overloads it

Line numbers are from main at `b8714ee` (KZ-D03b merged) and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list and the cargo form below.

Written by Cassio, 13 Oct evening, from the KZ-D03b review (READY at `cd291c4`). The review re-took the kind cost table at loads 12 to 38, and Tom's live patch of 13 Oct is now costed at 61% against the 57% the wall measured. But the overload Tom heard tonight with the "grimy" save came from the machine: lane builds held the load average near 100, and the same modules took several times as long. The guard costs a wall it has not measured from the bench table (`kind_share`, `daemon/wall/cost.rs:114`), which cannot see that. The review's words: "That needs its own brief, for example scaling the incoming wall's costs by how the current wall measures against its bench costs." This is that brief.

Time: about 25 minutes of a builder's work. One mechanism: one factor, read from the wall up now, applied wherever a module is costed from the bench. Crates: kazoo-wall. Depends on KZ-D03b, which has landed. Runs before KZ-T05b (which costs a crossing's born modules through the same function).

Files (two):
1. `kazoo-wall/src/daemon/wall/cost.rs`: `BUSY_*` constants, `Wall::busy`, `Wall::kind_share_now`; its three uses; the sentences.
2. `kazoo-wall/src/daemon/wall/tests/cost.rs`: the tests.

## Outcome

On a quiet Mac nothing changes: the bench table errs high (61% for a 57% wall), so the factor is 1. On a busy Mac, where the wall up now renders at, say, 90% of real time while its bench costs add to 45%, every module the guard costs from the bench is costed at twice its bench cost. A seat's load or wake that would then pass 80% is refused, and the sentence says why: `…taking it to about 112% of its time rendering, past the 80% it keeps to (the machine is busy: the wall up now renders at 2.0× its quiet cost, and the new modules are costed the same), so it was not put up`. Tom's console still puts his loads up, with the same words in its warning.

## The design (decided; build exactly this)

### The factor (`cost.rs`)

```rust
/// Below this sum of bench shares, the wall up now is too small to say how
/// busy the machine is: one cheap module's timing is mostly noise.
pub(super) const BUSY_MIN_BENCH: f32 = 0.05;
/// The most the factor reads. A wall whose measure is eight times its bench
/// cost is already far past the guard; a larger factor says nothing more.
pub(super) const BUSY_MAX: f32 = 8.0;
/// From this factor up, the sentences say the machine is busy.
pub(super) const BUSY_SAID: f32 = 1.25;
```

`fn busy(&self) -> f32`:
- `bench`: the sum of `kind_share(kind, rate)` over `heard_modules(&self.patch)` (the bench cost of the wall up now, as `load_projection` costs an incoming wall).
- `bench < BUSY_MIN_BENCH`, or `render_load()` not finite → `1.0`.
- Else `(self.engine.shared().render_load() / bench).clamp(1.0, BUSY_MAX)`.
- Reason for the whole wall and not each module's own measured cost (`engine.cost`): a module's measured cost is the module alone, while a bench row is the module with a `vco` and an `out` and the engine's own work, so the two are not the same measure and their ratio is not 1 on a quiet machine. `render_load` and the sum of bench rows both count the whole render, and the review measured them at 57% and 61% on a quiet machine. Reason for the floor of 1: the bench errs high on purpose, and a quiet machine must never cost a module below its bench row.

`fn kind_share_now(&self, kind: Kind) -> f32`: `kind_share(kind, self.engine.sample_rate()) * self.busy()`.

### Where it is used

Every place a module the wall has not measured is costed from the bench, and nowhere else:
- `module_share` (`:178`): the fallback for a module with no measured cost (`kind_share`) becomes `kind_share_now`. A measured cost is used as it is: it was measured on the machine as it is.
- `load_projection` (`:265`): the incoming wall's `sum` uses `kind_share_now`. `current` and `rest` are unchanged.
- KZ-T05b's `crossing_load` (born modules): `kind_share_now` (T05b's brief is amended to say so; if T05b lands first, this lane changes its one call).
- `busy` itself uses plain `kind_share`, never `kind_share_now`.

`busy` is computed once per call of `load_projection`, `wake_room` and `crossing_load` (pass it down, or compute it at the top), not once per module. Reason: it walks the wall's modules, and a 90-module wall would walk them 90 times.

### What it says

When `busy() >= BUSY_SAID`, each of the three sentences (`wake_refusal`, `load_room`'s refusal, `load_room_as`'s warning) gains, before its last clause, ` (the machine is busy: the wall up now renders at <k:.1>× its quiet cost, and the new modules are costed the same)`. Below `BUSY_SAID` they are unchanged, byte for byte. Reason: Tom asked that the wall always say what is happening (memory: "Tom gets frustrated when UI doesn't communicate state"), and a refusal on a busy machine that names only the wall would read as the wall being wrong.

## Not this lane

Making reverb and delay cheaper or threaded (Tom, 13 Oct 19:5x; separate work). Measuring the machine's load average directly: the wall's own render time is the measure that matters to the sound, and it already includes the contention.

## Red tests first

In `daemon/wall/tests/cost.rs`, with D03b's harness (`guarded`, `set_render_load`, `set_cost`, `heavy`, `chain`).

1. `a_busy_machine_costs_new_modules_higher`: a wall whose heard modules' bench shares sum to 0.30; `set_render_load(0.60)`: `busy()` is 2.0; a save chosen so that its heard modules' bench sum `s` puts it inside the guard at a factor of 1 and past it at 2 is refused for a seat; the refusal's percentage is `rest + 2s` (computed in the test from the same functions, never typed in), and it holds the busy clause with `2.0×`. Every current module has a measured cost (`set_cost`), so `rest` does not move with the factor.
2. `a_quiet_machine_is_costed_at_the_bench`: the same wall at `set_render_load(0.25)`: `busy()` is 1.0, and the same save is put up; every sentence is byte for byte as before this lane (compare with D03b's expected strings).
3. `a_small_wall_says_nothing_of_the_machine`: a wall whose bench shares sum below `BUSY_MIN_BENCH`, at any render load: `busy()` is 1.0.
4. `the_factor_is_held_to_eight`: `set_render_load(9.0)` on a 0.30 wall: `busy()` is 8.0.
5. `a_measured_module_is_not_scaled`: D03b's `a_measured_cost_is_used_before_the_bench_cost` with `busy()` at 2.0: the measured module's share is its measured cost, unscaled; an unmeasured one's is twice its bench share.
6. `tom_hears_the_machine_is_busy`: a console load past the guard at `busy()` 2.0: put up, and the warning holds the busy clause.

## Checks

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall/src
```

## Definition of done

- [ ] The 6 tests fail first (paste each under RED), then pass.
- [ ] Files: the two above. D03b's tests all pass unchanged.
- [ ] Not the builder's (list it under NOT CONFIRMED): after landing and reinstalling, Cassio, with Tom's leave, on the live wall while two lanes build: `wall_listen`'s render load, and a seat's `wall_load` of the grimy save; the answer is pasted.

## Code quality

As PLAN.md's STACK RULES. `cost.rs` stays under 500 lines. No new state on `Wall`: the factor is read, never kept.

## END

```
START KZ-D03c base=<sha> branch=lane/kz-D03c df=<GB> at <time>
END KZ-D03c head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
