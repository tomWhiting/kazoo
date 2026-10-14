# Benchmarks

Numbers from Tom's Mac unless a row says otherwise. A block is 512 frames at
48 kHz (10,666,667 ns). Benches run as:

```sh
nice -n 10 env CARGO_BUILD_JOBS=4 cargo bench -p <crate> --bench <name> -- '<filter>'
```

## 2026-10-13 at c391124 (lane KZ-D01, taken under load, not the baseline)

Every number in this section was taken on 13 Oct 2026 while the machine ran
other builds with a load average near 100. These numbers are not the
baseline: no later lane measures against them. The baseline is to be taken
again on a quiet machine and written as its own section; until then no row
of this file is a baseline.

Machine: MacBook Pro (MacBookPro18,2), Apple M1 Max, 10 cores, macOS 26.6.2,
rustc 1.97.1 (8bab26f4f 2026-07-14).

Source base: b6a1fea, with the bench lint fixes then uncommitted. The values
are Criterion console interval midpoints. Several bench commands ran at the
same time as each other and as the other builds, so they include that load
and do not establish isolated costs. The wall run used the filter
`wall/(chains/1000|cables/10000)` and exited 0.

| bench | N | ns per block | blocks per second | % of real time |
|---|---|---|---|---|
| wall/chains/1000 | 1000 | 291,080,000 | 3.435 | 2728.875 |
| wall/cables/10000 | 10000 | 141,610,000 | 7.062 | 1327.594 |
| fx/klon | 1 | 5,883,300 | 169.973 | 55.156 |
| fx/screamer | 1 | 11,393,000 | 87.773 | 106.809 |
| fx/fuzz | 1 | 4,213,800 | 237.315 | 39.504 |
| fx/amp | 1 | 12,736,000 | 78.518 | 119.400 |
| fx/fold | 1 | 4,990,400 | 200.385 | 46.785 |
| fx/crush | 1 | 809,860 | 1234.781 | 7.592 |
| fx/ringmod | 1 | 1,884,900 | 530.532 | 17.671 |
| fx/comp | 1 | 1,721,900 | 580.754 | 16.143 |
| fx/tape | 1 | 2,300,200 | 434.745 | 21.564 |
| fx/bbd | 1 | 311,070 | 3214.711 | 2.916 |
| fx/digital | 1 | 245,040 | 4080.966 | 2.297 |

