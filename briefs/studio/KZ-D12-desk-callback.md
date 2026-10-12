# KZ-D12: the desk callback stops doing trig, idle reverb and denormal maths

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-mix -p kazoo-fx`; benches from KZ-D01 before and after. Depends: KZ-D01.

## Outcome

Three costs on the desk's audio callback go: EQ coefficient recomputes with trig while a gain glides, the aux reverb running when nobody sends to it, and denormal slow-downs in the delay and chorus feedback paths.

## The design (decided; build exactly this)

- EQ (`kazoo-mix/src/engine.rs`, the per-strip 3-band EQ whose coefficients are recomputed about every 32 frames during a glide; survey: hub.md perf risk 4): per band, a table of biquad coefficient sets at 0.25 dB steps from -15 to +15 dB (121 entries × 5 coefficients) built once at engine creation for the device rate; during a glide the band interpolates linearly between the two nearest entries each 32 frames (linear interpolation of coefficients between neighbouring small steps is stable for peaking/shelving biquads; the test proves no instability). No `sin`/`cos`/`tan` on the callback after construction.
- Aux reverb: the master's Freeverb return is skipped for a block when every strip's send is below -90 dB **and** the reverb's own tail energy (sum of absolute values of its delay lines sampled every 64th sample, kept as a running number) is below 1e-7; otherwise it runs. The tail therefore rings out and only then stops costing.
- Denormals: `kazoo-fx` delay and chorus feedback adds (find `delay.rs` and `chorus.rs` feedback lines) use the same hand-flush helper the wall's filters use (find it in kazoo-wall; move it to `kazoo-core` as `dsp::flush_denormal(f32) -> f32` so both use one copy; it adds and subtracts a tiny DC, or clamps `abs < 1e-20` to 0: use whichever the wall already does).

## Not this lane

SIMD (KZ-D11), inserts (KZ-C02).

## Red tests first

- `eq::tests::table_interpolation_matches_direct_coefficients_within_1e_4` across every step.
- `eq::tests::a_glide_from_minus_15_to_plus_15_db_produces_no_sample_above_plus_20_db` (stability).
- `engine::tests::idle_reverb_costs_nothing_after_its_tail` (a counter on the reverb's process; sends at zero; after 3 s of silence the counter stops rising).
- `fx::tests::delay_feedback_never_goes_denormal` (feed a short impulse, run 60 s, every state value is 0 or `abs >= f32::MIN_POSITIVE`).

## Definition of done

- [ ] Four red tests then green; `desk_callback` bench at 64 strips before and after in the END block (target: at least 25% lower with gains gliding).
