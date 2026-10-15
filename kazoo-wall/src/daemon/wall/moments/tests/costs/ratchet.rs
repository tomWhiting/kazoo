use super::*;
use std::fs;

fn voiced(count: usize) -> Patch {
    let mut patch = Patch::empty(120.0);
    patch.add("screamer", None, None).unwrap();
    let mut previous = "screamer1.left".to_string();
    for _ in 0..count - 2 {
        let What::Add { module } = patch.add("vca", None, None).unwrap() else {
            panic!("not an add")
        };
        patch
            .plug(&previous, &format!("{}.in", module.id), None, None)
            .unwrap();
        previous = format!("{}.out", module.id);
    }
    patch.add("out", None, None).unwrap();
    patch.plug(&previous, "out1.left", None, None).unwrap();
    patch
}

fn after_a_go_to_small() -> (Rig, u64) {
    let mut rig = Rig::new(None);
    let saves = rig.wall.store.as_ref().unwrap().saves_dir();
    fs::create_dir_all(&saves).unwrap();
    let mut small = 0;
    let mut incoming = 0;
    for (name, count) in [("incoming", 61), ("small", 9), ("large", 141)] {
        fs::write(
            saves.join(format!("{name}.json")),
            crate::store::encode(&voiced(count)).unwrap(),
        )
        .unwrap();
        let change = rig
            .ask("Tom", true, serde_json::json!({"op":"load", "name":name}))
            .unwrap()
            .change
            .unwrap();
        match name {
            "incoming" => incoming = change.seq,
            "small" => small = change.seq,
            "large" => assert_eq!(rig.wall.patch.modules().len(), 141),
            _ => panic!("unknown fixture"),
        }
    }
    rig.wall.set_load_guard(true);
    let shared = rig.wall.engine.shared();
    shared.set_frames(rig.wall.load_watch.measured_from);
    shared.set_render_load(0.30);
    shared.set_fixed_load(0.001);
    rig.go(&format!("#{small}"));
    assert_eq!(rig.wall.patch.modules().len(), 9);
    assert!(rig.wall.engine.shared().frames() < rig.wall.load_watch.measured_from);
    (rig, incoming)
}

#[test]
fn a_seats_load_after_a_go_to_small_uses_the_new_walls_cost() {
    let (mut rig, _) = after_a_go_to_small();
    let outcome = rig.ask(
        "Seat",
        false,
        serde_json::json!({"op":"load", "name":"incoming"}),
    );
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(rig.wall.patch.modules().len(), 61);
    rig.wall.close(rig.now);
}

#[test]
fn a_go_after_a_go_to_small_has_no_stale_load_warning() {
    let (mut rig, incoming) = after_a_go_to_small();
    let outcome = rig
        .ask(
            "Seat",
            false,
            serde_json::json!({"op":"go", "moment":format!("#{incoming}")}),
        )
        .unwrap();
    let result: crate::protocol::LoadResult = serde_json::from_value(outcome.result).unwrap();
    assert!(
        !result
            .notes
            .iter()
            .any(|note| note.contains("% of the wall's time")),
        "{:?}",
        result.notes
    );
    rig.wall.close(rig.now);
}

#[test]
fn busy_measures_the_small_wall_again_after_its_window() {
    let (mut rig, _) = after_a_go_to_small();
    assert_eq!(rig.wall.busy().to_bits(), 1.0_f32.to_bits());
    let bench = rig.wall.render_costs().2;
    let shared = rig.wall.engine.shared();
    shared.set_frames(rig.wall.load_watch.measured_from - 1);
    assert_eq!(rig.wall.busy().to_bits(), 1.0_f32.to_bits());
    shared.set_frames(rig.wall.load_watch.measured_from);
    shared.set_render_load(bench.mul_add(2.0, 0.001));
    assert!((rig.wall.busy() - 2.0).abs() < 1e-5);
    rig.wall.close(rig.now);
}
