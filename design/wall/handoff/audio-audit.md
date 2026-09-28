> **DONE (26 Sep 2026, evening):** everything below is landed in live, including `--rate` as an explicit device override. kazoo-wall gates: fmt clean; clippy clean (no findings in kazoo-wall; kazoo-fx has its own in-progress warnings); tests 228 + 44 + 11 + 2 pass; ast-grep 0 findings. This file is kept for the record.

# Audio-audit handoff (phase 2): the wall's audio fixes

**Status (26 Sep 2026):** everything is built and tested in a scratch copy of kazoo-wall, but none of it is in the live tree yet. The copy's full lib suite passed 208 of 211 tests before the last few changes: the limiter's 30 ms hold, 96-tap f32 detection, the VCF oversampler resting at zero drive, the f32 sine in the VCO and the VCO output bound of ±4. The per-module tests were re-run after those changes; the full suite was not. The 3 failures are listed under "Test changes to apply when landing" below.

Landing was blocked because live `kazoo-fx` did not compile. `drive/{fuzz,klon,screamer}.rs` import `super::pair`, and `drive/mod.rs` did not declare `mod pair` (fx-drive was mid-edit). **Do not land until `kazoo-fx` compiles.** Update: `drive/mod.rs` now declares `mod pair`. Check that it builds, then land.

## Where the work is

- **Durable copy:** `~/kazoo-audio-audit-scratch/audit/`, a copy of the session scratchpad without its `target/`.
  - `snap/kazoo-wall/src/…`: the working copy of kazoo-wall, holding my files.
  - `snap/kazoo-{core,fx,perc,speech}`: snapshots of the other crates.
  - `sync.sh`: refreshes the snapshot from live without touching my files. It also adds `sweeps` to any `Io { .. }` literal outside my files. Its paths point to the old scratchpad, so edit `S=` first.
  - `meas/`: the measurement harness. It uses `CARGO_TARGET_DIR` inside that dir.
  - `meas-before`: the harness binary built against the old code.
  - `out-*.txt`: the before and after numbers.
- `~/kazoo-audio-audit-scratch/phase2/`: prototypes. Superseded; ignore them.

## Files to land (all under kazoo-wall/src), copied from `~/kazoo-audio-audit-scratch/audit/snap/kazoo-wall/src/`

Land all at once; they depend on each other.

1. `dsp/oversample.rs` (new): polyphase IIR halfband up/downsamplers (HIIR design) and `factor_for`.
2. `dsp/mod.rs`:
   - `Sweep`, the new `Io.sweeps` field, and `Io::knob_at`, which now reads per frame and follows glides. Adds `Io::knob_moves`.
   - `Smooth` and `poly_blep` are removed.
   - vco, noise and vcf now receive the rate from their builders.
   - The test bench has `Bench::at(kind, rate)`, `testing::tone` and `testing::db`.
   - **Merge before copying:** live `dsp/mod.rs` now has wall-engine's arm `Builder::Adapted(adapter) => crate::adapters::build(adapter, sample_rate),` in `build()`. Keep it. Diff live against the scratch copy and carry across any other live-only lines.
3. `dsp/vco.rs`:
   - Cubic B-spline BLEP and BLAMP, 2-sample delayed corrections.
   - Oversampled to at least 176.4 kHz, f64 phase.
   - Output bound ±4.
4. `dsp/vcf.rs`:
   - Drive oversampled to at least 352.8 kHz. It rests when the drive is at zero, primes on the last 64 samples, and crossfades over one sub-block.
   - The output is linear up to ±2, with a knee up to ±4.
   - Knobs are read per frame.
5. `dsp/vca.rs`, `dsp/mix.rs`, `dsp/out.rs`: per-frame knobs, no smoothing.
6. `dsp/noise.rs`: normalised to 48 kHz. Above 60 kHz it is band-limited to 22 kHz (4th order) and raised by √(fs/48k). Pink and brown poles are fixed in Hz.
7. `engine/mod.rs`:
   - `Glide` moves in knob position, and `advance` returns a `Sweep`.
   - The minimum glide is 1 ms (`MIN_GLIDE_SECONDS`).
   - Adds the `sweeps` scratch and `min_glide` fields.
8. `engine/master.rs`, a new chain:
   1. f64 SVF Butterworth high-pass at 10 Hz.
   2. Look-ahead true-peak limiter, −1 dBTP (`CEILING = 0.891_250_9`, detector target −1.2 dB):
      - look-ahead 1.5 ms, hold 30 ms, release 150 ms;
      - peaks found by 8× interpolation, 96 taps, f32, 8 lanes, skipped below an exact bound.
   3. A guard that silences NaN and holds ±1.
   The saturation stage is removed. Latency is the look-ahead plus 48 frames.
