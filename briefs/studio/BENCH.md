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
1000 and 10000. Look byte sizes were not measured.

The tree's mix has four named inputs and four knobs, not 28 named jacks.
The fixture uses those eight jacks, `ceil(C / 8)` leaves, and counts the
summing-tree routes within C. This preserves the support test's exact total
route count and makes every leaf audible. It differs from the brief's fixture.

The Criterion logs these rows came from were under `target/`, which was
deleted outside the lane at about 01:23 on 13 Oct; they no longer exist, so
the rows above cannot be checked against their source.
