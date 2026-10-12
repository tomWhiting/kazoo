# KZ-C09: a master chain on the desk, and `kazoo master <session>` renders a session offline

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-mix -p kazoo-fx`. Depends: KZ-C02 (insert chains), KZ-C08 (sessions).

## Outcome

What leaves the desk goes through a mastering chain: EQ, compressor, limiter, dither, in that order, each bypassable, all saved with the session. `kazoo master <session>` renders the session's takes through the strips and the chain to one WAV, faster than real time, without an audio device.

## The design (decided; build exactly this)

- Master bus (`kazoo-mix/src/engine.rs`, the master strip after the aux return and before the existing soft limiter): an insert chain (the type from KZ-C02) fixed to four slots: `kazoo_fx` parametric EQ (4 bands), compressor (threshold, ratio, attack, release, makeup; read kazoo-fx for the existing compressor; if none exists, implement a feed-forward RMS compressor with 1 ms lookahead in kazoo-fx with its own tests), the existing soft limiter moved into the chain as a slot with ceiling, and TPDF dither to 24 or 16 bits at the output only when rendering to a file (live output stays f32). Each slot pre-allocated; bypass is a flag, not a swap.
- Offline render: `kazoo-mix/src/render.rs`: `pub fn render(session: &Session, out: &Path, bits: Bits) -> Result<RenderReport>`: builds the same engine with no device, pulls blocks of 512 frames from the take players (KZ-C06) from the session start to the last clip end, runs strips, buses, master chain and dither, writes with hound as f32 (`Bits::F32`) or i24/i16 with dither. Report: frames, peak dBFS, true-peak estimate (4× oversampled peak), clipped samples.
- `kazoo master <session> [--out path] [--bits 24]` in the `kazoo` binary (KZ-A06) calls it.

## Not this lane

Per-strip inserts (KZ-C02), sessions (KZ-C08), a separate tape app (design/terminal-daw/TAPE.md; not in this plan).

## Red tests first

- `render::tests::silent_session_renders_silence_of_the_right_length`.
- `render::tests::a_take_at_minus_six_db_comes_out_at_minus_six_db_with_the_chain_bypassed` (sample-accurate within 1e-4).
- `render::tests::limiter_ceiling_is_never_exceeded` (a +6 dB take; every output sample under the ceiling).
- `render::tests::dither_changes_only_the_last_bit_on_average` (noise floor measured).
- `master::tests::chain_settings_round_trip_through_session_json`.

## Definition of done

- [ ] Five red tests then green.
- [ ] `kazoo master demo` writes a WAV Tom can open; the report prints peak and clipped count.
