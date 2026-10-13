use super::*;

fn play(wall: &mut Wall, engine: &mut Engine) {
    let mut buffer = [0.0; SUB_BLOCK * 2];
    loop {
        engine.render(&mut buffer, 2);
        assert!(wall.engine.pump().is_empty());
        if wall.engine.backlog() == 0 {
            break;
        }
    }
}

#[test]
fn only_a_delayed_cable_gets_a_line_and_keeps_it() {
    let (mut wall, mut engine) = wall();
    play(&mut wall, &mut engine);
    let needed = wall
        .timing
        .delays
        .values()
        .filter(|delay| **delay > 0)
        .count();
    assert_eq!(wall.engine.shared().lines(), needed);
    let reverb = wall
        .patch
        .modules()
        .iter()
        .find(|module| crate::seed::SEED_REVERBS.contains(&module.kind.name()))
        .unwrap()
        .id
        .clone();
    ask(&mut wall, &add("klon")).unwrap();
    ask(&mut wall, &add("mix")).unwrap();
    ask(&mut wall, &patch("vco1.out", "klon1.left")).unwrap();
    ask(&mut wall, &patch("klon1.left", "mix2.a")).unwrap();
    ask(&mut wall, &patch(&format!("{reverb}.left"), "mix2.b")).unwrap();
    ask(&mut wall, &patch("mix2.out", "out1.left")).unwrap();
    play(&mut wall, &mut engine);
    let before = wall.engine.shared().lines();
    assert!(before > 0);
    let dry = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.from_module == reverb && cable.to_module == "mix2")
        .unwrap()
        .id;
    let tag = wall.lined[&dry];
    let wet = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.from_module == "klon1" && cable.to_module == "mix2")
        .unwrap()
        .id;
    ask(
        &mut wall,
        &Request::Unpatch {
            cable: Some(wet),
            to: None,
        },
    )
    .unwrap();
    play(&mut wall, &mut engine);
    assert!(!wall.timing.delays.contains_key(&dry));
    assert_eq!(wall.lined[&dry], tag);
    assert_eq!(wall.engine.shared().lines(), before);
    ask(&mut wall, &patch("klon1.left", "mix2.a")).unwrap();
    play(&mut wall, &mut engine);
    ask(&mut wall, &patch("lfo2.out", "vcf1.cutoff")).unwrap();
    play(&mut wall, &mut engine);
    assert_eq!(wall.engine.shared().lines(), before);
    ask(&mut wall, &Request::Remove { module: reverb }).unwrap();
    play(&mut wall, &mut engine);
    assert!(wall.engine.shared().lines() < before);
}

#[derive(Debug)]
struct Constant;

impl DspModule for Constant {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        io.outputs[0].fill(0.25);
    }

    fn reset(&mut self) {}
}

#[derive(Debug)]
struct HearDry {
    samples: Arc<[AtomicU32; SUB_BLOCK]>,
}

impl DspModule for HearDry {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for (sample, cell) in io.inputs[1].iter().zip(self.samples.iter()) {
            cell.store(sample.to_bits(), Ordering::Relaxed);
        }
        io.outputs[0].copy_from_slice(&io.inputs[0]);
    }

    fn reset(&mut self) {}
}

fn replace(wall: &mut Wall, id: &str, kind: Kind, module: Box<dyn DspModule>) {
    let slot = wall.rack.slot_of(id).unwrap();
    wall.engine.send(Command::Insert {
        slot,
        tag: wall.rack.slots[slot].as_ref().unwrap().1,
        kind,
        module,
        knobs: [0.0; MAX_KNOBS],
    });
}

fn direct_wall() -> (Wall, Engine, Arc<[AtomicU32; SUB_BLOCK]>, u32) {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    for kind in ["vco", "klon", "mix", "readout"] {
        ask(&mut wall, &add(kind)).unwrap();
    }
    ask(&mut wall, &patch("vco1.out", "mix1.b")).unwrap();
    ask(&mut wall, &patch("mix1.out", "readout1.in")).unwrap();
    replace(&mut wall, "vco1", Kind::VCO, Box::new(Constant));
    let samples = Arc::new(std::array::from_fn(|_| AtomicU32::new(0)));
    replace(
        &mut wall,
        "mix1",
        Kind::MIX,
        Box::new(HearDry {
            samples: Arc::clone(&samples),
        }),
    );
    let dry = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.to_module == "mix1")
        .unwrap()
        .id;
    (wall, engine, samples, dry)
}

fn delayed_path(wall: &mut Wall) {
    ask(wall, &patch("vco1.out", "klon1.left")).unwrap();
    ask(wall, &patch("klon1.left", "mix1.a")).unwrap();
}

fn hear_block(engine: &mut Engine) {
    let mut block = [0.0; SUB_BLOCK * 2];
    engine.render(&mut block, 2);
}

#[test]
fn a_direct_cable_keeps_playing_when_it_first_needs_delay() {
    let (mut wall, mut engine, samples, dry) = direct_wall();
    play(&mut wall, &mut engine);
    assert_eq!(wall.engine.shared().lines(), 0);
    delayed_path(&mut wall);
    assert!(wall.timing.delays[&dry] > 0);
    hear_block(&mut engine);
    assert!(
        samples
            .iter()
            .all(|cell| cell.load(Ordering::Relaxed) == 0.25_f32.to_bits())
    );
    assert_eq!(wall.engine.shared().lines(), 1);
    for _ in 0..100 {
        hear_block(&mut engine);
        assert!(
            samples
                .iter()
                .all(|cell| cell.load(Ordering::Relaxed) == 0.25_f32.to_bits())
        );
    }
}