Control-thread benches (not real time: ns per operation and operations per
second; the look's size in bytes in the last column):

| bench | N | ns per op | ops per second | bytes |
|---|---|---|---|---|
| control/build/100 | 100 | 421,750,000 | 2.371 | |

Not measured at all (no number exists for these rows):
all `wall/kind/*`; wall chains 10 and 100; unplugged 100 and 1000; cables 10,
100 and 1000; culled 1000; the remaining 20 FX catalogue entries; desk callback
8, 64 and 512; link encode and decode 512; control build 1000 and look 100,
1000 and 10000.

Two rows taken later, on 13 Oct 2026 at `7f4b334` (the D01 merge) by the
KZ-D03b review, in its worktree, with
`nice lockf -k ~/.kazoo-cargo.lock cargo bench -p kazoo-wall --bench wall_render -j 4 -- 'wall/culled|wall/chains/10$'`,
one-minute load average 12.59 at the start and 20.98 at the end (5- and
15-minute 29.98/56.19 and 29.17/53.29), on the same machine; log
`target/kz-logs/review-bench-d01.log`:

| bench | ns per block | interval ns | % of real time |
|---|---:|---|---:|
| wall/chains/10 | 560,460 | [558,580, 562,540] | 5.254 |
| wall/culled/1000 | 13,936,000 | [13,747,000, 14,140,000] | 130.650 |

Criterion's lines: `wall/chains/10 time: [558.58 µs 560.46 µs 562.54 µs]`
and `wall/culled/1000 time: [13.747 ms 13.936 ms 14.140 ms]`.

The look's size is a count of bytes, not a timing, so load does not move it.
The review's one-pass run of the control bench (`-- --test`, at 3ab9adb)
printed the serialised `look` result as 90,133 bytes for 100 vcos, 900,837
bytes for 1000 and 9,043,841 bytes for 10000.

The tree's mix has four named inputs and four knobs, not 28 named jacks.
The fixture uses those eight jacks, `ceil(C / 8)` leaves, and counts the
summing-tree routes within C. This preserves the support test's exact total
route count and makes every leaf audible. It differs from the brief's fixture.

The Criterion logs these rows came from were under `target/`, which was
deleted outside the lane at about 01:23 on 13 Oct; they no longer exist, so
the rows above cannot be checked against their source.

## KZ-D02

Before: unchanged 62622ce on lane/kz-D02. The prescribed wall_render filter
exited 0. Values are Criterion interval midpoints for 512 frames at 48 kHz.
Every number below was taken on 13 Oct with the machine under heavy load
(load average about 200 to 280 from other work) and is not evidence of the
wall's cost before or after this lane. The release build took 10m 24s; the
first filtered nextest build took 8m 03s.

| bench | before ns per block | before % real time | after ns per block | after % real time |
|---|---:|---:|---:|---:|
| wall/unplugged/100 | 2,925,000 | 27.421875 | not measured | not measured |
| wall/unplugged/1000 | 31,766,000 | 297.806250 | not measured | not measured |
| wall/chains/1000 | 256,210,000 | 2401.968750 | not measured | not measured |

Before log: target/kz-logs/bench-before.txt. The after column was not
measured: the after run did not finish inside the builder's time box, and
the review did not run benches under that load. The 5x speedup, the below
25% of real time, and the chains within 3% requirements are not verified;
both columns are to be measured on a quiet machine.


## KZ-D03

Base: 158d731, lane/kz-D03, uncommitted. The same prescribed filter ran
before and after, serially: `wall/culled|wall/chains/10$`. The before
fixture rendered every slot. The after fixture applies the same `heard`
reachability as the wall before sorting its order. The engine render code
is unchanged. Both fixtures retain every cable.

Criterion interval midpoints, ns per 512-frame block at 48 kHz:

| bench | D01 ns per block | D03 before ns per block | D03 after ns per block |
|---|---:|---:|---:|
| wall/chains/10 | 560,460 | 1,538,300 | 2,529,700 |
| wall/culled/1000 | 13,936,000 | 29,436,000 | 2,295,500 |

The measured ratio is 0.9074x after, within the 1.2x limit, versus
19.1354x before. After intervals: chains/10 [2,132,500, 2,981,300] ns;
culled/1000 [1,881,700, 2,722,700] ns. These runs shared the machine with
other lane work and scoped validation builds. Their broad intervals and
the chains/10 variation do not establish isolated render costs or a quiet
machine baseline. The limit above compares the observed midpoints.

The D01 column was filled by the KZ-D03b review from its run at `7f4b334`
(see the D01 section): a far lighter load than these D03 runs, so the
columns are not comparable with each other.

Receipts: target/kz-logs/bench-before.txt and
target/kz-logs/bench-after.txt. Both commands exited 0.

### KZ-D03 review: base and head alternately

The reviewer ran `cargo bench -p kazoo-wall --bench wall_render --
'wall/culled|wall/chains/10$'` three rounds, base 158d731 then head
44c08e5 each round, 14:13 to 14:18 on 13 October, load average 100 to
130 (the builder's run shared the machine at about 190 to 260). Criterion
midpoints, per 512 frame block; log `target/kz-logs/review-bench-ab.log`.

| round | chains/10 base | chains/10 head | culled/1000 base | culled/1000 head |
|---|---:|---:|---:|---:|
| 1 | 884.23 µs | 538.28 µs | 10.408 ms | 166.95 µs |
| 2 | 520.93 µs | 518.70 µs | 10.946 ms | 166.04 µs |
| 3 | 538.15 µs | 525.40 µs | 9.7957 ms | 189.20 µs |

chains/10 did not regress: its patch has no unheard module, so its order
is the same on both sides and the engine is unchanged; the builder's 1.54
to 2.53 ms was load. culled/1000 after is 0.32 to 0.36 times chains/10
after, inside the 1.2 limit; before it was about 20 times.

## KZ-L02 loop

Target: each of `wall/loop/play` and `wall/loop/record` costs at most
106,667 ns per 512-frame block at 48 kHz (1% of real time). Eight loops
then consume less than a tenth of one core, leaving room for circuit models.
The fixture holds eight bars; the record row overdubs with keep 0.7.
As in the existing module fixtures, the measured wall also has an input
oscillator and a stereo output.

These measurements are under load, not a quiet-machine baseline. The
one-minute host load was 267.84 before the initial run, 380.65 during the
second run, and 410.18 before the final checks. Package checks ran during
the first two measurements. No performance acceptance is established.

| revision measured | play ns/block | record ns/block | play real time | record real time |
|---|---:|---:|---:|---:|
| initial implementation | 113,490 | 124,340 | 1.064% | 1.166% |
| wrapped-index and knob caching revision | 565,260 | 708,050 | 5.299% | 6.638% |
| finite-marker snapshot revision | 136,450 | 128,240 | 1.279% | 1.202% |

All three runs exited 0. None met the target. The final observed one-minute
host load was 312.92. The last change preserving delayed punch stamps
was made after the final benchmark, so that small change was not measured. Their different host load
prevents attributing the timing change to the implementation. Logs:
`target/kz-logs/bench-loop.txt`, `target/kz-logs/bench-loop-final.txt`, and
`target/kz-logs/bench-loop-current.txt`.

Review at 16faf61, 13 Oct 2026, back to back with the builder's head
ae15e80, under load (one-minute load 68.39 before the ae15e80 run, 48.29
before the 16faf61 run). The filter `wall/(loop|kind/vco$)` adds
`wall/kind/vco` (two vcos and an out) beside the loop rows (one vco, the
loop and an out), as a measure of what the fixture costs:

| revision | play ns/block | record ns/block | kind/vco ns/block |
|---|---:|---:|---:|
| ae15e80 (builder) | 119,680 | 120,930 | 119,510 |
| 16faf61 (review) | 113,110 | 122,760 | 120,320 |

The review's changes do not move the rows beyond the run's spread. Each
loop row costs about what two vcos do, so the loop itself costs less than
its fixture's vco; the rows as measured still sit above the 106,667 ns
target, and no quiet-machine measurement exists. Logs:
`target/kz-logs/review-bench-lane.log`, `review-bench-head-2.log`.

## KZ-V01

Source base: 1e616d7, with the voice judge changes uncommitted in lane/kz-V01.
Each operation is one 512-sample push at 48 kHz, after priming with 4096
samples of a 130.81 Hz saw, harmonics 1 through 20 weighted by 1/k, peak
0.5. Detector construction, priming and destruction are outside the timed
operation. N is the number of pYIN frames per run. Values are Criterion's
reported time estimates; each run collected 100 samples and exited 0.

| bench | N | ns per block | blocks per second | % of real time |
|---|---|---|---|---|
| judge/hop/48k | 5 | 69,472,000 | 14.394 | 651.300 |
| judge/hop/48k | 3 | 63,456,000 | 15.759 | 594.900 |

The five-frame interval was 63.183–76.289 ms. The three-frame interval was
57.071–70.229 ms. The 1/5/15-minute load averages before the commands were
119.95/126.84/115.98 and 114.00/123.96/118.15, respectively. A sample during
the five-frame measurement was 127.81/126.02/117.95; after the three-frame
measurement it was 99.12/115.75/116.23. Other lane work and
this lane's nextest runs were active. These are loaded measurements, not
quiet-machine costs or evidence of a speedup.

The five-frame result exceeded 2.1 ms, so the judge was reduced to three
frames and measured again. Three frames remain the final configuration,
with two preceding frames of HMM context. Both loaded results exceed the
budget; the quiet-machine 2.1 ms budget is not verified.

Logs: target/kz-logs/bench-five.txt and target/kz-logs/bench-three.txt, with
load samples in target/kz-logs/bench-five-load.txt,
target/kz-logs/bench-five-measure-load.txt, target/kz-logs/bench-three-load.txt
and target/kz-logs/bench-three-after-load.txt.

Review, on lane/kz-V01 at 34793ee with JUDGE_FRAMES = 3: judge/hop/48k read
18.802 ms (interval 18.129 to 19.515 ms), load 75.07/82.82/108.05 before and
75.60/82.14/107.06 after; log target/kz-logs/review-bench.log. To separate
the cost from the load, pyin 1.2.0 alone was timed in a scratch binary as
user CPU time over 100 runs at 48 kHz, frame 2048, hop 512, the same saw:

| range | resolution | pYIN frames | CPU ms per run |
|---|---|---|---|
| 50 to 4000 Hz | 0.1 | 1 | 7.9 |
| 50 to 4000 Hz | 0.1 | 3 | 14.8 to 15.5 |
| 50 to 4000 Hz | 0.1 | 5 | 21.7 to 23.5 |
| 60 to 1000 Hz | 0.1 | 3 | 6.0 |
| 50 to 4000 Hz | 0.2 | 3 | 3.6 |
| 50 to 4000 Hz | 0.34 | 3 | 1.4 |

The cost is CPU, not load: pyin's Viterbi runs over 1518 states (759 pitch
bins, voiced and unvoiced) and takes the log of the whole 1518 by 1518
transition matrix on every call. At three frames one run costs more CPU than
the 10.7 ms hop it judges, so on a live 48 kHz device the analysis thread
cannot keep up. The 2.1 ms budget is out of reach at this range and
resolution with any frame count.

### Amendment 1

Source head: d52bb81, with Amendment 1 uncommitted in lane/kz-V01. Both
detectors use three pYIN frames and return the newest frame. The voice
judge uses 50–4000 Hz, resolution 0.34 and a 512-sample hop. The engine
uses its default 60–1000 Hz range, resolution 0.1 and a 2048-sample hop.

CPU costs below use `/usr/bin/time -p` on the release voice bench's
`--cpu-judge` and `--cpu-engine` modes, each making 100 consecutive
hop-sized pushes after 4096 samples of the same saw. Matching
`--cpu-judge-setup` and `--cpu-engine-setup` modes construct and prime the
detector and prepare the identical signal without those 100 pushes. The
setup-only user time is subtracted before dividing by 100. All four
commands exited 0. The timer reports hundredths of a second, giving about
0.1 ms precision per run after subtraction.

| bench | N | ns per block (user CPU) | blocks per second | % of real time |
|---|---|---|---|---|
| judge/hop/48k | 3 | 1,500,000 | 666.667 | 14.063 |
| engine/hop/48k | 3 | 6,300,000 | 158.730 | 14.766 |

Judge user time was 0.17 s for 100 runs plus setup and 0.02 s for setup:
1.5 ms per run, below 2.1 ms. Even including setup gives 1.7 ms per run.
Engine user time was 0.66 s and 0.03 s: 6.3 ms per run, below 8.533 ms
(20% of its 42.667 ms hop); including setup gives 6.6 ms. The judge's
recorded review cost at resolution 0.1 was 14.8–15.5 ms above. Resolution
0.34 meets the CPU budget, so resolution 0.5 was not needed or measured.
CPU-run load averages were 120.60/107.71/98.92 before and
120.55/107.92/99.04 after. These runs were serial, with this lane's tests
and builds finished.

Criterion wall-time results, each with 100 samples and exit 0:

| bench | N | ns per block | blocks per second | % of real time |
|---|---|---|---|---|
| judge/hop/48k | 3 | 1,987,300 | 503.195 | 18.631 |
| engine/hop/48k | 3 | 6,811,500 | 146.810 | 15.964 |

The judge interval was 1.8073–2.2104 ms, with load averages
124.09/105.76/97.70 before the command. The engine interval was
6.5374–7.1340 ms, with load averages 95.98/103.62/97.86 before and
87.78/99.22/96.56 after. These are loaded wall times; CPU time is the
amendment's budget gate. pYIN rounds `1 / resolution` upward, so 0.34
actually produces 456 voiced/unvoiced states rather than the brief's 446.

Logs: target/kz-logs/amendment-cpu-{judge,engine}.txt,
target/kz-logs/amendment-cpu-{judge,engine}-setup.txt,
target/kz-logs/amendment-cpu-load-{before,after}.txt,
target/kz-logs/amendment-bench-{judge,engine}.txt and
target/kz-logs/amendment-bench{,-engine}-load-before.txt.

### Second review (Opus, 13 Oct, head 10649b9)

Measured the same way as Amendment 1: the release voice bench's `--cpu-*`
modes under `/usr/bin/time -p`, 100 hop-sized pushes, setup-only user time
subtracted. All commands exited 0. Load averages 71.52/77.66/90.59 before
the Criterion runs and 57.07/72.70/88.01 before and after the CPU runs.

| bench | N | ns per block (user CPU) | blocks per second | % of real time |
|---|---|---|---|---|
| judge/hop/48k | 3 | 1,500,000 | 666.667 | 14.063 |
| engine/hop/48k | 3 | 6,400,000 | 156.250 | 15.000 |

Judge 0.17 s − 0.02 s, engine 0.67 s − 0.03 s, over 100 runs. Both are
under budget (2.1 ms; 8.533 ms). Criterion wall time: judge/hop/48k
1.3870 ms (1.3697–1.4050), engine/hop/48k 6.2770 ms (6.1513–6.4304).
Logs: target/kz-logs/review-bench.log, review-cpu-*.log,
review-bench-load-{before,after}.log, review-cpu-load.log.
||||||| 42664d7
## KZ-D03b

Base `42664d7`, 13 October 2026, this worktree on Tom's Mac.
The base kind run exited 0. All rows are nanoseconds per 512-frame block.
The initial uptime load averages were 106.31 / 93.51 / 88.70; at completion
96.80 / 103.24 / 99.54. Per-row uptime was not captured, so these rows
carry the run's load range, not an independently verified per-row load.
The loaded fixture includes its feeding vco and receiving out, so these
are conservative fallback costs rather than isolated module CPU costs.

| kind | lower ns | midpoint ns | upper ns | real time at 48 kHz % | load averages at run start; end |
|---|---:|---:|---:|---:|---|
| vco | 246,220 | 315,280 | 406,400 | 2.955750 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| lfo | 153,900 | 190,800 | 233,450 | 1.788750 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| noise | 81,851 | 122,460 | 182,900 | 1.148062 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| vcf | 431,570 | 528,620 | 633,510 | 4.955812 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| vca | 326,010 | 423,150 | 551,850 | 3.967031 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| env | 239,290 | 299,770 | 367,480 | 2.810344 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| clock | 310,990 | 349,550 | 388,360 | 3.277031 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| seq | 384,250 | 439,570 | 502,010 | 4.120969 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| sh | 369,300 | 448,740 | 536,020 | 4.206937 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| quant | 374,690 | 440,970 | 509,630 | 4.134094 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| slew | 392,040 | 466,330 | 546,360 | 4.371844 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| mix | 393,160 | 447,710 | 510,670 | 4.197281 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| out | 437,480 | 502,350 | 581,620 | 4.709531 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| compare | 583,510 | 694,290 | 826,410 | 6.508969 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| logic | 504,750 | 583,050 | 662,680 | 5.466094 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| branch | 635,350 | 718,550 | 795,330 | 6.736406 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| switch | 957,240 | 1,368,000 | 1,865,100 | 12.825000 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| select | 487,310 | 590,900 | 707,630 | 5.539687 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| divide | 366,480 | 422,790 | 486,340 | 3.963656 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| math | 448,860 | 518,570 | 591,690 | 4.861594 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| scope | 433,630 | 469,920 | 508,980 | 4.405500 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| readout | 451,300 | 490,350 | 529,310 | 4.597031 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| loop | 336,340 | 404,320 | 477,630 | 3.790500 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| klon | 4,292,600 | 4,950,700 | 5,640,600 | 46.412812 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| screamer | 5,179,900 | 6,504,800 | 7,963,700 | 60.982500 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| fuzz | 2,008,400 | 2,310,000 | 2,607,300 | 21.656250 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| amp | 4,320,000 | 5,187,200 | 6,185,800 | 48.630000 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| fold | 3,142,600 | 3,523,200 | 3,905,000 | 33.030000 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| crush | 570,340 | 612,630 | 659,570 | 5.743406 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| ringmod | 1,312,500 | 1,496,800 | 1,725,900 | 14.032500 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| comp | 1,316,100 | 1,455,000 | 1,624,400 | 13.640625 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| tape | 970,900 | 1,067,000 | 1,175,200 | 10.003125 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| bbd | 264,600 | 291,030 | 319,250 | 2.728406 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| digital | 246,140 | 289,280 | 349,840 | 2.712000 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| chorus | 327,960 | 348,070 | 370,520 | 3.263156 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| flanger | 472,550 | 533,770 | 601,190 | 5.004094 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| phaser | 429,270 | 484,020 | 540,060 | 4.537687 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| trem | 197,470 | 243,690 | 300,380 | 2.284594 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| shift | 718,140 | 783,240 | 856,220 | 7.342875 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| granular | 332,460 | 371,160 | 415,930 | 3.479625 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| pitch | 535,280 | 609,790 | 698,720 | 5.716781 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| plate | 445,940 | 476,870 | 510,820 | 4.470656 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| hall | 669,930 | 741,010 | 812,870 | 6.946969 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| spring | 1,485,200 | 1,675,400 | 1,894,400 | 15.706875 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| shimmer | 1,276,100 | 1,420,200 | 1,561,300 | 13.314375 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| resonator | 269,320 | 304,400 | 341,480 | 2.853750 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| eq | 252,020 | 282,360 | 309,890 | 2.647125 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| vhs | 595,550 | 725,820 | 883,640 | 6.804562 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| cassette | 690,450 | 779,760 | 878,210 | 7.310250 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| reel | 798,160 | 868,550 | 958,640 | 8.142656 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| vinyl | 329,220 | 371,960 | 427,200 | 3.487125 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| genloss | 633,840 | 703,390 | 792,180 | 6.594281 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| radio | 420,230 | 465,940 | 515,860 | 4.368187 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| sampler12 | 934,280 | 1,212,500 | 1,588,500 | 11.367187 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| kick | 147,920 | 163,680 | 183,440 | 1.534500 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| snare | 134,550 | 153,260 | 173,630 | 1.436812 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| clap | 131,810 | 152,390 | 174,870 | 1.428656 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| hat | 155,970 | 169,840 | 186,910 | 1.592250 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| cymbal | 183,550 | 202,540 | 228,370 | 1.898812 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| tom | 157,930 | 176,060 | 197,790 | 1.650562 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| conga | 193,280 | 215,240 | 241,370 | 2.017875 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| rim | 158,800 | 179,960 | 205,290 | 1.687125 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| cowbell | 150,060 | 165,290 | 179,460 | 1.549594 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| shaker | 365,790 | 405,470 | 450,960 | 3.801281 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| membrane | 232,400 | 264,440 | 297,040 | 2.479125 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| modal | 167,560 | 191,950 | 219,120 | 1.799531 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| metal | 180,120 | 191,350 | 203,740 | 1.793906 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| noiseperc | 164,950 | 172,790 | 180,940 | 1.619906 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| euclid | 146,290 | 155,730 | 165,480 | 1.459969 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| prob | 151,740 | 164,390 | 181,520 | 1.541156 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| grids | 135,760 | 144,240 | 153,210 | 1.352250 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| poly | 118,330 | 123,770 | 130,310 | 1.160344 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| burst | 105,420 | 117,960 | 135,300 | 1.105875 | 106.31/93.51/88.70; 96.80/103.24/99.54 |
| vocoder | 244,080 | 267,700 | 295,690 | 2.509687 | 106.31/93.51/88.70; 96.80/103.24/99.54 |

`speak` uses the vca midpoint (423,150 ns). The speaker adapter applies
parameters and delegates a gate block to SpeechPlayer. Its phrase player
reads a pre-rendered buffer with gain, but also handles gate edges, fades,
varispeed interpolation and granular stretch. This exceeds a vca buffer
read; the mandated vca fallback is retained and this limitation is reported.

D01's missing rows were not run by the builder (it read the stack rules as
forbidding the checkout at `7f4b334`; they do not); the review took them,
in the D01 section and D03's D01 column.

