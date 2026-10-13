# Queue

One line per START, END and REVIEWED, newest last.
START KZ-D01 base=b6a1fea branch=lane/kz-D01 df=47GB at 00:30
START KZ-A01a base=b6a1fea branch=lane/kz-A01a df=47GB at 00:30
START KZ-B01 base=b6a1fea branch=lane/kz-B01 df=47GB at 00:30
START KZ-A01b base=02713cb branch=lane/kz-A01b df=32GB at 00:52
END KZ-A01a head=d948a55 reviewed=READY merged=b252fda at 00:54
START KZ-A01b base=c90f1b7 branch=lane/kz-A01b df=33GB at 00:55
START KZ-A01b base=c90f1b7 branch=lane/kz-A01b df=33GB at 01:01
START KZ-D01 base=b6a1fea branch=lane/kz-D01 df=29GB at 01:05
START KZ-B01 base=b6a1fea branch=lane/kz-B01 df=29GB at 01:05
START KZ-A01b base=c90f1b7 branch=lane/kz-A01b df=29GB at 01:06
START KZ-A01c base=c90f1b7 branch=lane/kz-A01c df=47GB at 01:19
END KZ-A01b head=5c34c65 reviewed=READY merged=5aab8cf at 01:23
START KZ-A01d base=ba390c3 branch=lane/kz-A01d df=59GB at 01:34
START KZ-A01e base=4a96a6a branch=lane/kz-A01e df=68GB at 01:35
TIMEOUT KZ-B01 branch=lane/kz-B01 kept at 81af833 (5 of 6 new tests green; instrument wake latency not met under load) at 01:38
END KZ-A01c head=4f59f35 reviewed=READY merged=3918c54 at 01:41
START KZ-A01f base=df857ac branch=lane/kz-A01f df=164GB at 01:44
END KZ-A01d head=0ac4585 reviewed=READY merged=a7cd574 at 01:47
START KZ-A01g base=7838f6d branch=lane/kz-A01g df=151GB at 01:52
END KZ-A01e head=bf65c50 reviewed=READY merged=7e951b9 at 01:55
END KZ-D01 head=b724197 reviewed=READY(harness; numbers under load, re-take) merged=7f4b334 at 02:00
REJECTED KZ-E01 head=8f54d24: builder split five existing kazoo-wall files into numbered parts (~13k lines churn); header now forbids splitting existing files; E01 to re-run fresh at 02:06
START KZ-D02 base=62622ce branch=lane/kz-D02 df=146GB at 02:06
START KZ-E02 base=62622ce branch=lane/kz-E02 df=143GB at 02:12
START KZ-B07 base=62622ce branch=lane/kz-B07 df=135GB at 02:20
END KZ-A01f head=a755c72 reviewed=READY merged=4103423 at 02:26
END KZ-A01g head=f8aae5a reviewed=READY(daemon timing tests to confirm on quiet machine) merged=e2934d0 at 02:33
START KZ-A02 base=b8f0d7d branch=lane/kz-A02 df=136GB at 02:35
START KZ-G01 base=24c4415 branch=lane/kz-G01 df=124GB at 02:44
END KZ-E02 head=a83b934 reviewed=READY(live-wall fold check for Tom) merged=d7384a5 at 02:45
START KZ-E01 base=ef2a040 branch=lane/kz-E01 df=127GB at 02:45
END KZ-B07 head=4130696 reviewed=READY(brief's hand check must use --target all or a UUID) merged=7f56fe8 at 02:49
PARKED KZ-D02 head=b9dbd6f reviewed=code correct, NOT READY until bench on a quiet machine (interleaved run at load ~200: base unplugged/1000 33.0/21.7 ms, lane 14.3/23.8 ms; noise ~2x, no clear 5x) at 02:59
START KZ-D07a base=f5069f1 branch=lane/kz-D07a df=116GB at 03:00
START KZ-D10a base=f5069f1 branch=lane/kz-D10a df=110GB at 03:03
END KZ-G01 head=56e05be reviewed=READY(reviewer made the halver work in usize with a bit-for-bit pin; open: Resampler::process last= unwrap_or clamp on an impossible length) merged=9668aa2 at 03:07
REVIEWED KZ-A02 head=e2de947 reviewed=fixed in review (any callback size, Bluetooth fallback, B07 merge); NOT landed until A02b (any channel count, finding 3); branch carried as lane/kz-A02b at 03:11
START KZ-A02b base=f5069f1 branch=lane/kz-A02b df=102GB at 03:12
INCOMPLETE KZ-D07a head=9fa8a35 branch=lane/kz-D07a kept: cost 0.40 default / 0.30 hot (needs 0.25), no_subharmonics -66.4 vs -80 dBc, latency 0.097 vs 0.05 samples at 192k; builder made a new 'models' bench + src/bench.rs instead of using D01's circuit_models (undo on retry) at 03:26
START KZ-G01b base=b632e4d branch=lane/kz-G01b df=76GB at 03:27
END KZ-E01 head=e546fed reviewed=READY(review fixed stepped-knob check, console overwrite now asks via :chain replace, mcp test folder; Tom's by-hand check: save beat as a chain, add it, hear two kicks) merged=f711c65 at 03:29
INCOMPLETE KZ-D10a head=76d23ae branch=lane/kz-D10a kept: sinc table -66.2 dB vs -70 (brief error: 1024 phases, retry at 2048), spring/two 0.82 vs 0.6 at load 240, two spring tests interrupted; hall 0.26-0.29, plate 0.12-0.24 at 03:31
START KZ-D12 base=303fc88 branch=lane/kz-D12 df=71GB at 03:31
START KZ-D10a base=f5069f1 branch=lane/kz-D10a df=62GB at 03:32
START KZ-D07a base=f5069f1 branch=lane/kz-D07a df=55GB at 03:33
END KZ-G01b head=036c50e reviewed=READY(nothing to fix; pin passes on old and new code) merged=a71619a at 03:35
END KZ-A02b head=50cd375 (A02 e2de947 + A02b) reviewed=READY(review added io/channel_adapt.rs so the tui engine's duplex path adapts to the opened width; open items -> A02c: double probe in run.rs, processing.rs stereo-only render) merged=6aaf7c1 at 03:41
INCOMPLETE KZ-D12 head=aca9f89 branch=lane/kz-D12 kept: EQ coefficient bound 7e-4 vs 1e-4 (wrong measure; retry judges response within 0.01 dB), glide bench 5.62->5.59 ms (pan trig per sample found; retry adds it), fx suite interrupted at load at 03:58
START KZ-A02c base=a7bfad1 branch=lane/kz-A02c df=69GB at 03:58
START KZ-D12 base=303fc88 branch=lane/kz-D12 df=68GB at 04:00
START KZ-B01 base=b6a1fea branch=lane/kz-B01 df=66GB at 04:01
END KZ-D07a head=72b512e reviewed=READY(review made the step count a compile-time const; carried into D07b: latency correction at every factor, name sinh(T_MAX); cost target 0.25 withdrawn, design gives ~0.5) merged=c4c0454 at 04:07
END KZ-D10a head=2452a90 reviewed=READY after hand merge with D07a's bench files (review fixed hall diffusers jumping on a size glide; spring default target withdrawn, spring items carried into D10b; merged-lane checks: clippy 0, fmt 0, ast-grep 0, 66 space/drive tests pass) merged=49848a6 at 04:12
START KZ-D07b base=41b4de1 branch=lane/kz-D07b df=49GB at 04:13
END KZ-A02c head=62fd8ee reviewed=READY(review removed unused OutputRequest::info and resolve_sample_rate; smoke 808 and mini exit 0) merged=0e055e9 at 04:19
START KZ-A03 base=e16415c branch=lane/kz-A03 df=43GB at 04:24
START KZ-D10b base=a8390fe branch=lane/kz-D10b df=40GB at 04:26
REVIEWED KZ-B01 head=6597d95 reviewed=fixed in review (close unparks before join; wake test now at production priority, which fails p50 388-488 us at load ~100); NOT landed until B01b (instrument links run real-time; refusal shown on screen); branch carried as lane/kz-B01b at 04:35
INCOMPLETE KZ-D12 head=e287d13 branch=lane/kz-D12 kept: reviewed NOT READY, callback slower (flat +4.2%, gliding +7.3% instructions); causes and second retry decisions in the brief at 04:48
START KZ-D09a base=611fced branch=lane/kz-D09a df=46GB at 04:51
END KZ-D07b head=7a7ee28 reviewed=READY(screamer padding back to factor 2 only with reason; amp tail is the input coupling cap draining, test waits 2 s at 1e-7 and asserts decay; amp null -42.7/-33.4 dB; bench ratios need a quiet machine) merged=137c80a at 04:53
INCOMPLETE KZ-D10b head=ec921b7 branch=lane/kz-D10b kept: the bounded alignment search misses the period (shimmer 63/200, pitch 14/200 within a sample), shimmer contract 2.10 dB at 192 kHz vs 1.5, bench slower at load 167; the design's search budget needs rethinking before a retry. The carried spring item (third coil skipped when silent) is done and green in this branch at 04:54
START KZ-D09b base=06aba3a branch=lane/kz-D09b df=44GB at 04:57
END KZ-A03 head=2229dd7 reviewed=READY(review fixed: a link with no desk keeps looking; a refusing desk leaves the instrument playing direct; live desk showed 'kazoo-808 plugged into strip 1'; open items -> A03b) merged=64db48b at 05:01
START KZ-B01b base=a8390fe branch=lane/kz-B01b df=54GB at 05:01
END KZ-D09a head=c48cd72 reviewed=READY(refresh proven by mutant; caching walk strengthened; open: octave on-off-on test, written into the D09 brief; bench needs a quiet machine) merged=a79c79c at 05:25
END KZ-D09b head=e5a92a6 reviewed=code READY(review fixed: missing table falls back to the exact curve instead of panicking; null -147 dB; VHS 0.7 target withdrawn by Cassio, VHS hi-fi path and parts.rs size written into the D09 brief) merged=2255bda at 05:31
END KZ-B01b head=0520853 (B01 6597d95 + B01b) reviewed=READY(real merge with main kept every A03 fix; review fixed five instrument badges and the desk's 808 line hiding the connection state; wake p50 158.6 us real-time at load 125; open items appended to the A03b brief) merged=e58e857 at 05:39
START KZ-G02 base=26c6ab0 branch=lane/kz-G02 df=34GB at 05:47
END KZ-G02 head=65bf7b3 reviewed=READY(review fixed: each test names its log folder once, socket-length message gives path and length; speech test EmptyRender under load is unrelated, needs its own kazoo-speech brief) merged=faa48c9 at 06:10
START KZ-A06 base=a38d9dd branch=lane/kz-A06 df=37GB at 07:41
START KZ-A04 base=4335ad1 branch=lane/kz-A04 df=26GB at 07:48
START KZ-A05 base=4335ad1 branch=lane/kz-A05 df=24GB at 07:49
START KZ-A08 base=4335ad1 branch=lane/kz-A08 df=26GB at 07:49
START KZ-D08b base=f8b38a8 branch=lane/kz-D08b df=36GB at 07:51
START KZ-D14 base=f8b38a8 branch=lane/kz-D14 df=36GB at 07:51
START KZ-D12 base=c8702ba branch=lane/kz-D12 df=36GB at 07:52
START KZ-D10b (third run, resume) branch=lane/kz-D10b df=35GB at 07:52
END KZ-A06 head=1941fe7 reviewed=READY(review fixed: kazoo arp keeps --target and --silent; built before A04/A05 with color_eyre::install; kazoo-arp plugged_in_the_arp_asks_the_desk test flaky at load 236, passes alone) merged=bb06923 at 08:09
START KZ-A04 base=3d82ea1 branch=lane/kz-A04 df=28GB at 08:22
START KZ-A05 base=3d82ea1 branch=lane/kz-A05 df=28GB at 08:22
START KZ-D08b base=3d82ea1 branch=lane/kz-D08b df=28GB at 08:22
START KZ-D14 base=3d82ea1 branch=lane/kz-D14 df=28GB at 08:22
START KZ-A04 base=c65de47 branch=lane/kz-A04 df=34GB at 13:04
START KZ-A05 base=c65de47 branch=lane/kz-A05 df=34GB at 13:04
START KZ-B02 base=c65de47 branch=lane/kz-B02 df=34GB at 13:04
START KZ-W01a base=158d731 branch=lane/kz-W01a df=25GB at 13:14
START KZ-A07 base=158d731 branch=lane/kz-A07 df=96GB at 13:27
START KZ-D03 base=158d731 branch=lane/kz-D03 df=95GB at 13:29
REVIEWED KZ-A05 head=6aa336c reviewed=READY(review fixed cs80 Ctrl+S/Ctrl+L playing notes, arp footer and cs80 sketch; arp desk socket test fails same on base at load 455); merge after A04 at 13:43
TIMEOUT KZ-W01a code complete, verification cut short; committed and sent to review to finish checks at 13:45
REVIEWED KZ-W01a READY head=e2bfe7c (Opus: 1439/1439 nextest, clippy, fmt, ast-grep, doc tests clean; live-wall seed checks wait for the landing reinstall) at 14:1x; merge HELD until Tom says go
REVIEWED KZ-A07 READY head=d127fcd (Opus fixed 9 faults incl. `up` dying at the first instrument on tmux 3.5a target syntax, `down` never finding the desk, `down` killing foreign sessions, failed attach killing the studio; 28/28, clippy, fmt, ast-grep clean; live up/down run in an isolated HOME) at 14:1x; merge HELD until Tom says go. Follow-up: the desk has no SIGTERM handler (kazoo-mix)
REVIEWED KZ-D03 READY head=cc0a97a (Opus fixed 3: stale delay-line replay when a module is heard again, unheard speaker refusal, MCP singular wording; 846/846; bench A/B: chains/10 no regression (builder figure was load), culled/1000 10 ms -> 0.17 ms) at 14:3x; merge HELD until Tom says go. Follow-up decided by Cassio: the load guard must cost every module a patch makes heard (the whole newly-heard set, at its measured or catalogue cost), not the cable as one unit, and unheard modules must not pull the per-module average down; goes in D04 with D01 bench rows for chains/10 and culled/1000
END KZ-B02 head=797e59c reviewed=READY(fixed refusal Display panic, uncounted backlog drop, docs; p99 A/B overlaps base) merged=1140b44 at 13:48
START KZ-B03a base=1872329 branch=lane/kz-B03a df=53GB at 15:10
MERGED KZ-A05 as 6214771 (+ fmt c-commit), with A04 part 2 fa3c8d8; merged tree clippy clean, nextest 3752/3752; pushed at 15:50 on Tom's 'move through it as best we can'
START KZ-V01 base=1e616d7 branch=lane/kz-V01 df=28GB at 15:50
START KZ-L01 base=1e616d7 branch=lane/kz-L01 df=22GB at 16:05
MERGED KZ-W01a 779e316, KZ-A07 4369e77, KZ-D03 247ca07 (render.rs clash resolved); combined fmt/clippy/nextest green; pushed at 16:19
START KZ-L02 base=4c2b6a0 branch=lane/kz-L02 df=31GB at 16:20
REVIEWED KZ-L01 READY head=2203f2d (Opus fixed 7: existing dirs to 0700, new dirs flushed to parent, cut-short reel is WrongFormat, cleanup keeps the real error, two messages, sweep lists once; 941/941) at 17:46. Decided by Cassio for KZ-L04: put's dedupe verifies the existing file's hash (rewrite if corrupt); .corrupt-* files count against the budget and are swept after SWEEP_GRACE. Brief nits: WAV header is 68 bytes; serde was a second new dep.

## Merged 13 Oct, about 18:4x
- KZ-L01 the reel store (review head 2203f2d), KZ-L02 the loop module (review head 652404d; feed messages boxed, see the note in L03/L04), KZ-B03a named streams (review head 1b81a1c; old desks fall back, ring for 16 stereo streams). Merge check on main: fmt 0, clippy 0, nextest 3855/3855, doc tests 0.
- To re-measure on a quiet Mac: L02's loop bench rows (113–123 µs per 512-frame block against 106.7 µs, measured at load 48–68).
START KZ-W01b base=42664d7 branch=lane/kz-W01b df=43GB at 18:24
START KZ-W01c base=42664d7 branch=lane/kz-W01c df=41GB at 18:24
START KZ-D03b base=42664d7 branch=lane/kz-D03b df=41GB at 18:25
START KZ-A03b base=42664d7 branch=lane/kz-A03b df=29GB at 19:50

## Merged 13 Oct, about 20:3x
- KZ-W01b (review head 26526da), KZ-W01c (review head a86327d), KZ-V01 amendment 1 (review head dc5777a; judge 1.5 ms of 2.1, engine 6.4 of 8.53; engine starts at every rate from 8 kHz). Merge check on main: fmt 0, clippy 0, nextest 3880/3880, doc tests 0. BENCH.md kept both sections.
START KZ-V02 base=96051a2 branch=lane/kz-V02 df=10GB at 20:25
START KZ-V07a base=96051a2 branch=lane/kz-V07a df=9GB at 20:26
START KZ-F01 base=96051a2 branch=lane/kz-F01 df=6GB at 20:27
START KZ-T01 base=acf8b92 branch=lane/kz-T01 df=1GB at 20:29

## Merged 13 Oct, about 22:1x
- KZ-D03b (review READY at cd291c4; the review fixed four: undos that replug skipped the guard, a zero timing pinned a cost at 0, the kind table taken at load ~100 was 3-6x high and refused every save, its own clippy). Merge check on main b8714ee: fmt 0, clippy 0, doc 0, ast-grep 0; nextest first run 887 passed and 1 failed (kazoo-core `ipc::link::tests::wake::a_queued_block_wakes_the_link_thread_at_once`, p50 219 us against 200 us, load 72-98; not touched by the lane), rerun --no-fail-fast 3892/3892 and that test alone passed. Logs: target/kz-logs/full-D03b-2139.log, full-D03b-nff-2147.log.
- Defect to fix (rule 8: a test that fails under load is a defect, never hidden): the wake test's 200 us p50 bound fails at load ~90. Needs a brief: measure the wake against a same-run baseline, or assert on the wake mechanism, not wall time.
- To re-measure on a quiet Mac: D03b's culled/1000 +20.8% (160.3 to 193.6 us); cause not found (clock reads are ~33 ns).
- Follow-up brief written: KZ-D03c the guard sees a busy machine (Waffles read it; taken).
- KZ-A03b (review READY at 0229a01; the review fixed: one stuck link counted twice, hub_link left public as a second setup path, a footer test widened). Merge check on main efef1b6: fmt 0, clippy 0, nextest 3905/3905, doc 0, ast-grep 0 (target/kz-logs/full-A03b-2227.log).
- Follow-up to brief (A03b review finding 4): `stuck_shutdowns` can never be above 0 while an instrument runs, so the 808's badge never shows; instruments should print the stuck and undelivered counts on exit, and the badge go.
- Held: KZ-A08 (its Justfile gate is a per-repository gate script, against Tom's rule 9; to ask Tom).
- Briefs written for the two follow-ups above: KZ-B01c (the wake test counts woken parks instead of a 200 us bound) and KZ-A03c (stuck link counts printed on exit; the 808 badge goes). Both ready, no dependencies.
- Track P: all 17 briefs read by Cassio and committed; Track P merged into PLAN.md (P08's key is `v`, P15's is `C`: `t` is tap tempo and `c` the click on the desk). Openable now: P01, P04, P10.
- KZ-T01 merged (review READY c9a545c; fixed duplicate birth marks). Merge check on main 09511e0: fmt 0, clippy 0, nextest 3920/3920, doc 0, ast-grep 0 (target/kz-logs/full-T01-2311.log).
- KZ-V02 merged (review READY 102afca; seven fixed: pYIN reference at the judge's hop, latency one step short, tie rule, steps_missed, float headroom, linked fixtures, suite test). Merge check on main 196ea41: fmt 0, clippy 0, nextest 3935/3935, doc 0, ast-grep 0 (target/kz-logs/full-V02-2354.log). V03's brief updated from the review (per-event latency, no missed events, pyin opt-level 3).
START KZ-V03 base=ac26501 branch=lane/kz-V03 df=48GB at 00:26
START KZ-D03c base=ac26501 branch=lane/kz-D03c df=48GB at 00:27
- Follow-ups from F01's review (to brief): kazoo-fx/src/space/spring.rs is 1250 lines (over 1000); kazoo-speech's test Scratch (testing.rs:15) makes folders in the system temp directory (against the no-temp rule).
START KZ-T02a base=ac26501 branch=lane/kz-T02a df=109GB at 02:14
START KZ-L03 base=ac26501 branch=lane/kz-L03 df=109GB at 02:15
START KZ-P01 base=ac26501 branch=lane/kz-P01 df=108GB at 02:15
- KZ-F01 merged (review READY 67267cf), KZ-D03c merged (review de3682a: wake costed against its own pending change, refused wake costed twice; doc tests ran here), KZ-V03 merged (review READY d3b4c27: mechanism changed to marks at both crossings, half-period window, glide carried forward; targets unchanged and met). Merge check on main 4cdb36f: fmt 0, clippy 0, nextest 3965/3965, doc 0, ast-grep 0 (full-F01-D03c-V03-0205.log).
- To hear with Tom: V03's glide carry (any overshoot at a glide's end) once a synth follows the voice (V09).
START KZ-V04 base=538cfbe branch=lane/kz-V04 df=64GB at 03:26
START KZ-B01c base=538cfbe branch=lane/kz-B01c df=64GB at 03:26
START KZ-A03c base=538cfbe branch=lane/kz-A03c df=63GB at 03:26
- KZ-T02a merged (review READY 0607f1c: kazoo-mcp Change literals, a refused load no longer starts a clock line). Merge check on main 1617372: fmt 0, clippy 0, nextest 3977/3977, doc 0, ast-grep 0 (full-T02a-0326.log).
START KZ-T02b base=da446f9 branch=lane/kz-T02b df=61GB at 04:20
START KZ-P04 base=da446f9 branch=lane/kz-P04 df=61GB at 04:21
START KZ-P10 base=da446f9 branch=lane/kz-P10 df=61GB at 04:22
- Follow-up to brief (V04 review finding 4): the cycle tracker moving an octave from our own hint or slipping mid-note counts as a sung leap and turns folding off for ~43 ms; no suite take triggers it.
- Follow-up (A03c review finding 7): a desk link dropped on an early return (stream open or terminal enter failing) is not reported on exit; report on those paths or from each main after run.
- KZ-V04 merged (review READY d5083ee: hot input no longer resets the tracker; one re-sung syllable no longer two onsets; tests tightened). Merge check on main 382ee42: fmt 0, clippy 0, nextest 3992/3992, doc 0, ast-grep 0 (full-V04-0437.log).
- Follow-ups from L03's review (to brief): L04 should store the guard frame in a reel so a restored tape is exact at the seam; L02 defect: recording fades linearly over the seam and playback crossfades equal-power on top, so a sustained sound dips about 3 dB for 8 ms at every seam; L02: the write position rounds to the nearest cell, so a pass's last frame can land on cell 0. Size: daemon/wall.rs 4804 lines, store.rs 3394.
- KZ-B01c merged (review READY 455f153: woken bound kept, ticked reported only) and KZ-A03c merged (review READY f11af29). Merge check on main 430e0ed: fmt 0, clippy 0, nextest 3996/3996, doc 0, ast-grep 0 (full-B01c-A03c-0534.log).
- KZ-P01 merged (review READY e4972ed: full 1 ms crossfade on every jump, reset_slack at once, tests strengthened, 22 mutants caught; holds the last sample at zero lag, accepted) and KZ-L03 merged (review READY 8ee6e7c; tick conflict kept both). Merge check on main 83eeda7: fmt 0, clippy 0, nextest 4041/4041, doc 0, ast-grep 0 (full-P01-L03-0556.log).
- Follow-ups from T02b's review: (defect to brief) KAZOO_TREE_DIR is inherited by in-process test daemons, so if Tom sets it in his shell kazoo-wall's tests would commit into his real tree: the daemon's tree directory must come from explicit config in tests, never the environment. To re-measure on a quiet Mac: T02b's feed_tree 2.56 ms a turn at load 120-167 (target 0.5 ms). Older: a plain --release build of kazoo-wall's tests fails (assert_no_alloc disable_release removes AllocDisabler).
START KZ-V05 base=cf3cac7 branch=lane/kz-V05 df=66GB at 06:33
START KZ-V06 base=cf3cac7 branch=lane/kz-V06 df=65GB at 06:33
- KZ-T02b merged (review READY 948b422: the tree's packs are repacked, not kept). Merge check on main e241b75: fmt 0, clippy 0, nextest 4057/4057, doc 0, ast-grep 0 (full-T02b-0632.log).
START KZ-T03 base=1c2dbfb branch=lane/kz-T03 df=60GB at 07:18
START KZ-D04 base=1c2dbfb branch=lane/kz-D04 df=60GB at 07:20
- Follow-up from V05's review (decided by Cassio, to brief as V05b): on an onset frame the quantiser's hysteresis must not hold the previous note's target; a new note picks its nearest scale note (hold A, silence, sing A# -40 cents -> A#, not A). Hysteresis is for wobble within a held note, not across notes.
- KZ-V05 merged (review READY 3a2a7b2: a scale change retargets a held note; off is Hz for Hz through silence; 11 mutants caught). Merge check on main c19a52a: fmt 0, clippy 0, nextest 4072/4072, doc 0, ast-grep 0 (full-V05-0727.log).
