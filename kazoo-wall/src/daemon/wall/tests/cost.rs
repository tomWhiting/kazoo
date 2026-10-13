use super::*;

fn request(wall: &mut Wall, request: &Request) -> Result<Outcome, WallError> {
    let seat = format!("Cost{}", wall.next_seq);
    wall.request_as(&seat, false, request, Instant::now())
}

fn chain(wall: &mut Wall, kind: Kind, count: usize) -> Vec<String> {
    let input = kind
        .spec()
        .inputs
        .iter()
        .find(|port| port.signal == crate::catalogue::Signal::Audio)
        .unwrap()
        .name;
    let output = kind
        .spec()
        .outputs
        .iter()
        .find(|port| port.signal == crate::catalogue::Signal::Audio)
        .unwrap()
        .name;
    let mut ids: Vec<String> = Vec::new();
    for _ in 0..count {
        let outcome = request(wall, &add(kind.name())).unwrap();
        let id = changed(outcome.result).module.unwrap();
        if let Some(previous) = ids.last() {
            request(
                wall,
                &patch(&format!("{previous}.{output}"), &format!("{id}.{input}")),
            )
            .unwrap();
        }
        ids.push(id);
    }
    ids
}

fn audio_kind(dearest: bool) -> Kind {
    let rows = super::super::cost::KIND_BENCH_NS;
    let mut audio: Vec<(Kind, u32)> = rows
        .iter()
        .filter_map(|&(name, ns)| {
            let kind = Kind::from_name(name)?;
            (kind
                .spec()
                .inputs
                .iter()
                .any(|port| port.signal == crate::catalogue::Signal::Audio)
                && kind
                    .spec()
                    .outputs
                    .iter()
                    .any(|port| port.signal == crate::catalogue::Signal::Audio))
            .then_some((kind, ns))
        })
        .collect();
    audio.sort_by_key(|(_, ns)| *ns);
    if dearest {
        audio.last().unwrap().0
    } else {
        audio[0].0
    }
}

fn heavy() -> Kind {
    audio_kind(true)
}
fn last_output(ids: &[String], kind: Kind) -> String {
    format!("{}.{}", ids.last().unwrap(), kind.spec().outputs[0].name)
}

fn guarded(wall: &mut Wall, load: f32) {
    wall.set_load_guard(true);
    measured_long_enough(wall);
    wall.engine.shared().set_render_load(load);
}