The requested cheap-set precondition cannot hold for the measured
audio-throughput rows at 48 kHz: even trem's 243,690 ns midpoint makes
40 modules consume 91.38375% of real time, before the 30% load and
addition average. A measured cheap-set regression covers allowance and
recent wake accounting without altering these benchmark values.

### KZ-D03b review: the kind rows again, and the render path

The kind rows above were taken with the machine near a load of 100 and are
three to six times what the same bench reads on a lighter machine (vco
315,280 against 133,880; klon 4,950,700 against 782,260). With them the
guard costed Tom's live patch of 13 Oct (41 modules, KZ-D14's bench patch,
57% of real time on the wall) at about 251%, so no seat could load any of
Tom's saves and every console load warned at three to four times its cost.
They are superseded by the rows below, which are in `KIND_BENCH_NS`; with
them the live patch is costed at 61%.

Kind run at `42664d7`, 13 Oct 2026, 20:38 to 20:48, filter `wall/kind/`,
exit 0. The load beside each row is the one-, five- and fifteen-minute
load average sampled every ten seconds during the run, the sample at or
before the row finished. Log `target/kz-logs/review-bench-kind-base.log`,
load samples `target/kz-logs/review-bench-load.log`.

| kind | lower ns | midpoint ns | upper ns | real time at 48 kHz % | finished | load |
|---|---:|---:|---:|---:|---|---|
| vco | 124,100 | 133,880 | 145,900 | 1.2551 | 20:39:07 | 15.37/26.77/51.31 |
| lfo | 71,463 | 72,895 | 74,747 | 0.6834 | 20:39:15 | 18.64/27.06/51.12 |
| noise | 39,375 | 39,575 | 39,813 | 0.3710 | 20:39:24 | 24.69/28.08/51.20 |
| vcf | 101,410 | 102,190 | 103,170 | 0.9580 | 20:39:31 | 29.03/28.90/51.22 |
| vca | 91,359 | 91,703 | 92,156 | 0.8597 | 20:39:40 | 29.03/28.90/51.22 |
| env | 67,779 | 67,967 | 68,187 | 0.6372 | 20:39:48 | 29.32/28.95/50.98 |
| clock | 70,453 | 71,175 | 72,024 | 0.6673 | 20:39:56 | 27.60/28.61/50.60 |
| seq | 98,818 | 99,326 | 99,968 | 0.9312 | 20:40:02 | 38.18/30.75/51.10 |
| sh | 69,282 | 69,456 | 69,642 | 0.6512 | 20:40:10 | 38.18/30.75/51.10 |
| quant | 96,524 | 97,784 | 99,841 | 0.9167 | 20:40:17 | 33.44/29.98/50.59 |
| slew | 71,038 | 71,237 | 71,478 | 0.6678 | 20:40:25 | 29.00/29.14/50.05 |
| mix | 72,226 | 73,098 | 74,105 | 0.6853 | 20:40:33 | 26.02/28.50/49.57 |
| out | 88,499 | 88,709 | 88,949 | 0.8316 | 20:40:43 | 23.40/27.85/49.10 |
| compare | 104,130 | 106,340 | 108,830 | 0.9969 | 20:40:50 | 23.40/27.85/49.10 |
| logic | 93,698 | 95,147 | 96,851 | 0.8920 | 20:40:56 | 21.05/27.20/48.62 |
| branch | 94,555 | 95,226 | 95,911 | 0.8927 | 20:41:03 | 21.97/27.21/48.37 |
| switch | 110,610 | 112,510 | 114,660 | 1.0548 | 20:41:10 | 21.97/27.21/48.37 |
| select | 100,350 | 101,270 | 102,320 | 0.9494 | 20:41:17 | 21.27/26.89/48.01 |
| divide | 86,167 | 88,433 | 90,708 | 0.8291 | 20:41:26 | 20.08/26.44/47.61 |
| math | 111,820 | 113,050 | 114,540 | 1.0598 | 20:41:34 | 18.94/25.99/47.19 |
| scope | 110,610 | 112,080 | 113,670 | 1.0508 | 20:41:41 | 18.82/25.72/46.85 |
| readout | 96,793 | 97,724 | 98,964 | 0.9162 | 20:41:48 | 18.82/25.72/46.85 |
| loop | 69,790 | 70,485 | 71,314 | 0.6608 | 20:41:56 | 23.73/26.57/46.91 |
| klon | 777,210 | 782,260 | 787,580 | 7.3337 | 20:42:04 | 22.39/26.19/46.53 |
| screamer | 1,541,400 | 1,551,300 | 1,563,100 | 14.5434 | 20:42:11 | 21.07/25.78/46.15 |
| fuzz | 368,530 | 370,870 | 373,520 | 3.4769 | 20:42:18 | 21.07/25.78/46.15 |
| amp | 1,194,200 | 1,209,300 | 1,226,700 | 11.3372 | 20:42:27 | 19.82/25.36/45.77 |
| fold | 817,830 | 836,620 | 861,670 | 7.8433 | 20:42:33 | 21.65/25.59/45.61 |
| crush | 276,470 | 279,640 | 283,040 | 2.6216 | 20:42:42 | 20.73/25.27/45.26 |
| ringmod | 376,300 | 379,420 | 383,840 | 3.5571 | 20:42:48 | 20.73/25.27/45.26 |
| comp | 590,390 | 592,430 | 594,820 | 5.5540 | 20:42:57 | 19.00/24.68/44.70 |
| tape | 488,000 | 491,630 | 495,560 | 4.6090 | 20:43:04 | 17.54/24.18/44.29 |
| bbd | 161,510 | 162,010 | 162,580 | 1.5188 | 20:43:13 | 16.75/23.80/43.92 |
| digital | 133,450 | 133,860 | 134,270 | 1.2549 | 20:43:21 | 16.21/23.45/43.56 |
| chorus | 231,970 | 233,340 | 234,960 | 2.1876 | 20:43:29 | 16.21/23.45/43.56 |
| flanger | 210,590 | 212,400 | 214,570 | 1.9912 | 20:43:36 | 16.20/23.21/43.24 |
| phaser | 157,480 | 159,110 | 160,930 | 1.4917 | 20:43:44 | 16.06/22.95/42.92 |
| trem | 116,430 | 117,200 | 118,040 | 1.0988 | 20:43:51 | 15.12/22.53/42.53 |
| shift | 337,660 | 344,140 | 352,050 | 3.2263 | 20:44:00 | 15.12/22.53/42.53 |
| granular | 152,510 | 153,700 | 154,860 | 1.4409 | 20:44:09 | 14.81/22.21/42.19 |
| pitch | 305,990 | 314,290 | 325,400 | 2.9465 | 20:44:18 | 13.92/21.78/41.80 |
| plate | 209,690 | 217,740 | 229,000 | 2.0413 | 20:44:25 | 15.46/21.84/41.58 |
| hall | 342,670 | 353,440 | 368,090 | 3.3135 | 20:44:34 | 16.33/21.80/41.34 |
| spring | 596,420 | 604,340 | 614,240 | 5.6657 | 20:44:42 | 16.14/21.59/41.03 |
| shimmer | 653,320 | 657,380 | 663,550 | 6.1629 | 20:44:52 | 14.66/21.09/40.63 |
| resonator | 158,800 | 160,690 | 163,360 | 1.5065 | 20:45:00 | 14.66/21.09/40.63 |
| eq | 119,710 | 120,420 | 121,200 | 1.1289 | 20:45:07 | 14.18/20.77/40.29 |
| vhs | 259,240 | 261,110 | 263,230 | 2.4479 | 20:45:15 | 13.46/20.40/39.93 |
| cassette | 381,090 | 385,510 | 391,000 | 3.6142 | 20:45:22 | 13.92/20.27/39.65 |
| reel | 528,290 | 534,100 | 541,730 | 5.0072 | 20:45:30 | 13.92/20.27/39.65 |
| vinyl | 203,580 | 208,800 | 215,610 | 1.9575 | 20:45:37 | 13.61/20.00/39.33 |
| genloss | 360,300 | 374,890 | 395,560 | 3.5146 | 20:45:47 | 13.60/19.78/39.03 |
| radio | 155,130 | 158,740 | 163,450 | 1.4882 | 20:45:55 | 12.96/19.45/38.68 |
| sampler12 | 129,630 | 131,510 | 134,020 | 1.2329 | 20:46:03 | 13.10/19.26/38.39 |
| kick | 81,676 | 81,975 | 82,345 | 0.7685 | 20:46:12 | 12.16/18.86/38.03 |
| snare | 82,993 | 83,282 | 83,626 | 0.7808 | 20:46:21 | 12.16/18.86/38.03 |
| clap | 74,918 | 75,216 | 75,567 | 0.7052 | 20:46:29 | 13.27/18.87/37.80 |
| hat | 88,891 | 89,219 | 89,587 | 0.8364 | 20:46:38 | 12.61/18.54/37.47 |
| cymbal | 110,240 | 111,510 | 112,940 | 1.0454 | 20:46:45 | 12.24/18.26/37.15 |
| tom | 84,062 | 84,614 | 85,203 | 0.7933 | 20:46:54 | 12.18/18.05/36.85 |
| conga | 89,059 | 89,861 | 90,961 | 0.8424 | 20:47:03 | 12.33/17.89/36.57 |
| rim | 87,324 | 89,555 | 92,132 | 0.8396 | 20:47:12 | 12.63/17.77/36.31 |
| cowbell | 77,913 | 78,316 | 78,812 | 0.7342 | 20:47:21 | 12.63/17.77/36.31 |
| shaker | 79,257 | 79,641 | 80,116 | 0.7466 | 20:47:30 | 11.81/17.43/35.98 |
| membrane | 88,988 | 89,672 | 90,487 | 0.8407 | 20:47:39 | 11.88/17.26/35.70 |
| modal | 78,189 | 78,788 | 79,476 | 0.7386 | 20:47:47 | 11.43/16.98/35.38 |
| metal | 79,621 | 79,826 | 80,048 | 0.7484 | 20:47:56 | 13.50/17.26/35.27 |
| noiseperc | 84,846 | 85,272 | 85,787 | 0.7994 | 20:48:05 | 12.68/16.96/34.95 |
| euclid | 76,486 | 77,447 | 78,670 | 0.7261 | 20:48:13 | 12.53/16.78/34.67 |
| prob | 80,398 | 81,155 | 81,902 | 0.7608 | 20:48:22 | 13.90/16.93/34.52 |
| grids | 77,002 | 82,980 | 91,792 | 0.7779 | 20:48:30 | 13.90/16.93/34.52 |
| poly | 80,632 | 80,973 | 81,355 | 0.7591 | 20:48:39 | 14.52/16.97/34.33 |
| burst | 69,333 | 69,658 | 69,981 | 0.6530 | 20:48:47 | 16.30/17.28/34.23 |
| vocoder | 140,480 | 140,790 | 141,130 | 1.3199 | 20:48:55 | 16.60/17.30/34.04 |

