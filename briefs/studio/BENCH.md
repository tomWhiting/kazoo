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
| wall/chains/10 | not measured | 1,538,300 | 2,529,700 |
| wall/culled/1000 | not measured | 29,436,000 | 2,295,500 |

The measured ratio is 0.9074x after, within the 1.2x limit, versus
19.1354x before. After intervals: chains/10 [2,132,500, 2,981,300] ns;
culled/1000 [1,881,700, 2,722,700] ns. These runs shared the machine with
other lane work and scoped validation builds. Their broad intervals and
the chains/10 variation do not establish isolated render costs or a quiet
machine baseline. The limit above compares the observed midpoints.

The D01 section has no measurements for either requested row; its
chains/1000 row is 291,080,000 ns under load and explicitly not a baseline.
No missing D01 value has been inferred.

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

D01's missing historical rows were not run: the mandated checkout at
`7f4b334` conflicts with the stack rule forbidding branch/check-out changes.
D03's D01 column remains explicitly not measured.

The requested cheap-set precondition cannot hold for the measured
audio-throughput rows at 48 kHz: even trem's 243,690 ns midpoint makes
40 modules consume 91.38375% of real time, before the 30% load and
addition average. A measured cheap-set regression covers allowance and
recent wake accounting without altering these benchmark values.

### Head render receipt

One head round exited 0. Uptime at launch: 19:22  6 users, load averages: 115.49 110.43 103.21

| row | lower ns | midpoint ns | upper ns |
|---|---:|---:|---:|
| wall/chains/10 | 649,400 | 667,620 | 690,150 |
| wall/chains/1000 | 277,200,000 | 307,150,000 | 341,270,000 |
| wall/culled/1000 | 226,100 | 232,310 | 240,210 |

The three alternating base/head rounds were not completed in the lane's
25-minute time box. No within-3% acceptance claim follows from this row.
