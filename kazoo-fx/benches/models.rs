use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use kazoo_fx::Context;

fn models(c: &mut Criterion) {
    let mut group = c.benchmark_group("model");
    group
        .sample_size(30)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2));
    let input = kazoo_fx::bench::fixture(48_000.0);
    let context = Context { bpm: 120.0 };
    for id in ["hall", "plate", "spring"] {
        let kind = kazoo_fx::find(id).expect("catalogue contains the reverb");
        for variant in ["default", "hot", "two"] {
            if variant == "two" && id != "spring" {
                continue;
            }
            let mut effect = (kind.build)();
            for (index, param) in kind.params.iter().enumerate() {
                let value = match (variant, param.name) {
                    ("two", "springs") => 2.0,
                    ("hot", "decay" | "mod" | "drive") => param.max,
                    _ => param.default,
                };
                effect.set_param(index, value);
            }
            effect.prepare(48_000.0);
            let mut left = vec![0.0; input.len()];
            let mut right = vec![0.0; input.len()];
            for _ in 0..64 {
                effect.process(&context, [&input, &input], [&mut left, &mut right]);
            }
            group.bench_function(format!("{id}/{variant}"), |b| {
                b.iter(|| effect.process(&context, [&input, &input], [&mut left, &mut right]));
            });
        }
    }
    group.finish();
}

criterion_group!(benches, models);
criterion_main!(benches);