`speak` takes the vca midpoint (91,703 ns), as the brief says; the builder
read the speaker adapter (it applies its knobs and hands a gate block to
the phrase player, which also handles gate edges, fades, varispeed and
granular stretch, more than a vca's buffer read), and the brief's choice is
kept.

The render path, base `42664d7` then head alternately, filter
`wall/chains/1000$|wall/chains/10$|wall/culled`, ns per block (five rounds:
three from 20:48 to 20:54, two more from 20:55 to 21:02; one-minute load at
the start of each run in brackets). Logs `target/kz-logs/review-bench-base-N.log`,
`review-bench-head-N.log` (head `52eda51`) and `review-bench2-base-N.log`,
`review-bench2-head-N.log`.

| round | chains/1000 base | chains/1000 head | chains/10 base | chains/10 head | culled/1000 base | culled/1000 head |
|---|---:|---:|---:|---:|---:|---:|
| 1 (16.6 / 13.8) | 49,179,000 | 48,219,000 | 515,640 | 512,530 | 158,820 | 171,230 |
| 2 (12.3 / 11.2) | 48,278,000 | 50,585,000 | 504,080 | 512,270 | 157,640 | 181,720 |
| 3 (15.1 / 20.2) | 52,106,000 | 50,900,000 | 633,630 | 626,520 | 160,310 | 194,950 |
| 4 (17.9 / 20.4) | 50,286,000 | 56,152,000 | 551,050 | 654,580 | 163,200 | 208,290 |
| 5 (25.3 / 43.3) | 52,680,000 | 50,974,000 | 607,590 | 556,850 | 175,600 | 193,620 |
| median | 50,286,000 | 50,900,000 | 551,050 | 556,850 | 160,310 | 193,620 |

The head's chains/1000 median is 1.2% over the base's over five rounds, and
2.9% over the brief's three (49,179,000 against 50,585,000): within 3%.
culled/1000 is not: every head run is above the base run of its round, 20.8% at
the median, about 33,000 ns a block. The two clock reads cost 16.7 ns each
on this machine (`clock_gettime(CLOCK_UPTIME_RAW)` timed in a loop), about
0.5 us a block, so they do not account for it; chains/10, with as many heard
modules, does not move beyond its noise. A third set at loads of 37 to 83
(`review-bench3-*.log`, with variants that drop the timing or call
`render_module` once) is too noisy to attribute it. Not confirmed either way;
a quiet machine is needed.

### Head render receipt

One head round exited 0. Uptime at launch: 19:22  6 users, load averages: 115.49 110.43 103.21

| row | lower ns | midpoint ns | upper ns |
|---|---:|---:|---:|
| wall/chains/10 | 649,400 | 667,620 | 690,150 |
| wall/chains/1000 | 277,200,000 | 307,150,000 | 341,270,000 |
| wall/culled/1000 | 226,100 | 232,310 | 240,210 |

The builder did not complete the three alternating base/head rounds; the
review's five are in the review section above.

## KZ-V02 voice trackers (baseline: the judge alone)

Date: 2026-10-13, 23:51 AEDT. HEAD: `55f1cf3f978a9698d01851cbdf2454f41ad52d96` (lane/kz-V02 after its Opus review).
Load average at launch (1/5/15 minutes): 122.60 / 142.50 / 152.08.

Command: `nice lockf -k ~/.kazoo-cargo.lock cargo run --release -j 4 -p kazoo-core --example voice_score` (exit 0).
Synthetic rate: 48,000 Hz. No recorded WAVs were present. The identical table is saved at
`target/kz-logs/voice-score.md`. Latency is counted in samples, not CPU time: from each event's sample
to the reading after the step on which the judge lands and stays, so it is never shorter than the
judge's own latency and at most one step (0.67 ms) longer.

How to read it:
- The judge is pYIN at one estimate per 512 samples over a 2048-sample window, so its first pitch comes
  about 45 ms after an onset and every figure is unshifted: on `glide` (two octaves in 500 ms,
  48 semitones a second) a 45 ms lag is about two semitones, which is the 55.67% gross error; on
  `vibrato` it is the 38 cents median.
- Cents of about 1.66 on held notes are the judge's pitch bins (a third of a semitone, `JUDGE_RESOLUTION`),
  not noise.
- `rearticulate`'s 13.75% missed is the judge going unvoiced in each -24 dB dip between syllables.

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pYIN | sine_ladder | 38.00 | 46.67 | 0 | - | - | 0 | 0 | 1.66 | 11.73 | 0.00 | 0.00 | 6.69 | 7.78 | - |
| pYIN | hum_low | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.67 | 1.67 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | steps | 45.33 | 45.33 | 0 | 45.00 | 49.33 | 6 | 0 | 1.66 | 1.66 | 0.00 | 4.72 | 4.52 | 0.00 | - |
| pYIN | vibrato | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 38.04 | 69.68 | 0.00 | 0.00 | 2.91 | 0.00 | - |
| pYIN | glide | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.66 | 65.00 | 0.00 | 55.67 | 1.76 | 2.88 | - |
| pYIN | octave_bait | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.96 | 1.96 | 0.00 | 0.00 | 3.92 | 0.00 | - |
| pYIN | vowel_ah | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.70 | 1.70 | 0.00 | 0.00 | 2.50 | 0.00 | - |
| pYIN | noise_bursts | 45.33 | 46.67 | 0 | - | - | 0 | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 10.50 | 1.31 | - |
| pYIN | whistle | 34.67 | 34.67 | 0 | - | - | 0 | 0 | 8.27 | 31.60 | 0.00 | 0.00 | 3.77 | 3.96 | - |
| pYIN | rearticulate | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 13.75 | 0.00 | - |


## KZ-V03 voice trackers

Review figures, 2026-10-14 01:59 to 02:01 AEDT, on `lane/kz-V03` at the
review's tracker commit `53842cd` (base `ac26501`). The machine was shared
and busy: load averages 90.44 / 99.07 / 126.43 before the benchmark,
89.00 / 97.89 / 124.90 after it, and 83.78 / 93.81 / 120.19 around the
scoreboard run. These are observations under that load.

`cargo bench -p kazoo-core --bench voice -- 'cycle'` exited 0.
`cycle/second/48k`: **1.2854 ms**, Criterion interval **1.2805 to 1.2911 ms**,
for one second of the suite's `vowel_ah` including its onset: 0.13% of real
time, under the 10 ms budget. Receipt: `target/kz-logs/review/bench.txt`.
(The builder's two runs read 1.8150 ms and 12.939 ms at load averages near
93 and 258; the second was not a regression of the source.)

The brief's prescribed mechanism did not meet its targets (the builder's
receipts are under `target/kz-logs`). The review changed the mechanism, not
the targets: marks at rising and falling crossings with a half-period NSDF
window (onsets and steps inside two periods at every phase), a climb to the
NSDF peak instead of taking an edge lag, the held period as a locked
candidate (continuous following through the filter closing), one window for
every candidate at a mark (no sub-harmonic winning a fade), the hint taking
precedence, input clipped to full scale, and a steady glide carried forward
between readings. Every target in `cycle_meets_its_targets` passes.

