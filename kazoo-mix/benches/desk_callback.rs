use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kazoo_core::audio_transport::{AudioBlock, AudioRingConfig, audio_block_ring};
use kazoo_core::protocol::{AudioBlockHeader, BlockFlags, BufferId};
use kazoo_mix::engine::{MixerEngine, short_name};

fn desk_callback(c: &mut Criterion) {
    let mut group = c.benchmark_group("desk");
    group.sample_size(50);
    let input: Vec<f32> = (0..512)
        .flat_map(|frame| {
            let sample = (std::f32::consts::TAU * 110.0 * frame as f32 / 48_000.0).sin()
                * 10.0_f32.powf(-12.0 / 20.0);
            [sample, sample]
        })
        .collect();
    for strips in [8, 64, 512] {
        let mut engine = MixerEngine::new(strips, 48_000).unwrap();
        let mut producers = Vec::with_capacity(strips);
        for slot in 0..strips {
            let (producer, consumer) = audio_block_ring(AudioRingConfig::new(
                BufferId(u32::try_from(slot).unwrap()),
                2,
                512,
                8,
            ));
            assert!(
                engine
                    .attach_consumer(slot, short_name("b"), consumer)
                    .unwrap()
                    .is_none()
            );
            producers.push(producer);
        }
        let mut buffer = vec![0.0; 512 * 2];
        group.bench_function(BenchmarkId::new("callback", strips), |b| {
            b.iter_custom(|iterations| {
                let mut total = Duration::ZERO;
                for _ in 0..iterations {
                    let frame = engine.next_frame();
                    for producer in &mut producers {
                        producer
                            .push_block(AudioBlock {
                                header: AudioBlockHeader {
                                    start_frame: frame,
                                    frames: 512,
                                    channels: 2,
                                    sequence: frame / 512,
                                    flags: BlockFlags::default(),
                                },
                                samples: &input,
                            })
                            .unwrap();
                    }
                    let start = Instant::now();
                    engine.render_f32(&mut buffer, 2);
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

criterion_group!(benches, desk_callback);
criterion_main!(benches);
