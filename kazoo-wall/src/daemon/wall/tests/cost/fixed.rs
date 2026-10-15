use super::*;

fn small(wall: &mut Wall) -> f32 {
    heard_chain(wall, 1);
    let ids = chain(wall, Kind::VCA, 7);
    request(
        wall,
        &patch(&last_output(&ids, Kind::VCA), "screamer1.left"),
    )
    .unwrap();
    assert_eq!(wall.patch.modules().len(), 9);
    bench_sum(wall)
}

#[test]
fn fixed_work_does_not_inflate_a_nine_module_wall() {
    let (mut wall, _engine, _dir) = saving_wall();
    let bench = small(&mut wall);
    let fixed = bench * 1.4 * 0.3 / 0.7;
    guarded(&mut wall, bench.mul_add(1.4, fixed));
    wall.engine.shared().set_fixed_load(fixed);
    let nine = wall.busy();
    let mut large_patch = wall.patch.clone();
    let mut previous: Option<String> = None;
    for _ in 0..132 {
        let What::Add { module } = large_patch.add("vca", None, None).unwrap() else {
            panic!("not an add")
        };
        if let Some(previous) = previous {
            large_patch
                .plug(
                    &format!("{previous}.out"),
                    &format!("{}.in", module.id),
                    None,
                    None,
                )
                .unwrap();
        }
        previous = Some(module.id);
    }
    large_patch
        .plug(&format!("{}.out", previous.unwrap()), "vca1.in", None, None)
        .unwrap();
    let (_large_engine, control) =
        crate::engine::engine(crate::engine::EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(large_patch, control, None, Vec::new(), None);
    wall.engine.shared().set_fixed_load(fixed);
    assert_eq!(wall.patch.modules().len(), 141);
    let bench = bench_sum(&wall);
    guarded(&mut wall, bench.mul_add(1.4, fixed));
    let large = wall.busy();
    eprintln!("nine={nine}, large={large}, fixed={fixed}");
    assert!((nine - large).abs() <= large * 0.1);
    assert!((nine - 1.4).abs() < 1e-5);
}

#[test]
fn fixed_work_is_added_once_to_a_seats_load_projection() {
    let (mut wall, _engine, _dir) = saving_wall();
    let (incoming, load) = incoming_save(&mut wall);
    let bench = small(&mut wall);
    let fixed = 0.1;
    guarded(&mut wall, bench.mul_add(2.0, fixed));
    wall.engine.shared().set_fixed_load(fixed);
    let sum: f32 = incoming
        .modules()
        .iter()
        .map(|module| super::super::super::cost::kind_share(module.kind, 48_000))
        .sum();
    let err = request(&mut wall, &load).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(
        err.message.contains("so it was not put up"),
        "{}",
        err.message
    );
    assert!(
        (projected_percent(&err.message) - (sum.mul_add(2.0, fixed) * 100.0).round()).abs()
            < f32::EPSILON
    );
}

#[test]
fn seventeen_sleeping_seqs_wake_inside_the_guard() {
    let (mut wall, _engine, _dir) = saving_wall();
    small(&mut wall);
    let mut ids: Vec<String> = Vec::new();
    for _ in 0..17 {
        let id = changed(request(&mut wall, &add("seq")).unwrap().result)
            .module
            .unwrap();
        if let Some(previous) = ids.last() {
            request(
                &mut wall,
                &patch(&format!("{previous}.gate"), &format!("{id}.clock")),
            )
            .unwrap();
        }
        ids.push(id);
    }
    for id in &ids {
        wall.engine.set_cost(wall.rack.slot_of(id).unwrap(), 1e9);
        assert!(wall.unheard.contains(id));
    }
    guarded(&mut wall, 0.30);
    wall.engine.shared().set_fixed_load(0.09);
    let seq_share = super::super::super::cost::kind_share(Kind::SEQ, 48_000);
    let expected = 17.0f32.mul_add(seq_share * wall.busy(), 0.30);
    assert!(expected < LOAD_GUARD);
    request(
        &mut wall,
        &patch(&format!("{}.pitch", ids.last().unwrap()), "vca1.cv"),
    )
    .unwrap();
    assert!(ids.iter().all(|id| !wall.unheard.contains(id)));
    assert!((wall.load_watch.recent.back().unwrap().2 - (expected - 0.30)).abs() < 1e-5);
}
