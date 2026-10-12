use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use kazoo_wall::catalogue::Kind;

pub mod support;
use support::{Fixture, cables, chains, culled, module_kind, unplugged};

fn measure(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    id: &str,
    mut fixture: Fixture,
) {
    let mut buffer = vec![0.0; 512 * 2];
    for _ in 0..64 {
        fixture.render(&mut buffer);
    }
    group.bench_function(id, |b| b.iter(|| fixture.render(&mut buffer)));
}

fn wall_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("wall");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    for kind in Kind::all().filter(|kind| kind.name() != "speak") {
        measure(
            &mut group,
            &format!("kind/{}", kind.name()),
            module_kind(kind),
        );
    }
    for count in [10, 100, 1000] {
        measure(&mut group, &format!("chains/{count}"), chains(count));
    }
    for count in [100, 1000] {
        measure(&mut group, &format!("unplugged/{count}"), unplugged(count));
    }
    for count in [10, 100, 1000, 10000] {
        measure(&mut group, &format!("cables/{count}"), cables(count));
    }
    measure(&mut group, "culled/1000", culled(1000));
    group.finish();
}

criterion_group!(benches, wall_render);
criterion_main!(benches);
