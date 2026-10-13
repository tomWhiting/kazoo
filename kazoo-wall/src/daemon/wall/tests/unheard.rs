use super::*;

#[derive(Debug)]
struct CountRenders(Arc<AtomicU32>);

impl DspModule for CountRenders {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        self.0.fetch_add(1, Ordering::Relaxed);
        io.outputs[0].copy_from_slice(&io.inputs[0]);
    }

    fn reset(&mut self) {}
}

#[test]
fn a_module_reaching_nothing_is_unheard_and_said_so() {
    let (mut wall, mut engine) = wall();
    let calls = Arc::new(AtomicU32::new(0));
    let slot = wall.rack.slot_of("vcf1").unwrap();
    let mut knobs = [0.0; MAX_KNOBS];
    for (target, value) in knobs
        .iter_mut()
        .zip(&wall.patch.module("vcf1").unwrap().knobs)
    {
        *target = *value;
    }
    wall.engine.send(Command::Insert {
        slot,
        tag: wall.rack.slots[slot].as_ref().unwrap().1,
        kind: Kind::VCF,
        module: Box::new(CountRenders(Arc::clone(&calls))),
        knobs,
    });
    let tail = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.to_module == "out1")
        .map(|cable| {
            format!(
                "{}.{}",
                cable.from_module,
                wall.patch
                    .module(&cable.from_module)
                    .unwrap()
                    .kind
                    .spec()
                    .outputs[cable.from_port]
                    .name
            )
        })
        .unwrap();
    ask(
        &mut wall,
        &Request::Remove {
            module: "out1".to_string(),
        },
    )
    .unwrap();
    let expected: Vec<String> = wall
        .patch
        .modules()
        .iter()
        .map(|module| module.id.clone())
        .collect();
    let look: Snapshot = serde_json::from_value(ask(&mut wall, &Request::Look).unwrap()).unwrap();
    assert_eq!(look.timing.unheard, expected);
    ask(
        &mut wall,
        &Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: 200.0,
            glide_beats: Some(1.0),
        },
    )
    .unwrap();
    let before = wall.engine.shared().frames();
    let mut buffer = vec![0.0; 48_000 * 2];
    assert_no_alloc::assert_no_alloc(|| engine.render(&mut buffer, 2));
    assert_eq!(wall.engine.shared().frames() - before, 48_000);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(buffer.iter().all(|sample| *sample == 0.0));
    assert!(wall.snapshot().probes.is_empty());
    let frozen = wall
        .snapshot()
        .modules
        .into_iter()
        .find(|module| module.id == "vcf1")
        .unwrap();
    assert_eq!(frozen.knobs[0].target.to_bits(), 200.0_f64.to_bits());
    assert_ne!(frozen.knobs[0].value.to_bits(), 200.0_f64.to_bits());
    ask(&mut wall, &add("out")).unwrap();
    ask(&mut wall, &patch(&tail, "out2.left")).unwrap();
    ask(&mut wall, &add("readout")).unwrap();
    ask(&mut wall, &patch("vcf1.out", "readout1.in")).unwrap();
    assert!(wall.snapshot().timing.unheard.is_empty());
    assert_no_alloc::assert_no_alloc(|| engine.render(&mut buffer, 2));
    assert!(calls.load(Ordering::Relaxed) > 0);
    assert!(
        probed(&mut wall, Some("readout1")).probes[0]
            .readout
            .is_some()
    );
}

#[test]
fn a_scope_makes_what_it_reads_heard() {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut isolated = Patch::empty(120.0);
    isolated.add("lfo", None, None).unwrap();
    let mut wall = Wall::new(isolated, control, None, Vec::new(), None);
    assert_eq!(wall.snapshot().timing.unheard, ["lfo1"]);
    ask(&mut wall, &add("scope")).unwrap();
    assert_eq!(wall.snapshot().timing.unheard, ["lfo1"]);
    ask(&mut wall, &patch("lfo1.out", "scope1.a")).unwrap();
    assert!(wall.snapshot().timing.unheard.is_empty());
    drop(engine);
}