#[test]
fn a_joining_line_can_arrive_before_its_cable_table() {
    let (mut wall, mut engine, samples, dry) = direct_wall();
    play(&mut wall, &mut engine);
    let slot = wall.rack.slot_of("mix1").unwrap();
    wall.timing.delays.insert(dry, 43);
    wall.place_lines(&BTreeMap::from([(dry, (slot, 1))]), &BTreeSet::from([slot]));
    play(&mut wall, &mut engine);
    wall.patch
        .plug("vco1.out", "klon1.left", None, None)
        .unwrap();
    wall.patch.plug("klon1.left", "mix1.a", None, None).unwrap();
    wall.send_tables();
    assert!(wall.timing.delays[&dry] > 0);
    hear_block(&mut engine);
    assert!(
        samples
            .iter()
            .all(|cell| cell.load(Ordering::Relaxed) == 0.25_f32.to_bits())
    );
    assert_eq!(wall.engine.shared().lines(), 1);
}

#[test]
fn a_direct_cable_heard_again_starts_silent_at_its_first_delay() {
    let (mut wall, mut engine, samples, dry) = direct_wall();
    play(&mut wall, &mut engine);
    ask(
        &mut wall,
        &Request::Unpatch {
            cable: None,
            to: Some("readout1.in".to_string()),
        },
    )
    .unwrap();
    delayed_path(&mut wall);
    play(&mut wall, &mut engine);
    assert!(!wall.lined.contains_key(&dry));
    ask(&mut wall, &patch("mix1.out", "readout1.in")).unwrap();
    let delay = usize::from(wall.timing.delays[&dry]);
    hear_block(&mut engine);
    assert!(
        samples[..delay.min(SUB_BLOCK)]
            .iter()
            .all(|cell| cell.load(Ordering::Relaxed) == 0.0_f32.to_bits())
    );
    for _ in 0..delay.div_ceil(SUB_BLOCK) + 1 {
        hear_block(&mut engine);
    }
    assert!(
        samples
            .iter()
            .all(|cell| cell.load(Ordering::Relaxed) == 0.25_f32.to_bits())
    );
}

#[test]
fn a_failed_line_allocation_reports_the_cable_without_delay() {
    let (mut wall, mut engine, samples, dry) = direct_wall();
    play(&mut wall, &mut engine);
    wall.line_allocator = |_, _, _| None;
    delayed_path(&mut wall);
    let timing = wall.snapshot().timing;
    assert!(!timing.cables.contains_key(&dry.to_string()));
    assert_eq!(timing.uncompensated, [dry]);
    assert!(!wall.lined.contains_key(&dry));
    assert!(!wall.line_places.contains_key(&dry));
    hear_block(&mut engine);
    assert_eq!(wall.engine.shared().lines(), 0);
    assert!(
        samples
            .iter()
            .all(|cell| cell.load(Ordering::Relaxed) == 0.25_f32.to_bits())
    );
    wall.line_allocator = super::super::delay_lines::make_line;
    wall.send_tables();
    assert!(wall.snapshot().timing.cables.contains_key(&dry.to_string()));
    assert!(wall.snapshot().timing.uncompensated.is_empty());
    hear_block(&mut engine);
    assert_eq!(wall.engine.shared().lines(), 1);
    assert!(
        samples
            .iter()
            .all(|cell| cell.load(Ordering::Relaxed) == 0.25_f32.to_bits())
    );
}

#[test]
fn a_cable_without_a_route_gets_no_line() {
    let (mut wall, mut engine, _, dry) = direct_wall();
    play(&mut wall, &mut engine);
    wall.timing.delays.insert(dry, 43);
    wall.place_lines(&BTreeMap::new(), &BTreeSet::new());
    assert!(!wall.lined.contains_key(&dry));
    assert!(!wall.line_places.contains_key(&dry));
    play(&mut wall, &mut engine);
    assert_eq!(wall.engine.shared().lines(), 0);
}

#[test]
fn a_line_is_removed_even_before_its_route_is_recorded() {
    let (mut wall, mut engine, _, dry) = direct_wall();
    play(&mut wall, &mut engine);
    let destination = (wall.rack.slot_of("mix1").unwrap(), 1);
    wall.timing.delays.insert(dry, 43);
    assert_eq!(wall.sent_cables.remove(&dry), Some(destination));
    wall.place_lines(&BTreeMap::from([(dry, destination)]), &BTreeSet::new());
    assert_eq!(wall.line_places[&dry], destination);
    play(&mut wall, &mut engine);
    assert_eq!(wall.engine.shared().lines(), 1);
    wall.patch.unplug(dry, None).unwrap();
    wall.send_tables();
    play(&mut wall, &mut engine);
    assert_eq!(wall.engine.shared().lines(), 0);
    assert!(!wall.line_places.contains_key(&dry));
    assert!(!wall.lined.contains_key(&dry));
}