The four scoreboard test times before and after `[profile.dev.package.pyin] opt-level = 3`:

| test | before seconds | after seconds |
|---|---:|---:|
| the_judge_holds_its_floors | 31.454 | 117.079 |
| a_recorded_take_is_read_and_referenced | 25.216 | 86.947 |
| the_fixtures_folder_may_be_empty | 24.874 | 88.865 |
| a_recorded_slide_and_a_step_are_referenced | 26.122 | 92.411 |

Before receipts: `target/kz-logs/pyin-before.txt` (1 passed, exit 0) and
`target/kz-logs/pyin-before-wav.txt` (3 passed, exit 0).
After receipt: `target/kz-logs/pyin-after.txt` (4 passed, exit 0).
The runs have different concurrency and machine load, so their ratio is not
an attributable optimization result. No dependency besides pyin was optimized:
`cargo tree -p pyin --depth 1` lists approx, getset, libc, ndarray, ndarray-npy,
ndarray-stats, realfft and statrs; no isolated cost measurement establishes
which dependency would justify another profile override. The pYIN and
Viterbi implementations are generic; their contribution in this unchanged
debug build has not been isolated.

`voice_score` (`cargo run --release -p kazoo-core --example voice_score`,
exit 0), complete analytic takes at 48,000 Hz; no recorded takes were
present. Receipt: `target/kz-logs/review/voice-score-final.md`.

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pYIN | sine_ladder | 38.00 | 46.67 | 0 | - | - | 0 | 0 | 1.66 | 11.73 | 0.00 | 0.00 | 6.69 | 7.78 | - |
| cycle | sine_ladder | 3.00 | 14.00 | 0 | - | - | 0 | 0 | 0.01 | 0.37 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | hum_low | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.67 | 1.67 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | hum_low | 30.67 | 30.67 | 0 | - | - | 0 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | steps | 45.33 | 45.33 | 0 | 45.00 | 49.33 | 6 | 0 | 1.66 | 1.66 | 0.00 | 4.72 | 4.52 | 0.00 | - |
| cycle | steps | 12.00 | 12.00 | 0 | 7.33 | 12.67 | 6 | 0 | 0.00 | 0.05 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | vibrato | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 38.04 | 69.68 | 0.00 | 0.00 | 2.91 | 0.00 | - |
| cycle | vibrato | 7.33 | 7.33 | 0 | - | - | 0 | 0 | 1.27 | 2.49 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | glide | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.66 | 65.00 | 0.00 | 55.67 | 1.76 | 2.88 | - |
| cycle | glide | 14.00 | 14.00 | 0 | - | - | 0 | 0 | 2.42 | 3.30 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | octave_bait | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.96 | 1.96 | 0.00 | 0.00 | 3.92 | 0.00 | - |
| cycle | octave_bait | 14.00 | 14.00 | 0 | - | - | 0 | 0 | 0.02 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | vowel_ah | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.70 | 1.70 | 0.00 | 0.00 | 2.50 | 0.00 | - |
| cycle | vowel_ah | 12.00 | 12.00 | 0 | - | - | 0 | 0 | 0.02 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | noise_bursts | 45.33 | 46.67 | 0 | - | - | 0 | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 10.50 | 1.31 | - |
| cycle | noise_bursts | 4.67 | 7.33 | 0 | - | - | 0 | 0 | 0.00 | 0.17 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | whistle | 34.67 | 34.67 | 0 | - | - | 0 | 0 | 8.27 | 31.60 | 0.00 | 0.00 | 3.77 | 3.96 | - |
| cycle | whistle | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.94 | 2.67 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | rearticulate | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 13.75 | 0.00 | - |
| cycle | rearticulate | 7.33 | 7.33 | 0 | - | - | 0 | 0 | 0.00 | 2.88 | 0.00 | 0.00 | 11.11 | 0.00 | - |


