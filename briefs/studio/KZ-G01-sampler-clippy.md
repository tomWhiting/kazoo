# KZ-G01: kazoo-sampler passes clippy, so the workspace gate can go green

Line numbers are from the gate run that found these and may have moved; find each item by what it does, never rename what you find. Crate: kazoo-sampler only. Depends: nothing.

## Outcome

`cargo clippy -p kazoo-sampler --all-targets -- -D warnings` exits 0. Today it fails with 43 errors, and so the pooled gate (`cargo clippy --workspace --all-targets -- -D warnings`) fails on main. The crate came in with the commit that added the wall, effects, percussion, speech and sampler crates, and no lane tonight touched it. Behaviour does not change: every existing kazoo-sampler test passes before and after, unchanged.

## The design (decided; build exactly this)

Fix each lint at its cause. No `#[allow]`, no `#[expect]`, no `cfg_attr`, no lint configuration changes.

- `redundant_pub_crate` (about 26: `pub(crate)` struct, function, constant, enum or type alias inside a private module, in `player/envelope.rs`, `player/params.rs`, `player/voice.rs` and the rest the run names): make the item `pub` inside its private module. Visibility does not widen, because the module itself stays private; this is the same fix lane A01a used in kazoo-303.
- `suboptimal_flops` (4, e.g. `onset.rs:54`): rewrite `a * b + c` as `a.mul_add(b, c)`, keeping the same operands.
- `cast_possible_wrap` (4, `usize` to `i64`): convert with `i64::try_from(x)` and handle the error the way the surrounding code handles a bad length (return the existing error or clamp to the existing bound); never `as`, never `.unwrap()`. If a length can never exceed `i64::MAX` because of a bound checked earlier, say so in a comment and use `try_from` with that bound's error.
- `option_if_let_else` (2): use `map_or` or `map_or_else` as clippy suggests.
- `missing_const_for_fn` (2): add `const`.
- `float_cmp`-style `while` condition comparing floats (1): compare against an epsilon or restructure the loop to count steps; keep the loop's iteration count identical for every input the tests use.
- `unused_self` (1): make it an associated function or free function, updating its callers.
- `trivially_copy_pass_by_ref` (1, `player/envelope.rs:45`): pass by value.
- `struct_excessive_bools` (1): group the bools into a small enum or a bitflag-like struct of named states inside the crate; keep the public field names that callers outside the module use, if any (check with a grep across the workspace).
- `too_long_first_doc_paragraph` (1): split the doc comment into a one-line summary and a second paragraph.

## Red tests first

The red is the clippy run itself: save its full output to the logs as `red-clippy.txt` before any fix. No new tests are needed unless a fix changes a computed value; if `mul_add` or the float loop change any output, add a test pinning today's values within 1e-6 first and keep it green.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-sampler --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-sampler
cargo fmt --all --check
ast-grep scan kazoo-sampler
```

## Definition of done

- [ ] clippy exits 0 for kazoo-sampler, all targets, `-D warnings`.
- [ ] Every existing kazoo-sampler test passes; the count is the same before and after (or higher by any pinning tests added).
- [ ] No lint suppression anywhere; no file outside kazoo-sampler changed.
