# KZ-D14: Tom's live patch renders at half the cost it does today

Crates: kazoo-wall, plus kazoo-fx where the cost lives. Tom's words at 07:50 on 13 Oct: "there's only a handful of modules in there and it's more than half full rendering. So something has got to be done." The wall reads 57% of real time for 41 modules and 51 cables. The patch is in `kazoo-wall/benches/live-patch-2026-10-13.json`, copied from `~/.kazoo/wall/patch.json`. The heavy modules are klon1 (gain 100), fuzz2, fuzz3 and fuzz4, tape1, radio2 (megaphone), pitch2, hall1, digital1 and bbd1. The rest are vco, vcf, vca, env, lfo, clock, euclid, seq, mix, metal, kick, scope, readout and out. D02 (no cost for unplugged jacks) is on main. D08b (klon) runs in parallel in another lane, so leave the klon's diode solve alone.

## Outcome

1. A bench `wall/live_patch` in `kazoo-wall/benches/wall_render.rs` loads this JSON through the same patch-loading code the daemon uses and renders 48000 frames. Next to it, a per-module measure: render the patch once with every module present, then with each module's kind swapped out in turn, or time each module's `process` directly through the harness. Write a table of each module's share of the render to `briefs/studio/BENCH.md` under "live patch, 13 Oct". Record the machine load with the numbers.
2. Take the top three costs that are not the klon and make each cheaper without changing its sound beyond its existing null-test thresholds. Use the techniques this repo already uses: tables in place of transcendental functions, oversampling only where the drive needs it, skipping work when an input is silent or a mix is 0 or 1, and caching the values of knobs that are not moving. Each change gets a null test against the old code, or uses one that exists.
3. Re-run the bench and add the after column.

## Rules

The time box is 25 minutes. Measure first, so the table exists even if a fix runs out of time. No `#[allow]`, `#[expect]`, `.ok()` or `let _ =`, and nothing in the system temp folder. Run nextest for the crates you touch, clippy with `-D warnings`, `cargo fmt --all --check` and ast-grep.

## Measured by the first D14 run (13 Oct 08:09, before its fix; machine load 80 to 340, so shares, not absolute times)

| module | kind | before share % |
|---|---|---:|
| klon1 | klon | 22.697 |
| tape1 | tape | 8.049 |
| vcf1 | vcf | 1.587 |
| fuzz2 | fuzz | 14.453 |
| crush2 | crush | 5.062 |
| vcf2 | vcf | 2.491 |
| fuzz3 | fuzz | 11.171 |
| radio2 | radio | 1.562 |
| pitch2 | pitch | 3.636 |
| hall1 | hall | 7.002 |
| fuzz4 | fuzz | 12.052 |

Next run: klon1 is 23% and the three fuzzes 38% together; D08b (klon) and the shared Fuzz implementation are the two fixes. The first run had started on `kazoo-fx/src/drive/fuzz.rs` and `pair.rs`; it was cancelled at 08:10 so the install build could run alone.
