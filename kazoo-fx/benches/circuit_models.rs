use std::{hint::black_box, process::Command, time::Duration};

use criterion::{
    BatchSize, BenchmarkGroup, Criterion, criterion_group, criterion_main, measurement::WallTime,
};
use kazoo_fx::Context;

fn circuit_models(c: &mut Criterion) {
    let load = Command::new("uptime").output().expect("read load average");
    assert!(load.status.success(), "uptime failed: {}", load.status);
    println!("{}", String::from_utf8(load.stdout).expect("load is UTF-8"));
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
        if kind.id == "octaves" {
            octaves(&mut group, kind);
            continue;
        }
        let mut effect = (kind.build)();
        effect.prepare(48_000.0);
        for (index, param) in kind.params.iter().enumerate() {
            let value = match (kind.id, param.name) {
                ("sidestep", "square" | "sub1" | "sub2" | "ring" | "half" | "fold") => 50.0,
                ("sidestep", _) => param.default,
                (_, name) => match name {
                    "mix" => param.max,
                    "drive" | "gain" => 0.9_f32.mul_add(param.max - param.min, param.min),
                    _ => param.default,
                },
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

fn octaves(group: &mut BenchmarkGroup<'_, WallTime>, kind: &kazoo_fx::EffectKind) {
    let context = Context { bpm: 120.0 };
    let chord = [110.0, 146.83, 196.0, 246.94, 329.63, 440.0];
    for (name, notes) in [
        ("octaves", chord.as_slice()),
        ("octaves/single_note", &[110.0][..]),
    ] {
        let mut effect = (kind.build)();
        for (index, param) in kind.params.iter().enumerate() {
            let value = match param.name {
                "up1" | "up2" | "down1" | "down2" => 100.0,
                "dry" => 0.0,
                _ => param.default,
            };
            effect.set_param(index, value);
        }
        effect.prepare(48_000.0);
        // Continuous phase across blocks keeps six separate notes in the bank.
        let input: Vec<f32> = (0..128 * 1024)
            .map(|frame| {
                notes
                    .iter()
                    .map(|frequency| {
                        (1..=6)
                            .map(|harmonic| {
                                let phase = std::f64::consts::TAU
                                    * frequency
                                    * f64::from(harmonic)
                                    * f64::from(frame)
                                    / 48_000.0;
                                phase.sin() / f64::from(harmonic)
                            })
                            .sum::<f64>()
                    })
                    .sum::<f64>() as f32
                    * 0.2
                    / notes.len() as f32
            })
            .collect();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        for block in input.chunks_exact(128) {
            effect.process(&context, [block, block], [&mut left, &mut right]);
        }
        let mut cursor = 0;
        group.bench_function(name, |b| {
            b.iter(|| {
                let block = &input[cursor..cursor + 128];
                effect.process(
                    &context,
                    [black_box(block), black_box(block)],
                    [&mut left, &mut right],
                );
                black_box(&left);
                black_box(&right);
                cursor = (cursor + 128) % input.len();
            });
        });
        if name == "octaves" {
            group.bench_function("octaves/redesign", |b| {
                b.iter_batched_ref(
                    || {
                        let mut left = [0.0; 128];
                        let mut right = [0.0; 128];
                        let mut effect = (kind.build)();
                        for (index, param) in kind.params.iter().enumerate() {
                            let value = match param.name {
                                "up1" | "up2" | "down1" | "down2" => 100.0,
                                "dry" => 0.0,
                                _ => param.default,
                            };
                            effect.set_param(index, value);
                        }
                        effect.prepare(48_000.0);
                        for block in input[..8192].chunks_exact(128) {
                            effect.process(&context, [block, block], [&mut left, &mut right]);
                        }
                        effect.set_param(2, 1.25);
                        for block in input[..2304].chunks_exact(128) {
                            effect.process(&context, [block, block], [&mut left, &mut right]);
                        }
                        effect
                    },
                    |effect| {
                        effect.process(&context, [&input[2304..2432]; 2], [&mut left, &mut right]);
                        black_box(&left);
                        black_box(&right);
                    },
                    BatchSize::SmallInput,
                );
            });
        }
    }
}

criterion_group!(benches, circuit_models);
criterion_main!(benches);
