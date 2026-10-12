# KZ-G01b: the resampler indexes in usize, with no fallback for lengths that cannot happen

Crate: kazoo-sampler only, file `kazoo-sampler/src/resample.rs`. Depends: G01 (landed). Line numbers move; find each item by what it does.

## Outcome

`Resampler::process` and `Resampler::output_len` hold no conversion with a made-up fallback. Today `process` computes `last = i64::try_from(input.len()).unwrap_or(i64::MAX) - 1`, casts `whole` to `i64` with `as`, and walks a signed index; `output_len` ends with `usize::try_from(..).unwrap_or(usize::MAX)`. The G01 review left these open: the fallbacks can never be taken, and if one were it would hide the fault. G01's reviewer fixed the same shape in `Halver::process` by working in `usize` throughout; do the same here. The output does not change by a single bit.

## The design (decided; build exactly this)

- `process` works in `usize`: the centre `whole` is `usize` (from the `u128` numerator by `usize::try_from`, see below), the window runs `first = whole.saturating_sub(reach - 1)` to `end = min(whole + reach, len - 1)` inclusive, with `reach` stored as `usize`, and an empty input returns an empty output before the loop. The distance is `(index as f64 - whole as f64 - frac)` written without a signed cast, e.g. `(f64_from_usize(index) - f64_from_usize(whole)) - frac` using whatever exact conversion the crate already uses for index to float (counts below 2^53 are exact); match the old arithmetic order so the bits are identical.
- The numerator: `whole = numerator / to` is at most the input length, so `usize::try_from(whole)` can fail only on a broken invariant. `process` has no error path: assert the invariant with a `debug_assert!` and break out of the loop with the samples produced so far if it fails. Never `unwrap_or` a value. Write the reason in one comment.
- `output_len`: return `Option<usize>` or keep `usize` and saturate with a comment that says why saturation is correct (an output longer than `usize::MAX` cannot be allocated anyway, and `Vec::with_capacity` would panic first). Pick saturation only if every caller is happy with it; check every caller in the workspace with grep.
- No `#[allow]`, `#[expect]`, `as` signed casts on indexes, `.unwrap()`, `.ok()`, `let _ =`.

## Red tests first

Before changing code add `resampling_matches_the_tap_by_tap_sum_bit_for_bit` next to G01's halver pin: for rates 44100->48000, 48000->44100 and 22050->48000, input lengths 0, 1, 2, 7, 64, 999 and 1000 of a fixed deterministic signal, compare `process` against a slow reference written in the test (plain f64 loop over every in-range tap) with `to_bits` equality. It is green on today's code (the change keeps behaviour); save its output to `red-resample-pin.txt` anyway, as the pin.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-sampler --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-sampler
cargo fmt --all --check
ast-grep scan kazoo-sampler
```

## Definition of done

- [ ] No `unwrap_or` fallback and no signed `as` cast left in `Resampler::process` or `output_len`.
- [ ] The bit-for-bit pin passes, and so does every existing kazoo-sampler test.
- [ ] No file outside kazoo-sampler changed; no lint suppression.
