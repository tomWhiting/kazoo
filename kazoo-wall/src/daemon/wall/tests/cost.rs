use super::*;

mod fixed;

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
    let kind = Kind::from_name(if dearest { "screamer" } else { "mix" }).unwrap();
    assert_audio(kind);
    kind
}

fn assert_audio(kind: Kind) {
    assert!(
        kind.spec()
            .inputs
            .iter()
            .any(|port| port.signal == crate::catalogue::Signal::Audio)
    );
    assert!(
        kind.spec()
            .outputs
            .iter()
            .any(|port| port.signal == crate::catalogue::Signal::Audio)
    );
}

fn heavy() -> Kind {
    let kind = Kind::from_name("screamer").unwrap();
    assert_audio(kind);
    kind
}
fn last_output(ids: &[String], kind: Kind) -> String {
    format!("{}.{}", ids.last().unwrap(), kind.spec().outputs[0].name)
}

/// The load a refusal says the change would take the wall to, in percent.
fn projected_percent(message: &str) -> f32 {
    let (_, after) = message.split_once("to about ").unwrap();
    let (number, _) = after.split_once('%').unwrap();
    number.parse().unwrap()
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
        assert!(projected_percent(&err.message) > 80.0, "{}", err.message);
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
    assert!(projected_percent(&err.message) > 80.0, "{}", err.message);
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
    let busy = wall.busy();
    let share = super::super::cost::kind_share(Kind::VCO, 48_000) * busy;
    request(&mut wall, &patch("vco1.out", "out1.left")).unwrap();
    assert!((wall.load_watch.recent.back().unwrap().2 - share).abs() < 1e-6);
    assert!((wall.module_share("vco1", Kind::VCO, busy) - 0.9).abs() < 1e-6);
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
    assert!(projected_percent(&err.message) > 80.0, "{}", err.message);
    assert_eq!(wall.patch.to_file(), before);
    assert_eq!(
        wall.load_buckets["Loader"].tokens.to_bits(),
        LOAD_BURST.to_bits()
    );
    let err = request(&mut wall, &Request::Undo { change: emptied }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(
        err.message.contains("renders 80 modules"),
        "{}",
        err.message
    );
    assert_eq!(wall.patch.to_file(), before);
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
    let share = super::super::cost::kind_share(kind, 48_000);
    for id in &ids {
        wall.engine.set_cost(wall.rack.slot_of(id).unwrap(), ns);
    }
    let before = wall.held();
    guarded(&mut wall, 0.30);
    wall.engine.shared().set_fixed_load(0.15);
    // Sleeping modules use the current bench factor rather than old timings.
    let busy = wall.busy();
    assert!((busy - 1.0).abs() < f32::EPSILON, "{busy}");
    assert!(
        (0.30 / before as f32).mul_add(2.0, 40.0f32.mul_add(share * busy, 0.30))
            < LOAD_GUARD * SETTLE_SHARE
    );
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
    assert!((40.0 * share).mul_add(-busy, recent.2).abs() < 1e-6);
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

/// Undoing a patch that replaced a cable puts the old cable back (an
/// unplugging that replugs): when the old cable leads from modules nobody
/// hears, the undo wakes them, and is costed at what it wakes.
#[test]
fn undoing_a_replacing_patch_is_costed_at_what_it_wakes() {
    let (mut wall, _engine) = wall();
    let mixer = changed(request(&mut wall, &add("mix")).unwrap().result)
        .module
        .unwrap();
    let ids = chain(&mut wall, heavy(), 40);
    // Into a mix nobody hears: nothing wakes.
    request(
        &mut wall,
        &patch(&last_output(&ids, heavy()), &format!("{mixer}.a")),
    )
    .unwrap();
    // A cheap cable replaces it, and the mix is heard.
    let replacing = request(&mut wall, &patch("vco1.out", &format!("{mixer}.a")))
        .unwrap()
        .change
        .unwrap()
        .seq;
    request(&mut wall, &patch(&format!("{mixer}.out"), "out1.left")).unwrap();
    assert!(
        ids.iter()
            .all(|id| wall.snapshot().timing.unheard.contains(id))
    );
    guarded(&mut wall, 0.30);
    let before = wall.patch.to_file();
    let err = request(&mut wall, &Request::Undo { change: replacing }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 40 modules"), "{}", err.message);
    assert_eq!(wall.patch.to_file(), before);
    assert!(
        ids.iter()
            .all(|id| wall.snapshot().timing.unheard.contains(id))
    );
}

/// Undoing a module brought back (a removal undone) takes it away again
/// and plugs back the cables its own had replaced: when one of those leads
/// from modules nobody hears, the undo wakes them, and is costed at what
/// it wakes.
#[test]
fn undoing_a_restore_that_replaced_a_cable_is_costed_at_what_it_wakes() {
    let (mut wall, _engine) = wall();
    let mixer = changed(request(&mut wall, &add("mix")).unwrap().result)
        .module
        .unwrap();
    let cheap = changed(request(&mut wall, &add("vco")).unwrap().result)
        .module
        .unwrap();
    request(
        &mut wall,
        &patch(&format!("{cheap}.out"), &format!("{mixer}.a")),
    )
    .unwrap();
    let removed = request(
        &mut wall,
        &Request::Remove {
            module: cheap.clone(),
        },
    )
    .unwrap()
    .change
    .unwrap()
    .seq;
    let ids = chain(&mut wall, heavy(), 40);
    request(
        &mut wall,
        &patch(&last_output(&ids, heavy()), &format!("{mixer}.a")),
    )
    .unwrap();
    // The cheap module comes back with its cable, which replaces the
    // heavy chain's; then the mix is heard.
    let restored = request(&mut wall, &Request::Undo { change: removed })
        .unwrap()
        .change
        .unwrap()
        .seq;
    request(&mut wall, &patch(&format!("{mixer}.out"), "out1.left")).unwrap();
    assert!(
        ids.iter()
            .all(|id| wall.snapshot().timing.unheard.contains(id))
    );
    guarded(&mut wall, 0.30);
    let before = wall.patch.to_file();
    let err = request(&mut wall, &Request::Undo { change: restored }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 40 modules"), "{}", err.message);
    assert_eq!(wall.patch.to_file(), before);
    assert!(wall.patch.module(&cheap).is_some());
}

/// Tom's live patch of the morning of 13 Oct (41 modules, KZ-D14's bench
/// patch), which the wall rendered at 57% of its time: on an idle wall it
/// is inside the guard, and a seat can put it up. Bench costs taken on a
/// machine too busy to measure (a load average near 100 on ten cores)
/// costed it at about 251%, so no seat could load any of Tom's saves.
#[test]
fn a_save_the_wall_renders_inside_the_guard_is_put_up() {
    let (mut wall, _engine, _dir) = saving_wall();
    let saves = wall.store.as_ref().unwrap().saves_dir();
    std::fs::create_dir_all(&saves).unwrap();
    std::fs::write(
        saves.join("live patch.json"),
        include_bytes!("../../../../benches/live-patch-2026-10-13.json"),
    )
    .unwrap();
    guarded(&mut wall, 0.05);
    let load = Request::Load {
        name: "live patch".to_string(),
    };
    let loaded = wall.request_as("Loader", false, &load, Instant::now());
    assert!(loaded.is_ok(), "{:?}", loaded.err());
    assert_eq!(wall.patch.modules().len(), 41);
}

fn heard_chain(wall: &mut Wall, count: usize) -> Vec<String> {
    wall.set_load_guard(false);
    request(wall, &Request::New).unwrap();
    let mut ids = chain(wall, heavy(), count);
    let out = changed(request(wall, &add("out")).unwrap().result)
        .module
        .unwrap();
    request(
        wall,
        &patch(&last_output(&ids, heavy()), &format!("{out}.left")),
    )
    .unwrap();
    ids.push(out);
    ids
}

fn bench_sum(wall: &Wall) -> f32 {
    wall.patch
        .modules()
        .iter()
        .map(|module| super::super::cost::kind_share(module.kind, wall.engine.sample_rate()))
        .sum()
}

fn measured_chain(wall: &mut Wall) -> f32 {
    let ids = heard_chain(wall, 2);
    let rate = wall.engine.sample_rate();
    for id in ids {
        let kind = wall.patch.module(&id).unwrap().kind;
        let share = super::super::cost::kind_share(kind, rate);
        wall.engine.set_cost(
            wall.rack.slot_of(&id).unwrap(),
            share * SUB_BLOCK as f32 * 1e9 / rate as f32,
        );
    }
    let bench = bench_sum(wall);
    assert!((bench - 0.30).abs() < 0.01, "{bench}");
    bench
}

fn incoming_save(wall: &mut Wall) -> (Patch, Request) {
    heard_chain(wall, 3);
    save(wall, "busy-save", false).unwrap();
    let incoming = wall.patch.clone();
    let load = Request::Load {
        name: "busy-save".to_string(),
    };
    (incoming, load)
}

#[test]
fn a_busy_machine_costs_new_modules_higher() {
    let (mut wall, _engine, _dir) = saving_wall();
    let (incoming, load) = incoming_save(&mut wall);
    let bench = measured_chain(&mut wall);
    guarded(&mut wall, 2.0 * bench);
    let busy = wall.busy();
    assert!((busy - 2.0).abs() < f32::EPSILON);
    let rest = wall.engine.shared().fixed_load();
    let sum: f32 = incoming
        .modules()
        .iter()
        .map(|module| super::super::cost::kind_share(module.kind, wall.engine.sample_rate()))
        .sum();
    assert!(rest + sum < LOAD_GUARD);
    let projected = 2.0f32.mul_add(sum, rest);
    assert!(projected > LOAD_GUARD);
    let before = wall.patch.to_file();
    let err = request(&mut wall, &load).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!((projected_percent(&err.message) - (projected * 100.0).round()).abs() < f32::EPSILON);
    assert!(
        err.message.contains("2.0× its quiet cost"),
        "{}",
        err.message
    );
    assert_eq!(wall.patch.to_file(), before);
}

#[test]
fn a_quiet_machine_is_costed_at_the_bench() {
    let (mut wall, _engine, _dir) = saving_wall();
    let (incoming, load) = incoming_save(&mut wall);
    measured_chain(&mut wall);
    guarded(&mut wall, 0.25);
    assert!((wall.busy() - 1.0).abs() < f32::EPSILON);
    assert_eq!(
        wall.costed_wake(&[]).refusal(0.25, 0.9).message,
        "this would wake 0 modules nobody heard (), taking the wall from 25% to about 90% of its time rendering, past the 80% it keeps to; take something away first, or wake them a few at a time"
    );
    let sum: f32 = incoming
        .modules()
        .iter()
        .map(|module| super::super::cost::kind_share(module.kind, wall.engine.sample_rate()))
        .sum();
    // A larger bench sum keeps the factor quiet while exercising refusal text.
    heard_chain(&mut wall, 6);
    guarded(&mut wall, 0.75);
    wall.engine.shared().set_fixed_load(0.75);
    for module in wall.patch.modules() {
        wall.engine
            .set_cost(wall.rack.slot_of(&module.id).unwrap(), 0.0);
    }
    assert!((wall.busy() - 1.0).abs() < f32::EPSILON);
    let err = wall.load_room(&incoming).unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "the wall this would put up renders 4 modules, taking it to about {:.0}% of its time rendering, past the 80% it keeps to, so it was not put up",
            (0.75 + sum) * 100.0
        )
    );
    let mut notes = Vec::new();
    let warning = wall
        .load_room_as(true, &incoming, &Source::Empty, &mut notes)
        .unwrap()
        .unwrap();
    assert_eq!(
        warning,
        format!(
            "Tom loaded \"new\": about {:.0}% of the wall's time, past the 80% guard",
            (0.75 + sum) * 100.0
        )
    );
    assert_eq!(notes, [warning]);
    measured_chain(&mut wall);
    guarded(&mut wall, 0.25);
    wall.engine.shared().set_fixed_load(0.0);
    request(&mut wall, &load).unwrap();
    assert_eq!(wall.patch.to_file().modules, incoming.to_file().modules);
}

#[test]
fn a_small_wall_says_nothing_of_the_machine() {
    let (mut wall, _engine, _dir) = saving_wall();
    request(&mut wall, &Request::New).unwrap();
    request(&mut wall, &add("out")).unwrap();
    assert!(bench_sum(&wall) < super::super::cost::BUSY_MIN_BENCH);
    for load in [0.0, 0.6, 9.0, f32::NAN, f32::INFINITY] {
        guarded(&mut wall, load);
        assert!((wall.busy() - 1.0).abs() < f32::EPSILON, "{load}");
        assert!(
            !wall
                .costed_wake(&[])
                .refusal(load, 0.9)
                .message
                .contains("machine is busy")
        );
    }
}

#[test]
fn the_factor_is_held_to_eight() {
    let (mut wall, _engine, _dir) = saving_wall();
    let bench = measured_chain(&mut wall);
    guarded(&mut wall, 9.0);
    assert!((wall.busy() - super::super::cost::BUSY_MAX).abs() < f32::EPSILON);
    for load in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        wall.engine.shared().set_render_load(load);
        assert!((wall.busy() - 1.0).abs() < f32::EPSILON);
    }
    for factor in [1.24, super::super::cost::BUSY_SAID] {
        wall.engine.shared().set_render_load(bench * factor);
        assert_eq!(
            wall.costed_wake(&[])
                .refusal(0.6, 0.9)
                .message
                .contains("machine is busy"),
            factor >= super::super::cost::BUSY_SAID
        );
    }
}

#[test]
fn a_measured_module_is_not_scaled() {
    let (mut wall, _engine, _dir) = saving_wall();
    let bench = measured_chain(&mut wall);
    let heard = wall.patch.modules()[0].id.clone();
    let kind = wall.patch.module(&heard).unwrap().kind;
    let share = 0.02;
    wall.engine.set_cost(
        wall.rack.slot_of(&heard).unwrap(),
        share * SUB_BLOCK as f32 * 1e9 / wall.engine.sample_rate() as f32,
    );
    let dormant = chain(&mut wall, heavy(), 1).pop().unwrap();
    wall.engine.set_cost(
        wall.rack.slot_of(&dormant).unwrap(),
        share * SUB_BLOCK as f32 * 1e9 / wall.engine.sample_rate() as f32,
    );
    guarded(&mut wall, 2.0 * bench);
    let busy = wall.busy();
    assert!((busy - 2.0).abs() < f32::EPSILON);
    assert!(
        wall.costed_wake(&[])
            .refusal(2.0 * bench, 1.12)
            .message
            .contains("2.0× its quiet cost")
    );
    assert!((wall.module_share(&heard, kind, busy) - share).abs() < 1e-6);
    assert!(wall.unheard.contains(&dormant));
    let stale_share = wall.kind_share_now(heavy(), busy);
    assert!((wall.module_share(&dormant, heavy(), busy) - stale_share).abs() < 1e-6);
    wall.engine.forget_cost(wall.rack.slot_of(&heard).unwrap());
    let new_share = 2.0 * super::super::cost::kind_share(kind, wall.engine.sample_rate());
    assert!((wall.module_share(&heard, kind, busy) - new_share).abs() < 1e-6);
}

#[test]
fn tom_hears_the_machine_is_busy() {
    let (mut wall, _engine, _dir) = saving_wall();
    let (incoming, load) = incoming_save(&mut wall);
    let bench = measured_chain(&mut wall);
    guarded(&mut wall, 2.0 * bench);
    assert!((wall.busy() - 2.0).abs() < f32::EPSILON);
    let loaded = wall.request_as("Tom", true, &load, Instant::now()).unwrap();
    let result: LoadResult = serde_json::from_value(loaded.result).unwrap();
    assert!(result.notes.iter().any(|note| note.contains("the machine is busy: the wall up now renders at 2.0× its quiet cost, and the new modules are costed the same")), "{:?}", result.notes);
    assert_eq!(wall.patch.to_file().modules, incoming.to_file().modules);
}

/// The bench cost of the modules the engine renders now: those with a slot
/// that are not culled.
fn rendered_bench(wall: &Wall) -> f32 {
    let rate = wall.engine.sample_rate();
    wall.patch
        .modules()
        .iter()
        .filter(|module| {
            wall.rack.slot_of(&module.id).is_some() && !wall.unheard.contains(&module.id)
        })
        .map(|module| super::super::cost::kind_share(module.kind, rate))
        .sum()
}

/// A second `out` beside the seed's: heard (an output hears itself), with
/// both its inputs free.
fn second_out(wall: &mut Wall) -> String {
    changed(request(wall, &add("out")).unwrap().result)
        .module
        .unwrap()
}

/// A cable that wakes culled modules is costed at the factor of the wall
/// the engine renders now. The modules it would wake render nothing yet:
/// counted in the bench, they would lower the factor, and the busier the
/// machine, the more a wake of heavy modules would be under-costed.
#[test]
fn a_wake_is_costed_at_the_machine_as_it_renders_now() {
    let (mut wall, _engine) = wall();
    wall.set_load_guard(false);
    let out = second_out(&mut wall);
    let dormant = chain(&mut wall, heavy(), 2);
    assert!(dormant.iter().all(|id| wall.unheard.contains(id)));
    let bench = rendered_bench(&wall);
    assert!(bench >= super::super::cost::BUSY_MIN_BENCH, "{bench}");
    guarded(&mut wall, 2.0 * bench);
    assert!((wall.busy() - 2.0).abs() < 1e-5);
    let woken = 2.0 * super::super::cost::kind_share(heavy(), wall.engine.sample_rate());
    let before = wall.patch.to_file();
    let refused = request(
        &mut wall,
        &patch(&last_output(&dormant, heavy()), &format!("{out}.left")),
    );
    let Err(err) = refused else {
        panic!("the wake was put up: costed below twice its bench cost");
    };
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 2 modules"), "{}", err.message);
    assert!(
        err.message.contains(
            "(the machine is busy: the wall up now renders at 2.0× its quiet cost, and the new modules are costed the same)"
        ),
        "{}",
        err.message
    );
    assert!(
        projected_percent(&err.message) >= (2.0f32.mul_add(woken, 2.0 * bench) * 100.0).floor(),
        "{}",
        err.message
    );
    assert_eq!(wall.patch.to_file(), before);
}

/// Undoing a module brought back takes it away and plugs back the cable
/// its own had replaced, waking what that cable leads from. The module
/// taken away still renders until the change is made: its bench cost
/// stays in the factor's bench, as its render time stays in the load.
#[test]
fn a_wake_that_takes_a_module_away_counts_it_as_rendered_now() {
    let (mut wall, _engine) = wall();
    wall.set_load_guard(false);
    let out = second_out(&mut wall);
    let taken = chain(&mut wall, heavy(), 1);
    request(
        &mut wall,
        &patch(&last_output(&taken, heavy()), &format!("{out}.left")),
    )
    .unwrap();
    let taken = taken[0].clone();
    let removed = request(
        &mut wall,
        &Request::Remove {
            module: taken.clone(),
        },
    )
    .unwrap()
    .change
    .unwrap()
    .seq;
    let dormant = chain(&mut wall, heavy(), 2);
    request(
        &mut wall,
        &patch(&last_output(&dormant, heavy()), &format!("{out}.left")),
    )
    .unwrap();
    // The heavy module comes back with its cable, which replaces the
    // dormant chain's.
    let restored = request(&mut wall, &Request::Undo { change: removed })
        .unwrap()
        .change
        .unwrap()
        .seq;
    assert!(dormant.iter().all(|id| wall.unheard.contains(id)));
    assert!(!wall.unheard.contains(&taken));
    let bench = rendered_bench(&wall);
    guarded(&mut wall, 2.0 * bench);
    assert!(2.0 * bench < LOAD_GUARD, "{bench}");
    assert!((wall.busy() - 2.0).abs() < 1e-5);
    let before = wall.patch.to_file();
    let err = request(&mut wall, &Request::Undo { change: restored }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Full);
    assert!(err.message.contains("wake 2 modules"), "{}", err.message);
    assert!(
        err.message.contains("renders at 2.0× its quiet cost"),
        "{}",
        err.message
    );
    assert_eq!(wall.patch.to_file(), before);
    assert!(wall.patch.module(&taken).is_some());
}
