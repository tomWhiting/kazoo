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
