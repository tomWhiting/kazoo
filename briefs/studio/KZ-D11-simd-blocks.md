# KZ-D11: block maths through the `wide` crate

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-wall -p kazoo-mix`; benches from KZ-D01 before and after. Depends: KZ-D02.

## Outcome

The loops every block pays for on every module and strip (gathers, knob application, the out sum, the finiteness scan, the desk's strip sum) run eight samples at a time, with the output sample-identical to today.

## The design (decided; build exactly this)

- Add `wide = "0.7"` (safe SIMD, no `unsafe` needed by the caller) to kazoo-wall and kazoo-mix.
- Wall (`kazoo-wall/src/engine/mod.rs`, the per-slot block loop after KZ-D02): the input gather (source buffer × cable amount into the jack buffer), knob-jack application (knob value + cable × amount, clamped), the `out` sum over the out list, and the finiteness scan (`is_finite` over the four output buffers; replace with a `wide::f32x8` compare + `any`) become helpers in a new `engine/simd.rs`: `gather(dst: &mut [f32; 32], src: &[f32; 32], amount: f32)`, `add_scaled`, `sum_into`, `all_finite(&[f32; 32]) -> bool`. 32-frame blocks are four `f32x8` lanes; no remainder path.
- Desk (`kazoo-mix/src/engine.rs`): the strip sum into the master and the per-strip gain/pan apply use the same helpers (a `kazoo-core` home for them: `kazoo-core/src/dsp/simd.rs`, re-exported; decide that so both crates share one copy).
- Scalar fallbacks are not kept: `wide` compiles to scalar where no SIMD exists.

## Not this lane

Changing which modules render (KZ-D03), cable lines (KZ-D04), parallel render (KZ-D06).

## Red tests first

- `simd::tests::gather_matches_scalar_bit_for_bit` (random blocks, 10,000 trials, exact equality: the ops are mul-add free so bit equality holds; if `wide` uses FMA on this target and equality fails, assert `<= 1 ulp` and say so under FOUND).
- `simd::tests::all_finite_sees_a_nan_in_any_lane`.
- `engine::tests::render_is_sample_identical_before_and_after` (a fixed 20-module patch rendered 1,000 blocks; the reference rendered by the scalar helpers kept in the test module).

## Definition of done

- [ ] Three red tests then green; `wall_render` and `desk_callback` benches before and after in the END block (target: the fixed per-module cost at least 2× lower than after KZ-D02).