## KZ-V04 voice trackers

Date: 2026-10-13T17:35:22Z (14 Oct 04:35 AEDT), after review. HEAD: `3154dcd` on `lane/kz-V04`.
Load at completion: load averages 178.66 165.03 161.13.

`nice lockf -k ~/.kazoo-cargo.lock cargo run --release -j 4 -p kazoo-core --example voice_score` exited 0.
Complete analytic takes at 48,000 Hz; no recorded fixtures were present. The review's fixes (a hot
input is clipped rather than reset; a recovery's remaining climb is not a second rise) leave every
row below identical to the builder's run at the uncommitted tree.
Receipt: `target/kz-logs/review-voice-score.txt`; tables: `target/kz-logs/voice-score.md`.
Latency measures sample counts, independently of the shared machine load.

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | steps missed | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pYIN | sine_ladder | 38.00 | 46.67 | 0 | - | - | 0 | 0 | 1.66 | 11.73 | 0.00 | 0.00 | 6.69 | 7.78 | - |
| cycle | sine_ladder | 3.00 | 14.00 | 0 | - | - | 0 | 0 | 0.01 | 0.37 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | sine_ladder | 3.33 | 14.67 | 0 | - | - | 0 | 0 | 0.01 | 0.37 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | hum_low | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.67 | 1.67 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| cycle | hum_low | 30.67 | 30.67 | 0 | - | - | 0 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | hum_low | 30.67 | 30.67 | 0 | - | - | 0 | 0 | 0.00 | 0.01 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | steps | 45.33 | 45.33 | 0 | 45.00 | 49.33 | 6 | 0 | 1.66 | 1.66 | 0.00 | 4.72 | 4.52 | 0.00 | - |
| cycle | steps | 12.00 | 12.00 | 0 | 7.33 | 12.67 | 6 | 0 | 0.00 | 0.05 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | steps | 12.00 | 12.00 | 0 | 7.67 | 13.33 | 6 | 0 | 0.00 | 0.07 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | vibrato | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 38.04 | 69.68 | 0.00 | 0.00 | 2.91 | 0.00 | - |
| cycle | vibrato | 7.33 | 7.33 | 0 | - | - | 0 | 0 | 1.27 | 2.49 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | vibrato | 8.00 | 8.00 | 0 | - | - | 0 | 0 | 1.17 | 2.62 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | glide | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.66 | 65.00 | 0.00 | 55.67 | 1.76 | 2.88 | - |
| cycle | glide | 14.00 | 14.00 | 0 | - | - | 0 | 0 | 2.42 | 3.30 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | glide | 14.67 | 14.67 | 0 | - | - | 0 | 0 | 0.20 | 3.25 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | octave_bait | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.96 | 1.96 | 0.00 | 0.00 | 3.92 | 0.00 | - |
| cycle | octave_bait | 14.00 | 14.00 | 0 | - | - | 0 | 0 | 0.02 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | octave_bait | 14.67 | 14.67 | 0 | - | - | 0 | 0 | 0.02 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | vowel_ah | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.70 | 1.70 | 0.00 | 0.00 | 2.50 | 0.00 | - |
| cycle | vowel_ah | 12.00 | 12.00 | 0 | - | - | 0 | 0 | 0.02 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | vowel_ah | 12.00 | 12.00 | 0 | - | - | 0 | 0 | 0.02 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | noise_bursts | 45.33 | 46.67 | 0 | - | - | 0 | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 10.50 | 1.31 | - |
| cycle | noise_bursts | 4.67 | 7.33 | 0 | - | - | 0 | 0 | 0.00 | 0.17 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | noise_bursts | 5.33 | 8.00 | 0 | - | - | 0 | 0 | 0.00 | 0.18 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | whistle | 34.67 | 34.67 | 0 | - | - | 0 | 0 | 8.27 | 31.60 | 0.00 | 0.00 | 3.77 | 3.96 | - |
| cycle | whistle | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 0.94 | 2.67 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| voice | whistle | 1.33 | 1.33 | 0 | - | - | 0 | 0 | 1.00 | 2.82 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | rearticulate | 45.33 | 45.33 | 0 | - | - | 0 | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 13.75 | 0.00 | - |
| cycle | rearticulate | 7.33 | 7.33 | 0 | - | - | 0 | 0 | 0.00 | 2.88 | 0.00 | 0.00 | 11.11 | 0.00 | - |
| voice | rearticulate | 8.00 | 8.00 | 0 | - | - | 0 | 0 | 0.00 | 2.88 | 0.00 | 0.00 | 10.84 | 0.00 | - |

