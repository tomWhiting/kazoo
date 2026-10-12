use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use kazoo_fx::Context;

fn circuit_models(c: &mut Criterion) {
    let mut group = c.benchmark_group("fx");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    let input: Vec<f32> = (0..512)
        .map(|frame| {
            (std::f32::consts::TAU * 110.0 * frame as f32 / 48_000.0).sin()
                * 10.0_f32.powf(-12.0 / 20.0)
        })
        .collect();
    let context = Context { bpm: 120.0 };
    for kind in kazoo_fx::catalogue() {
        let mut effect = (kind.build)();
        effect.prepare(48_000.0);
        for (index, param) in kind.params.iter().enumerate() {
            let value = match param.name {
                "mix" => param.max,
                "drive" | "gain" => param.min + 0.9 * (param.max - param.min),
                _ => param.default,
            };
            effect.set_param(index, value);
        }
        let mut left = vec![0.0; 512];
        let mut right = vec![0.0; 512];
        for _ in 0..64 {
            effect.process(&context, [&input, &input], [&mut left, &mut right]);
        }
        group.bench_function(kind.id, |b| {
            b.iter(|| effect.process(&context, [&input, &input], [&mut left, &mut right]));
        });
    }
    group.finish();
}

criterion_group!(benches, circuit_models);
criterion_main!(benches);
