use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use kazoo_wall::catalogue::Kind;
use kazoo_wall::engine::Command;

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
    if id == "cables/10000" {
        eprintln!(
            "wall/cables/10000 lines={}",
            fixture.control.shared().lines()
        );
    }
    group.bench_function(id, |b| b.iter(|| fixture.render(&mut buffer)));
}

fn loop_fixture(record: bool) -> Fixture {
    let mut fixture = module_kind(Kind::LOOP);
    let slot = fixture
        .slots
        .iter()
        .find(|(_, kind)| *kind == Kind::LOOP)
        .unwrap()
        .0;
    for (name, value) in [("bars", 8.0), ("record", 1.0), ("keep", 0.7)] {
        fixture.control.send(Command::Knob {
            slot,
            knob: Kind::LOOP.spec().knob_index(name).unwrap(),
            target: value,
            glide_frames: 0,
        });
    }
    let mut buffer = [0.0; 1024];
    // Nine bars include the first bar-line arm and a full eight-bar tape.
    for _ in 0..(9 * 96_000 / 512 + 2) {
        fixture.render(&mut buffer);
    }
    if !record {
        fixture.control.send(Command::Knob {
            slot,
            knob: Kind::LOOP.spec().knob_index("record").unwrap(),
            target: 0.0,
            glide_frames: 0,
        });
        for _ in 0..(96_000 / 512 + 2) {
            fixture.render(&mut buffer);
        }
    }
    fixture
}

fn wall_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("wall");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    measure(&mut group, "loop/play", loop_fixture(false));
    measure(&mut group, "loop/record", loop_fixture(true));
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
    for kind in Kind::all().filter(|kind| kind.name() != "speak") {
        measure(
            &mut group,
            &format!("kind/{}", kind.name()),
            module_kind(kind),
        );
    }
    group.finish();
}

criterion_group!(benches, wall_render);
criterion_main!(benches);
