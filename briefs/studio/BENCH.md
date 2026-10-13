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
