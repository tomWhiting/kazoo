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

## KZ-V02 voice trackers (baseline: the judge alone)

Date: 2026-10-13, 21:28 AEDT. HEAD: `96051a29e59cb8a11dafa405123619258546b849`
with the KZ-V02 scoreboard implementation uncommitted in `lane/kz-V02`.
Load average at launch (1/5/15 minutes): 158.70 / 93.78 / 74.39.

Command: `nice -n 10 env CARGO_BUILD_JOBS=4 NEXTEST_STATE_DIR=target/kz-logs/nextest cargo run --release -p kazoo-core --example voice_score` (exit 0).
Synthetic rate: 48,000 Hz. No recorded WAVs were present. Latency is measured
in sample offsets; these figures do not measure CPU time or audible output.
The identical printed table is saved at `target/kz-logs/voice-score.md`.

| tracker | take | onset median ms | onset max ms | onsets missed | step median ms | step max ms | steps | cents median | cents p95 | octave % | gross % | missed % | false % | aligned lag ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pYIN | sine_ladder | 37.33 | 46.00 | 0 | - | - | 0 | 1.66 | 11.73 | 0.00 | 0.00 | 6.69 | 7.78 | - |
| pYIN | hum_low | 44.67 | 44.67 | 0 | - | - | 0 | 1.67 | 1.67 | 0.00 | 0.00 | 0.00 | 0.00 | - |
| pYIN | steps | 44.67 | 44.67 | 0 | 44.33 | 48.67 | 6 | 1.66 | 1.66 | 0.00 | 4.72 | 4.52 | 0.00 | - |
| pYIN | vibrato | 44.67 | 44.67 | 0 | - | - | 0 | 38.04 | 69.68 | 0.00 | 0.00 | 2.91 | 0.00 | - |
| pYIN | glide | 44.67 | 44.67 | 0 | - | - | 0 | 1.66 | 65.00 | 0.00 | 55.67 | 1.76 | 2.88 | - |
| pYIN | octave_bait | 44.67 | 44.67 | 0 | - | - | 0 | 1.96 | 1.96 | 0.00 | 0.00 | 3.92 | 0.00 | - |
| pYIN | vowel_ah | 44.67 | 44.67 | 0 | - | - | 0 | 1.70 | 1.70 | 0.00 | 0.00 | 2.50 | 0.00 | - |
| pYIN | noise_bursts | 44.67 | 46.00 | 0 | - | - | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 10.50 | 1.31 | - |
| pYIN | whistle | 34.00 | 34.00 | 0 | - | - | 0 | 8.27 | 31.60 | 0.00 | 0.00 | 3.77 | 3.96 | - |
| pYIN | rearticulate | 44.67 | 44.67 | 0 | - | - | 0 | 1.66 | 1.66 | 0.00 | 0.00 | 13.75 | 0.00 | - |
