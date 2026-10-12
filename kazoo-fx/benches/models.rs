use std::{process::Command, time::Duration};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kazoo_fx::{
    Context,
    bench::{fixture, screamer_reference},
};

fn report_load(id: &str) {
    let output = match Command::new("uptime").output() {
        Ok(output) => output,
        Err(error) => panic!("cannot read benchmark load: {error}"),
    };
    assert!(output.status.success(), "uptime failed: {}", output.status);
    let load = match String::from_utf8(output.stdout) {
        Ok(load) => load,
        Err(error) => panic!("benchmark load is not UTF-8: {error}"),
    };
    println!("load at {id}: {}", load.trim());
}

fn models(c: &mut Criterion) {
    let mut group = c.benchmark_group("model");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    let input = fixture(48_000.0);
    let context = Context { bpm: 120.0 };
    for kind in kazoo_fx::catalogue().filter(|kind| kind.id == "screamer") {
        for (variant, hot) in [("default", false), ("hot", true)] {
            for (suffix, reference) in [
                ("/before-1", true),
                ("", false),
                ("/before-2", true),
                ("/after-2", false),
            ] {
                let mut effect = if reference {
                    screamer_reference()
                } else {
                    (kind.build)()
                };
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
                let id = format!("model/{}/{variant}{suffix}", kind.id);
                report_load(&id);
                group.bench_function(
                    BenchmarkId::new(kind.id, format!("{variant}{suffix}")),
                    |b| {
                        b.iter(|| {
                            let start = cursor * 256;
                            let block = &input[start..start + 256];
                            effect.process(&context, [block, block], [&mut left, &mut right]);
                            cursor = (cursor + 1) % blocks;
                        });
                    },
                );
                report_load(&id);
            }
        }
    }
    group.finish();
}

criterion_group!(benches, models);
criterion_main!(benches);