| tracker | take | rise max ms | fall max ms | noise high % | onsets counted | onsets expected | rises missed | falls missed |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| voice | sine_ladder | 14.67 | 30.67 | 0.00 | 6 | 6 | 0 | 0 |
| voice | hum_low | 30.67 | 38.67 | 2.16 | 1 | 1 | 0 | 0 |
| voice | steps | 12.00 | 26.00 | 0.00 | 1 | 1 | 0 | 0 |
| voice | vibrato | 8.00 | 18.67 | 0.00 | 1 | 1 | 0 | 0 |
| voice | glide | 14.67 | 14.67 | 0.00 | 1 | 1 | 0 | 0 |
| voice | octave_bait | 14.67 | 20.00 | 0.00 | 1 | 1 | 0 | 0 |
| voice | vowel_ah | 12.00 | 25.33 | 0.00 | 1 | 1 | 0 | 0 |
| voice | noise_bursts | 8.00 | 18.67 | 0.00 | 3 | 3 | 0 | 0 |
| voice | whistle | 1.33 | 12.00 | 0.00 | 1 | 1 | 0 | 0 |
| voice | rearticulate | 8.00 | 18.67 | 0.00 | 5 | 5 | 0 | 0 |

The fused tracker adds at most one 64-sample frame boundary to the cycle readings.
Both octave traps have zero octave errors. Noise has zero false pitch and zero gate-high percentage.
Rearticulation counts five onsets, the ladder six and noise bursts three.
The low-hum gate remains high for 38.67 ms on release, within its two-period plus 15 ms limit;
the gate table reports 2.16% against its separate fixed 30 ms noise allowance.

The unvoiced veto uses the same post-leap window condition as octave folding: silence verdicts
otherwise delay a new note until the slower judge becomes voiced (57.33 ms maximum on the ladder).
A four-dB recovery is timestamped on its threshold crossing, rather than on every frame it remains
above threshold; refreshing it repeatedly counted nine onsets for five syllables.

## KZ-P10 sidestep

2026-10-14, source head `1233bcf`, base `da446f9`, with uncommitted
`lane/kz-P10` write-only review fixes.
The fixture processes 512 frames at 48 kHz with square, sub1, sub2, ring,
half and fold at 50%, and the remaining controls at their defaults.

| bench | frames | time per block | load average (1/5/15 minutes) | result |
|---|---:|---|---|---|
| fx/sidestep | 512 | not measured | 143.31 / 153.73 / 157.58 | queued command cancelled before execution, exit 130 |
| fx/sidestep resume | 512 | not measured | 260.72 / 233.51 / 184.05 | owned exec session cancelled while waiting for the cargo lock, exit 130; benchmark did not run |
| fx/sidestep write-only | 512 | to be measured in the build round | not sampled | not run: write-only round |

The required cargo lock and dependency compilation consumed the allocation.
The benchmark never started. The 100 microsecond budget is not verified.
There is no before number because this effect did not exist at the base.
The load above was read at cancellation; it is not a benchmark load sample.
The resume load was read while waiting for the cargo lock on 2026-10-14.
It is also a queue observation, not a benchmark load sample. The resume
has not obtained a timing result, so the 100 microsecond budget remains
unverified.

The `circuit_models` bench target registers `fx/sidestep` through the
effect catalogue. No benchmark ran in the write-only round. The wall's
sidestep cost provisionally uses ringmod's 379,420 ns per 512-frame block
until `fx/sidestep` is measured on a quiet machine; this is a scheduling
estimate, not a sidestep measurement.

## KZ-D04 page-owned delay lines

Measured at base `1c2dbfb` and the uncommitted `lane/kz-D04` tree. Both
commands were `nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=target/kz-logs/nextest cargo bench -p kazoo-wall --bench wall_render -- 'wall/cables'`
(the actual state-directory argument was the absolute path in this lane).
Both exited 0. Values are Criterion interval midpoints, in nanoseconds per
512-frame block at 48 kHz.

| row | before ns | after ns | before interval ns | after interval ns |
|---|---:|---:|---|---|
| wall/cables/10 | 290,630 | 557,680 | 268,580 to 316,260 | 466,530 to 672,640 |
| wall/cables/100 | 607,520 | 492,910 | 496,510 to 737,230 | 460,990 to 531,800 |
| wall/cables/1000 | 14,402,000 | 5,212,000 | 6,537,400 to 25,925,000 | 4,805,200 to 5,732,000 |
| wall/cables/10000 | 46,997,000 | 70,849,000 | 43,061,000 to 51,402,000 | 62,546,000 to 79,898,000 |

The after fixture printed `wall/cables/10000 lines=0`. The before fixture
did not print its count. Receipts: `target/kz-logs/bench-before.txt` and
`target/kz-logs/bench-after.txt`.

The 10,000-cable midpoint regressed by 50.8%; the requested 3x improvement
is not verified. These runs were under heavy shared-machine load. An uptime
reading during the before run was 167.74 / 128.90 / 130.88; during the after
run it was 94.41 / 116.17 / 122.25, and crate checks also ran concurrently.
These observations do not isolate the mechanism's performance. The older
D01 row of 141,610,000 ns is explicitly marked as not a baseline in this
file; its ratio of 2.00x to this after row also does not meet 3x.

A repeat on the frozen final source also exited 0
(`target/kz-logs/bench-final.txt`). It again printed `lines=0` at 10,000.
The one-minute / five-minute / fifteen-minute load during this repeat was
77.70 / 93.24 / 109.02; the final crate suite was still running.

| row | final repeat ns | interval ns |
|---|---:|---|
| wall/cables/10 | 626,170 | 541,580 to 715,840 |
| wall/cables/100 | 1,495,400 | 1,239,700 to 1,767,100 |
| wall/cables/1000 | 10,845,000 | 9,202,400 to 12,587,000 |
| wall/cables/10000 | 100,870,000 | 89,220,000 to 112,930,000 |

The final repeat is 2.15x slower than the measured base midpoint at 10,000,
and 1.40x faster than the non-baseline D01 row. Neither meets the 3x target.
Performance acceptance remains open; the observed regressions are retained.

### KZ-D04 review: base and head interleaved (not a verdict)

The reviewer built `wall_render` at base `1c2dbfb` and head `1c1dba2` (the
same bench file at both, so both print the line count) and ran
`wall_render-<side> --bench 'wall/cables' --noplot` three rounds, base then
head each round, under `lockf ~/.kazoo-cargo.lock` so no kazoo cargo job ran
beside them, 08:11 to 08:19 on 14 October. Load averages (1/5/15 min) at
each start are in the table. Every run exited 0. Criterion midpoints, ns per
512-frame block at 48 kHz:

| round | side | load | cables/10 | cables/100 | cables/1000 | cables/10000 | lines at 10000 |
|---|---|---|---:|---:|---:|---:|---:|
| 1 | base | 102.38 / 78.18 / 73.00 | 718,970 | 1,585,900 | 12,596,000 | 122,970,000 | 10,048 |
| 1 | head | 112.42 / 86.28 / 76.47 | 417,000 | 1,186,700 | 12,196,000 | 143,050,000 | 0 |
| 2 | base | 145.13 / 102.58 / 83.55 | 887,870 | 1,366,600 | 15,114,000 | 252,110,000 | 10,048 |
| 2 | head | 124.90 / 107.86 / 87.64 | 704,020 | 2,591,800 | 11,866,000 | 156,180,000 | 0 |
| 3 | base | 118.48 / 108.40 / 89.48 | 669,680 | 1,523,300 | 14,772,000 | 162,120,000 | 10,048 |
| 3 | head | 163.77 / 126.35 / 98.09 | 217,950 | 880,200 | 5,103,800 | 44,240,000 | 0 |

The same binary moved 2x between rounds (base 10000: 123 to 252 ms), so at
loads of 100 to 160 these rows cannot show 3x or rule it out. Head is
faster than the base run beside it in 10 of 12 pairs; at 10000 it was
slower in round 1 (1.16x) and faster in rounds 2 (1.61x) and 3 (3.66x).
The line count is confirmed: 10,048 lines at base (157 pages of 64) and 0 at
head. The 3x target is NOT CONFIRMED. A load-independent count (instructions and
cycles per block for a fixed number of renders, `/usr/bin/time -l`) was
being built when the review was stopped at 08:40 for Tom's 09:00 deadline,
and no numbers came from it. Log: `target/kz-review/ab1.log`.

## KZ-V12 high notes

14 October 2026, write-only round. Base and unchanged HEAD:
`dd1bfc606ebfd152164c8b3092ed6f9b52657472`, branch `lane/kz-V12`.
The code changes are uncommitted. `uptime` at 10:55 AEDT reported load averages
31.76, 39.13 and 43.79. `sysctl -n vm.loadavg` was refused with
`Operation not permitted`. Disk available at the start: 53 GiB (`df -g /`).

Before and after score tables were not measured: Tom reserved the CPU and
prohibited all build, test and benchmark commands in this round. No numerical
result or red/green result is claimed. The tests write the scoreboard tables
and counts of octave error steps after 100 ms before asserting their targets.
Their files are relative to the worktree:

