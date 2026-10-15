use kazoo_wall::daemon::wall::Wall;
use kazoo_wall::patch::Patch;
use kazoo_wall::protocol::What;

fn chains(count: usize) -> Patch {
    let mut patch = Patch::empty(120.0);
    for _ in 0..count {
        let mut ids = Vec::new();
        for kind in ["vco", "vcf", "out"] {
            let What::Add { module } = patch.add(kind, None, None).unwrap() else {
                panic!("not an add")
            };
            ids.push(module.id);
        }
        patch
            .plug(
                &format!("{}.out", ids[0]),
                &format!("{}.in", ids[1]),
                None,
                None,
            )
            .unwrap();
        for input in ["left", "right"] {
            patch
                .plug(
                    &format!("{}.out", ids[1]),
                    &format!("{}.{input}", ids[2]),
                    None,
                    None,
                )
                .unwrap();
        }
    }
    patch
}

pub fn report_real_walls() {
    for count in [3, 47] {
        let patch = chains(count);
        let (mut engine, control) = kazoo_wall::engine::engine(
            kazoo_wall::engine::EngineConfig::new(48_000, 120.0, 0.0),
            None,
        );
        let mut wall = Wall::new(patch, control, None, Vec::new(), None);
        let mut buffer = [0.0; 512 * 2];
        for _ in 0..512 {
            engine.render(&mut buffer, 2);
            let faults = wall.engine_mut().pump();
            assert!(faults.is_empty(), "{faults:?}");
        }
        let (load, fixed, bench, busy) = wall.render_costs();
        assert_eq!(wall.patch().modules().len(), count * 3);
        assert!(load.is_finite() && load > 0.0, "load={load}");
        assert!(
            fixed.is_finite() && fixed > 0.0 && fixed <= load,
            "fixed={fixed}"
        );
        assert!(buffer.iter().all(|sample| sample.is_finite()));
        eprintln!(
            "real wall modules={} load={load:.8} fixed={fixed:.8} bench={bench:.8} busy={:.8} module_load={:.8}",
            count * 3,
            busy,
            load - fixed
        );
    }
}
