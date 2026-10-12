use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kazoo_fx::{Context, bench::fixture};

fn models(c: &mut Criterion) {
    let mut group = c.benchmark_group("model");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    let input = fixture(48_000.0);
    let context = Context { bpm: 120.0 };
    for kind in kazoo_fx::catalogue().filter(|kind| kind.id == "screamer") {
        for (variant, hot) in [("default", false), ("hot", true)] {
            let mut effect = (kind.build)();
            effect.prepare(48_000.0);
            if hot {
                effect.set_param(0, 100.0);
                effect.set_param(1, 100.0);
            }
            effect.reset();
            let mut left = [0.0; 256];
            let mut right = [0.0; 256];
            let blocks = input.len() / 256;
            let mut cursor = 0;
            group.bench_function(BenchmarkId::new(kind.id, variant), |b| {
                b.iter(|| {
                    let start = cursor * 256;
                    let block = &input[start..start + 256];
                    effect.process(&context, [block, block], [&mut left, &mut right]);
                    cursor = (cursor + 1) % blocks;
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, models);
criterion_main!(benches);
