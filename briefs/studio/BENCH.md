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