#[test]
fn one_cable_waking_forty_heavy_unheard_modules_is_refused() {
    let (mut wall, _engine) = wall();
    let mixer = changed(request(&mut wall, &add("mix")).unwrap().result)
        .module
        .unwrap();
    request(&mut wall, &patch(&format!("{mixer}.out"), "out1.left")).unwrap();
    let ids = chain(&mut wall, heavy(), 40);
    assert!(
        40.0f32.mul_add(
            super::super::cost::kind_share(heavy(), wall.engine.sample_rate()),
            0.30
        ) > LOAD_GUARD
    );
    guarded(&mut wall, 0.30);
    for to in [format!("{mixer}.a"), "out1.left".to_string()] {
        let before = wall.patch.to_file();
        let err = request(&mut wall, &patch(&last_output(&ids, heavy()), &to)).unwrap_err();
        assert_eq!(err.code, ErrorCode::Full);
        assert!(err.message.contains("wake 40 modules"), "{}", err.message);
        eprintln!("{}", err.message);
        assert_eq!(wall.patch.to_file(), before);
        assert!(
            ids.iter()
                .all(|id| wall.snapshot().timing.unheard.contains(id))
        );
        assert!(!wall.load_watch.refusing);
    }
    wall.set_load_guard(false);
    let plugged = request(
        &mut wall,
        &patch(&last_output(&ids, heavy()), &format!("{mixer}.a")),
    )
    .unwrap();
    let cable = changed(plugged.result).cable;
    let unplugged = request(&mut wall, &Request::Unpatch { cable, to: None }).unwrap();
    guarded(&mut wall, 0.30);
    let err = request(
        &mut wall,
        &Request::Undo {
            change: unplugged.change.unwrap().seq,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 40 modules"), "{}", err.message);
}

#[test]
fn unheard_modules_do_not_lower_the_average() {
    let (mut wall, _engine) = wall();
    let units = wall.patch.modules().len() + wall.patch.cables().len();
    assert!(units >= COSTED_FROM);
    for _ in 0..200 {
        request(&mut wall, &add("lfo")).unwrap();
    }
    assert_eq!(wall.snapshot().timing.unheard.len(), 200);
    guarded(&mut wall, LOAD_GUARD / units as f32 * (units as f32 - 0.5));
    let err = request(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("would take it"), "{}", err.message);
}

#[test]
fn a_measured_cost_is_used_before_the_bench_cost() {
    let (mut wall, _engine) = wall();
    let cable = request(&mut wall, &patch("vco1.out", "out1.left")).unwrap();
    let slot = wall.rack.slot_of("vco1").unwrap();
    wall.engine
        .set_cost(slot, 0.9 * SUB_BLOCK as f32 * 1e9 / 48_000.0);
    let outgoing: Vec<u32> = wall
        .patch
        .cables()
        .iter()
        .filter(|cable| cable.from_module == "vco1")
        .map(|cable| cable.id)
        .collect();
    for id in outgoing {
        request(
            &mut wall,
            &Request::Unpatch {
                cable: Some(id),
                to: None,
            },
        )
        .unwrap();
    }
    assert!(wall.unheard.iter().any(|id| id == "vco1"));
    drop(cable);
    guarded(&mut wall, 0.10);
    let err = request(&mut wall, &patch("vco1.out", "out1.left")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 1 modules"), "{}", err.message);
    wall.engine.forget_cost(slot);
    request(&mut wall, &patch("vco1.out", "out1.left")).unwrap();
}

#[test]
fn a_save_too_heavy_to_render_is_not_put_up() {
    let (mut wall, _engine, _dir) = saving_wall();
    request(&mut wall, &Request::New).unwrap();
    let mut heavy_ids = Vec::new();
    for _ in 0..40 {
        let ids = chain(&mut wall, heavy(), 1);
        let out = changed(request(&mut wall, &add("out")).unwrap().result)
            .module
            .unwrap();
        request(
            &mut wall,
            &patch(&last_output(&ids, heavy()), &format!("{out}.left")),
        )
        .unwrap();
        heavy_ids.extend(ids);
    }
    save(&mut wall, "heavy", false).unwrap();
    let emptied = request(&mut wall, &Request::New)
        .unwrap()
        .change
        .unwrap()
        .seq;
    guarded(&mut wall, 0.05);
    let before = wall.patch.to_file();
    let load = Request::Load {
        name: "heavy".to_string(),
    };
    let now = Instant::now();
    let err = wall.request_as("Loader", false, &load, now).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(
        err.message.contains("renders 80 modules"),
        "{}",
        err.message
    );
    assert_eq!(wall.patch.to_file(), before);
    assert_eq!(
        wall.load_buckets["Loader"].tokens.to_bits(),
        LOAD_BURST.to_bits()
    );
    let err = request(&mut wall, &Request::Undo { change: emptied }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    save(&mut wall, "empty", false).unwrap();
    wall.engine.shared().set_render_load(2.0);
    request(
        &mut wall,
        &Request::Load {
            name: "empty".to_string(),
        },
    )
    .unwrap();
    request(&mut wall, &Request::New).unwrap();
    let loaded = wall.request_as("Tom", true, &load, now).unwrap();
    let result: LoadResult = serde_json::from_value(loaded.result).unwrap();
    assert!(result.notes.iter().any(|note| note.contains("past the")));
    assert!(heavy_ids.iter().all(|id| wall.patch.module(id).is_some()));
}

#[test]
fn a_chain_brought_in_with_its_out_is_costed_at_what_it_wakes() {
    let (mut wall, _engine, _dir) = saving_wall();
    request(&mut wall, &Request::New).unwrap();
    let mut ids = chain(&mut wall, heavy(), 40);
    let out = changed(request(&mut wall, &add("out")).unwrap().result)
        .module
        .unwrap();
    request(
        &mut wall,
        &patch(&last_output(&ids, heavy()), &format!("{out}.left")),
    )
    .unwrap();
    ids.push(out);
    request(
        &mut wall,
        &Request::ChainSave {
            name: "heavy-chain".to_string(),
            modules: ids,
            overwrite: false,
        },
    )
    .unwrap();
    request(&mut wall, &Request::New).unwrap();
    guarded(&mut wall, 0.30);
    let before = wall.patch.to_file();
    let err = request(
        &mut wall,
        &Request::ChainAdd {
            name: "heavy-chain".to_string(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 41 modules"), "{}", err.message);
    assert_eq!(wall.patch.to_file(), before);
}

#[test]
fn a_cable_waking_a_cheap_set_is_allowed() {
    let (mut wall, _engine) = wall();
    let mixer = changed(request(&mut wall, &add("mix")).unwrap().result)
        .module
        .unwrap();
    let out = changed(request(&mut wall, &add("out")).unwrap().result)
        .module
        .unwrap();
    request(
        &mut wall,
        &patch(&format!("{mixer}.out"), &format!("{out}.left")),
    )
    .unwrap();
    let kind = audio_kind(false);
    let ids = chain(&mut wall, kind, 40);
    let ns = 100.0;
    let share = ns / (SUB_BLOCK as f32 * 1e9 / 48_000.0);
    for id in &ids {
        wall.engine.set_cost(wall.rack.slot_of(id).unwrap(), ns);
    }
    let before = wall.held();
    assert!(
        (0.30 / before as f32).mul_add(2.0, 40.0f32.mul_add(share, 0.30))
            < LOAD_GUARD * SETTLE_SHARE
    );
    guarded(&mut wall, 0.30);
    request(
        &mut wall,
        &patch(&last_output(&ids, kind), &format!("{mixer}.a")),
    )
    .unwrap();
    assert!(
        ids.iter()
            .all(|id| !wall.snapshot().timing.unheard.contains(id))
    );
    let recent = wall.load_watch.recent.back().unwrap();
    assert!(recent.1 >= 41);
    assert!(40.0f32.mul_add(-share, recent.2).abs() < 1e-6);
    let held = wall.held();
    let delta: i64 = wall
        .load_watch
        .recent
        .iter()
        .map(|(_, units, _)| units)
        .sum();
    let measured = i64::try_from(held).unwrap() - delta;
    wall.engine
        .shared()
        .set_render_load(20.0f32.mul_add(-share, LOAD_GUARD) * measured as f32 / (held + 1) as f32);
    let err = request(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
}
