use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion};
use kazoo_core::analysis::{
    PitchDetector, PitchDetectorConfig, cycle::CycleTracker, score::signals,
};
use kazoo_core::engine::{EngineConfig, judge_config};
use kazoo_core::host::live::InputProfile;

fn saw_samples(length: u32) -> Vec<f32> {
    let mut saw: Vec<f64> = (0..length)
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
    saw.into_iter().map(|sample| sample as f32).collect()
}

fn judge(c: &mut Criterion, name: &str, label: &str, config: &PitchDetectorConfig) {
    let hop = config.hop_length;
    let warmup = 4096.max(config.frame_length + 2 * hop);
    let samples = saw_samples(u32::try_from(warmup + hop).expect("small bench signal"));
    let mut group = c.benchmark_group(name);
    group.bench_function(BenchmarkId::new("hop", label), |b| {
        b.iter_batched_ref(
            || {
                let mut detector =
                    PitchDetector::new(config.clone()).expect("valid detector configuration");
                black_box(detector.push_samples(&samples[..warmup]));
                detector
            },
            |detector| black_box(detector.push_samples(black_box(&samples[warmup..]))),
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn cpu_runs(config: PitchDetectorConfig, setup_only: bool) {
    let hop = config.hop_length;
    // Both modes prepare the same signal so their user-CPU difference excludes setup.
    let samples = saw_samples(u32::try_from(4096 + 100 * hop).expect("small bench signal"));
    let mut detector = PitchDetector::new(config).expect("valid detector configuration");
    black_box(detector.push_samples(&samples[..4096]));
    if !setup_only {
        for chunk in samples[4096..].chunks_exact(hop) {
            black_box(detector.push_samples(black_box(chunk)));
        }
    }
    println!("CPU runs: {}", if setup_only { 0 } else { 100 });
}

fn cycle(c: &mut Criterion) {
    let take = signals::suite(48_000)
        .into_iter()
        .find(|take| take.name == "vowel_ah")
        .expect("vowel fixture");
    let samples = &take.samples[..48_000];
    let mut group = c.benchmark_group("cycle");
    group.bench_function(BenchmarkId::new("second", "48k"), |b| {
        b.iter_batched_ref(
            || CycleTracker::new(48_000),
            |tracker| {
                tracker.process(black_box(samples));
                black_box(tracker.estimate());
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--cpu-judge") => cpu_runs(PitchDetectorConfig::for_rate(48_000), false),
        Some("--cpu-judge-setup") => cpu_runs(PitchDetectorConfig::for_rate(48_000), true),
        Some("--cpu-engine") => {
            cpu_runs(judge_config(&EngineConfig::default().pitch, 48_000), false);
        }
        Some("--cpu-engine-setup") => {
            cpu_runs(judge_config(&EngineConfig::default().pitch, 48_000), true);
        }
        _ => {
            let mut criterion = Criterion::default().configure_from_args();
            judge(
                &mut criterion,
                "judge",
                "48k",
                &PitchDetectorConfig::for_rate(48_000),
            );
            judge(
                &mut criterion,
                "judge",
                "48k/bass",
                &InputProfile::Bass.tracking().judge_config(48_000),
            );
            judge(
                &mut criterion,
                "engine",
                "48k",
                &judge_config(&EngineConfig::default().pitch, 48_000),
            );
            cycle(&mut criterion);
            criterion.final_summary();
        }
    }
}
