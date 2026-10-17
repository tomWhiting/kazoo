use std::time::{Duration, Instant};

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use kazoo_wall::daemon::wall::Wall;
use kazoo_wall::engine::Engine;
use kazoo_wall::patch::Patch;
use kazoo_wall::protocol::Request;

pub mod support;
use support::Fixture;

fn fresh_wall() -> (Wall, Engine) {
    let Fixture {
        engine, control, ..
    } = Fixture::new();
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    wall.set_load_guard(false);
    (wall, engine)
}

fn change(wall: &mut Wall, request: &Request, changes: &mut usize, now: Instant) {
    let seat = format!("bench-{}", *changes / 30);
    wall.request(&seat, request, now)
        .unwrap_or_else(|error| panic!("{error}"));
    *changes += 1;
}

fn add(kind: &str) -> Request {
    Request::add(kind.to_owned(), None, None, None)
}

const fn patch(from: String, to: String) -> Request {
    Request::patch(from, to, None)
}

fn build_requests(count: usize) -> Vec<(String, Request)> {
    let mut requests = Vec::with_capacity(count * 4 + 1);
    requests.push(add("out"));
    for index in 1..=count {
        requests.push(add("vco"));
        requests.push(add("vcf"));
        requests.push(patch(format!("vco{index}.out"), format!("vcf{index}.in")));
        requests.push(patch(format!("vcf{index}.out"), "out1.left".to_owned()));
    }
    requests
        .into_iter()
        .enumerate()
        .map(|(change, request)| (format!("bench-{}", change / 30), request))
        .collect()
}

fn control(c: &mut Criterion) {
    let mut group = c.benchmark_group("control");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(3));
    for count in [100, 1000] {
        let requests = build_requests(count);
        let now = Instant::now();
        group.bench_function(BenchmarkId::new("build", count), |b| {
            b.iter_batched(
                fresh_wall,
                |(mut wall, engine)| {
                    for (seat, request) in &requests {
                        wall.request(seat, request, now)
                            .unwrap_or_else(|error| panic!("{error}"));
                    }
                    (wall, engine)
                },
                BatchSize::PerIteration,
            );
        });
    }
    for count in [100, 1000, 10000] {
        let (mut wall, mut engine) = fresh_wall();
        let now = Instant::now();
        let mut changes = 0;
        for _ in 0..count {
            change(&mut wall, &add("vco"), &mut changes, now);
            // Consume setup commands so their tables do not accumulate.
            engine.render(&mut [], 2);
        }
        let outcome = wall.request("seat", &Request::Look, now).unwrap();
        let bytes = serde_json::to_vec(&outcome.result).unwrap().len();
        eprintln!("control/look/{count} bytes: {bytes}");
        group.bench_function(BenchmarkId::new("look", count), |b| {
            b.iter(|| wall.request("seat", &Request::Look, now).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, control);
criterion_main!(benches);
