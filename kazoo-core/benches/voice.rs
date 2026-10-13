use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use kazoo_core::analysis::{PitchDetector, PitchDetectorConfig};

fn judge(c: &mut Criterion) {
    let mut saw: Vec<f64> = (0..4608_u32)
        .map(|index| {
            let phase = std::f64::consts::TAU * 130.81 * f64::from(index) / 48_000.0;
            (1..=20_u32)
                .map(|harmonic| (phase * f64::from(harmonic)).sin() / f64::from(harmonic))
                .sum()
        })
        .collect();
    let peak = saw.iter().copied().map(f64::abs).fold(0.0_f64, f64::max);
    for sample in &mut saw {
        *sample *= 0.5 / peak;
    }
    let samples: Vec<f32> = saw.into_iter().map(|sample| sample as f32).collect();
    let mut group = c.benchmark_group("judge");
    group.bench_function(BenchmarkId::new("hop", "48k"), |b| {
        b.iter_batched_ref(
            || {
                let mut detector = PitchDetector::new(PitchDetectorConfig::for_rate(48_000))
                    .expect("valid judge configuration");
                black_box(detector.push_samples(&samples[..4096]));
                detector
            },
            |detector| black_box(detector.push_samples(black_box(&samples[4096..]))),
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, judge);
criterion_main!(benches);