| test | rows | table written when run |
|---|---|---|
| `high_whistles_hold` | 800, 1100, 1500, 2200 Hz; cycle and voice | `target/kz-logs/kz-v12/high_whistles_hold.md` |
| `high_sung_seconds_hold` | 800, 1100, 1500, 2200 Hz; cycle and voice | `target/kz-logs/kz-v12/high_sung_seconds_hold.md` |
| `high_sung_thirds_hold` | 800, 1100, 1500, 2200 Hz; cycle and voice | `target/kz-logs/kz-v12/high_sung_thirds_hold.md` |
| `high_splices_hold` | 800, 1100, 1500, 2200 Hz; cycle and voice | `target/kz-logs/kz-v12/high_splices_hold.md` |
| `high_notes_hold_at_96k` | whistle and sung second at 2200 Hz; cycle and voice | `target/kz-logs/kz-v12/high_notes_hold_at_96k.md` |
| `high_notes_hold_through_the_engine_judge` | whistle and sung second at all four pitches; engine-configured voice | `target/kz-logs/kz-v12/high_notes_hold_through_the_engine_judge.md` |

B1 and B2 were not built. Their selection requires these measurements; the
write-only rule prevents deciding that neither is needed. The build round
must apply the stated cycle-then-voice decision rule without changing targets.
Fix A uses the full voice range and its rate-scaled hop and bins. Fix A2
exempts a matching hint landing within 10 ms from the sung-leap clock.

At this base the engine still constructs its detector for `analysis_thread`;
`ProcessingState` has no voice tracker and `start` has no `judge::spawn` call.
The range check is therefore before detector construction and audio-device
preparation. V06's engine wiring must be present in the integrated tree to
prove the live mouth outcome. The offline engine-judge tests exercise its
configuration through `FusedUnderTest::with_judge`.

The cached pYIN 1.2.0 source accepts a maximum equal to Nyquist, and the local
validator permits the 50–4000 Hz, 512-sample-hop configuration at 8000 Hz.
The conditional 0.45 × rate cap was not applied; construction at every listed
device rate remains unverified until the build round.

## KZ-P06 profiles

Date: 2026-10-14 11:37 AEDT. Base and HEAD: `dd1bfc606ebfd152164c8b3092ed6f9b52657472`,
branch `lane/kz-P06`, changes uncommitted. Load at source writing: 60.12, 32.12, 32.71.
This is a write-only round with the CPU reserved. No build, test, scoreboard or benchmark ran.
All measurements below are **not verified**; no previous voice result is reused as instrument proof.

`judge/hop/48k/bass`: not run. The implemented row uses the bass range (29–500 Hz),
a 4096-sample frame, the unchanged 512-sample hop and a complete judge history before measurement.
The acceptance ceiling is 4.2 ms per run; neither a before value nor an after value was measured.

The `voice_score` example writes pitch and gate tables for both profiles and all eight exact takes.
The instrument fixture directory contains no takes in this checkout. If recordings are added, the
example loads them with the line profile's 4096-sample reference window so B0 is inside its range.

| tracker | take | pitch measurements | gate and onset measurements |
|---|---|---|---|
| voice:guitar | bass_open | not run | not run |
| voice:guitar | guitar_open | not run | not run |
| voice:guitar | sweep | not run | not run |
| voice:guitar | slide | not run | not run |
| voice:guitar | bend | not run | not run |
| voice:guitar | vibrato | not run | not run |
| voice:guitar | glide | not run | not run |
| voice:guitar | noise_hum | not run | not run |
| voice:bass | bass_open | not run | not run |
| voice:bass | guitar_open | not run | not run |
| voice:bass | sweep | not run | not run |
| voice:bass | slide | not run | not run |
| voice:bass | bend | not run | not run |
| voice:bass | vibrato | not run | not run |
| voice:bass | glide | not run | not run |
| voice:bass | noise_hum | not run | not run |

## KZ-P07 low notes

Date: 2026-10-14 12:04 AEDT. Base: `dd1bfc6`. HEAD:
`a7adc01d2a9743bd2fca02bfc670fe259191b1b2`, branch `lane/kz-P07`.
Load average at source writing: 59.62, 52.49, 46.27.
This write-only round reserves the CPU. No build, test, scoreboard or benchmark ran.
Neither latency, accuracy, octave rate, handover smoothness nor callback cost is verified.

The `early_meets_its_targets` scoreboard test prints the instrument table with
early disabled and enabled, with delayed real judge verdicts. It also prints each
note's first voiced gated frame, its individual period-based latency budget and
its settled cents error after 100 ms. These are measured only when the test runs.

| tracker | take | onset latency max | cents median after 100 ms | octave rate |
|---|---|---|---|---|
| early off | bass_open | not run | not run | not run |
| early on | bass_open | not run | not run | not run |
| early off | guitar_open | not run | not run | not run |
| early on | guitar_open | not run | not run | not run |

The B0 deadline is 50.6 ms and the E1 deadline is 38.4 ms. These are acceptance
targets, not measurements. The search is bounded to 18 NSDF lag evaluations
per unlock. Its elapsed time and the 64-frame callback budget are not measured.
The low-B listening comparison with the live follower remains for the integrated
input path and an actual bass performance.


## KZ-R02 measured verification

14 October 2026. Branch `lane/kz-R02`, HEAD `65028079eb4b663a980d4eab111614c265458d6b`,
merge-base `8ed511d`. Changes are uncommitted. START: 74 GiB available, load 36.89.

V12 selects B1 because the base cycle `splice_800` gross error was 2.56%, above 1%.
The literal first B1 implementation produced two settled octave-error steps on cycle
`splice_2200` and four on voice. Receipt: `target/kz-logs/r02-b1.txt`.
The retained implementation prefers the held period, gives high-note recovery
three milliseconds before accepting an unrelated lag, and uses the 1 ms window
while locked. Acquisition retains 0.5 ms: using 1 ms at acquisition delayed the
1500 Hz voice onset to 2.667 ms against 2.333 ms. No acceptance threshold changed.

`high_splices_hold` passes after these corrections. Every splice row has zero
settled octave errors. Full before/after tables: `target/kz-logs/r02-high-splices-before.md`
and `target/kz-logs/r02-high-splices-after.md`. Other high-note tables remain at the
`target/kz-logs/kz-v12/` paths listed above. `r02-b1-final.txt` exits 0: all 14
focused tests pass, including every high cycle/voice row and both original
scoreboards. B2 is not needed under V12's rule after these B1 results.

| tracker | take | median cents before / after | gross % before / after | octave % before / after |
|---|---|---:|---:|---:|
| cycle | splice_800 | 4.52 / 3.33 | 2.56 / 0.00 | 0.00 / 0.00 |
| voice | splice_800 | 4.50 / 3.32 | 2.58 / 0.00 | 0.00 / 0.00 |
| cycle | splice_1100 | 4.64 / 3.16 | 0.98 / 0.00 | 0.00 / 0.00 |
| voice | splice_1100 | 4.64 / 3.20 | 1.02 / 0.00 | 0.00 / 0.00 |
| cycle | splice_1500 | 4.20 / 2.78 | 0.00 / 0.16 | 0.00 / 0.00 |
| voice | splice_1500 | 4.28 / 2.72 | 0.00 / 0.18 | 0.00 / 0.00 |
| cycle | splice_2200 | 4.75 / 2.54 | 1.11 / 0.11 | 0.00 / 0.00 |
| voice | splice_2200 | 4.78 / 2.54 | 0.93 / 0.13 | 0.00 / 0.00 |

P06 `judge/hop/48k/bass`: Criterion 10 samples, 1 s warmup, 2 s measurement,
7.2574 / 13.160 / 19.235 ms confidence interval; command exit 0.
Load around the run rose from 99.98 to 296.08. This does not verify the 4.2 ms budget.
Receipt: `target/kz-logs/r02-judge-bench.txt`. No controlled before/after timing exists.
The full P06 instrument table was not run; the base open-string rows, early off
and on, are in `target/kz-logs/red-early_meets_its_targets.txt`.

P07 remains blocked: the refused first B0 lobe proposes 149.69 samples against a
1555.02-sample true period, spends its one search, then the normal path locks at
46.17 ms. The shaped E1 glide schedules 38.68 ms, but normal marks lock at 36.69 ms.
Receipt: `target/kz-logs/r02-diagnostics.txt`. P07 line 60 demands early-off take
at least two B0 periods (64.79 ms), while its normal-path preservation rule forbids
changing the already faster path. Neither the waveform nor the assertion was weakened.

Fix-run measurements and remaining conflicts: `target/kz-logs/r02-fix-bench.md`.
