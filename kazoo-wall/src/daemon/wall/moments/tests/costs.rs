use super::*;

fn noisy(count: usize) -> Patch {
    let mut patch = Patch::empty(120.0);
    let mut previous: Option<String> = None;
    for _ in 0..count - 1 {
        let What::Add { module } = patch.add("noise", None, None).unwrap() else {
            panic!("not an add")
        };
        if let Some(previous) = previous {
            patch
                .plug(
                    &format!("{previous}.out"),
                    &format!("{}.level", module.id),
                    Some(1.0),
                    None,
                )
                .unwrap();
        }
        previous = Some(module.id);
    }
    patch.add("out", None, None).unwrap();
    patch
        .plug(
            &format!("{}.out", previous.unwrap()),
            "out1.left",
            Some(1.0),
            None,
        )
        .unwrap();
    patch
}

#[test]
fn a_console_and_a_seat_go_to_141_modules_under_or_over_the_guard() {
    for console in [false, true] {
        for over in [false, true] {
            let mut rig = Rig::new(None);
            let incoming = noisy(141);
            let saves = rig.wall.store.as_ref().unwrap().saves_dir();
            std::fs::create_dir_all(&saves).unwrap();
            std::fs::write(
                saves.join("large.json"),
                crate::store::encode(&incoming).unwrap(),
            )
            .unwrap();
            let loaded = rig
                .ask(
                    "Tom",
                    true,
                    serde_json::json!({"op":"load", "name":"large"}),
                )
                .unwrap();
            let seq = loaded.change.unwrap().seq;
            let small = noisy(9);
            std::fs::write(
                saves.join("small.json"),
                crate::store::encode(&small).unwrap(),
            )
            .unwrap();
            rig.ask(
                "Tom",
                true,
                serde_json::json!({"op":"load", "name":"small"}),
            )
            .unwrap();
            assert_eq!(rig.wall.patch.modules().len(), 9);
            rig.wall.set_load_guard(true);
            rig.wall
                .engine
                .shared()
                .set_render_load(if over { 0.50 } else { 0.30 });
            rig.wall
                .engine
                .shared()
                .set_fixed_load(if over { 0.35 } else { 0.09 });
            let outcome = rig
                .ask(
                    "Seat",
                    console,
                    serde_json::json!({"op":"go", "moment":format!("#{seq}")}),
                )
                .unwrap();
            let result: crate::protocol::LoadResult =
                serde_json::from_value(outcome.result).unwrap();
            assert_eq!(rig.wall.patch.modules().len(), 141);
            assert_eq!(
                result
                    .notes
                    .iter()
                    .any(|note| note.contains("about") && note.contains("% of the wall's time")),
                over,
                "{:?}",
                result.notes
            );
            rig.wall.close(rig.now);
        }
    }
}