9. `engine/tests.rs`:
   - `knobs_glide_linearly_to_their_target` is replaced by `knobs_glide_evenly_along_their_travel`, which checks the geometric midpoint and that a zero glide lands after about 96 frames.
   - Adds `a_glide_reaches_the_module_frame_by_frame`.
   - `outs_feed_the_device_channels` now settles 480 frames before reading peaks.
10. `daemon/audio.rs`:
    - `Device::open(Option<u32>)`; `open_default()` still exists and calls `open(None)`.
    - Asks for a fixed buffer (512 frames, 1024 above 96 kHz) when the device's supported range allows it.
    - Adds the `Dither` / `Quantise` TPDF dither for i16, u16 and i32 (i32 dithered at 24 bits), with tests.

**Merge, never copy over, `dsp/mod.rs`, `dsp/effect.rs` and `engine/mod.rs`.** wall-engine is adding to them in the meantime:
- `Module::latency()`, default 0, in `dsp/mod.rs`;
- `EffectModule::latency()` in `dsp/effect.rs` (not otherwise in my set);
- latency-compensation gather/routing in `engine/mod.rs`.

Port only my hunks onto the live versions: Glide/Sweep, `insert`, `knob`, `render_module`, the `Io` literal, and `min_glide`/`sweeps`. The VCO and VCF report latency 0: their delays are fractional, frequency-dependent and at most a couple of samples.

`MAX_KNOBS` is now 24 in live. My code only refers to it by name, so that is fine.

## Test changes to apply when landing (agreed with wall-engine)

1. **`daemon/wall/tests.rs`, `the_snapshot_shows_values_on_their_way`:** replace
   `assert!((cutoff.value - 1_400.0).abs() < 20.0, "{cutoff:?}");`
   with
   `assert!((cutoff.value - (900.0_f32 * 1_900.0).sqrt()).abs() < 20.0, "{cutoff:?}");`
   and add the comment "half way along a log knob is the geometric middle".
2. **`adapters/speech/tests.rs`:** in the `Io { .. }` literal in `play()`, add `sweeps: &[crate::dsp::Sweep::REST; MAX_KNOBS],`.
3. **Zero-glide renders:** any test that expects the exact value one frame after a `glide_frames: 0` turn must now render at least 64 frames first. The two known engine tests are already updated in my `engine/tests.rs`. The full `-p kazoo-wall` run will find any others.
4. **perc `prob` (22 knobs):** already fixed in live by `MAX_KNOBS = 24`. Nothing to do.

## Still left

- **`--rate` as a device override.** wall-engine confirmed they are not editing these files, so go ahead:
  - `audio.rs`: `AudioMode::Device` becomes `Device { sample_rate: Option<u32> }`.
  - `main.rs`: `--rate` without `--no-audio` sets it (default `None`, which follows the device's rate; it must not switch Tom's DAC unless asked).
  - `daemon/mod.rs`: around lines 88 and 513.
  - `daemon/control.rs`: around lines 195–236, `Device::open_default()` becomes `Device::open(rate)`.
  - Update the usage text.
- **Documentation.** The team lead updates DESIGN.md (ceiling −1 dBTP, no saturation). Nothing of mine refers to the old −3 dBFS except the old `master.rs`, which is being replaced.
- **Tell the speech agent** that `Io` gained `sweeps`.

## Next commands (after kazoo-fx compiles)

```sh
# 1. copy my files into live (after merging dsp/mod.rs as above), apply the test changes
# 2. gates, one at a time, low priority:
cd /Users/tom/Developer/projects/deno_rust/kazoo
nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo fmt -p kazoo-wall --check
nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo clippy -p kazoo-wall --all-targets -j 4 -- -D warnings
nice -n 19 lockf -k /tmp/kazoo-cargo.lock cargo test -p kazoo-wall --lib -j 4
ast-grep scan kazoo-wall/src
```

Never build `--release` of kazoo-wall into the repo's `target/`: Tom's live wall runs `target/release/kazoo-wall`. Measurements use the harness with its own target dir.

## Gotchas

- **The pulse overshoot must not be clamped.** The VCO bound used to be ±1.5. At that bound it clipped the band-limited pulse's overshoot (Gibbs plus the halfbands' phase), and the clipping aliased at −33 dBc at 48 kHz. That is why it is ±4.
- **Check the stopband at mid-band frequencies.** When testing the oversampler, frequencies whose alias lands near DC or Nyquist give misleading LS-fit numbers.
- **Noise tests are statistical.** They use a Q 2 band-pass over 8 s, with a tolerance of 0.6 dB.
- **The one true-peak case still over the ceiling:** a naive, fully aliased square at 192 kHz reads −0.75 dBTP against an ideal FFT reconstruction. It reads ≤ −1.0 against a 128-tap reconstruction, which is what the unit test uses.
- **CPU for a 40-module patch at 192 kHz:** 0.61 ms per 512 frames, 23% of that buffer's budget. It was 0.45 ms.
- **Before and after numbers** are in `out-after.txt` and in the report to the team lead.
