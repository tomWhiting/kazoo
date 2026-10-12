# KZ-D12: the desk callback stops doing trig, idle reverb and denormal maths

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-mix -p kazoo-fx`; benches from KZ-D01 before and after. Depends: KZ-D01.

## Outcome

Three costs on the desk's audio callback go: EQ coefficient recomputes with trig while a gain glides, the aux reverb running when nobody sends to it, and denormal slow-downs in the delay and chorus feedback paths.

RETRY NOTE for KZ-D12 (Cassio, 13 Oct 04:00). The first run (branch lane/kz-D12, kept on origin) built the EQ tables, the reverb tail gate, `kazoo_core::dsp::flush_denormal` and the delay and chorus flushes; three of four reds went green. Decisions for the retry:
- Accuracy is judged on the response, not on the coefficients. The coefficient bound of 1e-4 was the wrong measure: the worst case (192 kHz, 14.875 dB, high shelf) is -8.296713 against -8.295979, 0.0007 dB. Replace `table_interpolation_matches_direct_coefficients_within_1e_4` with `table_interpolation_matches_the_direct_response_within_a_hundredth_of_a_db`: for every band shape, every gain at 1/8 dB steps over -15..+15, at 44.1, 48, 96 and 192 kHz, the magnitude response of the interpolated biquad at 64 log-spaced frequencies from 20 Hz to 0.45 fs is within 0.01 dB of the directly designed one. The 0.25 dB table step stays.
- Speed: the first run measured no gain with gains gliding (5.62 ms to 5.59 ms at load about 130), so the EQ is not where a gain glide spends its time. The run found the pan law doing trigonometry per sample on the callback (`kazoo-core/src/lib.rs` Pan, used from `kazoo-mix/src/engine.rs` near 1022). Gliding pan and gain laws are computed once every 32 frames and linearly interpolated across them, as the EQ is; no `sin`/`cos`/`tan`/`powf`/`exp` per sample on the desk callback while anything glides. Add `engine::tests::a_gain_and_pan_glide_does_no_trigonometry_per_sample` using the existing design counter pattern. Record before and after interleaved (before, after, before, after) with the load average next to each; the reviewer judges the 25% on a quiet machine. Do not spend the time box chasing the number.
- `attach_to_invalid_slot_returns_the_consumer` LEAK in nextest: find out whether the test leaves a thread or handle behind; fix it if it is this crate's test.
- The fx suite's long tests (hall freeze, more than 300 s at load) are not this lane's; run `-p kazoo-fx` filtered to `delay` and `chorus` plus `-p kazoo-mix -p kazoo-core` in full.

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
