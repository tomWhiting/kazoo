# KZ-D06: the wall renders its graph on a pool of real-time threads

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-wall`; benches from KZ-D01 before and after. Depends: KZ-D05 (and KZ-D04 for the line rules).

## Outcome

A wall that is four independent chains of modules renders on four cores. With the pool set to one thread the output is bit-identical to today. This is the lane that makes "thousands of modules" possible on a 10-core Mac.

## The design (decided; build exactly this)

- Partition (`engine/tables.rs`, after the topological sort): a *chain* is a maximal run of consecutive slots in the sorted order where every input cable of a slot comes either from inside the run or from a cable that carries a delay line (KZ-D04 rule: a line is read from the previous block, so it is safe across threads). Chains are independent by construction; the partition is rebuilt with the tables (KZ-D05 keeps it incremental for the slots touched).
- Pool (`engine/pool.rs`): `n = available_parallelism() - 1` clamped to 1..=8 workers (the audio thread is one more), created once at engine start, each raised to real-time priority through the `audio_thread_priority` dependency already in the tree. Work is handed out per block: the audio thread writes the chain list into a pre-allocated `Vec<ChainJob>` and bumps an `AtomicU64` generation; workers spin on the generation for up to 50 µs then `park`; each claims chains with an `AtomicUsize` fetch_add; the audio thread works too; completion is an `AtomicUsize` counter the audio thread spins on (no `Mutex`, no `Condvar` on the audio path; `unpark` is allowed, as in KZ-B01).
- Output buffers are per slot and already pre-allocated, so a worker writes only its chain's slots. The `out` sum (KZ-D02's list) runs after the barrier on the audio thread.
- `pool = 1` (console `:pool 1`, env `KAZOO_WALL_POOL`) runs everything on the audio thread through the same code path.

## Not this lane

SIMD (KZ-D11), cull (KZ-D03), the circuit models.

## Red tests first

- `tables::tests::partition_splits_independent_chains_and_keeps_dependent_ones_together` (three hand-built graphs).
- `engine::tests::pool_four_is_sample_identical_to_pool_one` (a 200-module, 4-chain patch, 2,000 blocks, exact equality).
- `pool::tests::a_block_with_no_chains_completes` and `workers_park_when_idle` (CPU time of workers over one idle second under 5 ms).
- `engine::tests::no_allocation_on_the_audio_path_after_start` (the existing allocation-counting harness if there is one; otherwise a counting global allocator in the test binary).

## Definition of done

- [ ] Five red tests then green; `wall_render` bench for the 1,000-module 4-chain fixture before and after in the END block (target: at least 3× faster at pool 4).
- [ ] The live 41-module wall at pool 4 shows render load under half of today's 0.88 (say the number).
